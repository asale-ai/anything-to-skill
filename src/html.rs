//! HTML to readable text.
//!
//! Two jobs, and the second one is the reason this file is bigger than a tag
//! stripper. Stripping tags off a book chapter is easy — it is prose with
//! markup. Stripping them off a documentation page gives you the navigation
//! sidebar, the version switcher, the cookie banner and the footer, three times
//! over, once per page of the crawl. A skill built on that is mostly furniture.
//!
//! So: find the element that actually holds the page, drop the chrome around
//! it, and keep code blocks intact — a docs page whose examples lost their
//! indentation has lost the part a reader came for.

use regex::Regex;
use std::sync::LazyLock;

/// Elements that are never content, whatever they contain.
const CHROME_TAGS: &[&str] = &[
    "script", "style", "noscript", "svg", "template", "iframe", "form", "nav", "header", "footer",
    "aside", "dialog",
];

/// Containers that hold the page proper, most specific first. The first one
/// that yields real text wins.
const CONTENT_TAGS: &[&str] = &["main", "article"];

/// Class and id values the common documentation generators put on the content
/// element — MkDocs, Docusaurus, Sphinx, GitHub, VitePress, Jupyter Book.
const CONTENT_MARKERS: &[&str] = &[
    "markdown-body",
    "theme-doc-markdown",
    "md-content",
    "rst-content",
    "bd-article",
    "document",
    "article-content",
    "main-content",
    "content-wrapper",
    "vp-doc",
    "prose",
];

/// Class values that mark chrome even when they sit on a plain `<div>`.
const CHROME_MARKERS: &[&str] = &[
    "sidebar",
    "navbar",
    "navigation",
    "breadcrumb",
    "pagination",
    "toc",
    "tableofcontents",
    "table-of-contents",
    "cookie-banner",
    "announcement",
    "skip-link",
    "edit-this-page",
];

/// Tags a marker is honoured on. A marker means something about a *box* on the
/// page; the same word on `<html>` or `<body>` is theme state — mdbook writes
/// `class="sidebar-visible"` on `<html>`, and treating that as a sidebar
/// deletes the entire document.
const MARKABLE_TAGS: &[&str] = &[
    "div", "nav", "aside", "section", "header", "footer", "form", "ul", "ol", "article", "main",
    "table",
];

/// If dropping chrome takes almost everything, the page does not follow the
/// conventions this code knows and the safer reading is the whole document.
const CHROME_FLOOR: f64 = 0.1;

/// Below this, a candidate container is a stub — a heading with no body, or an
/// empty `<main>` filled in by JavaScript — and the next candidate is tried.
const MIN_CONTENT_CHARS: usize = 200;

static SCRIPT_STYLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(script|style)\b[^>]*>.*?</\s*(script|style)\s*>").unwrap()
});
static BLOCK_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)</\s*(p|div|section|article|li|tr|h[1-6]|blockquote|pre)\s*>|<\s*br\s*/?>")
        .unwrap()
});
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]+>").unwrap());
static BLANK_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());
static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static PRE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<pre\b[^>]*>(.*?)</\s*pre\s*>").unwrap());
static NUMERIC_ENTITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&#(x?)([0-9A-Fa-f]+);").unwrap());
static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<title\b[^>]*>(.*?)</\s*title\s*>").unwrap());
static H1: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<h1\b[^>]*>(.*?)</\s*h1\s*>").unwrap());
static HREF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<a\b[^>]*?\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).unwrap()
});
/// An opening tag and its attribute text. Quoted attribute values may contain
/// `>`, so they are matched as units rather than scanned past.
static OPEN_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<([a-z][a-z0-9-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>"#).unwrap()
});
static CLASS_OR_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)\b(class|id)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).unwrap()
});

/// Code blocks are lifted out before the tag strip and put back after it,
/// because the strip trims every line and code is the one place where leading
/// whitespace carries meaning. The marker sits in a private-use range so it
/// cannot collide with page text.
const PRE_MARK: char = '\u{E000}';

/// Strip HTML to readable text.
///
/// Script and style contents are dropped, block-level closers become newlines
/// so paragraphs survive, remaining tags are removed, and entities are decoded.
/// Anything beyond that is the job of a real parser, and prose rarely needs one.
pub fn strip(raw: &str) -> String {
    let (with_marks, blocks) = lift_code_blocks(raw);
    let no_comments = COMMENT.replace_all(&with_marks, " ");
    let no_scripts = SCRIPT_STYLE.replace_all(&no_comments, " ");
    let with_breaks = BLOCK_END.replace_all(&no_scripts, "\n");
    let no_tags = TAG.replace_all(&with_breaks, "");
    let decoded = decode_entities(&no_tags);
    let lines = decoded
        .lines()
        .map(str::trim)
        .collect::<Vec<&str>>()
        .join("\n");
    let squeezed = BLANK_RUN.replace_all(&lines, "\n\n");
    restore_code_blocks(squeezed.trim(), &blocks)
}

