//! Crawling a documentation site.
//!
//! The rules are deliberately tight: same origin as the URL you named, at or
//! below its directory, bounded page count and depth, one request at a time
//! with a pause between them. A documentation site is a bounded thing and
//! should be read like one — the failure mode of a loose crawler is not a
//! missing page, it is three thousand pages of changelog in your skill.

use crate::net::{self, Fetched, Robots};
use crate::url::Url;
use anyhow::{Context, Result, bail};
use std::collections::{HashSet, VecDeque};
use std::time::Duration;

/// Extensions that are never a page worth reading. Documents the crawler *does*
/// want — `.pdf`, `.md`, `.txt` — are deliberately absent.
const NOT_A_PAGE: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "bmp", "avif", "mp4", "webm", "mov", "mp3",
    "wav", "ogg", "css", "js", "mjs", "map", "zip", "gz", "tgz", "bz2", "xz", "tar", "7z", "rar",
    "woff", "woff2", "ttf", "otf", "eot", "exe", "dmg", "deb", "rpm", "msi", "iso", "wasm", "sig",
    "asc",
];

/// How many nested sitemaps a sitemap index is followed into.
const MAX_SITEMAPS: usize = 5;

#[derive(Debug, Clone)]
pub struct CrawlOptions {
    pub max_pages: usize,
    pub max_depth: usize,
    pub delay: Duration,
    /// Whether to prefer a site's own `llms.txt` over crawling it.
    pub use_llms_txt: bool,
}

#[derive(Debug, Default)]
pub struct CrawlStats {
    pub pages_fetched: usize,
    pub skipped_by_robots: usize,
    /// Set when the site's own text file was read instead of its pages.
    pub llms_txt: Option<String>,
    /// Pages that were reached but could not be read, with the reason.
    pub errors: Vec<String>,
    /// Links on the site that were not followed because they sit outside the
    /// starting directory. A high count on a short crawl means the URL was
    /// aimed one level too deep.
    pub outside_prefix: usize,
    /// True when the crawl stopped because it hit `max_pages`, not because it
    /// ran out of links — the difference between "this is the site" and "this
    /// is the first 50 pages of it", which the report must not blur.
    pub hit_page_limit: bool,
}

pub struct Crawl {
    pub pages: Vec<Fetched>,
    pub stats: CrawlStats,
}

/// Fetch one URL, with no crawling.
///
/// `robots.txt` is not consulted here, and that is deliberate: it governs
/// automated discovery, and a single URL the user typed is not discovery — it
/// is the same request their browser would make. `wget` draws the line in the
/// same place, consulting robots only when it recurses. The crawler below does
/// consult it, because that is discovery.
pub fn fetch_one(agent: &ureq::Agent, url: &Url) -> Result<Fetched> {
    net::fetch(agent, url)
}

