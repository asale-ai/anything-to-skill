//! Format dispatch: one entry point, one route per extension.

pub mod pdf;

use crate::config::Route;
use crate::source::{Doc, Payload};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

/// What one file yielded, plus everything the agent needs to decide whether to
/// trust it.
#[derive(Debug, Serialize)]
pub struct Extraction {
    /// The extracted text.
    #[serde(skip)]
    pub text: String,
    /// Which extractor produced it, for the run report.
    pub method: String,
    /// 1-indexed pages whose text is unreliable — scanned, image-only, or
    /// garbage-encoded. Empty for non-PDF formats. The skill renders these to
    /// images and reads them directly rather than shipping empty pages.
    pub pages_needing_ocr: Vec<u32>,
    /// 1-indexed pages where a table was detected.
    pub pages_with_tables: Vec<u32>,
    /// 1-indexed pages where a multi-column layout was detected.
    pub pages_with_columns: Vec<u32>,
    /// Total page count, when the format has pages.
    pub page_count: Option<u32>,
}

impl Extraction {
    /// A plain extraction with no page-level metadata.
    fn plain(text: String, method: &str) -> Self {
        Extraction {
            text,
            method: method.to_string(),
            pages_needing_ocr: Vec::new(),
            pages_with_tables: Vec::new(),
            pages_with_columns: Vec::new(),
            page_count: None,
        }
    }
}

/// Extract text from one document, whatever kind of source produced it.
pub fn extract_doc(doc: &Doc) -> Result<Extraction> {
    match &doc.payload {
        Payload::File(path) => extract(path),
        Payload::Text { text, method } => Ok(Extraction::plain(text.clone(), method)),
    }
}

/// Extract text from one file, dispatching on its extension.
pub fn extract(path: &Path) -> Result<Extraction> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let route = Route::for_extension(&ext).with_context(|| {
        format!(
            "unsupported format '.{ext}' — supported: {}",
            crate::config::supported_extensions().join(", ")
        )
    })?;

    match route {
        Route::Pdf => pdf::extract(path),
        Route::Text => {
            let text = read_text(path)?;
            Ok(Extraction::plain(text, "plain-text"))
        }
        Route::Html => {
            let raw = read_text(path)?;
            Ok(Extraction::plain(crate::html::strip(&raw), "html-strip"))
        }
        Route::Anydoc => {
            let text = anydoc::to_markdown(path)
                .map_err(|e| anyhow::anyhow!("{e:?}"))
                .with_context(|| format!("anydoc could not convert {}", path.display()))?;
            Ok(Extraction::plain(text, "anydoc"))
        }
        Route::Calibre => calibre(path),
    }
}

/// Read a file as UTF-8, falling back to a lossy decode rather than failing.
///
/// Books carry mixed encodings and stray bytes; refusing the whole file over
/// one bad byte helps nobody, and the replacement character is visible in the
/// output if it matters.
fn read_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Convert a Kindle format via Calibre's `ebook-convert`.
///
/// This is the one format with no library route. Calibre is an external
/// application, not a dependency — if it is missing the only honest thing to do
/// is say so and name the file, rather than emit an empty chapter.
fn calibre(path: &Path) -> Result<Extraction> {
    if which("ebook-convert").is_none() {
        bail!(
            "{} needs Calibre's `ebook-convert`, which is not on PATH.\n\
             Install Calibre (https://calibre-ebook.com/download), or convert the\n\
             file to EPUB yourself and pass that instead.",
            path.display()
        );
    }
    let out_dir = std::env::temp_dir().join("anything-to-skill-calibre");
    std::fs::create_dir_all(&out_dir).context("creating Calibre scratch directory")?;
    let out_file = out_dir.join("converted.txt");

    let status = Command::new("ebook-convert")
        .arg(path)
        .arg(&out_file)
        .output()
        .context("running ebook-convert")?;
    if !status.status.success() {
        bail!(
            "ebook-convert failed on {}: {}",
            path.display(),
            String::from_utf8_lossy(&status.stderr).trim()
        );
    }
    let text = read_text(&out_file)?;
    let _ = std::fs::remove_file(&out_file);
    Ok(Extraction::plain(text, "calibre"))
}

/// Locate an executable on PATH.
pub fn which(program: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(program);
            candidate.is_file().then_some(candidate)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_extension_names_the_alternatives() {
        let err = extract(Path::new("/tmp/whatever.xyz"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unsupported format"), "{err}");
        assert!(err.contains("epub"), "{err}");
    }
}
