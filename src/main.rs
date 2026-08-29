//! `anything-to-skill` — turn books and documents into text an agent can digest.
//!
//! The CLI does the deterministic half of the job: read the file, get the text
//! out faithfully, and report what it could not read. Deciding what the skill
//! should say is the model's half, and lives in `SKILL.md`.

mod audit;
mod build;
mod clean;
mod config;
mod eval;
mod extract;
mod html;
mod llm;
mod mcp;
mod net;
mod repo;
mod report;
mod sanitize;
mod skill;
mod source;
mod stamp;
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
        /// Which reader handles PDFs and Office formats. `docling` is an
        /// external application with a stronger table model; it must be
        /// installed separately and is slower.
        #[arg(long, value_enum, default_value = "builtin")]
        engine: extract::Engine,
        #[command(flatten)]
        web: WebArgs,
        #[command(flatten)]
        repo: RepoArgs,
    },
    /// Read a source and write the finished skill — extraction, reading and
    /// writing in one command, with no agent in the loop.
    Build {
        /// What to read: file paths, URLs, or `owner/repo`. Run `sources`.
        #[arg(required = true, value_name = "SOURCE")]
        sources: Vec<String>,
        /// Where the skill directory is created. Defaults to the working directory.
        #[arg(long, default_value = ".")]
        out: PathBuf,
        /// The skill's name. Derived from the source when not given.
        #[arg(long)]
        name: Option<String>,
        /// What the skill is for. This changes how much of the source survives
        /// into it more than any other choice. (`--depth` is the crawler's,
        /// and means something else entirely.)
        #[arg(long, value_enum, default_value = "working-guide")]
        purpose: build::Purpose,
        /// The model that reads and writes. Defaults to $ANYTHING_TO_SKILL_MODEL.
        #[arg(long)]
        model: Option<String>,
        /// Where to keep the intermediate extraction, so a failed build can be
        /// inspected or resumed by hand.
        #[arg(long)]
        work: Option<PathBuf>,
        /// Extract and print the plan, but make no requests and write no skill.
        #[arg(long)]
        dry_run: bool,
        /// Which reader handles PDFs and Office formats.
        #[arg(long, value_enum, default_value = "builtin")]
        engine: extract::Engine,
        #[command(flatten)]
        web: WebArgs,
        #[command(flatten)]
        repo: RepoArgs,
    },
    /// Ask the source's own questions of the skill, and see which it can answer.
    Eval {
        /// The skill directory to test.
        #[arg(value_name = "SKILL")]
        skill: PathBuf,
        /// The extracted text to test against. Defaults to re-reading the
        /// sources the skill's `.a2s.lock` names.
        #[arg(long, value_name = "FULL_TEXT")]
        against: Option<PathBuf>,
        /// How many questions to ask.
        #[arg(long, default_value_t = 12, value_name = "N")]
        questions: usize,
        /// The model that sets, answers and grades. Not the skill's own model.
        #[arg(long)]
        model: Option<String>,
        /// Print the results as JSON instead of prose.
        #[arg(long)]
        json: bool,
        /// Exit non-zero below this pass rate, as a percentage. For CI.
        #[arg(long, value_name = "PCT")]
        min_pass: Option<u32>,
        /// Where to keep a re-read of the sources, when one is needed.
        #[arg(long)]
        work: Option<PathBuf>,
    },
    /// Re-read a skill's sources and rebuild it if they have moved.
    Refresh {
        /// A skill directory carrying a `.a2s.lock`.
        #[arg(value_name = "SKILL")]
        skill: PathBuf,
        /// Report whether the sources moved and change nothing. Exits non-zero
        /// when they have, so a scheduled job can open a pull request.
        #[arg(long)]
        check: bool,
        /// The model to rebuild with. Defaults to the one the lock records.
        #[arg(long)]
        model: Option<String>,
        /// Where to keep the re-read.
        #[arg(long)]
        work: Option<PathBuf>,
    },
    /// Serve the reading and grading tools over MCP, for agents with no shell.
    Mcp {
        /// Where extractions are kept for the life of the server.
        #[arg(long)]
        work: Option<PathBuf>,
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
    /// Grade a skill, or a directory of them, for load cost and routability.
    Audit {
        /// A skill directory, a `SKILL.md`, or a directory holding many skills.
        /// Defaults to the agent skills directories on this machine.
        #[arg(value_name = "PATH")]
        paths: Vec<PathBuf>,
        /// Token ceiling for a `SKILL.md` body. Past it, the material belongs
        /// in `references/`, where it costs nothing until it is opened.
        #[arg(long, default_value_t = audit::DEFAULT_BODY_BUDGET, value_name = "N")]
        body_budget: usize,
        /// Print the report as JSON instead of prose.
        #[arg(long)]
        json: bool,
        /// Exit non-zero when anything at all is found, not only errors.
        #[arg(long)]
        strict: bool,
    },
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
            engine,
            web,
            repo,
        } => cmd_extract(sources, out, engine, web, repo),
        Commands::Build {
            sources,
            out,
            name,
            purpose,
            model,
            work,
            dry_run,
            engine,
            web,
            repo,
        } => cmd_build(
            sources, out, name, purpose, model, work, dry_run, engine, web, repo,
        ),
        Commands::Eval {
            skill,
            against,
            questions,
            model,
            json,
            min_pass,
            work,
        } => cmd_eval(skill, against, questions, model, json, min_pass, work),
        Commands::Refresh {
            skill,
            check,
            model,
            work,
        } => cmd_refresh(skill, check, model, work),
        Commands::Mcp { work } => {
            let work_dir = resolve_out_dir(work);
            std::fs::create_dir_all(&work_dir)
                .with_context(|| format!("creating {}", work_dir.display()))?;
            mcp::Server::new(work_dir).serve()
        }
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
        Commands::Audit {
            paths,
            body_budget,
            json,
            strict,
        } => cmd_audit(paths, body_budget, json, strict),
    }
}

