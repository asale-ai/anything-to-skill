//! `eval` — check that the skill can still answer what the source taught.
//!
//! Every other skill-authoring tool grades a skill against itself: it asks a
//! model whether the skill looks good. That test cannot fail for the right
//! reason, because nothing in it knows what the skill was supposed to contain.
//!
//! This one has the source text. So the questions come from the source, the
//! answers are graded against the source, and the skill is given nothing but
//! itself to answer with. A question the source answers plainly and the skill
//! cannot is a hole, and it can be pointed at: the report names the section the
//! question came from.
//!
//! The failures are the output. A pass rate is a number; "chapter 7 did not
//! survive the compression" is a thing to fix.

use crate::llm::Client;
use crate::skill::Skill;
use crate::tokens;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// How much of the skill can be put in front of the answering model. A skill
/// larger than this is already failing the only test that matters.
const MAX_SKILL_TOKENS: usize = 60_000;

/// Source text handed to the question-setter per batch.
const EXCERPT_TOKENS: usize = 12_000;

const SET_SYSTEM: &str = "\
You are setting an exam on a passage, to test whether a summary of it kept what\n\
mattered. Write questions the passage answers plainly and specifically — a\n\
value, a rule, a procedure, a name, a trade-off and its condition. Never write\n\
a question whose answer is a matter of opinion, and never one that could be\n\
answered from general knowledge without having read this passage.\n\
\n\
Answer each question yourself, from the passage, in one or two sentences.\n\
\n\
Reply with a JSON array and nothing else:\n\
[{\"question\": \"...\", \"answer\": \"...\", \"section\": \"...\"}]\n\
where `section` names the part of the passage the question came from.\n";

const ANSWER_SYSTEM: &str = "\
You are being asked a question, and the text below is everything you have.\n\
Answer only from it. If it does not contain the answer, reply exactly:\n\
NOT COVERED\n\
Do not answer from your own knowledge — the point of the exercise is to find\n\
out what this text does and does not carry, and a fluent guess destroys that.\n";

const GRADE_SYSTEM: &str = "\
You are grading answers against a reference answer taken from the source.\n\
\n\
Mark PASS when the given answer carries the substance of the reference: the\n\
same value, rule, or conclusion. Wording may differ; specifics may not. Extra\n\
correct detail is fine.\n\
Mark FAIL when it contradicts the reference, misses its point, is too vague to\n\
act on, or says NOT COVERED.\n\
\n\
One line per question, numbered, and nothing else:\n\
<n>: PASS — <six words on why>\n\
<n>: FAIL — <six words on what was missing>\n";

#[derive(Debug, Deserialize)]
struct Question {
    question: String,
    answer: String,
    #[serde(default)]
    section: String,
}

#[derive(Debug, Serialize)]
pub struct Result_ {
    pub question: String,
    pub expected: String,
    pub given: String,
    pub section: String,
    pub passed: bool,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct EvalReport {
    pub skill: String,
    pub model: String,
    pub questions: usize,
    pub passed: usize,
    pub results: Vec<Result_>,
    /// Sections where at least one question failed, worst first. This is the
    /// part worth acting on: it names where the skill lost the source.
    pub weak_sections: Vec<String>,
}

impl EvalReport {
    pub fn pass_rate(&self) -> f64 {
        if self.questions == 0 {
            return 0.0;
        }
        self.passed as f64 / self.questions as f64
    }
}

/// Everything the skill would put in front of an agent that loaded it.
fn skill_context(skill: &Skill) -> Result<String> {
    let mut text = format!("# Skill: {}\n\n{}\n", skill.name(), skill.body);
    for relative in &skill.references {
        let path = skill.dir.join(relative);
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        text.push_str(&format!(
            "\n\n## Reference file: {}\n\n{contents}\n",
            relative.display()
        ));
    }
    if tokens::estimate(&text) > MAX_SKILL_TOKENS {
        bail!(
            "the skill is ~{} tokens, past the {MAX_SKILL_TOKENS} this can hold at once. \
             Run `anything-to-skill audit` — a skill this size is the problem, not the test.",
            tokens::estimate(&text)
        );
    }
    Ok(text)
}

/// Take `count` excerpts spread across the source, so the exam covers the whole
/// of it rather than whatever happened to be at the front.
fn excerpts(text: &str, count: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }
    let count = count.max(1);
    let stride = lines.len().div_ceil(count);
    let mut out = Vec::new();
    for start in (0..lines.len()).step_by(stride) {
        let mut excerpt = String::new();
        for line in &lines[start..lines.len().min(start + stride)] {
            if tokens::estimate(&excerpt) > EXCERPT_TOKENS {
                break;
            }
            excerpt.push_str(line);
            excerpt.push('\n');
        }
        if !excerpt.trim().is_empty() {
            out.push(excerpt);
        }
    }
    out
}

