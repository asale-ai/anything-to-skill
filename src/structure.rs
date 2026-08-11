//! Chapter and table-of-contents detection across the languages books ship in.
//!
//! The goal is a *distinct chapter count*, not a list of every heading-looking
//! line: a table-of-contents entry and its body heading name the same chapter,
//! so counting distinct numbers keeps them from being double-counted.

use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

// Explicit chapter heading: "Chapter 5", "Capítulo 5: ...", "Chapter 1. Intro".
// French/German/Italian/Dutch chapter words are included alongside the ToC
// languages below. "ch.?" stays last so the longer words match in full. The
// number is bounded to 1..99 so a year ("2025.") is not read as a chapter.
static EXPLICIT_CHAPTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:chapter|chapitre|kapitel|cap[ií]tulo|capitolo|hoofdstuk|ch\.?)\s*(?:(\d{1,2})|(?P<roman>[IVXLCDMivxlcdm]{1,7}))\b(?P<rest>.*)$",
    )
    .unwrap()
});

// A heading's number is followed by end-of-line, punctuation, or a Capitalized
// title word. A lowercase continuation ("Chapter 6 explores...") is prose or a
// cross-reference, not a heading. The uppercase class includes À-Þ so titles
// starting with Ü/Û (common in German, e.g. "Überblick") are recognized.
static HEADING_TAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^\\s*$|^\\s*[.:\\-—–]|^\\s+[A-ZÀ-Þ0-9\"“(]").unwrap());

// Roman-numeral chapter heading: "I: Loomings", "II. The Carpet-Bag".
// Uppercase alone at line start is safe — no common English word is a valid
// uppercase Roman numeral. Lowercase ("i: Loomings") is only accepted inside a
// Markdown heading, to avoid false positives from words that happen to be valid
// Roman numerals ("vi: the editor" → 6).
static ROMAN_HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^\\s*([IVXLCDM]+)\\s*[:.]\\s+[A-ZÀ-Þ0-9\"“(]").unwrap());
static LC_MD_ROMAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^\\s*#{1,6}\\s+([ivxlcdm]+)\\s*[:.]\\s+[A-Za-zÀ-Þ\"“(]").unwrap());

// Optional Markdown / AsciiDoc heading prefix ("## Chapter 1", "== Section").
// Stripped as a second pass so the CJK/Thai/Korean matchers (which already
// tolerate the prefix inline) are untouched.
static MD_HEADING_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:#{1,6}|={1,6})\s+").unwrap());

// Chinese chapter headings, two common styles:
//   1. explicit "第N章" / "第 3 回" / "第十二节" — 第 + numeral + a classifier;
//   2. a Markdown heading led by a CJK ordinal and a separator, e.g.
//      "## 一 · 缘起" or "## 第一讲" — common in CJK ebooks and lecture notes.
// Scoped to CJK numerals, so Latin/Roman detection is unaffected. Full-width
// Arabic digits (U+FF10–U+FF19) are common in Japanese typesetting ("第１章").
static CN_CHAPTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*第\s*([0-9０-９〇零一二两三四五六七八九十百千]+)\s*[章回卷节篇讲]").unwrap()
});
static MD_CN_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^#{1,6}\s+第?\s*([0-9０-９〇零一二两三四五六七八九十百千]+)\s*[·、.:：章回卷节篇讲]",
    )
    .unwrap()
});

// Thai chapter headings: "บทที่ 3", "บทที่ ๑๒", "ตอนที่ ๘๗". Thai digits
// (U+0E50-U+0E59) are positional like Arabic — unlike the Chinese numerals they
// need no unit composition, only a digit remap.
static TH_CHAPTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:#{1,6}\s+)?(?:บทที่|ตอนที่|ภาคที่|บท|ตอน|ภาค)\s*([0-9๐-๙]+)\b").unwrap()
});

// Korean chapter headings: "제1장 총칙", "## 제4장 근로시간과 휴식", "제6장의2 …".
// The trailing group is the Korean analogue of HEADING_TAIL: Korean has no
// letter case, so the "capitalized title word" test does not transfer.
// Requiring end-of-line, punctuation, or whitespace-then-content is what
// separates a heading from a prose cross-reference, because Korean particles
// attach directly to the noun ("제5장에서") with no intervening space.
static KO_CHAPTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\s*(?:#{1,6}\s+)?제\s*([0-9]+)\s*[장편절관](?:\s*의\s*[0-9]+)?(?:\s*$|[.:\-]|\s+\S)",
    )
    .unwrap()
});