/// Resolve the output directory: explicit flag, then `$ANYTHING_TO_SKILL_WORKDIR`,
/// then a stable temp path so repeated runs land in the same place.
fn resolve_out_dir(explicit: Option<PathBuf>) -> PathBuf {
    explicit
        .or_else(|| std::env::var_os("ANYTHING_TO_SKILL_WORKDIR").map(PathBuf::from))
        .unwrap_or_else(|| std::env::temp_dir().join("anything_to_skill_work"))
}

impl WebArgs {
    /// Replay the crawl exactly as the build ran it. A refresh that used
    /// today's defaults would report the flags changing as the site changing.
    fn from_lock(options: &build::SourceOptions) -> WebArgs {
        WebArgs {
            crawl: options.crawl,
            max_pages: options.max_pages.max(1),
            depth: options.depth,
            delay_ms: options.delay_ms,
            no_llms_txt: options.no_llms_txt,
        }
    }
}

impl RepoArgs {
    fn from_lock(options: &build::SourceOptions) -> RepoArgs {
        RepoArgs {
            branch: options.branch.clone(),
            max_files: options.max_files.max(1),
            include: options.include.clone(),
            exclude: options.exclude.clone(),
        }
    }
}

fn source_options(web: &WebArgs, repo: &RepoArgs) -> build::SourceOptions {
    build::SourceOptions {
        crawl: web.crawl,
        max_pages: web.max_pages,
        depth: web.depth,
        delay_ms: web.delay_ms,
        no_llms_txt: web.no_llms_txt,
        branch: repo.branch.clone(),
        max_files: repo.max_files,
        include: repo.include.clone(),
        exclude: repo.exclude.clone(),
    }
}

/// The result of reading every source: the text, and the report about it.
struct Extracted {
    text: String,
    report: RunReport,
    text_path: PathBuf,
    meta_path: PathBuf,
}

