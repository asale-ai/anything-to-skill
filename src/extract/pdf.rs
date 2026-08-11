//! PDF extraction: per-page routing rather than one strategy for the whole file.
//!
//! No single PDF extractor wins on every page. Measured on two real papers:
//!
//! * `pdf-inspector` reconstructs multi-column reading order correctly and is
//!   the only one of the extractors tested that preserves superscripts
//!   (`O(n² · d)` rather than `O(n2 · d)`). It is also fast enough to be free —
//!   12 pages in 0.03s.
//! * It flattens *borderless* wide tables, the kind LaTeX `booktabs` produces:
//!   the caption survives and the numbers do not.
//! * `pdftotext -layout` keeps those same tables perfectly readable through
//!   whitespace alignment — but interleaves the columns of a two-column page
//!   line by line, which destroys the prose.
//!
//! So the two are used where each is strong: `pdf-inspector` for the document,
//! `pdftotext -layout` re-run on just the pages where it reported a table. Pages
//! it flags as unreadable are not guessed at — they are reported upward, and the
//! skill renders those to images for the model to read directly.

use super::{Extraction, which};
use crate::clean::clean_paged_text;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Marker introducing the whitespace-aligned rendering of a table page.
const TABLE_MARKER: &str = "<!-- layout-preserved rendering of this page's tables -->";

pub fn extract(path: &Path) -> Result<Extraction> {
    // `None` selects every page; the crate also accepts an explicit subset.
    let result = pdf_inspector::extract_pages_markdown(path, None)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .with_context(|| format!("pdf-inspector could not read {}", path.display()))?;

    let page_count = result.pages.len() as u32;
    let have_pdftotext = which("pdftotext").is_some();
    let supplement = pages_to_supplement(
        &result.pages_with_tables,
        &result.pages_with_columns,
        page_count,
    );

    let mut pages: Vec<String> = Vec::with_capacity(result.pages.len());
    for page in &result.pages {
        // `PageMarkdown::page` is 0-indexed; every reported page list on the
        // result is 1-indexed. Normalize to 1-indexed here, once.
        let page_no = page.page + 1;
        let mut body = page.markdown.clone();

        // This page holds layout the markdown above may have flattened. Append
        // the aligned rendering rather than replacing the page: the markdown
        // still carries the correct reading order for the prose.
        if have_pdftotext
            && supplement.contains(&page_no)
            && let Some(layout) = pdftotext_page(path, page_no)
            && !layout.trim().is_empty()
        {
            body.push_str("\n\n");
            body.push_str(TABLE_MARKER);
            body.push('\n');
            body.push_str(layout.trim_end());
        }
        pages.push(body);
    }

    // Join with form feeds so the page-boundary cleanup can see them.
    let joined = pages.join("\u{c}");
    let text = clean_paged_text(&joined);

    let method = if have_pdftotext && !supplement.is_empty() {
        "pdf-inspector + pdftotext(-layout) on table pages"
    } else {
        "pdf-inspector"
    };

    Ok(Extraction {
        text,
        method: method.to_string(),
        pages_needing_ocr: result.pages_needing_ocr,
        pages_with_tables: result.pages_with_tables,
        pages_with_columns: result.pages_with_columns,
        page_count: Some(page_count),
    })
}

/// Decide which pages get the `pdftotext -layout` supplement.
///
/// Table pages always qualify. Column pages are the subtle case, because
/// `pages_with_columns` means two different things depending on the document:
///
/// * On a genuinely two-column paper, nearly every page is flagged, and
///   `pdftotext -layout` there is actively harmful — it interleaves the left and
///   right columns line by line and destroys the prose.
/// * On a single-column book, a handful of flagged pages are almost always wide
///   tables that the column detector saw as columns. Those are exactly the pages
///   whose tables the markdown flattens, and `-layout` recovers them.
///
/// The document-level ratio separates the two: when most pages have columns the
/// document *is* multi-column, so column pages are left alone.
fn pages_to_supplement(with_tables: &[u32], with_columns: &[u32], page_count: u32) -> Vec<u32> {
    let mut pages = with_tables.to_vec();

    let mostly_multi_column = page_count > 0 && with_columns.len() * 2 > page_count as usize;
    if !mostly_multi_column {
        pages.extend_from_slice(with_columns);
    }

    pages.sort_unstable();
    pages.dedup();
    pages
}

/// Re-render a single page with `pdftotext -layout`, which preserves the column
/// alignment of borderless tables. Returns `None` on any failure — this is a
/// supplement, and a missing supplement must not fail the extraction.
fn pdftotext_page(path: &Path, page_no: u32) -> Option<String> {
    let out = Command::new("pdftotext")
        .arg("-layout")
        .arg("-f")
        .arg(page_no.to_string())
        .arg("-l")
        .arg(page_no.to_string())
        .arg(path)
        .arg("-")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::pages_to_supplement;

    #[test]
    fn single_column_book_supplements_its_wide_table_pages() {
        // Measured on "Attention Is All You Need": 15 pages, tables reported on
        // 6/8/10/13, and the wide Table 3 on page 9 reported only as columns.
        // Page 9 has to be picked up or that table is lost.
        let pages = pages_to_supplement(&[6, 8, 10, 13], &[8, 9, 10], 15);
        assert_eq!(pages, vec![6, 8, 9, 10, 13]);
    }

    #[test]
    fn two_column_paper_leaves_its_column_pages_alone() {
        // Measured on the ResNet paper: 12 pages, 10 of them two-column.
        // Appending `-layout` output there interleaves the columns and destroys
        // the prose, so only the table pages qualify.
        let pages = pages_to_supplement(&[5, 8, 11, 12], &[1, 2, 3, 4, 6, 7, 8, 9, 10, 12], 12);
        assert_eq!(pages, vec![5, 8, 11, 12]);
    }

    #[test]
    fn exactly_half_is_not_mostly() {
        // The guard is a strict majority, so a 50/50 split still supplements.
        assert_eq!(pages_to_supplement(&[], &[1, 2], 4), vec![1, 2]);
        assert_eq!(pages_to_supplement(&[], &[1, 2, 3], 4), Vec::<u32>::new());
    }

    #[test]
    fn no_layout_signals_means_no_supplement() {
        assert_eq!(pages_to_supplement(&[], &[], 10), Vec::<u32>::new());
    }
}
