//! Page-oriented text cleanup shared by every PDF path.
//!
//! Extractors emit what is on the page, which includes the furniture: a running
//! header on every page, a page number in the margin, and a word split across a
//! line break. None of it is content, and all of it survives into the model's
//! context if left alone.

use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

// A Roman numeral of the kind used to number front matter, bounded to 1..99.
//
// The pattern spells out the SHAPE of a canonical numeral instead of listing
// the letters one may contain. A naive `[ivxlcdm]{1,7}` matches any short word
// built from those letters, so "MIX", "CIVIL", "DIM", "MILD" and "VIVID" would
// all be silently deleted whenever they landed on a page's first or last
// non-blank line — and a one-word line is exactly what a part title or display
// heading looks like. Deleting real text is a worse failure than leaving a
// stray numeral, so the pattern is exact.
//
// The range is 1-99, which is what front matter uses; "c"/"d"/"m" therefore do
// not match on their own, so a lone "C" or "M" line is kept as text.
//
// The Python original guarded against a blank match with a `(?=[ivxl])`
// lookahead. Rust's regex crate has no lookahead, so `is_page_number` checks
// for a non-empty line before applying the anchored pattern instead — with `^`
// and `$` both present, an all-optional match can only succeed on an empty
// string, which is exactly what that check excludes.
static ROMAN_1_99: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:xc|xl|l?x{0,3})(?:ix|iv|v?i{0,3})$").unwrap());

// A word split across a line break by a hyphen.
static HYPHEN_WRAP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\w)-\n(\w)").unwrap());

/// True when the line is nothing but a page number (Arabic, or a front-matter
/// Roman numeral).
fn is_page_number(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    if t.len() <= 4 && t.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    ROMAN_1_99.is_match(t)
}

/// Join words split across a line by a hyphen.
///
/// Naive by design: it may join a genuinely-hyphenated wrapped compound
/// ("well-\nknown" -> "wellknown"). A dictionary-aware split is the fix if that
/// ever bites in practice.
pub fn dehyphenate(text: &str) -> String {
    HYPHEN_WRAP.replace_all(text, "$1$2").into_owned()
}

/// Clean form-feed-delimited page text: drop repeated running headers/footers
/// and edge page numbers, then dehyphenate.
///
/// Callers that produce per-page text should join it with `\x0c` so this sees
/// page boundaries — that is what makes header/footer detection possible at all.
pub fn clean_paged_text(text: &str) -> String {
    let pages: Vec<&str> = text.split('\u{c}').collect();

    if pages.len() < 3 {
        // Too few pages to tell a running header from a one-off line.
        return dehyphenate(&text.replace('\u{c}', "\n"));
    }

    // A top or bottom line repeated on more than half the pages is boilerplate.
    let mut edge_counts: HashMap<&str, usize> = HashMap::new();
    for page in &pages {
        let non_blank: Vec<&str> = page
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if let (Some(first), Some(last)) = (non_blank.first(), non_blank.last()) {
            *edge_counts.entry(*first).or_insert(0) += 1;
            if first != last {
                *edge_counts.entry(*last).or_insert(0) += 1;
            }
        }
    }
    let threshold = pages.len() / 2;
    let boilerplate: Vec<&str> = edge_counts
        .into_iter()
        .filter(|(_, count)| *count > threshold)
        .map(|(line, _)| line)
        .collect();

    let mut kept: Vec<&str> = Vec::new();
    for page in &pages {
        let lines: Vec<&str> = page.lines().collect();
        let non_blank_idx: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
            .map(|(i, _)| i)
            .collect();
        let first = non_blank_idx.first().copied();
        let last = non_blank_idx.last().copied();

        for (i, line) in lines.iter().enumerate() {
            let s = line.trim();
            if boilerplate.contains(&s) {
                continue;
            }
            // Drop a bare page number only at a page edge — the number varies
            // per page, so it cannot be caught by the repetition test above.
            if (Some(i) == first || Some(i) == last) && is_page_number(s) {
                continue;
            }
            kept.push(line);
        }
    }

    dehyphenate(&kept.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_hyphenated_line_breaks() {
        assert_eq!(dehyphenate("resid-\nual learning"), "residual learning");
    }

    #[test]
    fn strips_running_header_and_page_numbers() {
        let text = "The Book\nreal content one\n1\u{c}The Book\nreal content two\n2\u{c}\
                    The Book\nreal content three\n3\u{c}The Book\nreal content four\n4";
        let out = clean_paged_text(text);
        assert!(!out.contains("The Book"), "header survived:\n{out}");
        assert!(out.contains("real content one"));
        assert!(out.contains("real content four"));
        // Bare page numbers at the page edges are gone.
        assert!(!out.lines().any(|l| l.trim() == "1"));
    }

    #[test]
    fn keeps_real_words_that_look_like_roman_numerals() {
        // "MIX", "CIVIL", "DIM" are the regression this exactness protects.
        for word in ["MIX", "CIVIL", "DIM", "MILD", "VIVID", "C", "M"] {
            assert!(!is_page_number(word), "{word} was treated as a page number");
        }
    }

    #[test]
    fn recognizes_front_matter_numerals() {
        for numeral in ["i", "iv", "xiv", "XC", "42", "7"] {
            assert!(is_page_number(numeral), "{numeral} was not recognized");
        }
    }

    #[test]
    fn short_documents_keep_every_line() {
        let text = "only page\ncontent here";
        assert_eq!(clean_paged_text(text), "only page\ncontent here");
    }
}
