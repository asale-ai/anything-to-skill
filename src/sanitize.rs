//! Removal of invisible code points used for document-borne prompt injection.
//!
//! A book is untrusted input: it reaches the model verbatim, and anything the
//! model reads it may act on. The code points below all render as nothing, so a
//! human reviewing the extracted text and the agent consuming it can disagree
//! about what the document says. Stripping them makes the two agree.

/// 1. Zero-width and invisible spacers. Render as nothing, so text between them
///    is invisible to a human reading the page but plain to the model.
const ZERO_WIDTH: &[u32] = &[
    0x200B, // ZERO WIDTH SPACE
    0x200C, // ZERO WIDTH NON-JOINER
    0x200D, // ZERO WIDTH JOINER
    0x2060, // WORD JOINER
    0xFEFF, // ZERO WIDTH NO-BREAK SPACE / BOM outside position 0
    0x00AD, // SOFT HYPHEN — invisible except at a line break
    0x034F, // COMBINING GRAPHEME JOINER — no rendering effect at all
    0x180E, // MONGOLIAN VOWEL SEPARATOR
    0x2061, // FUNCTION APPLICATION
    0x2062, // INVISIBLE TIMES
    0x2063, // INVISIBLE SEPARATOR
    0x2064, // INVISIBLE PLUS
];

/// 2. Bidirectional formatting controls — the Trojan Source class
///    (CVE-2021-42574). These do not change the character sequence a model
///    reads, they change the order a human SEES. A crafted line can display as
///    innocuous study advice while the model consumes an injected instruction.
///
///    Legitimate right-to-left books are unaffected: the Unicode Bidi Algorithm
///    derives direction from the characters themselves, so Arabic and Hebrew
///    still render right-to-left without these. Only explicit embeddings,
///    overrides and isolates are dropped, and running prose essentially never
///    needs them.
const BIDI_CONTROLS: &[u32] = &[
    0x200E, // LEFT-TO-RIGHT MARK
    0x200F, // RIGHT-TO-LEFT MARK
    0x061C, // ARABIC LETTER MARK
    0x202A, // LEFT-TO-RIGHT EMBEDDING
    0x202B, // RIGHT-TO-LEFT EMBEDDING
    0x202C, // POP DIRECTIONAL FORMATTING
    0x202D, // LEFT-TO-RIGHT OVERRIDE
    0x202E, // RIGHT-TO-LEFT OVERRIDE
    0x2066, // LEFT-TO-RIGHT ISOLATE
    0x2067, // RIGHT-TO-LEFT ISOLATE
    0x2068, // FIRST STRONG ISOLATE
    0x2069, // POP DIRECTIONAL ISOLATE
];

/// 3. Characters that are not format controls (so a category-based filter
///    misses them) but still render as blank width. Unlike a space they are
///    letters, so they survive whitespace normalisation and can pad hidden text.
const INVISIBLE_LETTERS: &[u32] = &[
    0x115F, // HANGUL CHOSEONG FILLER
    0x1160, // HANGUL JUNGSEONG FILLER
    0x3164, // HANGUL FILLER
    0xFFA0, // HALFWIDTH HANGUL FILLER
];

/// 4. The Unicode tag block. Originally language tags, now used to smuggle an
///    entire ASCII payload as invisible "tag" characters.
const TAG_BLOCK: std::ops::RangeInclusive<u32> = 0xE0000..=0xE007F;

/// True when the code point renders as nothing and should be stripped.
///
/// Public so a generated-skill scanner can flag exactly what extraction strips.
/// When the two sets drift, the extractor lets a character through that the
/// scanner then warns about — or worse, neither layer covers it.
pub fn is_invisible(codepoint: u32) -> bool {
    ZERO_WIDTH.contains(&codepoint)
        || BIDI_CONTROLS.contains(&codepoint)
        || INVISIBLE_LETTERS.contains(&codepoint)
        || TAG_BLOCK.contains(&codepoint)
}

/// Strip invisible code points, returning the cleaned text and how many were
/// removed. A non-zero count is worth surfacing: it means the source carried
/// characters a reader could not see.
pub fn sanitize(text: &str) -> (String, usize) {
    let mut removed = 0usize;
    let cleaned: String = text
        .chars()
        .filter(|c| {
            if is_invisible(*c as u32) {
                removed += 1;
                false
            } else {
                true
            }
        })
        .collect();
    (cleaned, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_zero_width_and_counts() {
        let (out, n) = sanitize("he\u{200B}llo\u{FEFF}");
        assert_eq!(out, "hello");
        assert_eq!(n, 2);
    }

    #[test]
    fn strips_bidi_override() {
        let (out, n) = sanitize("safe\u{202E}reversed");
        assert_eq!(out, "safereversed");
        assert_eq!(n, 1);
    }

    #[test]
    fn strips_tag_block_payload() {
        let payload: String = "AB"
            .chars()
            .map(|c| char::from_u32(0xE0000 + c as u32).unwrap())
            .collect();
        let (out, n) = sanitize(&format!("visible{payload}"));
        assert_eq!(out, "visible");
        assert_eq!(n, 2);
    }

    #[test]
    fn leaves_ordinary_text_untouched() {
        let (out, n) = sanitize("普通文本 with ASCII — and em dash");
        assert_eq!(out, "普通文本 with ASCII — and em dash");
        assert_eq!(n, 0);
    }

    #[test]
    fn keeps_rtl_letters() {
        // Arabic letters must survive; only the explicit controls go.
        let (out, n) = sanitize("مرحبا\u{202B}");
        assert_eq!(out, "مرحبا");
        assert_eq!(n, 1);
    }
}
