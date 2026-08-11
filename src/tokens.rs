//! Deterministic token estimation.

use crate::config::{CJK_CHARS_PER_TOKEN, WORDS_PER_TOKEN};

/// True for CJK scripts that are written without word-separating whitespace.
///
/// Covers CJK Symbols and Punctuation, Hiragana, Katakana, the CJK Unified
/// Ideographs blocks (incl. Extension A), Hangul syllables, CJK Compatibility
/// Ideographs, and the halfwidth/fullwidth forms.
fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F   // CJK Symbols and Punctuation
        | 0x3040..=0x30FF // Hiragana + Katakana
        | 0x3400..=0x4DBF // CJK Unified Ideographs Extension A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xAC00..=0xD7AF // Hangul Syllables
        | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        | 0xFF00..=0xFFEF // Halfwidth and Fullwidth Forms
    )
}

/// Estimate the token count of `text` with a deterministic heuristic.
///
/// Latin / whitespace-delimited text is counted by words. CJK characters are
/// counted directly because they carry little or no whitespace; without this a
/// space-less Chinese or Japanese book estimates at a few tokens and the cost
/// pre-flight under-reports by ~1000x.
///
/// Dependency-free on purpose so the same book always yields the same number.
pub fn estimate(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let cjk = text.chars().filter(|c| is_cjk(*c)).count();
    if cjk == 0 {
        return (text.split_whitespace().count() as f64 / WORDS_PER_TOKEN) as usize;
    }
    // Replace CJK with spaces so the remaining Latin words still split cleanly,
    // then count both populations against their own ratios.
    let latin: String = text
        .chars()
        .map(|c| if is_cjk(c) { ' ' } else { c })
        .collect();
    let latin_words = latin.split_whitespace().count();
    (latin_words as f64 / WORDS_PER_TOKEN + cjk as f64 / CJK_CHARS_PER_TOKEN) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(estimate(""), 0);
    }

    #[test]
    fn latin_counts_by_words() {
        // 6 words / 0.75 = 8
        assert_eq!(estimate("one two three four five six"), 8);
    }

    #[test]
    fn cjk_counts_by_characters() {
        // 8 CJK chars / 1.5 = 5 (no Latin words)
        assert_eq!(estimate("这是一本技术书籍"), 5);
    }

    #[test]
    fn cjk_dominates_a_spaceless_book() {
        // Without CJK handling this would score ~1 token, not ~666.
        let text = "第一章".repeat(333);
        assert!(estimate(&text) > 600, "got {}", estimate(&text));
    }

    #[test]
    fn mixed_counts_both_populations() {
        // "Rust 是系统语言" -> 1 Latin word + 5 CJK chars
        let n = estimate("Rust 是系统语言");
        assert_eq!(
            n,
            (1.0 / WORDS_PER_TOKEN + 5.0 / CJK_CHARS_PER_TOKEN) as usize
        );
    }
}
