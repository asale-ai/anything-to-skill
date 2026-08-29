//! `mcp` — the same work, for agents that cannot run a command.
//!
//! Everything else here assumes a shell. That assumption excludes a whole class
//! of agent, and the exclusion is stated in `SKILL.md` as a limitation rather
//! than fixed. This is the fix: one long-lived process speaking JSON-RPC over
//! stdin and stdout, exposing the deterministic half — reading sources and
//! grading skills — as tools.
//!
//! The model half is deliberately absent. `build` and `eval` need an API key
//! and a model; an agent connected over MCP already *is* the model, so handing
//! it a second one to pay for would be nonsense. It calls `extract`, reads the
//! text through `read_text`, and writes the skill itself.
//!
//! One rule shapes the whole file: stdout carries protocol and nothing else.
//! Every message meant for a person goes to stderr. A stray `println!` here
//! corrupts the stream and the client disconnects with no explanation.

use crate::report::RunReport;
use crate::{audit, skill};
use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// How much text one `read_text` call returns by default. Large enough to be
/// worth a round trip, small enough not to blow up the caller's context.
const DEFAULT_READ_CHARS: usize = 40_000;

/// JSON-RPC error codes, as the specification names them.
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

pub struct Server {
    /// Where extractions are written, so `read_text` can page through them.
    work_dir: PathBuf,
}

impl Server {
    pub fn new(work_dir: PathBuf) -> Server {
        Server { work_dir }
    }

    /// Read requests until stdin closes.
    pub fn serve(&self) -> Result<()> {
        eprintln!(
            "anything-to-skill {} — MCP server on stdio, workdir {}",
            env!("CARGO_PKG_VERSION"),
            self.work_dir.display()
        );
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout();

        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let request: Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(err) => {
                    eprintln!("ignoring a line that is not JSON: {err}");
                    continue;
                }
            };
            // A notification carries no id and gets no reply — answering one
            // is a protocol error, not merely noise.
            let Some(id) = request.get("id").cloned() else {
                continue;
            };
            let method = request
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let params = request.get("params").cloned().unwrap_or(json!({}));

            let response = match self.dispatch(&method, &params) {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err(err) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": err.code, "message": err.message},
                }),
            };
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
        Ok(())
    }

    fn dispatch(&self, method: &str, params: &Value) -> Result<Value, RpcError> {
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {}},
                "serverInfo": {
                    "name": "anything-to-skill",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions()})),
            "tools/call" => self.call_tool(params),
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("unknown method `{other}`"),
            )),
        }
    }

    fn call_tool(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "no tool name"))?;
        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        // A tool that fails reports the failure through the result, not as a
        // protocol error: the model is supposed to see it and react.
        let outcome = match name {
            "extract" => self.tool_extract(&arguments),
            "read_text" => self.tool_read_text(&arguments),
            "audit" => tool_audit(&arguments),
            "sources" => Ok(crate::SOURCE_HELP.to_string()),
            other => Err(anyhow::anyhow!("unknown tool `{other}`")),
        };

        Ok(match outcome {
            Ok(text) => json!({"content": [{"type": "text", "text": text}], "isError": false}),
            Err(err) => json!({
                "content": [{"type": "text", "text": format!("error: {err:#}")}],
                "isError": true,
            }),
        })
    }

    fn tool_extract(&self, arguments: &Value) -> Result<String> {
        let sources = string_list(arguments, "sources");
        if sources.is_empty() {
            anyhow::bail!("`sources` must be a non-empty list");
        }
        let web = crate::WebArgs {
            crawl: arguments
                .get("crawl")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            max_pages: usize_arg(arguments, "max_pages", 50),
            depth: usize_arg(arguments, "depth", 3),
            delay_ms: usize_arg(arguments, "delay_ms", 250) as u64,
            no_llms_txt: false,
        };
        let repo = crate::RepoArgs {
            branch: arguments
                .get("branch")
                .and_then(Value::as_str)
                .map(str::to_string),
            max_files: usize_arg(arguments, "max_files", 200),
            include: string_list(arguments, "include"),
            exclude: string_list(arguments, "exclude"),
        };

        let engine = match arguments.get("engine").and_then(Value::as_str) {
            Some("docling") => crate::extract::Engine::Docling,
            _ => crate::extract::Engine::Builtin,
        };
        let extracted = crate::run_extraction(sources, &self.work_dir, web, repo, engine)?;
        Ok(format!(
            "{}\n\nThe text is {} characters. Read it with `read_text`, \
             {DEFAULT_READ_CHARS} characters at a time, starting at offset 0.",
            serde_json::to_string_pretty(&summary(&extracted.report))?,
            extracted.text.chars().count(),
        ))
    }

    fn tool_read_text(&self, arguments: &Value) -> Result<String> {
        let path = self.work_dir.join("full_text.txt");
        let text = std::fs::read_to_string(&path).map_err(|err| {
            anyhow::anyhow!("nothing has been extracted yet ({err}) — call `extract` first")
        })?;
        let offset = usize_arg(arguments, "offset", 0);
        let limit = usize_arg(arguments, "limit", DEFAULT_READ_CHARS).max(1);

        let total = text.chars().count();
        if offset >= total {
            return Ok(format!(
                "offset {offset} is past the end of the text ({total} characters)."
            ));
        }
        let slice: String = text.chars().skip(offset).take(limit).collect();
        let end = offset + slice.chars().count();
        let footer = if end < total {
            format!("\n\n[{end}/{total} characters — call again with offset {end}]")
        } else {
            format!("\n\n[{end}/{total} characters — this is the end]")
        };
        Ok(slice + &footer)
    }
}

