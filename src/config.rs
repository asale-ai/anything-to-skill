//! Constants and format classification.

/// Approximate tokens-per-word for Latin / whitespace-delimited text.
pub const WORDS_PER_TOKEN: f64 = 0.75;

/// CJK scripts carry little or no whitespace, so word-splitting under-counts
/// them by orders of magnitude. Count CJK codepoints directly against this
/// chars-per-token ratio instead (see `tokens::estimate`).
pub const CJK_CHARS_PER_TOKEN: f64 = 1.5;

/// Plain-text formats read straight off disk — no parsing library involved.
pub const TEXT_EXTENSIONS: &[&str] = &["txt", "text", "md", "markdown", "rst", "adoc", "asciidoc"];

/// HTML is read off disk and tag-stripped in-process.
pub const HTML_EXTENSIONS: &[&str] = &["html", "htm", "xhtml"];

/// Formats `anydoc` converts to Markdown natively.
pub const ANYDOC_EXTENSIONS: &[&str] = &[
    "doc", "docx", "docm", "odt", "ppt", "pps", "pot", "pptx", "pptm", "ppsx", "ppsm", "rtf",
    "epub", "xls", "xlsx", "xlsm", "xlsb", "ods", "odp", "csv",
];

/// Kindle formats. No library covers these — Calibre's `ebook-convert` is the
/// only route, and it is an external application rather than a dependency.
pub const CALIBRE_EXTENSIONS: &[&str] = &["mobi", "azw", "azw3"];

/// How a given input file will be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Read as UTF-8, no parsing.
    Text,
    /// Read as UTF-8, strip tags.
    Html,
    /// `pdf-inspector`: per-page routing, layout-aware Markdown.
    Pdf,
    /// `anydoc`: one call, Markdown out.
    Anydoc,
    /// Shell out to Calibre `ebook-convert`.
    Calibre,
}

impl Route {
    /// Classify by extension. Returns `None` for anything unsupported, which
    /// the caller reports rather than guessing at.
    pub fn for_extension(ext: &str) -> Option<Route> {
        let ext = ext.to_ascii_lowercase();
        let ext = ext.as_str();
        if ext == "pdf" {
            Some(Route::Pdf)
        } else if TEXT_EXTENSIONS.contains(&ext) {
            Some(Route::Text)
        } else if HTML_EXTENSIONS.contains(&ext) {
            Some(Route::Html)
        } else if ANYDOC_EXTENSIONS.contains(&ext) {
            Some(Route::Anydoc)
        } else if CALIBRE_EXTENSIONS.contains(&ext) {
            Some(Route::Calibre)
        } else {
            None
        }
    }
}

/// Every extension the tool accepts, for error messages and `--formats`.
pub fn supported_extensions() -> Vec<&'static str> {
    let mut all: Vec<&'static str> = vec!["pdf"];
    all.extend_from_slice(TEXT_EXTENSIONS);
    all.extend_from_slice(HTML_EXTENSIONS);
    all.extend_from_slice(ANYDOC_EXTENSIONS);
    all.extend_from_slice(CALIBRE_EXTENSIONS);
    all.sort_unstable();
    all
}
