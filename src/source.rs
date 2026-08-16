//! What you can point this tool at, and how each of those becomes documents.
//!
//! One string comes in — a path, a URL, a repository — and a list of documents
//! comes out. Everything downstream works on documents and never has to know
//! which kind of source produced them.

use crate::net::Fetched;
use crate::repo::{RepoOptions, RepoSpec};
use crate::url::Url;
use crate::web::{self, CrawlOptions};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// One thing to extract text from.
pub struct Doc {
    /// What to call it in the report — a path, a URL, or `owner/repo:file`.
    pub label: String,
    /// Where it came from, when that is not the label itself. A PDF fetched
    /// over HTTP has a local path as its label and the URL as its origin.
    pub origin: Option<String>,
    pub payload: Payload,
}

pub enum Payload {
    /// A file on disk, extracted by its extension.
    File(PathBuf),
    /// Text already extracted, as web pages are.
    Text { text: String, method: &'static str },
}

impl Doc {
    fn file(path: PathBuf) -> Doc {
        Doc {
            label: path.display().to_string(),
            origin: None,
            payload: Payload::File(path),
        }
    }

    /// The local path, when there is one. Only a real file can be re-opened
    /// later — to render its pages, say — so the report has to know.
    pub fn local_path(&self) -> Option<&Path> {
        match &self.payload {
            Payload::File(p) => Some(p),
            Payload::Text { .. } => None,
        }
    }
}

/// What one source string turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    File(PathBuf),
    Web(Url),
    Repo(RepoSpecLite),
}

/// The parsed form of a repository reference, before any options are applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSpecLite {
    pub url: String,
    pub branch: Option<String>,
    pub subdir: Option<String>,
    pub label: String,
}

impl RepoSpecLite {
    fn github(owner: &str, name: &str) -> RepoSpecLite {
        RepoSpecLite {
            url: format!("https://github.com/{owner}/{name}.git"),
            branch: None,
            subdir: None,
            label: format!("{owner}/{name}"),
        }
    }
}

/// How a run of `extract` should treat the sources it was given.
pub struct Options {
    pub crawl: bool,
    pub crawl_options: CrawlOptions,
    pub repo_options: RepoOptions,
    /// Overrides any branch named in the source string itself.
    pub branch: Option<String>,
    /// Where fetched non-HTML documents are written.
    pub download_dir: PathBuf,
}

