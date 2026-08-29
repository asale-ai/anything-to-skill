//! The on-disk shape of a skill, and how to read one back.
//!
//! A skill is a directory: `SKILL.md` with YAML frontmatter, and optionally
//! `references/` beside it. Three commands need to read that shape back —
//! `audit` grades it, `eval` loads it to answer questions, `refresh` rewrites
//! it — so the parsing lives here once rather than three times.
//!
//! The frontmatter parser is deliberately small. A skill's frontmatter is a
//! handful of scalar keys; pulling in a YAML engine to read them would buy
//! nothing and cost a dependency. Anything it cannot parse is preserved
//! verbatim in `extra` rather than dropped, so a rewrite never silently eats a
//! key this tool does not understand.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// How deep `discover` will look for `SKILL.md` under a directory. A skills
/// root is `<root>/<skill>/SKILL.md`; a plugin bundle adds one level for the
/// plugin. Past that, we are walking somebody's source tree.
const MAX_DISCOVERY_DEPTH: usize = 4;

/// The frontmatter of a `SKILL.md`, split into the keys that carry meaning and
/// everything else.
#[derive(Debug, Clone, Default)]
pub struct Frontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    /// Every other key, in the order it appeared, as `(key, raw_value)`.
    pub extra: Vec<(String, String)>,
}

impl Frontmatter {
    /// Split a `SKILL.md` into its frontmatter and its body.
    ///
    /// A file with no frontmatter is not an error here — it is a finding, and
    /// `audit` is the one that gets to say so.
    fn split(text: &str) -> (Option<String>, String) {
        // A leading BOM survives a lot of editors and would otherwise stop the
        // fence from matching on the first byte.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let rest = match text.strip_prefix("---\n") {
            Some(rest) => rest,
            None => match text.strip_prefix("---\r\n") {
                Some(rest) => rest,
                None => return (None, text.to_string()),
            },
        };
        // The closing fence is a line that is exactly `---`.
        for (index, line) in rest.match_indices('\n') {
            let line_start = index + line.len();
            let candidate = rest[..index].rsplit('\n').next().unwrap_or("");
            if candidate.trim_end() == "---" {
                let block_end = index - candidate.len();
                return (
                    Some(rest[..block_end].to_string()),
                    rest[line_start..].to_string(),
                );
            }
        }
        // An unterminated fence: treat the whole file as body so the reader
        // sees the text rather than nothing.
        (None, text.to_string())
    }

    fn parse(block: &str) -> Frontmatter {
        let mut fm = Frontmatter::default();
        let mut pending: Option<(String, String)> = None;

        for line in block.lines() {
            // A continuation line: either a list item or a wrapped scalar. It
            // belongs to the key above it, so fold it in rather than losing it.
            let is_continuation =
                line.starts_with(' ') || line.starts_with('\t') || line.starts_with('-');
            if is_continuation && pending.is_some() {
                if let Some((_, value)) = pending.as_mut() {
                    value.push(' ');
                    value.push_str(line.trim());
                }
                continue;
            }

            if let Some((key, value)) = pending.take() {
                fm.set(&key, value);
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            if key.is_empty() {
                continue;
            }
            pending = Some((key, value.trim().to_string()));
        }
        if let Some((key, value)) = pending.take() {
            fm.set(&key, value);
        }
        fm
    }

    fn set(&mut self, key: &str, value: String) {
        let value = unquote(value.trim());
        match key {
            "name" => self.name = Some(value),
            "description" => self.description = Some(value),
            "license" => self.license = Some(value),
            _ => self.extra.push((key.to_string(), value)),
        }
    }
}

/// Strip one layer of matching quotes, the way a YAML scalar would be read.
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        return value[1..value.len() - 1].to_string();
    }
    value.to_string()
}

/// One skill on disk.
#[derive(Debug, Clone)]
pub struct Skill {
    /// The directory holding `SKILL.md`.
    pub dir: PathBuf,
    /// The `SKILL.md` itself.
    pub path: PathBuf,
    pub frontmatter: Option<Frontmatter>,
    /// Everything after the frontmatter.
    pub body: String,
    /// The whole file, frontmatter included. A skill may name its reference
    /// files in the frontmatter rather than linking them from the body, so
    /// "is this file ever used" has to be asked of the file as a whole.
    pub raw: String,
    /// Files under `references/`, relative to `dir`, sorted.
    pub references: Vec<PathBuf>,
}

