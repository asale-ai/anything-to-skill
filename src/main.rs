//! `anything-to-skill` — turn books and documents into text an agent can digest.
//!
//! The CLI does the deterministic half of the job: read the file, get the text
//! out faithfully, and report what it could not read. Deciding what the skill
//! should say is the model's half, and lives in `SKILL.md`.

mod clean;
mod config;
mod extract;
mod report;
mod sanitize;
mod structure;
mod tokens;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use report::{FileReport, RunReport};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser)]
#[command(
    name = "anything-to-skill",
    about = "Extract text from books and documents for agent skill generation",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Extract text from one or more documents.
    Extract {
        /// Files to read. Directories are not walked — name the files.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Where to write `full_text.txt` and `metadata.json`.
        /// Defaults to `$ANYTHING_TO_SKILL_WORKDIR`, else a temp directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Report which optional external tools are available.
    Check,
    /// Render specific PDF pages to PNG, for pages the extractor could not read.
    Render {
        /// The PDF to render from.
        pdf: PathBuf,
        /// 1-indexed page numbers, comma-separated (e.g. `3,17,42`).
        #[arg(long, value_delimiter = ',', required = true)]
        pages: Vec<u32>,
        /// Directory to write PNGs into.
        #[arg(long)]
        out: PathBuf,
        /// Render resolution. 150 keeps dense body text legible while staying
        /// well inside the model's per-image budget.
        #[arg(long, default_value_t = 150)]
        dpi: u32,
    },
    /// List every extension the tool accepts.
    Formats,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Commands::Extract { paths, out } => cmd_extract(paths, out),
        Commands::Check => cmd_check(),
        Commands::Render {
            pdf,
            pages,
            out,
            dpi,
        } => cmd_render(&pdf, &pages, &out, dpi),
        Commands::Formats => {
            println!("{}", config::supported_extensions().join("\n"));
            Ok(())
        }
    }
}

/// Resolve the output directory: explicit flag, then `$ANYTHING_TO_SKILL_WORKDIR`,
/// then a stable temp path so repeated runs land in the same place.
fn resolve_out_dir(explicit: Option<PathBuf>) -> PathBuf {
    explicit
        .or_else(|| std::env::var_os("ANYTHING_TO_SKILL_WORKDIR").map(PathBuf::from))
        .unwrap_or_else(|| std::env::temp_dir().join("anything_to_skill_work"))
}

fn cmd_extract(paths: Vec<PathBuf>, out: Option<PathBuf>) -> Result<()> {
    let out_dir = resolve_out_dir(out);
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("creating output directory {}", out_dir.display()))?;

    let mut combined = String::new();
    let mut files: Vec<FileReport> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for path in &paths {
        let path = expand_tilde(path);
        if !path.is_file() {
            failures.push(format!("{}: not a file", path.display()));
            continue;
        }
        eprintln!("reading {} ...", path.display());

        match extract::extract(&path) {
            Ok(extraction) => {
                let (text, removed) = sanitize::sanitize(&extraction.text);
                if removed > 0 {
                    eprintln!(
                        "  note: removed {removed} invisible code point(s) — the source \
                         carried characters a reader cannot see"
                    );
                }
                if text.trim().is_empty() && extraction.pages_needing_ocr.is_empty() {
                    // Nothing came out and nothing explains why. That is a
                    // failure; there is no path forward for this file.
                    failures.push(format!("{}: extraction produced no text", path.display()));
                    continue;
                }

                // A wholly scanned book yields no text but is not a failure —
                // it is work for the visual path. Record the file either way so
                // its unreadable pages reach `metadata.json`, which is the only
                // machine-readable route the skill has to them.
                if !text.trim().is_empty() {
                    if !combined.is_empty() {
                        combined.push_str("\n\n");
                    }
                    combined.push_str(&text);
                }

                files.push(FileReport::new(&path, &extraction, &text, removed));
            }
            Err(err) => failures.push(format!("{}: {err:#}", path.display())),
        }
    }

    if files.is_empty() {
        bail!("no text extracted.\n  {}", failures.join("\n  "));
    }

    let text_path = out_dir.join("full_text.txt");
    std::fs::write(&text_path, &combined)
        .with_context(|| format!("writing {}", text_path.display()))?;

    let report = RunReport::new(&text_path, &combined, files, failures);
    if combined.trim().is_empty() && report.needs_visual_reading.is_empty() {
        bail!("no text extracted and no pages to read visually");
    }
    let meta_path = out_dir.join("metadata.json");
    std::fs::write(&meta_path, serde_json::to_string_pretty(&report)? + "\n")
        .with_context(|| format!("writing {}", meta_path.display()))?;

    report.print_summary(&text_path, &meta_path);
    Ok(())
}