/// Walk a documentation site from `start`, staying inside its directory.
pub fn crawl(agent: &ureq::Agent, start: &Url, opts: &CrawlOptions) -> Result<Crawl> {
    let robots = Robots::fetch(start);
    let mut stats = CrawlStats::default();

    if !robots.allows(&start.path) {
        bail!(
            "{start} is disallowed by that site's robots.txt — nothing was fetched.\n\
             If you have permission to read it, download the pages yourself and pass\n\
             the files instead."
        );
    }

    let prefix = start.dir();

    // A site that publishes its documentation as one file has already done this
    // job, better: that text is curated, complete, and one request instead of
    // fifty. Only at the site root, though — asked for `/guide/`, handing back
    // the whole site is not the same answer.
    if opts.use_llms_txt
        && start.path == "/"
        && let Some(found) = llms_full(start)
    {
        eprintln!("  {} — reading it instead of crawling", found.url);
        stats.pages_fetched = 1;
        stats.llms_txt = Some(found.url.to_string());
        return Ok(Crawl {
            pages: vec![found],
            stats,
        });
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(Url, usize)> = VecDeque::new();

    seen.insert(start.to_string());
    queue.push_back((start.clone(), 0));

    let mut seeds = sitemap_seeds(start, &prefix);
    if opts.use_llms_txt {
        seeds.extend(llms_index_seeds(start, &prefix));
    }
    for url in seeds {
        if seen.insert(url.to_string()) {
            queue.push_back((url, 1));
        }
    }
    if queue.len() > 1 {
        eprintln!("  sitemap: {} more page(s) to consider", queue.len() - 1);
    }

    let mut pages: Vec<Fetched> = Vec::new();
    // Pages on the same site but outside the starting directory, counted once
    // each so the report can say the crawl was aimed too narrowly.
    let mut elsewhere: HashSet<String> = HashSet::new();

    while let Some((url, depth)) = queue.pop_front() {
        if pages.len() >= opts.max_pages {
            stats.hit_page_limit = !queue.is_empty();
            break;
        }
        if !robots.allows(&url.path) {
            stats.skipped_by_robots += 1;
            continue;
        }
        if !pages.is_empty() {
            std::thread::sleep(opts.delay);
        }

        eprintln!("  [{}/{}] {url}", pages.len() + 1, opts.max_pages);
        let fetched = match net::fetch(agent, &url) {
            Ok(f) => f,
            Err(err) => {
                stats.errors.push(format!("{url}: {err:#}"));
                continue;
            }
        };

        if fetched.is_html() && depth < opts.max_depth {
            let body = fetched.text();
            for href in crate::html::links(&body) {
                let Some(next) = fetched.url.join(&href) else {
                    continue;
                };
                let next = drop_decorative_query(next);
                if !worth_following(&next, start, &prefix) {
                    if next.same_origin(start)
                        && !next.path.starts_with(&prefix)
                        && looks_like_a_page(&next)
                        && elsewhere.insert(next.to_string())
                    {
                        stats.outside_prefix += 1;
                    }
                    continue;
                }
                if seen.insert(next.to_string()) {
                    queue.push_back((next, depth + 1));
                }
            }
        }

        pages.push(fetched);
    }

    stats.pages_fetched = pages.len();
    if pages.is_empty() {
        bail!(
            "nothing could be fetched from {start}{}",
            if stats.errors.is_empty() {
                String::new()
            } else {
                format!("\n  {}", stats.errors.join("\n  "))
            }
        );
    }
    Ok(Crawl { pages, stats })
}

/// Same origin, at or below the starting directory, and not obviously an asset.
fn worth_following(candidate: &Url, start: &Url, prefix: &str) -> bool {
    candidate.same_origin(start)
        && candidate.path.starts_with(prefix)
        && looks_like_a_page(candidate)
}

fn looks_like_a_page(candidate: &Url) -> bool {
    match candidate.extension() {
        Some(ext) => !NOT_A_PAGE.contains(&ext.as_str()),
        None => true,
    }
}

/// The directory one level above a crawl's prefix — what to suggest when the
/// starting URL was aimed too deep to reach the rest of the documentation.
pub fn parent_of(prefix: &str) -> Option<String> {
    let trimmed = prefix.trim_end_matches('/');
    let cut = trimmed.rfind('/')?;
    Some(trimmed[..=cut].to_string())
}

/// Query strings on a static page are almost always decoration — a highlight
/// term, an analytics tag — and following each variant fetches one page many
/// times. On anything else the query may well be the page's identity, so it
/// stays.
fn drop_decorative_query(mut url: Url) -> Url {
    let static_page = url.path.ends_with('/')
        || matches!(
            url.extension().as_deref(),
            Some("html" | "htm" | "xhtml" | "md" | "txt" | "pdf")
        );
    if static_page {
        url.query = None;
    }
    url
}

/// The site's whole documentation as one plain-text file, if it publishes one.
///
/// `llms-full.txt` is the convention for exactly this: the complete text, meant
/// to be read by something like this tool. `llms.txt` is an index of links
/// rather than content, so it is handled separately.
fn llms_full(start: &Url) -> Option<Fetched> {
    let agent = net::probe_agent();
    for name in ["llms-full.txt", "llms_full.txt"] {
        let Ok(url) = Url::parse(&format!("{}/{name}", start.origin())) else {
            continue;
        };
        let Ok(fetched) = net::fetch(&agent, &url) else {
            continue;
        };
        if is_plain_text(&fetched) {
            return Some(fetched);
        }
    }
    None
}

/// `llms.txt` lists a site's pages as Markdown links. Where a sitemap says what
/// exists, this says what the site considers worth reading — a better seed.
fn llms_index_seeds(start: &Url, prefix: &str) -> Vec<Url> {
    let agent = net::probe_agent();
    let Ok(url) = Url::parse(&format!("{}/llms.txt", start.origin())) else {
        return Vec::new();
    };
    let Ok(fetched) = net::fetch(&agent, &url) else {
        return Vec::new();
    };
    if !is_plain_text(&fetched) {
        return Vec::new();
    }
    static LINK: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\]\(([^)\s]+)").unwrap());

    let body = fetched.text();
    let seeds: Vec<Url> = LINK
        .captures_iter(&body)
        .filter_map(|c| fetched.url.join(&c[1]))
        .filter(|u| u.same_origin(start) && u.path.starts_with(prefix))
        .collect();
    if !seeds.is_empty() {
        eprintln!("  llms.txt: {} page(s) the site points at", seeds.len());
    }
    seeds
}