/// Reduce a full page to the element that holds its content, then strip it.
///
/// Falls back to the whole document when nothing looks like a content
/// container — a page that does not follow any convention is better read with
/// its furniture than not read at all.
pub fn main_content(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    let cleaned = drop_chrome(raw, &lower);
    let lower = cleaned.to_ascii_lowercase();

    for tag in CONTENT_TAGS {
        if let Some(span) = first_element(&cleaned, &lower, tag) {
            let text = strip(&cleaned[span.clone()]);
            if text.chars().count() >= MIN_CONTENT_CHARS {
                return text;
            }
        }
    }
    for marker in CONTENT_MARKERS {
        if let Some(span) = element_with_marker(&cleaned, &lower, marker) {
            let text = strip(&cleaned[span.clone()]);
            if text.chars().count() >= MIN_CONTENT_CHARS {
                return text;
            }
        }
    }
    if let Some(span) = first_element(&cleaned, &lower, "body") {
        return strip(&cleaned[span]);
    }
    strip(&cleaned)
}

/// The page's own name for itself, for the header that separates one crawled
/// page from the next.
pub fn title(raw: &str) -> Option<String> {
    let from_tag = TITLE
        .captures(raw)
        .or_else(|| H1.captures(raw))
        .map(|c| strip(&c[1]));
    from_tag
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty())
}

/// Every `href` on the page, in document order, raw and unresolved.
pub fn links(raw: &str) -> Vec<String> {
    HREF.captures_iter(raw)
        .filter_map(|c| {
            c.get(1)
                .or_else(|| c.get(2))
                .or_else(|| c.get(3))
                .map(|m| decode_entities(m.as_str()))
        })
        .collect()
}

fn lift_code_blocks(raw: &str) -> (String, Vec<String>) {
    let mut blocks = Vec::new();
    let out = PRE.replace_all(raw, |caps: &regex::Captures| {
        let inner = TAG.replace_all(&caps[1], "");
        let code = decode_entities(&inner);
        let code = code.trim_matches('\n').trim_end();
        blocks.push(code.to_string());
        format!("\n{PRE_MARK}{}{PRE_MARK}\n", blocks.len() - 1)
    });
    (out.into_owned(), blocks)
}

fn restore_code_blocks(text: &str, blocks: &[String]) -> String {
    if blocks.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    for (i, code) in blocks.iter().enumerate() {
        let marker = format!("{PRE_MARK}{i}{PRE_MARK}");
        let fenced = if code.is_empty() {
            String::new()
        } else {
            format!("```\n{code}\n```")
        };
        out = out.replace(&marker, &fenced);
    }
    out
}

fn decode_entities(raw: &str) -> String {
    let named = raw
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&hellip;", "…")
        .replace("&rsquo;", "’")
        .replace("&lsquo;", "‘")
        .replace("&ldquo;", "“")
        .replace("&rdquo;", "”");
    let numeric = NUMERIC_ENTITY.replace_all(&named, |c: &regex::Captures| {
        let radix = if c[1].is_empty() { 10 } else { 16 };
        u32::from_str_radix(&c[2], radix)
            .ok()
            .and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_else(|| c[0].to_string())
    });
    // Sphinx and MkDocs hang a pilcrow off every heading as an anchor link.
    // It is furniture that survives the tag strip because it is text.
    let no_anchors = numeric.replace('¶', "");
    // Ampersand last, so `&amp;lt;` does not turn into a `<`.
    no_anchors.replace("&amp;", "&")
}

/// Remove navigation, footers, and the rest of the page furniture.
fn drop_chrome(raw: &str, lower: &str) -> String {
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    for tag in CHROME_TAGS {
        spans.extend(all_elements(raw, lower, tag));
    }
    for marker in CHROME_MARKERS {
        spans.extend(all_with_marker(raw, lower, marker));
    }
    if spans.is_empty() {
        return raw.to_string();
    }
    spans.sort_by_key(|s| (s.start, std::cmp::Reverse(s.end)));

    let mut out = String::with_capacity(raw.len());
    let mut cursor = 0usize;
    for span in spans {
        if span.start < cursor {
            continue; // nested inside one already removed
        }
        out.push_str(&raw[cursor..span.start]);
        out.push('\n');
        cursor = span.end;
    }
    out.push_str(&raw[cursor..]);

    // Every rule here is a guess about how somebody else builds pages, and a
    // guess that removes the whole page is worse than making none.
    if (out.len() as f64) < raw.len() as f64 * CHROME_FLOOR {
        return raw.to_string();
    }
    out
}