/// What one source contributed, for the run report.
#[derive(Debug, serde::Serialize)]
pub struct SourceSummary {
    pub source: String,
    pub kind: &'static str,
    pub documents: usize,
    /// Anything the user should know about how complete this is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

pub struct Resolved {
    pub docs: Vec<Doc>,
    pub summaries: Vec<SourceSummary>,
    pub failures: Vec<String>,
}

/// Turn every source string into documents, reporting rather than aborting when
/// one of them fails — three good sources and one dead link is still work worth
/// finishing.
pub fn resolve(sources: &[String], opts: &Options) -> Resolved {
    let mut docs = Vec::new();
    let mut summaries = Vec::new();
    let mut failures = Vec::new();
    // Built once, so a crawl reuses connections instead of reopening TLS per page.
    let mut agent: Option<ureq::Agent> = None;

    for raw in sources {
        let kind = match classify(raw) {
            Ok(k) => k,
            Err(err) => {
                failures.push(format!("{raw}: {err:#}"));
                continue;
            }
        };
        let agent = agent.get_or_insert_with(crate::net::agent);

        let result = match kind {
            Kind::File(path) => resolve_file(path),
            Kind::Web(url) => resolve_web(agent, &url, opts),
            Kind::Repo(spec) => resolve_repo(&spec, opts),
        };

        match result {
            Ok((mut new_docs, summary)) => {
                docs.append(&mut new_docs);
                summaries.push(SourceSummary {
                    source: raw.clone(),
                    ..summary
                });
            }
            Err(err) => failures.push(format!("{raw}: {err:#}")),
        }
    }

    Resolved {
        docs,
        summaries,
        failures,
    }
}

fn resolve_file(path: PathBuf) -> Result<(Vec<Doc>, SourceSummary)> {
    let path = expand_tilde(&path);
    if !path.is_file() {
        bail!("not a file");
    }
    Ok((
        vec![Doc::file(path)],
        SourceSummary {
            source: String::new(),
            kind: "file",
            documents: 1,
            notes: Vec::new(),
        },
    ))
}

fn resolve_web(
    agent: &ureq::Agent,
    url: &Url,
    opts: &Options,
) -> Result<(Vec<Doc>, SourceSummary)> {
    let mut notes = Vec::new();
    let pages: Vec<Fetched> = if opts.crawl {
        eprintln!("crawling {url} ...");
        let crawl = web::crawl(agent, url, &opts.crawl_options)?;
        if let Some(source) = &crawl.stats.llms_txt {
            notes.push(format!(
                "the site publishes its documentation as one file, so {source} was read \
                 instead of crawling its pages; pass --no-llms-txt to crawl anyway"
            ));
        }
        if crawl.stats.skipped_by_robots > 0 {
            notes.push(format!(
                "{} page(s) skipped because robots.txt disallows them",
                crawl.stats.skipped_by_robots
            ));
        }
        // A crawl that read almost nothing while seeing plenty of pages next
        // door was aimed one directory too deep. Saying so is the difference
        // between a one-page skill and a re-run that works.
        if crawl.stats.outside_prefix > 0 && crawl.pages.len() < opts.crawl_options.max_pages {
            let parent = web::parent_of(&url.dir())
                .map(|p| format!("{}{p}", url.origin()))
                .unwrap_or_else(|| url.origin());
            notes.push(format!(
                "{} page(s) on the same site were not read because they sit outside {}; \
                 start from {parent} to include them",
                crawl.stats.outside_prefix,
                url.dir()
            ));
        }
        if crawl.stats.hit_page_limit {
            notes.push(format!(
                "stopped at the --max-pages limit of {}; the site has more pages than were read",
                opts.crawl_options.max_pages
            ));
        }
        for err in &crawl.stats.errors {
            notes.push(format!("could not read {err}"));
        }
        crawl.pages
    } else {
        eprintln!("fetching {url} ...");
        vec![web::fetch_one(agent, url)?]
    };

    let mut docs = Vec::new();
    for page in pages {
        if page.is_html() {
            docs.push(Doc {
                label: page.url.to_string(),
                origin: None,
                payload: Payload::Text {
                    text: web::page_text(&page),
                    method: "web-page",
                },
            });
        } else {
            // A PDF or DOCX behind a URL is still that format; save it and let
            // the file extractors do what they already do well.
            let path = web::materialize(&page, &opts.download_dir)?;
            docs.push(Doc {
                label: path.display().to_string(),
                origin: Some(page.url.to_string()),
                payload: Payload::File(path),
            });
        }
    }

    let summary = SourceSummary {
        source: String::new(),
        kind: if opts.crawl { "site" } else { "page" },
        documents: docs.len(),
        notes,
    };
    Ok((docs, summary))
}

fn resolve_repo(spec: &RepoSpecLite, opts: &Options) -> Result<(Vec<Doc>, SourceSummary)> {
    let spec = RepoSpec {
        url: spec.url.clone(),
        branch: opts.branch.clone().or_else(|| spec.branch.clone()),
        subdir: spec.subdir.clone(),
        label: spec.label.clone(),
    };
    let checkout = crate::repo::checkout(&spec, &opts.repo_options)?;

    let mut notes = Vec::new();
    if checkout.dropped > 0 {
        notes.push(format!(
            "{} more file(s) matched but were dropped by --max-files {}",
            checkout.dropped, opts.repo_options.max_files
        ));
    }
    notes.push(format!("checkout kept at {}", checkout.root.display()));

    let docs: Vec<Doc> = checkout
        .files
        .iter()
        .map(|path| Doc {
            label: format!("{}:{}", spec.label, checkout.label(path)),
            origin: Some(spec.url.clone()),
            payload: Payload::File(path.clone()),
        })
        .collect();

    let summary = SourceSummary {
        source: String::new(),
        kind: "repo",
        documents: docs.len(),
        notes,
    };
    Ok((docs, summary))
}

/// Work out what a source string refers to.
///
/// An existing path always wins: a local directory called `docs/guide` should
/// never be mistaken for a repository because it happens to have a slash in it.
pub fn classify(raw: &str) -> Result<Kind> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("empty source");
    }

    let as_path = expand_tilde(Path::new(raw));
    if as_path.exists() {
        return Ok(Kind::File(as_path));
    }

    if raw.starts_with("http://") || raw.starts_with("https://") {
        let url = Url::parse(raw).context("parsing URL")?;
        return Ok(classify_url(url));
    }
    if let Some(rest) = raw.strip_prefix("gh:") {
        return github_shorthand(rest);
    }
    if raw.starts_with("git@") || raw.ends_with(".git") {
        let label = raw
            .rsplit(['/', ':'])
            .next()
            .unwrap_or(raw)
            .trim_end_matches(".git")
            .to_string();
        return Ok(Kind::Repo(RepoSpecLite {
            url: raw.to_string(),
            branch: None,
            subdir: None,
            label,
        }));
    }
    // `owner/repo`, the shorthand every git host understands.
    if looks_like_shorthand(raw) {
        return github_shorthand(raw);
    }

    // Nothing else matched: treat it as the file it was probably meant to be,
    // so the error names the path rather than guessing at intent.
    Ok(Kind::File(as_path))
}