/// Pull a JSON array out of a reply that may be wrapped in prose or a fence.
fn extract_json_array(reply: &str) -> Result<&str> {
    let start = reply
        .find('[')
        .context("the model's answer contained no JSON array")?;
    let end = reply
        .rfind(']')
        .context("the model's answer contained no JSON array")?;
    if end <= start {
        bail!("the model's answer contained no JSON array");
    }
    Ok(&reply[start..=end])
}

/// Parse the grader's numbered lines back into verdicts.
fn parse_grades(reply: &str, count: usize) -> Vec<(bool, String)> {
    let mut grades = vec![(false, "the grader said nothing about this one".to_string()); count];
    for line in reply.lines() {
        let line = line.trim();
        let Some((number, rest)) = line.split_once(':') else {
            continue;
        };
        let Ok(index) = number.trim().trim_start_matches('#').parse::<usize>() else {
            continue;
        };
        if index == 0 || index > count {
            continue;
        }
        let rest = rest.trim();
        let upper = rest.to_ascii_uppercase();
        let passed = upper.starts_with("PASS");
        if !passed && !upper.starts_with("FAIL") {
            continue;
        }
        let reason = rest
            .split_once(['—', '-'])
            .map(|(_, why)| why.trim().to_string())
            .unwrap_or_default();
        grades[index - 1] = (passed, reason);
    }
    grades
}

/// Run the exam.
pub fn run(client: &Client, skill: &Skill, source_text: &str, wanted: usize) -> Result<EvalReport> {
    let context = skill_context(skill)?;

    // ------------------------------------------------------------ set the exam
    // Questions come in batches, one per excerpt, so every part of the source
    // gets asked about rather than only the part that fits in one call.
    let batches = excerpts(source_text, wanted.div_ceil(3).max(1));
    let per_batch = wanted.div_ceil(batches.len().max(1));
    let mut questions: Vec<Question> = Vec::new();

    for (index, excerpt) in batches.iter().enumerate() {
        eprintln!("  setting questions [{}/{}]", index + 1, batches.len());
        let prompt = format!(
            "Write exactly {per_batch} question(s) on this passage.\n\n\
             ---- passage ----\n{excerpt}\n---- end ----\n"
        );
        let reply = client
            .complete(SET_SYSTEM, &prompt, 4_000)
            .context("writing the questions")?;
        let json = extract_json_array(&reply)?;
        let batch: Vec<Question> =
            serde_json::from_str(json).context("reading the questions the model wrote")?;
        questions.extend(batch);
        if questions.len() >= wanted {
            break;
        }
    }
    questions.truncate(wanted);
    if questions.is_empty() {
        bail!("no questions could be written from the source");
    }

    // ------------------------------------------------------- sit the exam
    let mut given = Vec::new();
    for (index, q) in questions.iter().enumerate() {
        eprintln!("  answering [{}/{}]", index + 1, questions.len());
        let prompt = format!(
            "---- the text you have ----\n{context}\n---- end ----\n\nQuestion: {}\n",
            q.question
        );
        // A wrong answer is data, not a failure — record it and carry on.
        let answer = client
            .complete(ANSWER_SYSTEM, &prompt, 1_000)
            .unwrap_or_else(|err| format!("NOT COVERED (the attempt failed: {err})"));
        given.push(answer.trim().to_string());
    }

    // ---------------------------------------------------------------- grade
    eprintln!("  grading ...");
    let mut sheet = String::new();
    for (index, (q, answer)) in questions.iter().zip(given.iter()).enumerate() {
        sheet.push_str(&format!(
            "\n{}. Question: {}\n   Reference answer: {}\n   Given answer: {}\n",
            index + 1,
            q.question,
            q.answer,
            answer
        ));
    }
    let reply = client
        .complete(GRADE_SYSTEM, &sheet, 4_000)
        .context("grading the answers")?;
    let grades = parse_grades(&reply, questions.len());

    let results: Vec<Result_> = questions
        .into_iter()
        .zip(given)
        .zip(grades)
        .map(|((q, answer), (passed, reason))| Result_ {
            question: q.question,
            expected: q.answer,
            given: answer,
            section: q.section,
            passed,
            reason,
        })
        .collect();

    let passed = results.iter().filter(|r| r.passed).count();
    let mut weak: Vec<String> = results
        .iter()
        .filter(|r| !r.passed && !r.section.trim().is_empty())
        .map(|r| r.section.clone())
        .collect();
    weak.sort();
    weak.dedup();

    Ok(EvalReport {
        skill: skill.name(),
        model: client.model().to_string(),
        questions: results.len(),
        passed,
        results,
        weak_sections: weak,
    })
}