fn first_element(raw: &str, lower: &str, tag: &str) -> Option<std::ops::Range<usize>> {
    all_elements(raw, lower, tag).into_iter().next()
}

/// Every non-nested occurrence of `<tag>…</tag>`, as byte ranges into `raw`.
fn all_elements(raw: &str, lower: &str, tag: &str) -> Vec<std::ops::Range<usize>> {
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    let mut out = Vec::new();
    let mut from = 0usize;

    while let Some(rel) = lower[from..].find(&open) {
        let start = from + rel;
        if !is_tag_boundary(lower, start + open.len()) {
            from = start + open.len();
            continue;
        }
        let Some(end) = element_end(lower, start, &open, &close) else {
            break;
        };
        out.push(start..end.min(raw.len()));
        from = end;
    }
    out
}

/// Walk forward from an opening tag, counting nested opens, and return the byte
/// just past the matching close. An element left unclosed runs to end of input,
/// which is what a browser does with it too.
fn element_end(lower: &str, start: usize, open: &str, close: &str) -> Option<usize> {
    let open_tag_end = lower[start..].find('>').map(|i| start + i + 1)?;
    if lower[start..open_tag_end].ends_with("/>") {
        return Some(open_tag_end);
    }

    let mut depth = 1usize;
    let mut cursor = open_tag_end;
    loop {
        let next_open = find_boundary(lower, cursor, open);
        let next_close = find_boundary(lower, cursor, close);
        match (next_open, next_close) {
            (_, None) => return Some(lower.len()),
            (Some(o), Some(c)) if o < c => {
                depth += 1;
                cursor = o + open.len();
            }
            (_, Some(c)) => {
                depth -= 1;
                let after = lower[c..]
                    .find('>')
                    .map(|i| c + i + 1)
                    .unwrap_or(lower.len());
                if depth == 0 {
                    return Some(after);
                }
                cursor = after;
            }
        }
    }
}

fn find_boundary(lower: &str, from: usize, needle: &str) -> Option<usize> {
    let mut cursor = from;
    while let Some(rel) = lower[cursor..].find(needle) {
        let at = cursor + rel;
        if is_tag_boundary(lower, at + needle.len()) {
            return Some(at);
        }
        cursor = at + needle.len();
    }
    None
}

