//! The run report — `metadata.json` plus the terminal summary.
//!
//! This file is the handoff to the model. Everything the skill needs to decide
//! what to do next has to be in here, especially the parts that went wrong:
//! pages that could not be read are the difference between a skill with a hole
//! in it and a skill that knows it has a hole.

use crate::extract::Extraction;
use crate::{structure, tokens};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct FileReport {
    pub path: String,
    pub method: String,
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
    pub fn new(path: &Path, extraction: &Extraction, text: &str, removed: usize) -> Self {
        FileReport {
            path: path.display().to_string(),
            method: extraction.method.clone(),
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
    pub characters: usize,
    pub estimated_tokens: usize,
    pub structure: structure::Structure,
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
        files: Vec<FileReport>,
        failures: Vec<String>,
    ) -> Self {
        let needs_visual_reading = files
            .iter()
            .filter(|f| !f.pages_needing_ocr.is_empty())
            .map(|f| VisualReadRequest {
                path: f.path.clone(),
                pages: f.pages_needing_ocr.clone(),
            })
            .collect();

        RunReport {
            output_text: text_path.display().to_string(),
            characters: combined.chars().count(),
            estimated_tokens: tokens::estimate(combined),
            structure: structure::detect(combined),
            files,
            failures,
            needs_visual_reading,
        }
    }

    pub fn print_summary(&self, text_path: &Path, meta_path: &Path) {
        eprintln!();
        eprintln!("extracted {} file(s)", self.files.len());
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
        for file in &self.files {
            eprintln!("  {} — {}", file.path, file.method);
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
