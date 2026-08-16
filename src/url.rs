//! A small URL type: enough to crawl with, and no more.
//!
//! Crawling needs three things from a URL — is it the same origin as where we
//! started, what does a relative link on this page resolve to, and have we seen
//! it already. A full RFC 3986 parser answers questions nobody here asks. What
//! matters is that the same-origin check is exact, because getting it wrong
//! means walking onto someone else's site.

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Url {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    /// Always starts with `/`.
    pub path: String,
    pub query: Option<String>,
}

impl Url {
    /// Parse an absolute http(s) URL. Fragments are dropped — they address a
    /// place within a page, and fetching the same page twice for two anchors is
    /// pure waste.
    pub fn parse(raw: &str) -> Result<Url> {
        let raw = raw.trim();
        let Some((scheme, rest)) = raw.split_once("://") else {
            bail!("not an absolute URL: {raw}");
        };
        let scheme = scheme.to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            bail!("unsupported scheme '{scheme}' — only http and https are fetched");
        }

        // Authority runs to the first '/', '?' or '#'.
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        // Userinfo, if present, is everything before the last '@'.
        let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        if authority.is_empty() {
            bail!("URL has no host: {raw}");
        }

        let (host, port) = split_host_port(authority)?;
        let tail = tail.split('#').next().unwrap_or("");
        let (path, query) = match tail.split_once('?') {
            Some((p, q)) => (p, (!q.is_empty()).then(|| q.to_string())),
            None => (tail, None),
        };
        let path = if path.is_empty() { "/" } else { path };

        Ok(Url {
            scheme,
            host: host.to_ascii_lowercase(),
            port,
            path: normalize_path(path),
            query,
        })
    }

    /// Resolve `href` as it appears on a page at `self`.
    ///
    /// Returns `None` for anything that is not a fetchable http(s) location —
    /// `mailto:`, `javascript:`, bare fragments — rather than erroring, because
    /// a page full of those is normal and not a problem worth reporting.
    pub fn join(&self, href: &str) -> Option<Url> {
        let href = href.trim();
        let href = href.split('#').next().unwrap_or("").trim();
        if href.is_empty() {
            return None;
        }

        if href.starts_with("//") {
            return Url::parse(&format!("{}:{href}", self.scheme)).ok();
        }
        if let Some((scheme, _)) = href.split_once("://") {
            // An absolute URL in some other scheme is not ours to follow.
            if !scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
            {
                return None;
            }
            return Url::parse(href).ok();
        }
        // A scheme-like prefix with no `//` — `mailto:`, `tel:`, `javascript:`.
        if let Some((head, _)) = href.split_once(':')
            && !head.contains('/')
            && head.chars().all(|c| c.is_ascii_alphabetic())
            && !head.is_empty()
        {
            return None;
        }

        let (raw_path, query) = match href.split_once('?') {
            Some((p, q)) => (p, (!q.is_empty()).then(|| q.to_string())),
            None => (href, None),
        };

        let path = if raw_path.is_empty() {
            self.path.clone()
        } else if let Some(abs) = raw_path.strip_prefix('/') {
            format!("/{abs}")
        } else {
            let dir = match self.path.rfind('/') {
                Some(i) => &self.path[..=i],
                None => "/",
            };
            format!("{dir}{raw_path}")
        };

        Some(Url {
            scheme: self.scheme.clone(),
            host: self.host.clone(),
            port: self.port,
            path: normalize_path(&path),
            query,
        })
    }

    /// `scheme://host[:port]` — the identity a same-origin check compares.
    pub fn origin(&self) -> String {
        match self.port {
            Some(p) => format!("{}://{}:{p}", self.scheme, self.host),
            None => format!("{}://{}", self.scheme, self.host),
        }
    }

    pub fn same_origin(&self, other: &Url) -> bool {
        self.scheme == other.scheme && self.host == other.host && self.port == other.port
    }

    /// The directory part of the path, ending in `/`. Crawls stay under this so
    /// pointing at `/docs/intro` reads the docs and not the whole marketing site.
    pub fn dir(&self) -> String {
        match self.path.rfind('/') {
            Some(i) => self.path[..=i].to_string(),
            None => "/".to_string(),
        }
    }

    /// The last path segment, for naming things after the page they came from.
    pub fn last_segment(&self) -> &str {
        self.path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("")
    }

    /// The extension of the final path segment, lowercased, without the dot.
    pub fn extension(&self) -> Option<String> {
        let seg = self.last_segment();
        let (_, ext) = seg.rsplit_once('.')?;
        (!ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()))
            .then(|| ext.to_ascii_lowercase())
    }
}

