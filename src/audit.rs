//! `audit` — grade a skill, or a whole skills directory, without an API key.
//!
//! The complaint about agent skills in practice is not that there are too few.
//! It is that each one costs context on every session whether it fires or not,
//! that vague descriptions never route, and that two skills claiming the same
//! trigger make the choice a coin flip. All three are visible in the files
//! themselves, so this is arithmetic and pattern-matching, not judgment — no
//! model is involved and the same tree always grades the same way.
//!
//! The cost model matters more than any single check. An agent loads every
//! skill's `name` and `description` at the start of every session; it loads a
//! `SKILL.md` body only when that skill fires; it loads `references/` only when
//! the body sends it there. So the number worth reducing is not the size of the
//! tree, it is the always-loaded part — and a skill that keeps its body small
//! by pushing detail into `references/` costs nothing extra until it is used.

use crate::skill::Skill;
use crate::tokens;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// The longest a `description` may be. Anthropic's skill loader rejects longer
/// ones outright, so this is a hard failure rather than a matter of taste.
const MAX_DESCRIPTION_CHARS: usize = 1024;

/// Below this a description cannot be carrying both what the skill is and when
/// to use it, which is the whole job of the field.
const MIN_DESCRIPTION_CHARS: usize = 40;

/// Default ceiling for a `SKILL.md` body, in tokens. Everything past it should
/// be in `references/`, where it costs nothing until it is needed.
pub const DEFAULT_BODY_BUDGET: usize = 2_000;

/// How much of two descriptions has to coincide before the pair is a routing
/// hazard. Jaccard over distinctive terms; two skills about genuinely different
/// subjects land far below this.
const OVERLAP_THRESHOLD: f64 = 0.34;

/// Phrases that make a description routable: they name the situation rather
/// than the subject. A description with none of these tells an agent what the
/// skill is about but never when to reach for it.
const TRIGGER_MARKERS: &[&str] = &[
    "use when",
    "use this when",
    "when the user",
    "when you",
    "when asked",
    "when working",
    "when building",
    "when creating",
    "when the task",
    "triggers when",
    "for when",
    "invoke when",
    "load when",
    "applies when",
    "should be used",
];

/// Words carried by every description, which say nothing about what it covers.
const STOPWORDS: &[&str] = &[
    "a", "about", "after", "against", "all", "also", "an", "and", "any", "are", "as", "at", "be",
    "been", "being", "both", "but", "by", "can", "code", "create", "creating", "do", "does",
    "doing", "each", "even", "every", "for", "from", "get", "gets", "has", "have", "how", "into",
    "is", "it", "its", "just", "like", "make", "makes", "making", "may", "more", "most", "must",
    "need", "needs", "not", "of", "on", "once", "one", "only", "or", "other", "out", "over",
    "same", "should", "skill", "so", "some", "such", "than", "that", "the", "their", "them",
    "then", "there", "these", "they", "this", "those", "through", "to", "too", "under", "up",
    "use", "used", "user", "uses", "using", "very", "want", "wants", "was", "were", "what", "when",
    "where", "which", "while", "who", "will", "with", "would", "you", "your",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The skill is broken: it will not load, or it points at nothing.
    Error,
    /// The skill loads but will cost more than it should, or route badly.
    Warning,
    /// Worth knowing, not worth blocking on.
    Note,
}

impl Severity {
    fn marker(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub severity: Severity,
    /// A stable identifier, so a finding can be grepped for or suppressed.
    pub rule: &'static str,
    pub message: String,
    /// What to do about it. A finding without one is a complaint.
    pub remedy: String,
}

fn finding(
    severity: Severity,
    rule: &'static str,
    message: impl Into<String>,
    remedy: impl Into<String>,
) -> Finding {
    Finding {
        severity,
        rule,
        message: message.into(),
        remedy: remedy.into(),
    }
}

#[derive(Debug, Serialize)]
pub struct SkillReport {
    pub name: String,
    pub path: String,
    /// Tokens this skill costs in every session, fired or not.
    pub always_loaded_tokens: usize,
    /// Tokens the body adds when the skill does fire.
    pub on_trigger_tokens: usize,
    /// Tokens sitting in `references/`, paid only when the body sends you there.
    pub on_demand_tokens: usize,
    pub reference_files: usize,
    pub findings: Vec<Finding>,
}

#[derive(Debug, Serialize)]
pub struct AuditReport {
    pub skills: Vec<SkillReport>,
    /// Findings about the tree as a whole rather than any one skill.
    pub tree_findings: Vec<Finding>,
    /// What every session pays for this tree before a single skill fires.
    pub always_loaded_tokens: usize,
    pub errors: usize,
    pub warnings: usize,
}

impl AuditReport {
    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }
}

