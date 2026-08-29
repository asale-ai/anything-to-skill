//! The model half of the job, when the CLI has to do it itself.
//!
//! `extract` never needs a model: reading a file is deterministic, and that is
//! the whole point of keeping it separate. `build` and `eval` do, because
//! deciding what a book is *for* is not a parsing problem. This module is the
//! smallest client that can carry them: one endpoint, retries on the failures
//! that are worth retrying, and a running token count so a build can say what
//! it cost.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::cell::Cell;
use std::time::Duration;

const API_VERSION: &str = "2023-06-01";
const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";

/// The default model. Skill-writing is a summarisation job with a long input
/// and a short output, which is the shape this tier is good at; `--model`
/// exists for when a particular source is worth more.
pub const DEFAULT_MODEL: &str = "claude-sonnet-5";

/// A single request can take a while — a long chapter in, a page of notes out.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// How many times to retry a request that failed for a reason that might not
/// repeat: rate limits, overload, and the 5xx class.
const MAX_ATTEMPTS: u32 = 4;

pub struct Client {
    agent: ureq::Agent,
    api_key: String,
    base_url: String,
    model: String,
    input_tokens: Cell<u64>,
    output_tokens: Cell<u64>,
}

#[derive(Debug, Deserialize)]
struct MessageResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    usage: Option<Usage>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Default, Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

impl Client {
    /// Build a client from the environment.
    ///
    /// The key is never taken from an argument: a key on a command line ends up
    /// in the shell history of whoever ran it.
    pub fn from_env(model: Option<String>) -> Result<Client> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .context(
                "ANTHROPIC_API_KEY is not set.\n  \
                 `build` and `eval` write with a model, so they need a key:\n    \
                 export ANTHROPIC_API_KEY=sk-ant-...\n  \
                 `extract` and `audit` need no key and no network account.",
            )?;

        let base_url = std::env::var("ANTHROPIC_BASE_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

        let model = model
            .or_else(|| std::env::var("ANYTHING_TO_SKILL_MODEL").ok())
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent(crate::net::USER_AGENT)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .build()
            .into();

        Ok(Client {
            agent,
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            model,
            input_tokens: Cell::new(0),
            output_tokens: Cell::new(0),
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Tokens spent so far, as `(input, output)`.
    pub fn usage(&self) -> (u64, u64) {
        (self.input_tokens.get(), self.output_tokens.get())
    }

    /// One turn: a system prompt, one user message, and the text that comes back.
    pub fn complete(&self, system: &str, user: &str, max_tokens: u32) -> Result<String> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [{ "role": "user", "content": user }],
        });

        let body = serde_json::to_string(&body).context("encoding the request")?;
        let url = format!("{}/v1/messages", self.base_url);
        let mut last_error = None;

        for attempt in 1..=MAX_ATTEMPTS {
            match self.send(&url, &body) {
                Ok(response) => {
                    if let Some(usage) = &response.usage {
                        self.input_tokens
                            .set(self.input_tokens.get() + usage.input_tokens);
                        self.output_tokens
                            .set(self.output_tokens.get() + usage.output_tokens);
                    }
                    // A truncated answer is worse than a failed one: it looks
                    // complete and is missing its end.
                    if response.stop_reason.as_deref() == Some("max_tokens") {
                        bail!(
                            "the model hit its output limit of {max_tokens} tokens and the \
                             answer is cut off — ask for less at once"
                        );
                    }
                    let text: String = response
                        .content
                        .iter()
                        .filter(|block| block.kind == "text")
                        .map(|block| block.text.as_str())
                        .collect();
                    if text.trim().is_empty() {
                        bail!("the model returned no text");
                    }
                    return Ok(text);
                }
                Err(err) => {
                    if attempt == MAX_ATTEMPTS || !err.retryable {
                        return Err(err.into_anyhow());
                    }
                    // Back off, and say so — a build that has gone quiet for a
                    // minute should not look like a hang.
                    let wait = Duration::from_secs(2u64.pow(attempt));
                    eprintln!(
                        "  {} — retrying in {}s ({}/{})",
                        err.message,
                        wait.as_secs(),
                        attempt,
                        MAX_ATTEMPTS - 1
                    );
                    std::thread::sleep(wait);
                    last_error = Some(err);
                }
            }
        }
        Err(last_error
            .map(RequestError::into_anyhow)
            .unwrap_or_else(|| anyhow::anyhow!("the request failed")))
    }

    fn send(&self, url: &str, body: &str) -> Result<MessageResponse, RequestError> {
        let mut response = self
            .agent
            .post(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .send(body)
            .map_err(|err| RequestError {
                // A transport failure is the retryable case by definition:
                // nothing about the request itself was rejected.
                retryable: true,
                message: format!("request to {url} failed: {err}"),
            })?;

        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .unwrap_or_else(|_| String::new());

        if status != 200 {
            return Err(RequestError {
                retryable: matches!(status, 408 | 409 | 429 | 500..=599),
                message: format!("HTTP {status}: {}", api_error_message(&text)),
            });
        }

        serde_json::from_str(&text).map_err(|err| RequestError {
            retryable: false,
            message: format!("could not read the response: {err}"),
        })
    }
}

struct RequestError {
    retryable: bool,
    message: String,
}

impl RequestError {
    fn into_anyhow(self) -> anyhow::Error {
        anyhow::anyhow!(self.message)
    }
}

/// Pull the human-readable part out of an API error body, falling back to the
/// body itself so a failure is never reported as an empty string.
fn api_error_message(body: &str) -> String {
    #[derive(Deserialize)]
    struct ErrorEnvelope {
        error: Option<ErrorBody>,
    }
    #[derive(Deserialize)]
    struct ErrorBody {
        message: Option<String>,
    }
    serde_json::from_str::<ErrorEnvelope>(body)
        .ok()
        .and_then(|e| e.error)
        .and_then(|e| e.message)
        .unwrap_or_else(|| {
            let trimmed = body.trim();
            if trimmed.is_empty() {
                "no response body".to_string()
            } else {
                trimmed.chars().take(300).collect()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_api_error_body_yields_its_message() {
        let body =
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad model"}}"#;
        assert_eq!(api_error_message(body), "bad model");
    }

    #[test]
    fn a_body_that_is_not_json_still_says_something() {
        assert_eq!(api_error_message("upstream timeout"), "upstream timeout");
        assert_eq!(api_error_message("   "), "no response body");
    }

    #[test]
    fn text_blocks_are_concatenated_and_others_dropped() {
        let response: MessageResponse = serde_json::from_str(
            r#"{"content":[{"type":"thinking","text":"hm"},{"type":"text","text":"answer"}]}"#,
        )
        .unwrap();
        let text: String = response
            .content
            .iter()
            .filter(|b| b.kind == "text")
            .map(|b| b.text.as_str())
            .collect();
        assert_eq!(text, "answer");
    }
}
