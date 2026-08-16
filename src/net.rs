//! HTTP fetching, and the manners that go with it.
//!
//! Reading somebody's documentation site is a privilege, not a right. This
//! module identifies itself, honours `robots.txt`, caps how much it will pull
//! down, and waits between requests. None of that is optional politeness — a
//! tool that hammers a docs site gets its user's IP blocked.

use crate::url::Url;
use anyhow::{Context, Result, bail};
use std::time::Duration;
use ureq::ResponseExt;

pub const USER_AGENT: &str = concat!(
    "anything-to-skill/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/asale-ai/anything-to-skill)"
);

/// The token a `robots.txt` `User-agent:` line would name us by.
const ROBOTS_TOKEN: &str = "anything-to-skill";

/// Per-response ceiling. Books are large; web pages are not. Anything past this
/// is a download the user did not ask for.
const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

pub struct Fetched {
    /// Where the bytes actually came from, after redirects.
    pub url: Url,
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

impl Fetched {
    /// Whether the response is HTML, by declared type first and URL shape second.
    pub fn is_html(&self) -> bool {
        match &self.content_type {
            Some(ct) => ct.contains("html") || ct.contains("xml+xhtml"),
            None => matches!(
                self.url.extension().as_deref(),
                Some("html" | "htm" | "xhtml") | None
            ),
        }
    }

    /// The file extension to save this response under, so the on-disk
    /// extractors see the format they expect.
    pub fn extension(&self) -> String {
        if let Some(ct) = &self.content_type {
            let ct = ct.split(';').next().unwrap_or("").trim();
            if let Some(ext) = extension_for_mime(ct) {
                return ext.to_string();
            }
        }
        self.url.extension().unwrap_or_else(|| "html".to_string())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

fn extension_for_mime(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "application/pdf" => "pdf",
        "application/epub+zip" => "epub",
        "application/rtf" | "text/rtf" => "rtf",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.oasis.opendocument.text" => "odt",
        "text/markdown" | "text/x-markdown" => "md",
        "text/csv" => "csv",
        "text/plain" => "txt",
        "text/html" | "application/xhtml+xml" => "html",
        _ => return None,
    })
}

pub fn agent() -> ureq::Agent {
    build_agent(Duration::from_secs(45))
}

/// A separate agent for the files a site may simply not have — `robots.txt`,
/// `sitemap.xml`, `llms.txt`. They get a short timeout because nothing depends
/// on them, and their own connection pool because a probe that ends badly must
/// not leave a half-read connection behind for the real fetches to reuse.
pub fn probe_agent() -> ureq::Agent {
    build_agent(Duration::from_secs(10))
}

fn build_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_global(Some(timeout))
        .max_redirects(10)
        .build()
        .into()
}

/// Fetch one URL. Non-2xx is an error with the status in it, because a crawl
/// that silently treats a 403 page as content produces a skill about nothing.
pub fn fetch(agent: &ureq::Agent, url: &Url) -> Result<Fetched> {
    let mut response = agent
        .get(url.to_string())
        .call()
        .with_context(|| format!("fetching {url}"))?;

    let status = response.status();
    if !status.is_success() {
        bail!("{url} returned HTTP {}", status.as_u16());
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase());

    let final_url = Url::parse(&response.get_uri().to_string()).unwrap_or_else(|_| url.clone());

    let bytes = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_vec()
        .with_context(|| format!("reading the body of {url}"))?;

    if bytes.is_empty() {
        bail!("{url} returned an empty body");
    }

    Ok(Fetched {
        url: final_url,
        bytes,
        content_type,
    })
}

/// Fetch a URL that is allowed to not exist — `robots.txt`, `sitemap.xml`.
fn fetch_optional_text(agent: &ureq::Agent, url: &Url) -> Option<String> {
    fetch(agent, url).ok().map(|f| f.text())
}

/// The subset of the Robots Exclusion Protocol that matters to a crawler which
/// only ever issues GETs: which paths this user agent may fetch.
#[derive(Debug, Default)]
pub struct Robots {
    /// `(pattern, allow)`, in file order. A site with no `robots.txt`, or one
    /// we could not fetch, has no rules and permits everything — that is the
    /// protocol's default, not a shortcut.
    rules: Vec<(String, bool)>,
}

impl Robots {
    /// Read `robots.txt` for a URL's origin. Never fails: an unreachable file
    /// means no restrictions, which is what every other crawler assumes too.
    pub fn fetch(origin_of: &Url) -> Robots {
        let Ok(url) = Url::parse(&format!("{}/robots.txt", origin_of.origin())) else {
            return Robots::default();
        };
        match fetch_optional_text(&probe_agent(), &url) {
            Some(body) => Robots::parse(&body),
            None => Robots::default(),
        }
    }

