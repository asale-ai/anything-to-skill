//! Reading a git repository's documentation.
//!
//! A shallow clone, then the prose files in a deliberate order: the README
//! first, then whatever lives under `docs/`, then the rest. Order matters more
//! here than anywhere else in this tool — a repository has no spine of its own,
//! so the order files are concatenated in *is* the structure the model reads.

use anyhow::{Context, Result, bail};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Extensions taken from a repository by default. Source code is excluded: a
/// skill built from a repo is built from what the repo *says*, and pulling in
/// every `.rs` file drowns that in implementation. `--include` overrides this.
const DOC_EXTENSIONS: &[&str] = &[
    "md", "mdx", "markdown", "rst", "txt", "text", "adoc", "asciidoc", "pdf",
];

/// Directories that hold generated or vendored content, never authored docs.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "vendor",
    "dist",
    "build",
    "out",
    "coverage",
    "__pycache__",
    "site-packages",
    "third_party",
];

/// Files past this size are generated — lockfiles rendered to text, bundled
/// API dumps — not something a person wrote for a reader.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RepoSpec {
    /// What `git clone` is given.
    pub url: String,
    pub branch: Option<String>,
    /// Restrict the read to one directory of the repository.
    pub subdir: Option<String>,
    /// `owner/repo`, for display and for naming the checkout directory.
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct RepoOptions {
    pub max_files: usize,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

pub struct Checkout {
    pub root: PathBuf,
    /// Selected files, in reading order, absolute.
    pub files: Vec<PathBuf>,
    /// Files that matched but were dropped by `--max-files`.
    pub dropped: usize,
}

impl Checkout {
    /// The path of `file` relative to the repository root, for display.
    pub fn label(&self, file: &Path) -> String {
        file.strip_prefix(&self.root)
            .unwrap_or(file)
            .display()
            .to_string()
    }
}

/// Shallow-clone a repository and pick its documentation out.
pub fn checkout(spec: &RepoSpec, opts: &RepoOptions) -> Result<Checkout> {
    if crate::extract::which("git").is_none() {
        bail!(
            "reading a repository needs `git`, which is not on PATH.\n\
             Install git, or clone the repository yourself and pass the directory's files."
        );
    }

    let dir = checkout_dir(&spec.label)?;
    eprintln!("cloning {} ...", spec.url);

    let mut cmd = Command::new("git");
    cmd.arg("clone")
        .arg("--depth")
        .arg("1")
        .arg("--single-branch")
        .arg("--quiet")
        // Fail on a private repository instead of blocking forever on a
        // credential prompt nobody is watching.
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(branch) = &spec.branch {
        cmd.arg("--branch").arg(branch);
    }
    let output = cmd
        .arg(&spec.url)
        .arg(&dir)
        .output()
        .context("running git clone")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "git clone failed for {}: {}",
            spec.url,
            stderr.trim().lines().last().unwrap_or("unknown error")
        );
    }

    let root = match &spec.subdir {
        Some(sub) => {
            let path = dir.join(sub);
            if !path.is_dir() {
                bail!("{} has no directory '{sub}'", spec.label);
            }
            path
        }
        None => dir,
    };

    let mut found = Vec::new();
    walk(&root, &root, opts, &mut found)?;
    if found.is_empty() {
        bail!(
            "no documentation files found in {}{}.\n\
             By default only {} are read — pass --include to widen that.",
            spec.label,
            spec.subdir
                .as_ref()
                .map(|s| format!("/{s}"))
                .unwrap_or_default(),
            DOC_EXTENSIONS.join(", ")
        );
    }

    found.sort_by_key(|path| {
        let rel = path.strip_prefix(&root).unwrap_or(path).to_path_buf();
        (reading_rank(&rel), rel)
    });

    let dropped = found.len().saturating_sub(opts.max_files);
    found.truncate(opts.max_files);

    Ok(Checkout {
        root,
        files: found,
        dropped,
    })
}