/// Grade every skill, then the tree they form together.
pub fn audit(skills: &[Skill], body_budget: usize) -> AuditReport {
    let mut reports: Vec<SkillReport> = skills
        .iter()
        .map(|skill| audit_one(skill, body_budget))
        .collect();

    let tree_findings = audit_tree(skills, &mut reports);

    let always_loaded_tokens = reports.iter().map(|r| r.always_loaded_tokens).sum();
    let (errors, warnings) = reports
        .iter()
        .flat_map(|r| &r.findings)
        .chain(tree_findings.iter())
        .fold((0, 0), |(e, w), f| match f.severity {
            Severity::Error => (e + 1, w),
            Severity::Warning => (e, w + 1),
            Severity::Note => (e, w),
        });

    AuditReport {
        skills: reports,
        tree_findings,
        always_loaded_tokens,
        errors,
        warnings,
    }
}

fn audit_one(skill: &Skill, body_budget: usize) -> SkillReport {
    let mut findings = Vec::new();
    let name = skill.name();

    if skill.frontmatter.is_none() {
        findings.push(finding(
            Severity::Error,
            "no-frontmatter",
            "SKILL.md has no YAML frontmatter, so nothing will load it",
            "add a `---` block at the top of the file with `name` and `description`",
        ));
    }

    // ------------------------------------------------------------ description
    match skill.description() {
        None => findings.push(finding(
            Severity::Error,
            "no-description",
            "no `description`, so the agent has nothing to match the skill against",
            "add a description saying what the skill does and when to use it",
        )),
        Some(description) => {
            let chars = description.chars().count();
            if chars > MAX_DESCRIPTION_CHARS {
                findings.push(finding(
                    Severity::Error,
                    "description-too-long",
                    format!(
                        "description is {chars} characters; the limit is {MAX_DESCRIPTION_CHARS}"
                    ),
                    "cut it to the trigger and the subject — the detail belongs in the body",
                ));
            } else if chars < MIN_DESCRIPTION_CHARS {
                findings.push(finding(
                    Severity::Warning,
                    "description-too-short",
                    format!("description is {chars} characters — too little to route on"),
                    "say what it does *and* when to reach for it, in one or two sentences",
                ));
            }
            let lower = description.to_ascii_lowercase();
            if !TRIGGER_MARKERS.iter().any(|m| lower.contains(m)) {
                findings.push(finding(
                    Severity::Warning,
                    "description-not-routable",
                    "description names the subject but never the situation",
                    "add an explicit trigger — \"Use when the user ...\" — with the words a \
                     user would actually type",
                ));
            }
        }
    }

    // ------------------------------------------------------------------- name
    if let Some(declared) = skill.frontmatter.as_ref().and_then(|f| f.name.as_deref()) {
        let dir_name = skill.dir_name();
        if declared != dir_name {
            findings.push(finding(
                Severity::Warning,
                "name-mismatch",
                format!("frontmatter says `{declared}` but the directory is `{dir_name}`"),
                "make them identical — tools disagree about which one wins",
            ));
        }
        if !declared.is_empty()
            && !declared
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            findings.push(finding(
                Severity::Warning,
                "name-not-kebab-case",
                format!("`{declared}` is not lowercase-with-hyphens"),
                "rename it to kebab-case; some loaders reject anything else",
            ));
        }
    }

    // ------------------------------------------------------------------- size
    let always_loaded_tokens = tokens::estimate(&format!(
        "{}\n{}",
        name,
        skill.description().unwrap_or_default()
    ));
    let on_trigger_tokens = tokens::estimate(&skill.body);
    let (on_demand_tokens, reference_bodies) = read_references(skill);

    if on_trigger_tokens > body_budget {
        let severity = if on_trigger_tokens > body_budget * 2 {
            Severity::Error
        } else {
            Severity::Warning
        };
        findings.push(finding(
            severity,
            "body-over-budget",
            format!(
                "SKILL.md body is ~{on_trigger_tokens} tokens against a budget of {body_budget}"
            ),
            if skill.references.is_empty() {
                "move the lookup material — tables, syntax, command lists — into \
                 `references/` and link to it from the body"
                    .to_string()
            } else {
                format!(
                    "the skill already has {} reference file(s); move more of the body there",
                    skill.references.len()
                )
            },
        ));
    }

    // ------------------------------------------------------------- references
    let linked = linked_paths(&skill.body);
    let existing: BTreeSet<String> = skill
        .references
        .iter()
        .map(|p| normalise(&p.to_string_lossy()))
        .collect();

    for link in &linked {
        // A trailing slash names the directory, not a file. `SKILL.md` bodies
        // draw the layout — `references/` on its own line — and that is not a
        // link anybody can follow or fail to follow.
        if link.starts_with("references/") && !link.ends_with('/') && !existing.contains(link) {
            findings.push(finding(
                Severity::Error,
                "broken-reference-link",
                format!("body links `{link}`, which does not exist"),
                "fix the path or write the file — an agent that follows it gets nothing",
            ));
        }
    }
    for reference in &existing {
        if !linked.contains(reference) && !mentioned_by_name(&skill.raw, reference) {
            findings.push(finding(
                Severity::Note,
                "orphan-reference",
                format!("`{reference}` is never linked from SKILL.md"),
                "link it from the body or delete it — nothing will ever open it",
            ));
        }
    }
    if on_trigger_tokens > body_budget && skill.references.is_empty() {
        findings.push(finding(
            Severity::Note,
            "no-progressive-disclosure",
            "everything is in one file, so firing the skill pays for all of it",
            "split lookup material into `references/` so the body stays a map, not the territory",
        ));
    }
    for (path, text) in &reference_bodies {
        if text.trim().is_empty() {
            findings.push(finding(
                Severity::Warning,
                "empty-reference",
                format!("`{path}` is empty"),
                "write it or remove it",
            ));
        }
    }

    findings.sort_by_key(|f| f.severity);

    SkillReport {
        name,
        path: skill.path.display().to_string(),
        always_loaded_tokens,
        on_trigger_tokens,
        on_demand_tokens,
        reference_files: skill.references.len(),
        findings,
    }
}

