//! The run report — `metadata.json` plus the terminal summary.
//!
//! This file is the handoff to the model. Everything the skill needs to decide
//! what to do next has to be in here, especially the parts that went wrong:
//! pages that could not be read are the difference between a skill with a hole
//! in it and a skill that knows it has a hole.

use crate::extract::Extraction;
use crate::source::{Doc, SourceSummary};
use crate::{structure, tokens};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct FileReport {
    /// What this document is called: a path, a URL, or `owner/repo:file`.
    pub path: String,
    /// The URL or repository it came from, when that is not `path` itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// Where it sits on disk, when it does. Only a real file can be reopened —
    /// to render its pages — so anything that acts on a document later uses
    /// this and not `path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_path: Option<String>,
    pub method: String,
    /// A fingerprint of this document's text. `refresh` compares these to say
    /// which pages of a site actually moved, rather than only that it did.
    pub content_hash: String,
    pub characters: usize,
    pub estimated_tokens: usize,
    pub page_count: Option<u32>,
    /// Pages whose text is unreliable. The skill renders these to images.
    pub pages_needing_ocr: Vec<u32>,
    pub pages_with_tables: Vec<u32>,
    pub pages_with_columns: Vec<u32>,
    /// Invisible code points removed as potential prompt injection.
    pub invisible_codepoints_removed: usize,
}

impl FileReport {
    pub fn new(doc: &Doc, extraction: &Extraction, text: &str, removed: usize) -> Self {
        let local_path = doc
            .local_path()
            .map(|p| p.display().to_string())
            .filter(|p| *p != doc.label);
        FileReport {
            path: doc.label.clone(),
            origin: doc.origin.clone(),
            local_path,
            method: extraction.method.clone(),
            content_hash: crate::stamp::fingerprint(text),
            characters: text.chars().count(),
            estimated_tokens: tokens::estimate(text),
            page_count: extraction.page_count,
            pages_needing_ocr: extraction.pages_needing_ocr.clone(),
            pages_with_tables: extraction.pages_with_tables.clone(),
            pages_with_columns: extraction.pages_with_columns.clone(),
            invisible_codepoints_removed: removed,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RunReport {
    pub output_text: String,
    /// A fingerprint of the whole extraction. Two runs that produce the same
    /// hash read the same text, which is the question `refresh` asks.
    pub content_hash: String,
    pub characters: usize,
    pub estimated_tokens: usize,
    pub structure: structure::Structure,
    /// One entry per source the user named, and what it contributed. A crawl
    /// that stopped at its page limit says so here — the difference between
    /// "this is the site" and "this is the first 50 pages of it".
    pub sources: Vec<SourceSummary>,
    pub files: Vec<FileReport>,
    /// Files that produced nothing, with the reason. Empty on a clean run.
    pub failures: Vec<String>,
    /// Pages the model should read as images, grouped by source file. Present
    /// only when there is something to do, so the skill can branch on its
    /// existence rather than parsing an empty structure.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub needs_visual_reading: Vec<VisualReadRequest>,
}

#[derive(Debug, Serialize)]
pub struct VisualReadRequest {
    pub path: String,
    pub pages: Vec<u32>,
}

impl RunReport {
    pub fn new(
        text_path: &Path,
        combined: &str,
        sources: Vec<SourceSummary>,
        files: Vec<FileReport>,
        failures: Vec<String>,
    ) -> Self {
        let needs_visual_reading = files
            .iter()
            .filter(|f| !f.pages_needing_ocr.is_empty())
            .map(|f| VisualReadRequest {
                // `render` opens a file, so point it at the one on disk.
                path: f.local_path.clone().unwrap_or_else(|| f.path.clone()),
                pages: f.pages_needing_ocr.clone(),
            })
            .collect();

        RunReport {
            output_text: text_path.display().to_string(),
            content_hash: crate::stamp::fingerprint(combined),
            characters: combined.chars().count(),
            estimated_tokens: tokens::estimate(combined),
            structure: structure::detect(combined),
            sources,
            files,
            failures,
            needs_visual_reading,
        }
    }

    pub fn print_summary(&self, text_path: &Path, meta_path: &Path) {
        eprintln!();
        eprintln!(
            "extracted {} document(s) from {} source(s)",
            self.files.len(),
            self.sources.len()
        );
        eprintln!(
            "  {} characters, ~{} tokens",
            self.characters, self.estimated_tokens
        );
        eprintln!(
            "  {} chapter(s) detected{}",
            self.structure.chapters_detected,
            if self.structure.has_toc {
                ", table of contents present"
            } else {
                ""
            }
        );
        for source in &self.sources {
            eprintln!(
                "  {} — {}, {} document(s)",
                source.source, source.kind, source.documents
            );
            for note in &source.notes {
                eprintln!("      note: {note}");
            }
        }
        // Listing every document is useful for a handful of files and noise for
        // a fifty-page crawl, where what matters is which extractors ran.
        if self.files.len() <= 10 {
            for file in &self.files {
                eprintln!("  {} — {}", file.path, file.method);
            }
        }
        if !self.needs_visual_reading.is_empty() {
            let total: usize = self
                .needs_visual_reading
                .iter()
                .map(|r| r.pages.len())
                .sum();
            eprintln!();
            eprintln!(
                "  {total} page(s) could not be read as text and are missing from the output."
            );
            eprintln!("  Render and read them with:");
            for request in &self.needs_visual_reading {
                let pages: Vec<String> = request.pages.iter().map(u32::to_string).collect();
                eprintln!(
                    "    anything-to-skill render '{}' --pages {} --out <dir>",
                    request.path,
                    pages.join(",")
                );
            }
        }
        if !self.failures.is_empty() {
            eprintln!();
            eprintln!("  {} file(s) failed:", self.failures.len());
            for failure in &self.failures {
                eprintln!("    {failure}");
            }
        }
        eprintln!();
        eprintln!("  text     {}", text_path.display());
        eprintln!("  metadata {}", meta_path.display());
    }
}