/// Where a file falls in reading order. A repository is read the way a person
/// opens one: the README, then the manual, then everything else.
fn reading_rank(rel: &Path) -> u8 {
    let text = rel.to_string_lossy().to_ascii_lowercase();
    let is_root = !text.contains('/') && !text.contains('\\');
    let stem = rel
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if is_root && stem == "readme" {
        0
    } else if text.starts_with("docs/") || text.starts_with("doc/") || text.starts_with("guide/") {
        1
    } else if stem == "readme" {
        2
    } else {
        3
    }
}

fn walk(dir: &Path, root: &Path, opts: &RepoOptions, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            // A symlink can point anywhere, including outside the checkout.
            continue;
        }
        if file_type.is_dir() {
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(&path, root, opts, out)?;
            continue;
        }
        if !file_type.is_file() || name.starts_with('.') {
            continue;
        }

        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if opts.exclude.iter().any(|p| glob_matches(p, &rel)) {
            continue;
        }
        let wanted = if opts.include.is_empty() {
            let ext = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            DOC_EXTENSIONS.contains(&ext.as_str())
        } else {
            opts.include.iter().any(|p| glob_matches(p, &rel))
        };
        if !wanted {
            continue;
        }
        if entry.metadata().map(|m| m.len()).unwrap_or(0) > MAX_FILE_BYTES {
            continue;
        }
        out.push(path);
    }
    Ok(())
}

fn checkout_dir(label: &str) -> Result<PathBuf> {
    let safe: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let base = std::env::temp_dir().join("anything-to-skill-repos");
    std::fs::create_dir_all(&base).with_context(|| format!("creating {}", base.display()))?;

    // A fresh directory per run: reusing one would mean deciding whether to
    // delete somebody else's checkout, and this tool does not delete things.
    let mut n = 0;
    loop {
        let candidate = base.join(if n == 0 {
            format!("{safe}-{}", std::process::id())
        } else {
            format!("{safe}-{}-{n}", std::process::id())
        });
        if !candidate.exists() {
            return Ok(candidate);
        }
        n += 1;
        if n > 100 {
            bail!(
                "could not find a free checkout directory under {}",
                base.display()
            );
        }
    }
}

/// Match a path against a glob: `*` within a segment, `**` across segments.
fn glob_matches(pattern: &str, path: &str) -> bool {
    let mut re = String::from("(?i)^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    if chars.peek() == Some(&'/') {
                        chars.next();
                        re.push_str("(?:.*/)?");
                    } else {
                        re.push_str(".*");
                    }
                } else {
                    re.push_str("[^/]*");
                }
            }
            '?' => re.push_str("[^/]"),
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    Regex::new(&re).map(|r| r.is_match(path)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readme_is_read_before_the_manual() {
        let mut paths = vec![
            PathBuf::from("src/notes.md"),
            PathBuf::from("docs/guide.md"),
            PathBuf::from("README.md"),
        ];
        paths.sort_by_key(|p| (reading_rank(p), p.clone()));
        assert_eq!(
            paths,
            vec![
                PathBuf::from("README.md"),
                PathBuf::from("docs/guide.md"),
                PathBuf::from("src/notes.md"),
            ]
        );
    }

    #[test]
    fn globs_respect_segment_boundaries() {
        assert!(glob_matches("*.md", "README.md"));
        assert!(!glob_matches("*.md", "docs/README.md"));
        assert!(glob_matches("**/*.md", "docs/a/b.md"));
        assert!(glob_matches("**/*.md", "b.md"));
        assert!(glob_matches("docs/**", "docs/a/b.md"));
        assert!(!glob_matches("docs/**", "src/a.md"));
    }

    #[test]
    fn globs_do_not_leak_regex_syntax() {
        assert!(glob_matches("a.b.md", "a.b.md"));
        assert!(!glob_matches("a.b.md", "axbxmd"));
    }
}