impl std::fmt::Display for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.origin(), self.path)?;
        if let Some(q) = &self.query {
            write!(f, "?{q}")?;
        }
        Ok(())
    }
}

fn split_host_port(authority: &str) -> Result<(&str, Option<u16>)> {
    // IPv6 literals carry colons of their own and are bracketed.
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, tail)) = rest.split_once(']') else {
            bail!("malformed IPv6 host: {authority}");
        };
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(p.parse().map_err(|_| anyhow::anyhow!("bad port: {p}"))?),
            None => None,
        };
        return Ok((host, port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse()
                .map_err(|_| anyhow::anyhow!("bad port in {authority}"))?;
            Ok((host, Some(port)))
        }
        None => Ok((authority, None)),
    }
}

/// Remove `.` and `..` segments so two spellings of one page compare equal.
fn normalize_path(path: &str) -> String {
    let trailing_slash = path.ends_with('/') && path.len() > 1;
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    let mut joined = String::from("/");
    joined.push_str(&out.join("/"));
    if trailing_slash && !joined.ends_with('/') {
        joined.push('/');
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_parts() {
        let u = Url::parse("HTTPS://Docs.Example.com:8443/a/b?x=1#frag").unwrap();
        assert_eq!(u.scheme, "https");
        assert_eq!(u.host, "docs.example.com");
        assert_eq!(u.port, Some(8443));
        assert_eq!(u.path, "/a/b");
        assert_eq!(u.query.as_deref(), Some("x=1"));
        assert_eq!(u.to_string(), "https://docs.example.com:8443/a/b?x=1");
    }

    #[test]
    fn empty_path_becomes_root() {
        assert_eq!(Url::parse("https://example.com").unwrap().path, "/");
    }

    #[test]
    fn joins_relative_and_absolute() {
        let base = Url::parse("https://example.com/docs/guide/intro.html").unwrap();
        assert_eq!(
            base.join("setup.html").unwrap().to_string(),
            "https://example.com/docs/guide/setup.html"
        );
        assert_eq!(
            base.join("../api/index.html").unwrap().to_string(),
            "https://example.com/docs/api/index.html"
        );
        assert_eq!(
            base.join("/top").unwrap().to_string(),
            "https://example.com/top"
        );
        assert_eq!(
            base.join("//cdn.example.com/x").unwrap().to_string(),
            "https://cdn.example.com/x"
        );
        assert_eq!(
            base.join("https://other.example/y").unwrap().to_string(),
            "https://other.example/y"
        );
    }

    #[test]
    fn refuses_non_fetchable_links() {
        let base = Url::parse("https://example.com/a").unwrap();
        assert!(base.join("mailto:someone@example.com").is_none());
        assert!(base.join("javascript:void(0)").is_none());
        assert!(base.join("#section").is_none());
        assert!(base.join("").is_none());
        assert!(base.join("ftp://example.com/f").is_none());
    }

    #[test]
    fn same_origin_is_exact() {
        let a = Url::parse("https://example.com/x").unwrap();
        assert!(a.same_origin(&Url::parse("https://example.com/y").unwrap()));
        assert!(!a.same_origin(&Url::parse("http://example.com/y").unwrap()));
        assert!(!a.same_origin(&Url::parse("https://evil.example.com/y").unwrap()));
        assert!(!a.same_origin(&Url::parse("https://example.com:8443/y").unwrap()));
    }

    #[test]
    fn dir_is_where_a_crawl_is_confined() {
        assert_eq!(Url::parse("https://e.com/docs/a").unwrap().dir(), "/docs/");
        assert_eq!(Url::parse("https://e.com/docs/").unwrap().dir(), "/docs/");
        assert_eq!(Url::parse("https://e.com/").unwrap().dir(), "/");
    }

    #[test]
    fn extension_reads_the_last_segment() {
        assert_eq!(
            Url::parse("https://e.com/paper.PDF").unwrap().extension(),
            Some("pdf".to_string())
        );
        assert_eq!(Url::parse("https://e.com/docs/").unwrap().extension(), None);
        assert_eq!(
            Url::parse("https://e.com/a.b.md").unwrap().extension(),
            Some("md".into())
        );
    }

    #[test]
    fn dot_segments_collapse_so_urls_dedupe() {
        let a = Url::parse("https://e.com/docs/./guide/../guide/x").unwrap();
        assert_eq!(a.path, "/docs/guide/x");
    }
}