    pub fn parse(body: &str) -> Robots {
        // Groups are runs of `User-agent:` lines followed by rules. We keep the
        // rules of the group naming us, and fall back to the `*` group.
        let mut ours: Vec<(String, bool)> = Vec::new();
        let mut wildcard: Vec<(String, bool)> = Vec::new();
        let mut named_us = false;

        let mut agents: Vec<String> = Vec::new();
        let mut collecting_agents = false;

        for line in body.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((field, value)) = line.split_once(':') else {
                continue;
            };
            let field = field.trim().to_ascii_lowercase();
            let value = value.trim();

            match field.as_str() {
                "user-agent" => {
                    if !collecting_agents {
                        agents.clear();
                        collecting_agents = true;
                    }
                    agents.push(value.to_ascii_lowercase());
                }
                "allow" | "disallow" => {
                    collecting_agents = false;
                    if value.is_empty() && field == "disallow" {
                        // "Disallow:" with no path is an explicit allow-all.
                        continue;
                    }
                    let rule = (value.to_string(), field == "allow");
                    for a in &agents {
                        if a == "*" {
                            wildcard.push(rule.clone());
                        } else if ROBOTS_TOKEN.starts_with(a.as_str()) || a == ROBOTS_TOKEN {
                            named_us = true;
                            ours.push(rule.clone());
                        }
                    }
                }
                _ => collecting_agents = false,
            }
        }

        Robots {
            rules: if named_us { ours } else { wildcard },
        }
    }

    /// Longest matching rule wins; `Allow` wins a tie, per the protocol.
    pub fn allows(&self, path: &str) -> bool {
        let mut best: Option<(usize, bool)> = None;
        for (pattern, allow) in &self.rules {
            let Some(len) = match_len(pattern, path) else {
                continue;
            };
            best = Some(match best {
                Some((best_len, _)) if len > best_len => (len, *allow),
                Some((best_len, best_allow)) if len == best_len => (len, best_allow || *allow),
                Some(previous) => previous,
                None => (len, *allow),
            });
        }
        best.is_none_or(|(_, allow)| allow)
    }
}

/// Match a robots pattern against a path, returning the pattern's specificity
/// (its literal length) on a match. Supports `*` and a trailing `$`.
fn match_len(pattern: &str, path: &str) -> Option<usize> {
    let (body, anchored) = match pattern.strip_suffix('$') {
        Some(b) => (b, true),
        None => (pattern, false),
    };

    let mut rest = path;
    let mut parts = body.split('*');
    let first = parts.next().unwrap_or("");
    rest = rest.strip_prefix(first)?;

    let mut last_was_wildcard = false;
    for part in parts {
        last_was_wildcard = true;
        if part.is_empty() {
            continue;
        }
        let at = rest.find(part)?;
        rest = &rest[at + part.len()..];
        last_was_wildcard = false;
    }

    if anchored && !rest.is_empty() && !last_was_wildcard {
        return None;
    }
    Some(body.replace('*', "").len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_robots_file_permits_everything() {
        let r = Robots::default();
        assert!(r.allows("/anything"));
    }

    #[test]
    fn wildcard_group_applies_when_we_are_not_named() {
        let r = Robots::parse("User-agent: *\nDisallow: /private/\n");
        assert!(!r.allows("/private/x"));
        assert!(r.allows("/docs/x"));
    }

    #[test]
    fn a_group_naming_us_wins_over_the_wildcard() {
        let r = Robots::parse(
            "User-agent: *\nDisallow: /\n\nUser-agent: anything-to-skill\nDisallow: /admin/\n",
        );
        assert!(r.allows("/docs/x"), "our own group should have applied");
        assert!(!r.allows("/admin/x"));
    }

    #[test]
    fn longest_match_wins_and_allow_breaks_the_tie() {
        let r = Robots::parse("User-agent: *\nDisallow: /docs/\nAllow: /docs/public/\n");
        assert!(!r.allows("/docs/secret"));
        assert!(r.allows("/docs/public/page"));
    }

    #[test]
    fn empty_disallow_means_allow_all() {
        let r = Robots::parse("User-agent: *\nDisallow:\n");
        assert!(r.allows("/anything"));
    }

    #[test]
    fn wildcards_and_end_anchors() {
        let r = Robots::parse("User-agent: *\nDisallow: /*.json$\n");
        assert!(!r.allows("/api/data.json"));
        assert!(r.allows("/api/data.json.html"));
    }

    #[test]
    fn mime_decides_the_extension_before_the_url_does() {
        let f = Fetched {
            url: Url::parse("https://arxiv.org/pdf/2501.00001").unwrap(),
            bytes: vec![1],
            content_type: Some("application/pdf".into()),
        };
        assert_eq!(f.extension(), "pdf");
        assert!(!f.is_html());
    }
}