fn cmd_check() -> Result<()> {
    println!("anything-to-skill — environment check\n");
    println!("Built in (no installation needed):");
    println!("  ✓ PDF            pdf-inspector");
    println!("  ✓ EPUB/DOCX/RTF  anydoc");
    println!("  ✓ ODT/PPTX/XLSX  anydoc");
    println!("  ✓ TXT/MD/RST     built-in");
    println!("  ✓ HTML           built-in\n");

    println!("Optional external tools:");
    let tools: [(&str, &str, &str); 3] = [
        (
            "pdftotext",
            "poppler",
            "recovers borderless tables on PDF table pages",
        ),
        (
            "pdftoppm",
            "poppler",
            "renders unreadable PDF pages to images",
        ),
        (
            "ebook-convert",
            "Calibre",
            "required for MOBI / AZW / AZW3 — no fallback",
        ),
    ];
    let mut missing_poppler = false;
    let mut missing_calibre = false;
    for (bin, pkg, why) in tools {
        if extract::which(bin).is_some() {
            println!("  ✓ {bin:<14} ({pkg}) — {why}");
        } else {
            println!("  ✗ {bin:<14} ({pkg}) — {why}");
            if pkg == "poppler" {
                missing_poppler = true;
            } else {
                missing_calibre = true;
            }
        }
    }
    if missing_poppler || missing_calibre {
        println!("\nTo install what is missing:");
        if missing_poppler {
            println!("  macOS:  brew install poppler");
            println!("  Debian: sudo apt install poppler-utils");
        }
        if missing_calibre {
            println!("  Calibre: https://calibre-ebook.com/download");
        }
        println!(
            "\nNothing here blocks a normal PDF or EPUB — poppler improves table\n\
             fidelity and page rendering, Calibre is only needed for Kindle formats."
        );
    } else {
        println!("\nEverything is available.");
    }
    Ok(())
}

fn cmd_render(pdf: &Path, pages: &[u32], out: &Path, dpi: u32) -> Result<()> {
    if extract::which("pdftoppm").is_none() {
        bail!(
            "rendering needs `pdftoppm` (poppler), which is not on PATH.\n\
             macOS: brew install poppler   Debian: sudo apt install poppler-utils"
        );
    }
    let pdf = expand_tilde(pdf);
    std::fs::create_dir_all(out)
        .with_context(|| format!("creating output directory {}", out.display()))?;

    for page in pages {
        let prefix = out.join(format!("page-{page:04}"));
        let status = Command::new("pdftoppm")
            .arg("-png")
            .arg("-r")
            .arg(dpi.to_string())
            .arg("-f")
            .arg(page.to_string())
            .arg("-l")
            .arg(page.to_string())
            .arg(&pdf)
            .arg(&prefix)
            .output()
            .context("running pdftoppm")?;
        if !status.status.success() {
            bail!(
                "pdftoppm failed on page {page}: {}",
                String::from_utf8_lossy(&status.stderr).trim()
            );
        }
        println!("{}", prefix.display());
    }
    Ok(())
}

/// Expand a leading `~` so paths pasted from a shell prompt work.
fn expand_tilde(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(rest) = s.strip_prefix('~') else {
        return path.to_path_buf();
    };
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_path_buf();
    };
    PathBuf::from(home).join(rest.trim_start_matches('/'))
}
