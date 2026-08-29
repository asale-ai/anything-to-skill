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

/// Which reader handles the formats that have more than one.
///
/// The built-in path is the one this tool ships and tests. Docling is an
/// external application with a research-grade table model behind it: slower,
/// heavier, and better on the documents where layout carries meaning. Offering
/// it is cheaper and more honest than trying to catch up with it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Engine {
    /// Every parser compiled into this binary. Needs nothing installed.
    #[default]
    Builtin,
    /// Shell out to `docling`, when it is on PATH.
    Docling,
}

/// The formats worth handing to Docling. Plain text through Docling would be a
/// subprocess and a temporary file to accomplish `read_to_string`.
const DOCLING_EXTENSIONS: &[&str] = &[
    "pdf", "docx", "pptx", "xlsx", "html", "htm", "xhtml", "adoc", "asciidoc",
];

/// Extract text from one document, whatever kind of source produced it.
pub fn extract_doc(doc: &Doc, engine: Engine) -> Result<Extraction> {
    match &doc.payload {
        Payload::File(path) => extract_with(path, engine),
        Payload::Text { text, method } => Ok(Extraction::plain(text.clone(), method)),
    }
}

/// Extract text from one file, dispatching on its extension.
pub fn extract_with(path: &Path, engine: Engine) -> Result<Extraction> {
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

    if engine == Engine::Docling && DOCLING_EXTENSIONS.contains(&ext.as_str()) {
        return docling(path);
    }

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

/// Convert a document with Docling.
///
/// Docling writes its output as a file next to the name it was given, and the
/// naming has changed between versions — so the file is found rather than
/// predicted. Anything else is a guess that breaks on an upgrade.
fn docling(path: &Path) -> Result<Extraction> {
    if which("docling").is_none() {
        bail!(
            "--engine docling needs `docling` on PATH, and it is not there.\n\
             Install it with `pip install docling`, or drop the flag to use the\n\
             built-in readers, which need nothing installed."
        );
    }
    let out_dir =
        std::env::temp_dir().join(format!("anything-to-skill-docling-{}", std::process::id()));
    // A stale directory from an earlier run would let its output be picked up
    // as if it belonged to this file.
    let _ = std::fs::remove_dir_all(&out_dir);
    std::fs::create_dir_all(&out_dir).context("creating the Docling scratch directory")?;

    let result = Command::new("docling")
        .arg(path)
        .arg("--to")
        .arg("md")
        .arg("--output")
        .arg(&out_dir)
        .output()
        .context("running docling")?;
    if !result.status.success() {
        let _ = std::fs::remove_dir_all(&out_dir);
        bail!(
            "docling failed on {}: {}",
            path.display(),
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }

    let produced = newest_markdown(&out_dir);
    let outcome = match produced {
        Some(file) => read_text(&file).map(|text| Extraction::plain(text, "docling")),
        None => Err(anyhow::anyhow!(
            "docling wrote no Markdown for {} — it exited cleanly with nothing to show",
            path.display()
        )),
    };
    let _ = std::fs::remove_dir_all(&out_dir);
    outcome
}

/// The Markdown file Docling just wrote, whatever it decided to call it.
fn newest_markdown(dir: &Path) -> Option<std::path::PathBuf> {
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(when, _)| modified >= *when) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, path)| path)
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
    fn docling_only_claims_the_formats_it_is_better_at() {
        assert!(DOCLING_EXTENSIONS.contains(&"pdf"));
        assert!(DOCLING_EXTENSIONS.contains(&"docx"));
        // Plain text through a subprocess would be slower and no more accurate.
        assert!(!DOCLING_EXTENSIONS.contains(&"txt"));
        assert!(!DOCLING_EXTENSIONS.contains(&"md"));
        assert!(!DOCLING_EXTENSIONS.contains(&"epub"));
    }

    #[test]
    fn the_newest_markdown_is_the_one_just_written() {
        let dir = std::env::temp_dir().join(format!("a2s-docling-pick-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "not markdown").unwrap();
        assert!(newest_markdown(&dir).is_none());
        std::fs::write(dir.join("out.md"), "# hi").unwrap();
        assert_eq!(
            newest_markdown(&dir).unwrap().file_name().unwrap(),
            "out.md"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn docling_says_how_to_install_it_when_it_is_missing() {
        if which("docling").is_some() {
            return; // Nothing to assert on a machine that has it.
        }
        let err = docling(Path::new("/tmp/whatever.pdf"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("pip install docling"), "{err}");
    }

    #[test]
    fn unsupported_extension_names_the_alternatives() {
        let err = extract_with(Path::new("/tmp/whatever.xyz"), Engine::Builtin)
            .unwrap_err()
            .to_string();
        assert!(err.contains("unsupported format"), "{err}");
        assert!(err.contains("epub"), "{err}");
    }
}