fn tool_audit(arguments: &Value) -> Result<String> {
    let path = arguments
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("`path` is required"))?;
    let budget = usize_arg(arguments, "body_budget", audit::DEFAULT_BODY_BUDGET);
    let skills = skill::discover(&crate::source::expand_tilde(std::path::Path::new(path)))?;
    let report = audit::audit(&skills, budget);
    Ok(serde_json::to_string_pretty(&report)?)
}

/// The report, minus the parts that only make sense next to a shell.
fn summary(report: &RunReport) -> Value {
    json!({
        "characters": report.characters,
        "estimated_tokens": report.estimated_tokens,
        "content_hash": report.content_hash,
        "structure": report.structure,
        "sources": report.sources,
        "failures": report.failures,
        "needs_visual_reading": report.needs_visual_reading,
    })
}

fn string_list(arguments: &Value, key: &str) -> Vec<String> {
    arguments
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn usize_arg(arguments: &Value, key: &str, default: usize) -> usize {
    arguments
        .get(key)
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(default)
}

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
        }
    }
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "extract",
            "description": "Read a book, document, web page, documentation site or git \
                            repository and return a report on what was read. The text \
                            itself is fetched afterwards with `read_text`. Pass `crawl` \
                            when the source is a documentation site rather than one page.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sources": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "File paths, URLs, or `owner/repo`.",
                    },
                    "crawl": {"type": "boolean", "description": "Follow links on the same site, at or below the directory named."},
                    "max_pages": {"type": "integer", "description": "Stop the crawl after this many pages (default 50)."},
                    "depth": {"type": "integer", "description": "How many links deep to follow (default 3)."},
                    "delay_ms": {"type": "integer", "description": "Pause between requests (default 250)."},
                    "branch": {"type": "string", "description": "Branch or tag, for a repository source."},
                    "max_files": {"type": "integer", "description": "Stop a repository read after this many files (default 200)."},
                    "include": {"type": "array", "items": {"type": "string"}, "description": "Globs to read instead of the default prose formats."},
                    "exclude": {"type": "array", "items": {"type": "string"}, "description": "Globs to skip."},
                    "engine": {"type": "string", "enum": ["builtin", "docling"], "description": "Reader for PDFs and Office formats. `docling` must be installed separately."}
                },
                "required": ["sources"],
            },
        },
        {
            "name": "read_text",
            "description": "Read the extracted text a slice at a time. Call `extract` first. \
                            The reply says where to continue from.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "offset": {"type": "integer", "description": "Character to start at (default 0)."},
                    "limit": {"type": "integer", "description": "How many characters to return."},
                },
            },
        },
        {
            "name": "audit",
            "description": "Grade a skill, or a directory of skills, for what it costs to \
                            load, whether its description can be routed to, and whether its \
                            reference files are reachable. Needs no model and no API key.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "A skill directory, a SKILL.md, or a directory of skills."},
                    "body_budget": {"type": "integer", "description": "Token ceiling for a SKILL.md body (default 2000)."},
                },
                "required": ["path"],
            },
        },
        {
            "name": "sources",
            "description": "Explain every kind of source `extract` accepts, with examples.",
            "inputSchema": {"type": "object", "properties": {}},
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server::new(std::env::temp_dir().join("a2s-mcp-test"))
    }

    #[test]
    fn initialize_announces_tools() {
        let result = server().dispatch("initialize", &json!({})).unwrap();
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[test]
    fn every_tool_has_a_schema_and_a_description() {
        let tools = tool_definitions();
        let tools = tools.as_array().unwrap();
        assert_eq!(tools.len(), 4);
        for tool in tools {
            assert!(tool["name"].as_str().is_some_and(|n| !n.is_empty()));
            assert!(tool["description"].as_str().is_some_and(|d| d.len() > 20));
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    #[test]
    fn an_unknown_method_is_a_protocol_error() {
        let err = server().dispatch("nonsense", &json!({})).unwrap_err();
        assert_eq!(err.code, METHOD_NOT_FOUND);
    }

    #[test]
    fn an_unknown_tool_is_reported_to_the_model_not_the_protocol() {
        // The model has to see this and pick a different tool, so it must come
        // back as a result with isError, never as a JSON-RPC error.
        let result = server()
            .call_tool(&json!({"name": "nope", "arguments": {}}))
            .unwrap();
        assert_eq!(result["isError"], true);
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("unknown tool")
        );
    }

    #[test]
    fn extract_without_sources_fails_usefully() {
        let result = server()
            .call_tool(&json!({"name": "extract", "arguments": {}}))
            .unwrap();
        assert_eq!(result["isError"], true);
        assert!(
            result["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("non-empty")
        );
    }

    #[test]
    fn arguments_fall_back_to_their_defaults() {
        assert_eq!(usize_arg(&json!({}), "max_pages", 50), 50);
        assert_eq!(usize_arg(&json!({"max_pages": 5}), "max_pages", 50), 5);
        assert_eq!(string_list(&json!({}), "include"), Vec::<String>::new());
        assert_eq!(
            string_list(&json!({"include": ["a", 1, "b"]}), "include"),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
