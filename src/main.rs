//! `anything-to-skill` — turn books and documents into text an agent can digest.
//!
//! The CLI does the deterministic half of the job: read the file, get the text
//! out faithfully, and report what it could not read. Deciding what the skill
//! should say is the model's half, and lives in `SKILL.md`.

mod clean;
mod config;
mod extract;
mod html;
mod net;
mod repo;
mod report;
mod sanitize;
mod source;
mod structure;
mod tokens;
mod url;
mod web;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use report::{FileReport, RunReport};
use source::{Payload, expand_tilde};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

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
    /// Extract text from files, web pages, documentation sites, or repositories.
    Extract {
        /// What to read: file paths, URLs, or `owner/repo`. Run `sources` for
        /// every accepted form. Local directories are not walked — name the files.
        #[arg(required = true, value_name = "SOURCE")]
        sources: Vec<String>,
        /// Where to write `full_text.txt` and `metadata.json`.
        /// Defaults to `$ANYTHING_TO_SKILL_WORKDIR`, else a temp directory.
        #[arg(long)]
        out: Option<PathBuf>,
        #[command(flatten)]
        web: WebArgs,
        #[command(flatten)]
        repo: RepoArgs,
    },
    /// Explain every kind of source `extract` accepts.
    Sources,
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

/// Options that only apply to URL sources.
#[derive(Args)]
#[command(next_help_heading = "Web sources")]
struct WebArgs {
    /// Follow links from the URL, staying on the same site at or below its
    /// directory. Without this, only the page you named is read.
    #[arg(long)]
    crawl: bool,
    /// Stop after this many pages.
    #[arg(long, default_value_t = 50, value_name = "N")]
    max_pages: usize,
    /// How many links deep to follow from the starting page.
    #[arg(long, default_value_t = 3, value_name = "N")]
    depth: usize,
    /// Pause between requests. Being fast is not worth being blocked.
    #[arg(long, default_value_t = 250, value_name = "MS")]
    delay_ms: u64,
    /// Crawl the pages even when the site publishes its documentation as one
    /// `llms.txt` file, which is normally read instead.
    #[arg(long)]
    no_llms_txt: bool,
}

/// Options that only apply to repository sources.
#[derive(Args)]
#[command(next_help_heading = "Repository sources")]
struct RepoArgs {
    /// Branch or tag to clone. Overrides one named in the source itself.
    #[arg(long, value_name = "REF")]
    branch: Option<String>,
    /// Stop after this many files.
    #[arg(long, default_value_t = 200, value_name = "N")]
    max_files: usize,
    /// Read these paths instead of the default documentation formats,
    /// e.g. `--include 'src/**/*.py'`. Repeatable.
    #[arg(long, value_name = "GLOB")]
    include: Vec<String>,
    /// Skip these paths, e.g. `--exclude 'docs/changelog/**'`. Repeatable.
    #[arg(long, value_name = "GLOB")]
    exclude: Vec<String>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Commands::Extract {
            sources,
            out,
            web,
            repo,
        } => cmd_extract(sources, out, web, repo),
        Commands::Sources => {
            print!("{}", SOURCE_HELP);
            Ok(())
        }
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

fn cmd_extract(
    sources: Vec<String>,
    out: Option<PathBuf>,
    web: WebArgs,
    repo: RepoArgs,
) -> Result<()> {
    let out_dir = resolve_out_dir(out);
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("creating output directory {}", out_dir.display()))?;

    let options = source::Options {
        crawl: web.crawl,
        crawl_options: web::CrawlOptions {
            max_pages: web.max_pages.max(1),
            max_depth: web.depth,
            delay: Duration::from_millis(web.delay_ms),
            use_llms_txt: !web.no_llms_txt,
        },
        repo_options: repo::RepoOptions {
            max_files: repo.max_files.max(1),
            include: repo.include,
            exclude: repo.exclude,
        },
        branch: repo.branch,
        download_dir: out_dir.join("downloads"),
    };

    let resolved = source::resolve(&sources, &options);
    let mut failures = resolved.failures;
    if resolved.docs.is_empty() {
        bail!("nothing to read.\n  {}", failures.join("\n  "));
    }

    // With more than one document in play, each needs a header: a run over a
    // repository or a crawled site is dozens of documents in one file, and
    // without a name on each, no reader can tell where a claim came from.
    let label_documents = resolved.docs.len() > 1;