/// Guard against a site that answers every unknown path with its 200-status
/// HTML error page — which would otherwise be read as the whole documentation.
fn is_plain_text(fetched: &Fetched) -> bool {
    let declared_text = fetched
        .content_type
        .as_deref()
        .is_none_or(|ct| ct.contains("text/plain") || ct.contains("markdown"));
    let head = fetched
        .text()
        .chars()
        .take(200)
        .collect::<String>()
        .to_ascii_lowercase();
    let looks_like_html = head.trim_start().starts_with("<!doctype")
        || head.trim_start().starts_with("<html")
        || head.contains("<head>");
    declared_text && !looks_like_html && !fetched.bytes.is_empty()
}

/// Seed the queue from `sitemap.xml` when the site publishes one. A sitemap is
/// the site telling you what it considers its pages, which beats guessing from
/// whatever happens to be linked off the first page.
fn sitemap_seeds(start: &Url, prefix: &str) -> Vec<Url> {
    let agent = net::probe_agent();
    let Ok(root) = Url::parse(&format!("{}/sitemap.xml", start.origin())) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut pending = vec![root];
    let mut fetched_sitemaps = 0usize;

    while let Some(url) = pending.pop() {
        if fetched_sitemaps >= MAX_SITEMAPS {
            break;
        }
        let Ok(body) = net::fetch(&agent, &url) else {
            continue;
        };
        fetched_sitemaps += 1;
        let text = body.text();
        let is_index = text.contains("<sitemapindex");
        for loc in locations(&text) {
            let Ok(found) = Url::parse(&loc) else {
                continue;
            };
            if is_index {
                if found.same_origin(start) {
                    pending.push(found);
                }
            } else if found.same_origin(start) && found.path.starts_with(prefix) {
                out.push(found);
            }
        }
    }
    out
}

fn locations(xml: &str) -> Vec<String> {
    static LOC: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?is)<loc>\s*(.*?)\s*</loc>").unwrap());
    LOC.captures_iter(xml)
        .map(|c| c[1].trim().to_string())
        .collect()
}

/// Turn a fetched page into text, with a header naming where it came from.
///
/// The header is not decoration: a crawl concatenates dozens of pages into one
/// file, and without it neither a reader nor a model can tell which claim came
/// from which page.
pub fn page_text(fetched: &Fetched) -> String {
    let raw = fetched.text();
    let body = crate::html::main_content(&raw);
    let heading = crate::html::title(&raw).unwrap_or_else(|| fetched.url.path.clone());
    format!("# {heading}\n\nsource: {}\n\n{body}", fetched.url)
}