/// True when a tag name ends here — `<div>` and `<div class=…>` are divs,
/// `<divider>` is not.
fn is_tag_boundary(lower: &str, at: usize) -> bool {
    match lower.as_bytes().get(at) {
        Some(b) => matches!(b, b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r'),
        None => false,
    }
}

/// Elements whose `class` or `id` value contains a marker word.
///
/// The marker has to appear in the attribute *value*, on a tag that can
/// plausibly be a box on the page. Matching the raw text of the document
/// instead — which is what a substring search does — turns any page that
/// mentions the word "navigation" into an empty one.
fn all_with_marker(raw: &str, lower: &str, marker: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    for tag in OPEN_TAG.captures_iter(lower) {
        let name = &tag[1];
        if !MARKABLE_TAGS.contains(&name) {
            continue;
        }
        if !identifiers(&tag[2])
            .iter()
            .any(|value| marks(value, marker))
        {
            continue;
        }
        let open = tag.get(0).map(|m| m.start()).unwrap_or(0);
        let (open_pat, close_pat) = (format!("<{name}"), format!("</{name}"));
        if let Some(end) = element_end(lower, open, &open_pat, &close_pat) {
            out.push(open..end.min(raw.len()));
        }
    }
    out
}

fn element_with_marker(raw: &str, lower: &str, marker: &str) -> Option<std::ops::Range<usize>> {
    all_with_marker(raw, lower, marker).into_iter().next()
}

/// Whether a `class` or `id` value marks its element.
///
/// Matched per token and only at a word start, so `tocCollapsible_ETCA` is a
/// table of contents and `protocol` is not. Generators mangle their class names
/// — CSS modules append a hash, utility frameworks prefix them — so a token has
/// to match by prefix rather than exactly.
fn marks(value: &str, marker: &str) -> bool {
    value.split_whitespace().any(|token| {
        token.starts_with(marker)
            || token.contains(&format!("-{marker}"))
            || token.contains(&format!("_{marker}"))
    })
}

/// The `class` and `id` values of an opening tag's attribute text.
fn identifiers(attrs: &str) -> Vec<&str> {
    CLASS_OR_ID
        .captures_iter(attrs)
        .filter_map(|c| c.get(2).or_else(|| c.get(3)).or_else(|| c.get(4)))
        .map(|m| m.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_prose_and_drops_markup() {
        let html = "<html><head><style>p{color:red}</style></head><body>\
                    <h1>Title</h1><p>First &amp; second.</p><p>Third</p>\
                    <script>alert('x')</script></body></html>";
        let out = strip(html);
        assert!(out.contains("Title"));
        assert!(out.contains("First & second."));
        assert!(out.contains("Third"));
        assert!(!out.contains("color:red"));
        assert!(!out.contains("alert"));
    }

    #[test]
    fn separates_block_elements() {
        let out = strip("<p>one</p><p>two</p>");
        assert!(out.contains('\n'), "blocks ran together: {out:?}");
    }

    #[test]
    fn code_blocks_keep_their_indentation() {
        let html = "<p>Example:</p><pre><code>def f():\n    return 1\n</code></pre>";
        let out = strip(html);
        assert!(
            out.contains("    return 1"),
            "indentation was trimmed: {out:?}"
        );
        assert!(out.contains("```"), "code was not fenced: {out:?}");
    }

    #[test]
    fn numeric_entities_decode() {
        assert_eq!(strip("<p>&#65;&#x42;</p>"), "AB");
    }

    #[test]
    fn escaped_ampersand_does_not_become_a_tag() {
        assert_eq!(strip("<p>&amp;lt;script&amp;gt;</p>"), "&lt;script&gt;");
    }

    #[test]
    fn main_content_drops_the_furniture() {
        let body = "x".repeat(300);
        let html = format!(
            "<body><nav><a href='/a'>Home</a><a href='/b'>API</a></nav>\
             <main><h1>Real</h1><p>{body}</p></main>\
             <footer>Copyright 1999</footer></body>"
        );
        let out = main_content(&html);
        assert!(out.contains("Real"));
        assert!(!out.contains("Home"), "nav survived: {out:?}");
        assert!(!out.contains("Copyright"), "footer survived: {out:?}");
    }

    #[test]
    fn nested_containers_do_not_end_the_element_early() {
        let tail = "y".repeat(300);
        let html =
            format!("<main><div><div>inner</div></div><p>{tail}</p></main><footer>bad</footer>");
        let out = main_content(&html);
        assert!(out.contains("inner"));
        assert!(out.contains(&tail));
        assert!(!out.contains("bad"));
    }

    #[test]
    fn falls_back_when_the_content_container_is_a_stub() {
        // A `<main>` that JavaScript would have filled: too short to be content.
        let body = "z".repeat(300);
        let html =
            format!("<body><main>Loading…</main><div class='markdown-body'>{body}</div></body>");
        let out = main_content(&html);
        assert!(
            out.contains(&body),
            "did not fall through the empty main: {out:?}"
        );
    }

    #[test]
    fn class_marked_chrome_is_dropped() {
        let body = "w".repeat(300);
        let html = format!(
            "<body><div class='md-sidebar'><a href='/x'>Nav link</a></div>\
             <main><p>{body}</p></main></body>"
        );
        let out = main_content(&html);
        assert!(!out.contains("Nav link"), "sidebar survived: {out:?}");
    }

    #[test]
    fn a_marker_matches_class_tokens_not_prose() {
        assert!(marks("md-sidebar", "sidebar"));
        assert!(marks("theme_sidebar", "sidebar"));
        assert!(marks(
            "tocCollapsible_ETCA".to_ascii_lowercase().as_str(),
            "toc"
        ));
        // The word inside another word is not the marker.
        assert!(!marks("protocol", "toc"));
        assert!(!marks("outocean", "toc"));
    }

    #[test]
    fn a_state_class_on_html_does_not_delete_the_page() {
        // mdbook writes `class="sidebar-visible"` on <html>. Read as a sidebar,
        // it takes the whole document with it.
        let body = "q".repeat(300);
        let html = format!(
            "<html class=\"light sidebar-visible\"><body><main><p>{body}</p></main></body></html>"
        );
        let out = main_content(&html);
        assert!(out.contains(&body), "the page was deleted: {out:?}");
    }

    #[test]
    fn sphinx_heading_anchors_do_not_survive() {
        assert_eq!(
            strip("<h2>Install<a class='headerlink'>¶</a></h2>"),
            "Install"
        );
    }

    #[test]
    fn links_are_found_in_every_quoting_style() {
        let found = links(r#"<a href="/a">1</a><a href='/b'>2</a><a href=/c>3</a>"#);
        assert_eq!(found, vec!["/a", "/b", "/c"]);
    }

    #[test]
    fn title_prefers_the_title_tag() {
        assert_eq!(
            title("<title>  Page   Name </title><h1>Other</h1>").as_deref(),
            Some("Page Name")
        );
        assert_eq!(title("<h1>Only H1</h1>").as_deref(), Some("Only H1"));
        assert_eq!(title("<p>none</p>"), None);
    }
}