fn classify_url(url: Url) -> Kind {
    let segments: Vec<&str> = url.path.split('/').filter(|s| !s.is_empty()).collect();

    if url.host == "github.com" || url.host == "www.github.com" {
        match segments.as_slice() {
            [owner, name] => return Kind::Repo(RepoSpecLite::github(owner, &strip_git(name))),
            [owner, name, "tree", rest @ ..] if !rest.is_empty() => {
                let mut spec = RepoSpecLite::github(owner, &strip_git(name));
                spec.branch = Some(rest[0].to_string());
                if rest.len() > 1 {
                    spec.subdir = Some(rest[1..].join("/"));
                }
                return Kind::Repo(spec);
            }
            [owner, name, "blob", rest @ ..] if rest.len() > 1 => {
                // One file in a repository: fetch it raw rather than cloning.
                let raw = format!(
                    "https://raw.githubusercontent.com/{owner}/{}/{}",
                    strip_git(name),
                    rest.join("/")
                );
                if let Ok(u) = Url::parse(&raw) {
                    return Kind::Web(u);
                }
            }
            _ => {}
        }
    }
    if url.host == "gitlab.com"
        && let [owner, name] = segments.as_slice()
    {
        return Kind::Repo(RepoSpecLite {
            url: format!("https://gitlab.com/{owner}/{}.git", strip_git(name)),
            branch: None,
            subdir: None,
            label: format!("{owner}/{}", strip_git(name)),
        });
    }

    Kind::Web(url)
}

fn strip_git(name: &str) -> String {
    name.trim_end_matches(".git").to_string()
}

fn github_shorthand(rest: &str) -> Result<Kind> {
    let (path, branch) = match rest.split_once('@') {
        Some((p, b)) if !b.is_empty() => (p, Some(b.to_string())),
        _ => (rest, None),
    };
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let ([owner, name], subdir) = (
        match parts.as_slice() {
            [owner, name, ..] => [*owner, *name],
            _ => bail!("expected owner/repo, got '{rest}'"),
        },
        (parts.len() > 2).then(|| parts[2..].join("/")),
    );

    let mut spec = RepoSpecLite::github(owner, &strip_git(name));
    spec.branch = branch;
    spec.subdir = subdir;
    Ok(Kind::Repo(spec))
}