/// Write a fetched non-HTML document to disk so the file extractors can read it.
///
/// The file is kept rather than deleted: if it is a PDF with unreadable pages,
/// the report has to be able to point `render` at something that still exists.
pub fn materialize(fetched: &Fetched, dir: &std::path::Path) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("creating download directory {}", dir.display()))?;
    let stem = {
        let raw = fetched.url.last_segment();
        let cleaned: String = raw
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let cleaned = cleaned.trim_matches('_').to_string();
        if cleaned.is_empty() {
            "download".to_string()
        } else {
            cleaned
        }
    };
    let ext = fetched.extension();
    let base = stem
        .strip_suffix(&format!(".{ext}"))
        .unwrap_or(&stem)
        .to_string();

    let mut path = dir.join(format!("{base}.{ext}"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{base}-{n}.{ext}"));
        n += 1;
    }
    std::fs::write(&path, &fetched.bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn a_crawl_stays_on_its_own_site() {
        let start = url("https://docs.example.com/guide/intro");
        let prefix = start.dir();
        assert!(worth_following(
            &url("https://docs.example.com/guide/setup"),
            &start,
            &prefix
        ));
        assert!(!worth_following(
            &url("https://evil.example.com/guide/x"),
            &start,
            &prefix
        ));
        assert!(!worth_following(
            &url("http://docs.example.com/guide/x"),
            &start,
            &prefix
        ));
    }

    #[test]
    fn a_crawl_stays_under_the_directory_it_started_in() {
        let start = url("https://example.com/docs/intro");
        let prefix = start.dir();
        assert!(worth_following(
            &url("https://example.com/docs/api"),
            &start,
            &prefix
        ));
        assert!(!worth_following(
            &url("https://example.com/blog/hello"),
            &start,
            &prefix
        ));
        assert!(!worth_following(
            &url("https://example.com/"),
            &start,
            &prefix
        ));
    }

    #[test]
    fn the_parent_of_a_prefix_is_what_a_too_narrow_crawl_should_retry() {
        assert_eq!(parent_of("/getting-started/").as_deref(), Some("/"));
        assert_eq!(parent_of("/docs/guide/").as_deref(), Some("/docs/"));
        assert_eq!(parent_of("/"), None);
    }

    #[test]
    fn assets_are_not_pages_but_documents_are() {
        let start = url("https://example.com/docs/");
        let prefix = start.dir();
        assert!(!worth_following(
            &url("https://example.com/docs/logo.png"),
            &start,
            &prefix
        ));
        assert!(!worth_following(
            &url("https://example.com/docs/app.js"),
            &start,
            &prefix
        ));
        assert!(worth_following(
            &url("https://example.com/docs/spec.pdf"),
            &start,
            &prefix
        ));
        assert!(worth_following(
            &url("https://example.com/docs/notes.md"),
            &start,
            &prefix
        ));
    }

    #[test]
    fn decorative_queries_are_dropped_but_identifying_ones_are_kept() {
        assert_eq!(
            drop_decorative_query(url("https://e.com/docs/?highlight=x")).to_string(),
            "https://e.com/docs/"
        );
        assert_eq!(
            drop_decorative_query(url("https://e.com/w/index.php?title=Main")).to_string(),
            "https://e.com/w/index.php?title=Main"
        );
    }

    fn fetched(body: &str, content_type: Option<&str>) -> Fetched {
        Fetched {
            url: url("https://e.com/llms.txt"),
            bytes: body.as_bytes().to_vec(),
            content_type: content_type.map(str::to_string),
        }
    }

    #[test]
    fn a_soft_404_is_not_mistaken_for_the_documentation() {
        // Plenty of sites answer an unknown path with a 200 and an HTML page.
        assert!(!is_plain_text(&fetched(
            "<!DOCTYPE html><html><head>...",
            Some("text/html")
        )));
        assert!(!is_plain_text(&fetched(
            "<!doctype html>\n<html>",
            Some("text/plain")
        )));
        assert!(is_plain_text(&fetched(
            "# Docs\n\nReal text.",
            Some("text/plain; charset=utf-8")
        )));
        assert!(!is_plain_text(&fetched("", Some("text/plain"))));
    }

    #[test]
    fn sitemap_locations_are_read() {
        let xml = "<urlset><url><loc>https://e.com/a</loc></url>\
                   <url><loc> https://e.com/b </loc></url></urlset>";
        assert_eq!(locations(xml), vec!["https://e.com/a", "https://e.com/b"]);
    }
}