// Table-of-contents header lines. Anchored to a whole line so an inline "the
// contents of this chapter" never matches.
static TOC_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?im)^\s*(?:table of contents|contents|índice|sumário|table des matières|inhaltsverzeichnis|indice|sommario|inhoudsopgave|目[ \t\u{3000}]*(?:录|錄|次))\s*$",
    )
    .unwrap()
});

// ATX-style heading: "# Title", AsciiDoc "== Section". The required space after
// the marker distinguishes AsciiDoc "== X" from a reStructuredText underline
// "=====" (no space), which is intentionally ignored.
static ATX_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,6}|={1,6})\s+(.+?)\s*#*$").unwrap());
// Setext/RST underline: a full line of "=" (level 1) or "-" (level 2).
static SETEXT_UNDERLINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:={2,}|-{2,})$").unwrap());

const ROMAN_VALUES: &[(char, u32)] = &[
    ('I', 1),
    ('V', 5),
    ('X', 10),
    ('L', 50),
    ('C', 100),
    ('D', 500),
    ('M', 1000),
];

fn int_to_roman(mut n: u32) -> String {
    const TABLE: &[(u32, &str)] = &[
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (val, sym) in TABLE {
        while n >= *val {
            out.push_str(sym);
            n -= val;
        }
    }
    out
}

/// Convert a Roman numeral to an int, rejecting non-canonical forms ("IIII",
/// "VV") by round-tripping through `int_to_roman`.
fn roman_to_int(s: &str) -> Option<u32> {
    let upper = s.to_uppercase();
    let mut total: i64 = 0;
    let mut prev: i64 = 0;
    for ch in upper.chars().rev() {
        let v = ROMAN_VALUES.iter().find(|(c, _)| *c == ch)?.1 as i64;
        total += if v < prev { -v } else { v };
        prev = prev.max(v);
    }
    if total <= 0 || total > 200 {
        return None;
    }
    let total = total as u32;
    (int_to_roman(total) == upper).then_some(total)
}

/// Parse a Chinese (or ASCII / full-width digit) chapter numeral into 1..=999.
fn cn_numeral_to_int(s: &str) -> Option<u32> {
    // Normalize full-width digits, then take the pure-digit fast path.
    let normalized: String = s
        .chars()
        .map(|c| match c as u32 {
            0xFF10..=0xFF19 => char::from_u32(c as u32 - 0xFF10 + '0' as u32).unwrap(),
            _ => c,
        })
        .collect();
    if !normalized.is_empty() && normalized.chars().all(|c| c.is_ascii_digit()) {
        let n: u32 = normalized.parse().ok()?;
        return (1..=999).contains(&n).then_some(n);
    }

    let value_of = |c: char| -> Option<u32> {
        Some(match c {
            '〇' | '零' => 0,
            '一' => 1,
            '二' | '两' => 2,
            '三' => 3,
            '四' => 4,
            '五' => 5,
            '六' => 6,
            '七' => 7,
            '八' => 8,
            '九' => 9,
            _ => return None,
        })
    };
    let unit_of = |c: char| -> Option<u32> {
        Some(match c {
            '十' => 10,
            '百' => 100,
            '千' => 1000,
            _ => return None,
        })
    };

    let mut section: u32 = 0;
    let mut current: u32 = 0;
    for ch in normalized.chars() {
        match (value_of(ch), unit_of(ch)) {
            // A digit sets the pending value ("三" in "三十").
            (Some(v), _) => current = v,
            // A unit multiplies it and banks the result. A bare unit means one
            // of it, which is what makes "十二" twelve rather than two.
            (_, Some(u)) => {
                section += if current == 0 { 1 } else { current } * u;
                current = 0;
            }
            // Anything else means this was never a numeral.
            _ => return None,
        }
    }
    let total = section + current;
    (1..=999).contains(&total).then_some(total)
}

fn thai_digits_to_int(s: &str) -> Option<u32> {
    let mapped: String = s
        .chars()
        .map(|c| match c as u32 {
            0x0E50..=0x0E59 => char::from_u32(c as u32 - 0x0E50 + '0' as u32).unwrap(),
            _ => c,
        })
        .collect();
    mapped.parse().ok()
}

/// The chapter number if this line is a genuine chapter heading, with no
/// Markdown/AsciiDoc prefix (the caller strips it first).
fn match_chapter_number(line: &str) -> Option<u32> {
    let s = line.trim();
    // A heading is short. Anything longer is prose that happens to start with
    // a chapter word.
    if s.chars().count() > 80 {
        return None;
    }
    if let Some(c) = EXPLICIT_CHAPTER.captures(s) {
        let rest = c.name("rest").map(|m| m.as_str()).unwrap_or("");
        if HEADING_TAIL.is_match(rest) {
            if let Some(arabic) = c.get(1) {
                return arabic.as_str().parse().ok();
            }
            if let Some(roman) = c.name("roman") {
                return roman_to_int(roman.as_str());
            }
        }
    }
    if let Some(c) = ROMAN_HEAD.captures(s).or_else(|| LC_MD_ROMAN.captures(s)) {
        return roman_to_int(&c[1]);
    }
    if let Some(c) = CN_CHAPTER.captures(s).or_else(|| MD_CN_HEADING.captures(s)) {
        return cn_numeral_to_int(&c[1]);
    }
    if let Some(c) = TH_CHAPTER.captures(s) {
        return thai_digits_to_int(&c[1]);
    }
    if let Some(c) = KO_CHAPTER.captures(s) {
        return c[1].parse().ok();
    }
    None
}

/// The chapter number if this line is a genuine chapter heading.
///
/// Handles Arabic ("Chapter 5", "Capítulo 5: ..."), Roman ("I: Loomings"),
/// Chinese ("第三章 …", "## 一 · …"), Thai ("บทที่ 3"), and Korean ("제1장 총칙")
/// styles — each optionally preceded by a Markdown/AsciiDoc heading marker.
pub fn chapter_number(line: &str) -> Option<u32> {
    if let Some(n) = match_chapter_number(line) {
        return Some(n);
    }
    // Second pass: a Markdown prefix hides the heading from the matchers that
    // anchor on line start. Strip it and retry, so Markdown-emitting extractors
    // detect the same chapters as plain-text extraction.
    let s = line.trim();
    let m = MD_HEADING_PREFIX.find(s)?;
    match_chapter_number(&s[m.end()..])
}

/// Count chapter-like structural headings in Markdown/AsciiDoc/RST sources.
///
/// Groups distinct (case-normalized) titles by depth and returns the count at
/// the shallowest depth with >= 2 distinct titles — this selects the real
/// chapter level in the common "# Book Title / ## Chapter" layout where the top
/// level appears once.
fn structural_chapter_count(text: &str) -> usize {
    let mut levels: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    let mut in_fence = false;
    let mut prev = String::new();

    for line in text.lines() {
        let s = line.trim();
        if s.starts_with("```") || s.starts_with("~~~") {
            in_fence = !in_fence;
            prev.clear();
            continue;
        }
        if in_fence {
            prev.clear();
            continue;
        }
        // Setext/RST underline directly under a title line at least as long.
        if SETEXT_UNDERLINE.is_match(s)
            && !prev.is_empty()
            && !SETEXT_UNDERLINE.is_match(&prev)
            && s.chars().count() >= prev.chars().count()
        {
            let depth = if s.starts_with('=') { 1 } else { 2 };
            levels.entry(depth).or_default().insert(prev.to_lowercase());
            prev.clear();
            continue;
        }
        if let Some(c) = ATX_HEADING.captures(s) {
            let title = c[2].trim().to_lowercase();
            // Reject empty, bare-digit-led ("## 5 Setup"), and all-punctuation
            // ("=====" table-border) titles — none are real chapter headings.
            let first_is_digit = title.chars().next().is_some_and(|ch| ch.is_ascii_digit());
            let has_word = title.chars().any(|ch| ch.is_alphanumeric() || ch == '_');
            if !title.is_empty() && !first_is_digit && has_word {
                levels.entry(c[1].len()).or_default().insert(title);
            }
            prev.clear();
            continue;
        }
        prev = s.to_string();
    }

    if levels.is_empty() {
        return 0;
    }
    for titles in levels.values() {
        if titles.len() >= 2 {
            return titles.len();
        }
    }
    // No level has >= 2 distinct headings: a thin doc. Count them all — this
    // path runs only when numeric chapter detection already found zero, so it
    // cannot inflate real books.
    levels.values().map(|t| t.len()).sum()
}

/// What the extractor learned about the document's shape.
#[derive(Debug, Serialize)]
pub struct Structure {
    pub chapters_detected: usize,
    pub chapter_headings_sample: Vec<String>,
    pub has_toc: bool,
}

/// Detect chapter count and table-of-contents presence.
pub fn detect(text: &str) -> Structure {
    let mut headings: Vec<String> = Vec::new();
    let mut numbers: BTreeSet<u32> = BTreeSet::new();
    for line in text.lines() {
        if let Some(n) = chapter_number(line) {
            numbers.insert(n);
            headings.push(line.trim().to_string());
        }
    }
    // Fall back to structural headings only when no numeric "Chapter N"
    // headings were found, so books with real chapters are unaffected.
    let chapters_detected = if numbers.is_empty() {
        structural_chapter_count(text)
    } else {
        numbers.len()
    };

    // Look for ToC indicators in the head of the document only.
    let head_end = text
        .char_indices()
        .nth(30_000)
        .map_or(text.len(), |(i, _)| i);
    let has_toc = TOC_PATTERN.is_match(&text[..head_end]);

    headings.truncate(10);
    Structure {
        chapters_detected,
        chapter_headings_sample: headings,
        has_toc,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arabic_and_localized_chapter_words() {
        assert_eq!(chapter_number("Chapter 5: Beginnings"), Some(5));
        assert_eq!(chapter_number("Capítulo 3. Inicio"), Some(3));
        assert_eq!(chapter_number("Kapitel 7 — Überblick"), Some(7));
        assert_eq!(chapter_number("## Chapter 12"), Some(12));
    }

    #[test]
    fn rejects_prose_cross_references() {
        // Lowercase continuation is prose, not a heading.
        assert_eq!(chapter_number("Chapter 6 explores the tradeoffs"), None);
        // A year is not a chapter number.
        assert_eq!(chapter_number("Chapter 2025."), None);
    }

    #[test]
    fn roman_numerals() {
        assert_eq!(chapter_number("I: Loomings"), Some(1));
        assert_eq!(chapter_number("II. The Carpet-Bag"), Some(2));
        // Lowercase only inside a Markdown heading.
        assert_eq!(chapter_number("## iv. introduction"), Some(4));
        assert_eq!(chapter_number("vi: the editor"), None);
        // Non-canonical forms are rejected.
        assert_eq!(roman_to_int("IIII"), None);
        assert_eq!(roman_to_int("XIV"), Some(14));
    }

    #[test]
    fn chinese_chapters() {
        assert_eq!(chapter_number("第三章 引言"), Some(3));
        assert_eq!(chapter_number("第十二节"), Some(12));
        assert_eq!(chapter_number("第一讲"), Some(1));
        assert_eq!(chapter_number("## 一 · 缘起"), Some(1));
        // Full-width digits (Japanese typesetting).
        assert_eq!(chapter_number("第１章"), Some(1));
        assert_eq!(cn_numeral_to_int("二十三"), Some(23));
        assert_eq!(cn_numeral_to_int("一百零八"), Some(108));
    }

    #[test]
    fn thai_and_korean_chapters() {
        assert_eq!(chapter_number("บทที่ 3"), Some(3));
        assert_eq!(chapter_number("## บทที่ ๑๒"), Some(12));
        assert_eq!(chapter_number("제1장 총칙"), Some(1));
        assert_eq!(chapter_number("## 제4장 근로시간과 휴식"), Some(4));
        // Korean particle attaches directly — a cross-reference, not a heading.
        assert_eq!(chapter_number("제5장에서"), None);
    }

    #[test]
    fn counts_distinct_chapters_not_lines() {
        // A ToC entry and its body heading name the same chapter.
        let text = "Contents\nChapter 1: A\nChapter 2: B\n\nChapter 1: A\nbody\nChapter 2: B\n";
        let s = detect(text);
        assert_eq!(s.chapters_detected, 2);
        assert!(s.has_toc);
    }

    #[test]
    fn structural_fallback_picks_the_chapter_level() {
        let text = "# Book Title\n\n## Alpha\nbody\n\n## Beta\nbody\n\n## Gamma\nbody\n";
        assert_eq!(detect(text).chapters_detected, 3);
    }

    #[test]
    fn ignores_headings_inside_code_fences() {
        let text =
            "# Book\n\n```\n## not a heading\n## also not\n```\n\n## Real One\n## Real Two\n";
        assert_eq!(detect(text).chapters_detected, 2);
    }

    #[test]
    fn detects_cjk_toc_header() {
        assert!(detect("目 录\n\n第一章 开始\n").has_toc);
        assert!(detect("Inhaltsverzeichnis\n\nKapitel 1: Start\n").has_toc);
    }
}