fn read_references(skill: &Skill) -> (usize, Vec<(String, String)>) {
    let mut total = 0;
    let mut bodies = Vec::new();
    for relative in &skill.references {
        let Ok(text) = std::fs::read_to_string(skill.dir.join(relative)) else {
            continue;
        };
        total += tokens::estimate(&text);
        bodies.push((normalise(&relative.to_string_lossy()), text));
    }
    (total, bodies)
}

/// Whether the file is named anywhere in `SKILL.md`, path or not.
///
/// Skills point at their references in more than one way: a Markdown link, a
/// bare path, or a `references:` list in the frontmatter naming the stems. The
/// orphan check is only worth having if it stays quiet for all three.
fn mentioned_by_name(raw: &str, reference: &str) -> bool {
    let file = reference.rsplit('/').next().unwrap_or(reference);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    if stem.is_empty() {
        return false;
    }
    raw.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_' && c != '.')
        .any(|token| token == file || token == stem)
}

/// Windows writes `references\x.md`; a Markdown link always says `references/x.md`.
fn normalise(path: &str) -> String {
    path.replace('\\', "/")
}

/// Every relative path the body points at: Markdown links and bare mentions.
fn linked_paths(body: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    // Markdown links and images: the target between `](` and `)`.
    let mut rest = body;
    while let Some(open) = rest.find("](") {
        rest = &rest[open + 2..];
        if let Some(close) = rest.find(')') {
            let target = rest[..close].split_whitespace().next().unwrap_or("");
            if !target.is_empty() {
                found.insert(normalise(target.trim_start_matches("./")));
            }
            rest = &rest[close + 1..];
        }
    }
    // Bare mentions — `references/foo.md` written in prose or in a shell line.
    for token in
        body.split(|c: char| c.is_whitespace() || matches!(c, '`' | '"' | '\'' | '(' | ')'))
    {
        let token = normalise(token.trim_start_matches("./"))
            .trim_end_matches([',', '.', ';', ':'])
            .to_string();
        if token.starts_with("references/") && !token.ends_with('/') {
            found.insert(token);
        }
    }
    found
}