pub fn print(report: &EvalReport) {
    for (index, r) in report.results.iter().enumerate() {
        let mark = if r.passed { "pass" } else { "FAIL" };
        println!("{mark}  {}. {}", index + 1, r.question);
        if !r.passed {
            if !r.section.trim().is_empty() {
                println!("        from: {}", r.section);
            }
            println!("        source says: {}", one_line(&r.expected));
            println!("        skill says:  {}", one_line(&r.given));
            if !r.reason.is_empty() {
                println!("        {}", r.reason);
            }
        }
    }
    println!();
    println!(
        "{} — {}/{} ({:.0}%) against the source, judged by {}",
        report.skill,
        report.passed,
        report.questions,
        report.pass_rate() * 100.0,
        report.model
    );
    if !report.weak_sections.is_empty() {
        println!("\nThe skill did not carry these parts of the source:");
        for section in &report.weak_sections {
            println!("  - {section}");
        }
        println!("\nRebuild at a greater depth, or narrow the source, and run this again.");
    }
}

fn one_line(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 160 {
        format!("{}...", flat.chars().take(157).collect::<String>())
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_survives_a_code_fence() {
        let reply = "Here you go:\n```json\n[{\"question\":\"q\"}]\n```\nHope that helps.";
        assert_eq!(extract_json_array(reply).unwrap(), "[{\"question\":\"q\"}]");
    }

    #[test]
    fn a_reply_without_an_array_is_an_error() {
        assert!(extract_json_array("I could not do that.").is_err());
    }

    #[test]
    fn grades_are_read_back_by_number() {
        let grades = parse_grades(
            "1: PASS — carries the rule\n2: FAIL — missed the default\n",
            2,
        );
        assert!(grades[0].0);
        assert_eq!(grades[0].1, "carries the rule");
        assert!(!grades[1].0);
        assert_eq!(grades[1].1, "missed the default");
    }

    #[test]
    fn an_ungraded_question_fails_rather_than_passes() {
        // Silence must never read as approval.
        let grades = parse_grades("1: PASS — fine\n", 2);
        assert!(grades[0].0);
        assert!(!grades[1].0);
    }

    #[test]
    fn out_of_range_numbers_are_ignored() {
        let grades = parse_grades("7: PASS — nonsense\n1: PASS — fine\n", 2);
        assert!(grades[0].0);
        assert!(!grades[1].0);
    }

    #[test]
    fn excerpts_span_the_whole_text() {
        let text: String = (0..100).map(|n| format!("line {n}\n")).collect();
        let parts = excerpts(&text, 4);
        assert_eq!(parts.len(), 4);
        assert!(parts[0].contains("line 0"));
        assert!(parts[3].contains("line 99"));
    }
}