/// Read every source into `out_dir`, writing `full_text.txt` and
/// `metadata.json`. Shared by `extract`, which stops here, and `build`, which
/// carries on into the writing.
fn run_extraction(
    sources: Vec<String>,
    out_dir: &Path,
    web: WebArgs,
    repo: RepoArgs,
    engine: extract::Engine,
) -> Result<Extracted> {
    std::fs::create_dir_all(out_dir)
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

        match extract::extract_doc(doc, engine) {
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

    Ok(Extracted {
        text: combined,
        report,
        text_path,
        meta_path,
    })
}

fn cmd_extract(
    sources: Vec<String>,
    out: Option<PathBuf>,
    engine: extract::Engine,
    web: WebArgs,
    repo: RepoArgs,
) -> Result<()> {
    let out_dir = resolve_out_dir(out);
    let extracted = run_extraction(sources, &out_dir, web, repo, engine)?;
    extracted
        .report
        .print_summary(&extracted.text_path, &extracted.meta_path);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cmd_build(
    sources: Vec<String>,
    out: PathBuf,
    name: Option<String>,
    purpose: build::Purpose,
    model: Option<String>,
    work: Option<PathBuf>,
    dry_run: bool,
    engine: extract::Engine,
    web: WebArgs,
    repo: RepoArgs,
) -> Result<()> {
    // The key is checked before a single page is fetched. Reading a site for
    // ten minutes and only then finding there is no way to write the skill
    // wastes the user's time and somebody else's bandwidth.
    let client = if dry_run {
        None
    } else {
        Some(llm::Client::from_env(model)?)
    };

    let work_dir = resolve_out_dir(work);
    let options = build::Options {
        name,
        purpose,
        out: expand_tilde(&out),
        dry_run,
        source_options: source_options(&web, &repo),
    };
    let inputs = sources.clone();
    let extracted = run_extraction(sources, &work_dir, web, repo, engine)?;
    extracted
        .report
        .print_summary(&extracted.text_path, &extracted.meta_path);

    let skill_dir = build::build(
        client.as_ref(),
        &extracted.text,
        &extracted.report,
        &options,
        &inputs,
    )?;
    let Some(client) = client else {
        return Ok(());
    };
    build::report_result(&skill_dir, &client)
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
    let tools: [(&str, &str, &str); 5] = [
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
        (
            "docling",
            "docling",
            "optional stronger table model, via --engine docling",
        ),
    ];
    let mut missing_poppler = false;
    let mut missing_calibre = false;
    let mut missing_git = false;
    let mut missing_docling = false;
    for (bin, pkg, why) in tools {
        if extract::which(bin).is_some() {
            println!("  ✓ {bin:<14} ({pkg}) — {why}");
        } else {
            println!("  ✗ {bin:<14} ({pkg}) — {why}");
            match pkg {
                "poppler" => missing_poppler = true,
                "git" => missing_git = true,
                "Calibre" => missing_calibre = true,
                _ => missing_docling = true,
            }
        }
    }
    if missing_poppler || missing_calibre || missing_git || missing_docling {
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
        if missing_docling {
            println!("  docling: pip install docling");
        }
        println!(
            "\nNothing here blocks a normal PDF, EPUB, URL or site — poppler improves\n\
             table fidelity and page rendering, git is only needed for repository\n\
             sources, Calibre only for Kindle formats, and docling only when you\n\
             ask for it with --engine docling."
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

/// Where skills live when the user did not say. These are the directories the
/// agents themselves read, so auditing them is auditing what actually loads.
fn default_skill_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from(".claude/skills"),
        PathBuf::from(".agents/skills"),
    ];
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        roots.push(home.join(".claude/skills"));
        roots.push(home.join(".agents/skills"));
    }
    roots.retain(|p| p.is_dir());
    roots
}

fn cmd_audit(paths: Vec<PathBuf>, body_budget: usize, json: bool, strict: bool) -> Result<()> {
    let roots = if paths.is_empty() {
        let found = default_skill_roots();
        if found.is_empty() {
            bail!(
                "no skills directory found. Name one:\n  \
                 anything-to-skill audit ./my-skill\n  \
                 anything-to-skill audit ~/.claude/skills"
            );
        }
        for root in &found {
            eprintln!("auditing {}", root.display());
        }
        found
    } else {
        paths.into_iter().map(|p| expand_tilde(&p)).collect()
    };

    // The recommended install symlinks one skill into several agent
    // directories, so auditing the defaults sees the same file more than once.
    // Counting it twice would inflate the session cost and invent a duplicate
    // name for every skill on the machine.
    let mut seen = std::collections::BTreeSet::new();
    let mut skills = Vec::new();
    for root in &roots {
        for found in skill::discover(root)? {
            let identity =
                std::fs::canonicalize(&found.path).unwrap_or_else(|_| found.path.clone());
            if seen.insert(identity) {
                skills.push(found);
            }
        }
    }

    let report = audit::audit(&skills, body_budget);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        audit::print(&report, body_budget);
    }

    // A non-zero exit is what makes this usable in CI. Errors always fail;
    // warnings only under --strict, so a tidy-up can be adopted gradually.
    if report.has_errors() || (strict && report.warnings > 0) {
        std::process::exit(1);
    }
    Ok(())
}

/// Load the one skill at `path`, refusing a directory holding several — every
/// command here acts on a single skill and guessing which is not acceptable.
fn one_skill(path: &Path) -> Result<skill::Skill> {
    let mut found = skill::discover(path)?;
    if found.len() > 1 {
        bail!(
            "{} holds {} skills — name one of them",
            path.display(),
            found.len()
        );
    }
    Ok(found.remove(0))
}

/// Read the sources a lock names, the way the lock says they were read.
fn re_extract_from_lock(lock: &build::Lock, work_dir: &Path) -> Result<Extracted> {
    if lock.source_inputs.is_empty() {
        bail!(
            "the lock records no sources to re-read — it was written by an \
             older version. Rebuild the skill with `build` to record them."
        );
    }
    eprintln!("re-reading {} ...", lock.source_inputs.join(", "));
    run_extraction(
        lock.source_inputs.clone(),
        work_dir,
        WebArgs::from_lock(&lock.source_options),
        RepoArgs::from_lock(&lock.source_options),
        extract::Engine::Builtin,
    )
}

#[allow(clippy::too_many_arguments)]
fn cmd_eval(
    skill_path: PathBuf,
    against: Option<PathBuf>,
    questions: usize,
    model: Option<String>,
    json: bool,
    min_pass: Option<u32>,
    work: Option<PathBuf>,
) -> Result<()> {
    if questions == 0 {
        bail!("--questions 0 would test nothing");
    }
    let skill_path = expand_tilde(&skill_path);
    let target = one_skill(&skill_path)?;
    let client = llm::Client::from_env(model)?;

    let source_text = match against {
        Some(path) => {
            let path = expand_tilde(&path);
            std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?
        }
        None => {
            let lock = build::Lock::load(&target.dir)?;
            re_extract_from_lock(&lock, &resolve_out_dir(work))?.text
        }
    };
    if source_text.trim().is_empty() {
        bail!("the source text is empty — there is nothing to ask about");
    }

    let report = eval::run(&client, &target, &source_text, questions)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        eval::print(&report);
    }

    if let Some(minimum) = min_pass
        && (report.pass_rate() * 100.0) < f64::from(minimum)
    {
        eprintln!(
            "\nbelow the {minimum}% required: {:.0}%",
            report.pass_rate() * 100.0
        );
        std::process::exit(1);
    }
    Ok(())
}