/// `owner/repo` — exactly one slash, no dots in the owner, no file extension.
/// Deliberately narrow: anything ambiguous is better read as a path, because a
/// wrong path produces a clear error and a wrong clone produces a surprise.
fn looks_like_shorthand(raw: &str) -> bool {
    let parts: Vec<&str> = raw.split('/').collect();
    if parts.len() != 2 || parts.iter().any(|p| p.is_empty()) {
        return false;
    }
    let (owner, name) = (parts[0], parts[1].split('@').next().unwrap_or(parts[1]));
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    };
    ok(owner) && ok(name) && !owner.contains('.') && !name.contains('.')
}

/// Expand a leading `~` so paths pasted from a shell prompt work.
pub fn expand_tilde(path: &Path) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(rest) = s.strip_prefix('~') else {
        return path.to_path_buf();
    };
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        return path.to_path_buf();
    };
    PathBuf::from(home).join(rest.trim_start_matches(['/', '\\']))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(kind: Kind) -> RepoSpecLite {
        match kind {
            Kind::Repo(s) => s,
            other => panic!("expected a repository, got {other:?}"),
        }
    }

    #[test]
    fn a_github_url_is_a_repository() {
        let s = repo(classify("https://github.com/rust-lang/book").unwrap());
        assert_eq!(s.url, "https://github.com/rust-lang/book.git");
        assert_eq!(s.label, "rust-lang/book");
        assert_eq!(s.branch, None);
    }

    #[test]
    fn a_tree_url_carries_its_branch_and_directory() {
        let s = repo(classify("https://github.com/rust-lang/book/tree/main/src/ch01").unwrap());
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.subdir.as_deref(), Some("src/ch01"));
    }

    #[test]
    fn a_blob_url_is_one_file_fetched_raw() {
        match classify("https://github.com/rust-lang/book/blob/main/README.md").unwrap() {
            Kind::Web(u) => assert_eq!(
                u.to_string(),
                "https://raw.githubusercontent.com/rust-lang/book/main/README.md"
            ),
            other => panic!("expected a web fetch, got {other:?}"),
        }
    }

    #[test]
    fn a_github_page_that_is_not_a_repository_stays_a_page() {
        assert!(matches!(
            classify("https://github.com/rust-lang/book/issues/1").unwrap(),
            Kind::Web(_)
        ));
    }

    #[test]
    fn shorthand_and_branches() {
        let s = repo(classify("gh:rust-lang/book@dev").unwrap());
        assert_eq!(s.label, "rust-lang/book");
        assert_eq!(s.branch.as_deref(), Some("dev"));

        let bare = repo(classify("rust-lang/book").unwrap());
        assert_eq!(bare.url, "https://github.com/rust-lang/book.git");
    }

    #[test]
    fn an_ssh_remote_is_a_repository() {
        let s = repo(classify("git@github.com:rust-lang/book.git").unwrap());
        assert_eq!(s.url, "git@github.com:rust-lang/book.git");
        assert_eq!(s.label, "book");
    }

    #[test]
    fn a_path_shaped_string_is_never_mistaken_for_a_repository() {
        // A relative path to a file that does not exist yet is still a path.
        assert!(matches!(
            classify("samples/book.pdf").unwrap(),
            Kind::File(_)
        ));
        assert!(matches!(classify("./docs/intro").unwrap(), Kind::File(_)));
        assert!(matches!(classify("/tmp/a/b").unwrap(), Kind::File(_)));
    }

    #[test]
    fn an_existing_path_wins_over_every_other_reading() {
        let dir = std::env::temp_dir().join("a2s-classify-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("owner-repo.md");
        std::fs::write(&file, "x").unwrap();
        assert!(matches!(
            classify(file.to_str().unwrap()).unwrap(),
            Kind::File(_)
        ));
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_plain_docs_url_is_a_page() {
        match classify("https://docs.example.com/guide/").unwrap() {
            Kind::Web(u) => assert_eq!(u.host, "docs.example.com"),
            other => panic!("expected a page, got {other:?}"),
        }
    }
}