impl Skill {
    /// Read the skill whose `SKILL.md` is at `path`.
    pub fn load(path: &Path) -> Result<Skill> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let dir = resolve_dir(path.parent().unwrap_or(Path::new(".")));
        let (block, body) = Frontmatter::split(&text);
        Ok(Skill {
            references: collect_references(&dir),
            frontmatter: block.as_deref().map(Frontmatter::parse),
            dir,
            path: path.to_path_buf(),
            body,
            raw: text,
        })
    }

    /// The name the skill goes by: its declared `name`, else its directory.
    pub fn name(&self) -> String {
        self.frontmatter
            .as_ref()
            .and_then(|f| f.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| self.dir_name())
    }

    /// The directory name, which is what an agent matches a skill folder by.
    pub fn dir_name(&self) -> String {
        self.dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<unnamed>".to_string())
    }

    pub fn description(&self) -> Option<&str> {
        self.frontmatter
            .as_ref()
            .and_then(|f| f.description.as_deref())
            .filter(|d| !d.is_empty())
    }
}

/// The directory a skill lives in, in a form that still has a last component.
///
/// `audit .` hands us a parent of `.`, whose `file_name()` is `None` — and the
/// directory name is what an agent matches a skill folder by, so losing it
/// turns every relative run into a spurious `name-mismatch`.
fn resolve_dir(dir: &Path) -> PathBuf {
    let named = dir
        .file_name()
        .is_some_and(|n| n != std::ffi::OsStr::new(".") && n != std::ffi::OsStr::new(".."));
    if named {
        return dir.to_path_buf();
    }
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

fn collect_references(dir: &Path) -> Vec<PathBuf> {
    let root = dir.join("references");
    let mut found = Vec::new();
    walk_files(&root, &mut found, 0);
    for path in &mut found {
        if let Ok(relative) = path.strip_prefix(dir) {
            *path = relative.to_path_buf();
        }
    }
    found.sort();
    found
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > MAX_DISCOVERY_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_files(&path, out, depth + 1);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

/// Find every skill at or under `root`.
///
/// `root` may be the skill itself (a directory holding `SKILL.md`, or the
/// `SKILL.md` file), or a skills directory holding many of them.
pub fn discover(root: &Path) -> Result<Vec<Skill>> {
    if root.is_file() {
        return Ok(vec![Skill::load(root)?]);
    }
    if !root.is_dir() {
        bail!("{} does not exist", root.display());
    }
    let mut paths = Vec::new();
    find_skill_files(root, &mut paths, 0);
    if paths.is_empty() {
        bail!(
            "no SKILL.md found in {} — point at a skill directory or a directory of them",
            root.display()
        );
    }
    paths.sort();
    paths.iter().map(|p| Skill::load(p)).collect()
}

fn find_skill_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > MAX_DISCOVERY_DEPTH {
        return;
    }
    let candidate = dir.join("SKILL.md");
    if candidate.is_file() {
        out.push(candidate);
        // A skill does not nest inside another skill; stop descending so a
        // bundled example does not get graded as a separate skill.
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Skip the directories every checkout carries and no skill lives in.
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || matches!(name.as_ref(), "node_modules" | "target" | "venv") {
            continue;
        }
        find_skill_files(&path, out, depth + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_from_body() {
        let (block, body) = Frontmatter::split("---\nname: x\n---\n# Title\n");
        assert_eq!(block.as_deref(), Some("name: x\n"));
        assert_eq!(body, "# Title\n");
    }

    #[test]
    fn a_file_without_frontmatter_is_all_body() {
        let (block, body) = Frontmatter::split("# Title\n");
        assert!(block.is_none());
        assert_eq!(body, "# Title\n");
    }

    #[test]
    fn an_unterminated_fence_keeps_the_text() {
        let (block, body) = Frontmatter::split("---\nname: x\n# Title\n");
        assert!(block.is_none());
        assert!(body.contains("# Title"));
    }

    #[test]
    fn parses_the_keys_that_matter() {
        let fm = Frontmatter::parse("name: demo\ndescription: \"Does a thing\"\nlicense: MIT\n");
        assert_eq!(fm.name.as_deref(), Some("demo"));
        assert_eq!(fm.description.as_deref(), Some("Does a thing"));
        assert_eq!(fm.license.as_deref(), Some("MIT"));
    }

    #[test]
    fn folds_wrapped_values_into_the_key_above() {
        let fm = Frontmatter::parse("description: one\n  two three\nname: demo\n");
        assert_eq!(fm.description.as_deref(), Some("one two three"));
        assert_eq!(fm.name.as_deref(), Some("demo"));
    }

    #[test]
    fn unknown_keys_survive() {
        let fm = Frontmatter::parse("name: demo\ncompatibility: needs a shell\n");
        assert_eq!(
            fm.extra,
            vec![("compatibility".to_string(), "needs a shell".to_string())]
        );
    }

    #[test]
    fn a_colon_in_the_value_is_not_a_separator() {
        let fm = Frontmatter::parse("description: Use when: the user asks\n");
        assert_eq!(fm.description.as_deref(), Some("Use when: the user asks"));
    }
}
