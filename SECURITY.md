# Security Policy

## Reporting a vulnerability

Report privately through
[GitHub Security Advisories](https://github.com/asale-ai/anything-to-skill/security/advisories/new).
Please do not open a public issue for a vulnerability.

Include what you can: the version, the input that triggers it, and what happens.
A document that reproduces the problem is the most useful thing you can send —
if it is sensitive, say so and we will arrange another way to receive it.

You can expect an acknowledgement within 3 working days and an assessment within
10. If a fix is warranted we will credit you in the advisory unless you would
rather we did not.

## Supported versions

Fixes go onto the latest release. There are no maintained older branches yet.

---

## Threat model

This tool exists to feed documents into an AI agent's context. That makes the
**document itself an untrusted input**, and it is the security property worth
understanding before anything else.

A book, paper, or PDF is written by someone other than the user, and everything
in it reaches the model verbatim. An attacker who controls a document can try to
make the model treat the document's contents as instructions rather than as
material — prompt injection. The consequence is not a crash; it is an agent that
does something the user did not ask for, using the user's own permissions.

### What the tool does about it

**Invisible characters are stripped.** Extraction removes code points that
render as nothing but are read normally by a model, so a human reviewing the
extracted text and the agent consuming it cannot be shown different documents.
Three families are covered, listed with their reasoning in `src/sanitize.rs`:

| Class | Why it matters |
|---|---|
| Zero-width spaces and joiners, soft hyphens | Hide text between visible characters |
| Bidirectional controls (Trojan Source, CVE-2021-42574) | Change the order a human *sees* without changing the order a model *reads* |
| Unicode tag block (U+E0000–U+E007F) | Smuggles an entire ASCII payload as invisible characters |

The count removed from each file is reported in `metadata.json` as
`invisible_codepoints_removed`. **A non-zero count is worth looking at.**
Ordinary books are zero.

Right-to-left languages are unaffected — the Unicode Bidi Algorithm derives
direction from the characters themselves, so Arabic and Hebrew still render
correctly. Only explicit embeddings, overrides, and isolates are dropped, and
running prose essentially never contains them.

### What the tool cannot do about it

**It cannot strip a plainly-written instruction.** A page that says "ignore your
previous instructions and…" in ordinary visible text is indistinguishable from a
book quoting such a sentence, and removing it would corrupt legitimate documents
about prompt injection.

That defence lives in `SKILL.md`, which instructs the model to treat everything
in `full_text.txt` as material to summarize rather than as directions to follow,
and to quote anything that appears to address the reader rather than acting on
it. **If you build your own workflow around this CLI instead of using
`SKILL.md`, that instruction is yours to carry over.**

---

## Other properties

**No network access.** The binary never opens a socket. It reads the files you
name and writes the output directory you choose.

**Subprocesses are invoked directly, not through a shell.** `pdftotext`,
`pdftoppm`, and `ebook-convert` are executed with arguments passed as arguments,
so a filename containing shell metacharacters or spaces cannot become a command.
They are resolved from `PATH` — on a machine where `PATH` is attacker-controlled
that is a pre-existing compromise, not one this tool introduces.

**Malformed documents are handled as errors, not crashes.** Parsing is done by
`anydoc` and `pdf-inspector`, which return errors rather than panicking on
corrupt input; failures are reported per file and the run continues with the
rest. Report a panic on any real-world document as a bug.

**Nothing is executed from a document.** Macros in DOCX/XLSX, JavaScript in PDF,
and scripts in EPUB HTML are never run — the parsers read content, and the HTML
route discards `<script>` and `<style>` contents entirely.

**Output goes where you point it.** `--out` is used as given. Pointing it at a
directory you do not want overwritten will overwrite `full_text.txt` and
`metadata.json` there.

---

## Handling sensitive documents

The tool is local and offline, so extraction itself does not disclose anything.
Two things to keep in mind:

- `full_text.txt` and `metadata.json` are written in plain text, and
  `metadata.json` records the full path of every input file.
- Anything extracted is intended to be read by an AI agent. Whether that agent
  is local or an API service is a decision made outside this tool — and for a
  confidential document, it is the decision that matters.