    let mut combined = String::new();
    let mut files: Vec<FileReport> = Vec::new();

    for doc in &resolved.docs {
        if matches!(doc.payload, Payload::File(_)) {
            eprintln!("reading {} ...", doc.label);
        }

        match extract::extract_doc(doc) {
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
                    // failure; there is no path forward for this document.
                    failures.push(format!("{}: extraction produced no text", doc.label));
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
                    // Web pages carry their own header, written when the page
                    // was fetched, and must not be labelled twice.
                    if label_documents && matches!(doc.payload, Payload::File(_)) {
                        combined.push_str(&format!("# {}\n\n", doc.label));
                    }
                    combined.push_str(&text);
                }

                files.push(FileReport::new(doc, &extraction, &text, removed));
            }
            Err(err) => failures.push(format!("{}: {err:#}", doc.label)),
        }
    }

    if files.is_empty() {
        bail!("no text extracted.\n  {}", failures.join("\n  "));
    }

    let text_path = out_dir.join("full_text.txt");
    std::fs::write(&text_path, &combined)
        .with_context(|| format!("writing {}", text_path.display()))?;

    let report = RunReport::new(&text_path, &combined, resolved.summaries, files, failures);
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
    println!("  ✓ HTML           built-in");
    println!("  ✓ URLs and sites built-in\n");

    println!("Optional external tools:");
    let tools: [(&str, &str, &str); 4] = [
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
            "git",
            "git",
            "required to read a repository source — no fallback",
        ),
        (
            "ebook-convert",
            "Calibre",
            "required for MOBI / AZW / AZW3 — no fallback",
        ),
    ];
    let mut missing_poppler = false;
    let mut missing_calibre = false;
    let mut missing_git = false;
    for (bin, pkg, why) in tools {
        if extract::which(bin).is_some() {
            println!("  ✓ {bin:<14} ({pkg}) — {why}");
        } else {
            println!("  ✗ {bin:<14} ({pkg}) — {why}");
            match pkg {
                "poppler" => missing_poppler = true,
                "git" => missing_git = true,
                _ => missing_calibre = true,
            }
        }
    }
    if missing_poppler || missing_calibre || missing_git {
        println!("\nTo install what is missing:");
        if missing_poppler {
            println!("  macOS:  brew install poppler");
            println!("  Debian: sudo apt install poppler-utils");
        }
        if missing_git {
            println!("  git:     https://git-scm.com/downloads");
        }
        if missing_calibre {
            println!("  Calibre: https://calibre-ebook.com/download");
        }
        println!(
            "\nNothing here blocks a normal PDF, EPUB, URL or site — poppler improves\n\
             table fidelity and page rendering, git is only needed for repository\n\
             sources, Calibre only for Kindle formats."
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

const SOURCE_HELP: &str = "\
anything-to-skill extract — what you can point it at

  A file
    anything-to-skill extract ~/books/ddia.pdf
    anything-to-skill extract vol1.epub vol2.epub
    Run `anything-to-skill formats` for the extensions it reads.

  A web page
    anything-to-skill extract https://arxiv.org/pdf/2501.00001
    HTML is reduced to the page's content element — the navigation, sidebar
    and footer are dropped. A PDF or DOCX behind a URL is downloaded and read
    as that format, not as a web page.

  A documentation site
    anything-to-skill extract https://docs.example.com/guide/ --crawl
    Follows links on the same site, at or below the directory you named, and
    seeds itself from sitemap.xml when the site publishes one. Bounded by
    --max-pages (default 50) and --depth (default 3), one request at a time,
    and it honours robots.txt.

  A git repository
    anything-to-skill extract rust-lang/book
    anything-to-skill extract https://github.com/owner/repo/tree/main/docs
    anything-to-skill extract git@github.com:owner/repo.git --branch v2
    A shallow clone, then the prose files in reading order: the README first,
    then docs/, then the rest. Source code is skipped unless --include asks
    for it. Needs git on PATH.

  One file inside a repository
    anything-to-skill extract https://github.com/owner/repo/blob/main/SPEC.md
    Fetched directly; nothing is cloned.

An existing path always wins over every other reading, so a local directory
named like a repository is still read as a directory.
";