fn cmd_refresh(
    skill_path: PathBuf,
    check: bool,
    model: Option<String>,
    work: Option<PathBuf>,
) -> Result<()> {
    let skill_path = expand_tilde(&skill_path);
    let target = one_skill(&skill_path)?;
    let lock = build::Lock::load(&target.dir)?;

    // The key is checked first when a rebuild may follow, so a ten-minute
    // re-crawl is not thrown away at the last step.
    let client = if check {
        None
    } else {
        Some(llm::Client::from_env(
            model.or_else(|| Some(lock.model.clone())),
        )?)
    };

    let extracted = re_extract_from_lock(&lock, &resolve_out_dir(work))?;
    let changes = build::diff(&lock, &extracted.report);
    changes.print(&lock);

    if !changes.moved() {
        return Ok(());
    }
    if check {
        // Non-zero on "the source moved" is the point: it is what makes this
        // usable as a scheduled job that opens a pull request.
        std::process::exit(1);
    }

    let client = client.context("a model is required to rebuild")?;
    let parent = target
        .dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let options = build::Options {
        name: Some(target.dir_name()),
        purpose: build::Purpose::from_lock(&lock.purpose)?,
        out: parent,
        dry_run: false,
        source_options: lock.source_options.clone(),
    };

    // Reference files from the previous build are not carried over: the new
    // one names its own, and a leftover file nothing links to is exactly what
    // `audit` flags as dead weight.
    let references = target.dir.join("references");
    if references.is_dir() {
        eprintln!("  replacing {}", references.display());
        std::fs::remove_dir_all(&references)
            .with_context(|| format!("removing {}", references.display()))?;
    }

    let skill_dir = build::build(
        Some(&client),
        &extracted.text,
        &extracted.report,
        &options,
        &lock.source_inputs,
    )?;
    changes.write_changelog(&skill_dir, client.model())?;
    build::report_result(&skill_dir, &client)
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
