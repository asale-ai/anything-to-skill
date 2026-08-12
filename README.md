# anything-to-skill

Turn a book, paper, or manual into an agent skill.

Point your agent at a PDF and it reads the whole thing, then writes a skill that
carries what the book actually teaches — for Claude Code, Codex, Gemini CLI,
opencode, and anything else that loads skills from a directory.

## Install

The skill, into every agent tool on the machine:

```bash
npx skills add asale-ai/anything-to-skill --all -g
```

That writes `~/.agents/skills/anything-to-skill/` — the path Codex, Cursor,
Gemini CLI, and opencode read directly — and symlinks it into the ones that look
elsewhere, Claude Code's `~/.claude/skills/` among them. Drop `-g` to install
into the current project instead.

The skill drives a small binary that does the reading:

```bash
curl -fsSL https://raw.githubusercontent.com/asale-ai/anything-to-skill/main/install.sh | sh
# or, if you have Rust: cargo install anything-to-skill
```

No Node on the machine? Pipe that to `SKILL=1 sh` instead and the installer
places the skill itself, out of the release archive it has already verified.

## Use

Ask your agent, and point at the file:

> turn this book into a skill — ~/books/designing-data-intensive-applications.pdf

It will read the book, ask what you want the skill *for* — quick reference,
a working guide, or deep study — and write the skill at that depth. When a page
cannot be read as text, it renders that page and reads it as an image instead of
quietly leaving a hole.

## Formats

| | |
|---|---|
| **Documents** | PDF · DOCX · DOC · RTF · ODT |
| **Books** | EPUB |
| **Slides & sheets** | PPTX · PPT · XLSX · XLS · ODS · ODP · CSV |
| **Text** | TXT · Markdown · reStructuredText · AsciiDoc · HTML |

Kindle (MOBI · AZW · AZW3) needs [Calibre](https://calibre-ebook.com/download).
PDFs come out better with [poppler](https://poppler.freedesktop.org/) installed
(`brew install poppler`) — run `anything-to-skill check` to see what you have.

---

[SKILL.md](SKILL.md) · [CONTRIBUTING.md](CONTRIBUTING.md) · [SECURITY.md](SECURITY.md) · Apache-2.0