/// Findings that only exist between skills: two skills claiming one trigger.
fn audit_tree(skills: &[Skill], reports: &mut [SkillReport]) -> Vec<Finding> {
    let mut findings = Vec::new();

    // Two skills with the same name is not a routing hazard, it is a collision:
    // whichever loads second wins and the other is simply gone.
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for skill in skills {
        by_name
            .entry(skill.name())
            .or_default()
            .push(skill.path.display().to_string());
    }
    for (name, paths) in by_name.iter().filter(|(_, p)| p.len() > 1) {
        findings.push(finding(
            Severity::Error,
            "duplicate-name",
            format!(
                "{} skills are named `{name}`: {}",
                paths.len(),
                paths.join(", ")
            ),
            "rename all but one — only one of them can ever load",
        ));
    }

    let terms: Vec<BTreeSet<String>> = skills
        .iter()
        .map(|s| distinctive_terms(s.description().unwrap_or_default()))
        .collect();

    for i in 0..skills.len() {
        for j in (i + 1)..skills.len() {
            if terms[i].len() < 3 || terms[j].len() < 3 {
                continue;
            }
            let shared: Vec<&String> = terms[i].intersection(&terms[j]).collect();
            let union = terms[i].union(&terms[j]).count();
            if union == 0 {
                continue;
            }
            let overlap = shared.len() as f64 / union as f64;
            if shared.len() >= 3 && overlap >= OVERLAP_THRESHOLD {
                let message = format!(
                    "`{}` and `{}` describe themselves with the same terms ({})",
                    skills[i].name(),
                    skills[j].name(),
                    shared
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                let entry = finding(
                    Severity::Warning,
                    "trigger-overlap",
                    message,
                    "make each description say what the other one is *not* for, \
                     or merge them into one skill",
                );
                // Attach it to both skills so a per-skill run still sees it.
                reports[i].findings.push(entry.clone());
                reports[j].findings.push(entry.clone());
                findings.push(entry);
            }
        }
    }

    findings
}

fn distinctive_terms(description: &str) -> BTreeSet<String> {
    description
        .split(|c: char| !c.is_alphanumeric() && c != '-')
        .map(|w| w.trim_matches('-').to_ascii_lowercase())
        .filter(|w| w.len() >= 4 && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// The human-readable report. `--json` prints the structure instead.
pub fn print(report: &AuditReport, body_budget: usize) {
    for skill in &report.skills {
        println!(
            "{}  {} always-loaded / {} on trigger / {} on demand ({} reference file(s))",
            skill.name,
            skill.always_loaded_tokens,
            skill.on_trigger_tokens,
            skill.on_demand_tokens,
            skill.reference_files,
        );
        println!("  {}", skill.path);
        for f in &skill.findings {
            println!("    {}: {}", f.severity.marker(), f.message);
            println!("      -> {}", f.remedy);
        }
        println!();
    }

    if !report.tree_findings.is_empty() {
        println!("Across the tree:");
        for f in &report.tree_findings {
            println!("    {}: {}", f.severity.marker(), f.message);
            println!("      -> {}", f.remedy);
        }
        println!();
    }

    println!("{} skill(s)", report.skills.len());
    println!(
        "  {} tokens are loaded in every session before any skill fires",
        report.always_loaded_tokens
    );
    let heaviest = report
        .skills
        .iter()
        .max_by_key(|s| s.on_trigger_tokens)
        .filter(|s| s.on_trigger_tokens > body_budget);
    if let Some(skill) = heaviest {
        println!(
            "  heaviest body: {} at ~{} tokens (budget {})",
            skill.name, skill.on_trigger_tokens, body_budget
        );
    }
    println!(
        "  {} error(s), {} warning(s)",
        report.errors, report.warnings
    );
    if report.errors == 0 && report.warnings == 0 {
        println!("\nNothing to fix.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::Skill;
    use std::path::PathBuf;

    fn skill_with(description: &str, body: &str) -> Skill {
        let text = format!("---\nname: demo\ndescription: {description}\n---\n{body}");
        // The directory name is the skill's name, so it must match the
        // frontmatter or every case here also trips `name-mismatch`.
        let dir = std::env::temp_dir()
            .join(format!(
                "a2s-audit-test-{}-{}",
                std::process::id(),
                tokens::estimate(&text)
            ))
            .join("demo");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("SKILL.md");
        std::fs::write(&path, text).unwrap();
        Skill::load(&path).unwrap()
    }

    #[test]
    fn a_routable_description_passes() {
        let skill = skill_with(
            "Convert a PDF into a skill. Use when the user asks to turn a book into a skill.",
            "# Demo\n",
        );
        let report = audit(std::slice::from_ref(&skill), DEFAULT_BODY_BUDGET);
        assert_eq!(report.errors, 0, "{:?}", report.skills[0].findings);
        assert_eq!(report.warnings, 0, "{:?}", report.skills[0].findings);
    }

    #[test]
    fn a_description_without_a_trigger_is_flagged() {
        let skill = skill_with(
            "A collection of helpful productivity utilities.",
            "# Demo\n",
        );
        let report = audit(std::slice::from_ref(&skill), DEFAULT_BODY_BUDGET);
        assert!(
            report.skills[0]
                .findings
                .iter()
                .any(|f| f.rule == "description-not-routable")
        );
    }

    #[test]
    fn an_oversized_body_is_flagged() {
        let body = "word ".repeat(4_000);
        let skill = skill_with("Use when the user needs the demo skill for demos.", &body);
        let report = audit(std::slice::from_ref(&skill), DEFAULT_BODY_BUDGET);
        assert!(
            report.skills[0]
                .findings
                .iter()
                .any(|f| f.rule == "body-over-budget")
        );
    }

    #[test]
    fn a_broken_reference_link_is_an_error() {
        let skill = skill_with(
            "Use when the user needs the demo skill for demos.",
            "See [the table](references/tables.md).\n",
        );
        let report = audit(std::slice::from_ref(&skill), DEFAULT_BODY_BUDGET);
        assert!(report.has_errors());
        assert!(
            report.skills[0]
                .findings
                .iter()
                .any(|f| f.rule == "broken-reference-link")
        );
    }

    #[test]
    fn a_reference_named_in_the_frontmatter_is_not_an_orphan() {
        assert!(mentioned_by_name(
            "---\nreferences:\n  - nextjs-app\n---\n",
            "references/nextjs-app.md"
        ));
        assert!(mentioned_by_name("see tables.md\n", "references/tables.md"));
        assert!(!mentioned_by_name("nothing here\n", "references/tables.md"));
    }

    #[test]
    fn a_directory_mention_is_not_a_link() {
        let found = linked_paths("The layout:\n\n```\nSKILL.md\nreferences/\n```\n");
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn markdown_and_bare_links_are_both_found() {
        let found = linked_paths("[a](references/a.md) and `references/b.md`.\n");
        assert!(found.contains("references/a.md"), "{found:?}");
        assert!(found.contains("references/b.md"), "{found:?}");
    }

    #[test]
    fn distinctive_terms_drop_the_filler() {
        let terms = distinctive_terms("Use this when the user wants to parse a PDF invoice");
        assert!(terms.contains("parse"));
        assert!(terms.contains("invoice"));
        assert!(!terms.contains("when"));
        assert!(!terms.contains("this"));
    }

    #[test]
    fn unrelated_skills_do_not_collide() {
        let a = distinctive_terms("Use when the user wants to parse a PDF invoice");
        let b = distinctive_terms("Use when deploying a Kubernetes cluster to production");
        assert_eq!(a.intersection(&b).count(), 0);
    }

    #[test]
    fn references_are_relative_to_the_skill() {
        let skill = skill_with("Use when the user needs the demo skill.", "body\n");
        assert!(skill.references.iter().all(|p| p.starts_with("references")));
        assert_eq!(PathBuf::from("references"), PathBuf::from("references"));
    }
}
