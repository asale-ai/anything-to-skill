# anything-to-skill

Turn a book, a paper, a documentation site, or a repository into an agent skill.

Point your agent at a source and it reads the whole thing, then writes a skill
that carries what the source actually teaches — for Claude Code, Codex, Gemini
CLI, opencode, and anything else that loads skills from a directory.

## What it does

You ask. It reads. You get a skill.

> turn the Rust book's installation chapter into a skill —
> https://doc.rust-lang.org/book/ch01-01-installation.html

Underneath, your agent runs one command and gets text back — not a page of
markup with the documentation buried in it:

````
# Installation - The Rust Programming Language

source: https://doc.rust-lang.org/book/ch01-01-installation.html

Installation

The first step is to install Rust. We'll download Rust through rustup, a
command line tool for managing Rust versions and associated tools. [...]

Installing rustup on Linux or macOS

If you're using Linux or macOS, open a terminal and enter the following command:

```
$ curl --proto '=https' --tlsv1.2 https://sh.rustup.rs -sSf | sh
```
````

The navigation, the sidebar, the version switcher and the footer are gone. The
code block kept its indentation. The page says where it came from. That last
part matters more than it looks: a crawl concatenates dozens of pages, and a
claim you cannot trace is a claim you cannot check.

## Sources

| | |
|---|---|
| **A book, paper, or manual** | `anything-to-skill extract ~/books/ddia.pdf` |
| **A page or a paper** | `anything-to-skill extract https://arxiv.org/pdf/2501.00001` |
| **A documentation site** | `anything-to-skill extract https://docs.example.com/guide/ --crawl` |
| **A repository** | `anything-to-skill extract owner/repo` |

Run `anything-to-skill sources` for every accepted form — SSH remotes, `tree`
and `blob` URLs, branches, subdirectories.

**Sites** are crawled one request at a time, on the same host, at or below the
directory you named, bounded by `--max-pages` and `--depth`, and `robots.txt` is
honoured. When a site publishes its documentation as an `llms.txt`, that is read
instead of crawling — the site's own curated text, in one request:

```
$ anything-to-skill extract https://svelte.dev/ --crawl
crawling https://svelte.dev/ ...
  https://svelte.dev/llms-full.txt — reading it instead of crawling

extracted 1 document(s) from 1 source(s)
  1179728 characters, ~208777 tokens
```

**Repositories** are shallow-cloned and read in the order a person opens one:
the README, then `docs/`, then the rest. Prose only — pass `--include` when the
source itself is the documentation.

Whatever a run could *not* read is written to `metadata.json` and repeated on
screen: a crawl that stopped at its page limit, pages `robots.txt` withheld, PDF
pages with no extractable text. A skill whose gaps are stated is usable; one
that hides them is a trap for whoever loads it next.

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

On Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/asale-ai/anything-to-skill/main/install.ps1 | iex
```

No Node on the machine? Pipe to `SKILL=1 sh` instead — or set `$env:SKILL = '1'`
before the PowerShell line — and the installer places the skill itself, out of
the release archive it has already verified.

Both installers verify the download against the release's published SHA256 and
install nothing if it does not match. Set `BIN_DIR` (`$env:BIN_DIR`) to install
somewhere other than `~/.local/bin`; `$env:ADD_TO_PATH = '1'` puts that
directory on your PATH on Windows.

## Use

Ask your agent, and point at the source:

> turn this book into a skill — ~/books/designing-data-intensive-applications.pdf

> make a skill from the pytest docs — https://docs.pytest.org/en/stable/

It will read the source, ask what you want the skill *for* — quick reference,
a working guide, or deep study — and write the skill at that depth. When a PDF
page cannot be read as text, it renders that page and reads it as an image
instead of quietly leaving a hole.

## Formats

| | |
|---|---|
| **Documents** | PDF · DOCX · DOC · RTF · ODT |
| **Books** | EPUB |
| **Slides & sheets** | PPTX · PPT · XLSX · XLS · ODS · ODP · CSV |
| **Text** | TXT · Markdown · reStructuredText · AsciiDoc · HTML |
| **Web & code** | any URL · documentation sites · git repositories |

Repositories need [git](https://git-scm.com/downloads). Kindle (MOBI · AZW ·
AZW3) needs [Calibre](https://calibre-ebook.com/download). PDFs come out better
with [poppler](https://poppler.freedesktop.org/) installed (`brew install
poppler`) — run `anything-to-skill check` to see what you have.

---

[SKILL.md](SKILL.md) · [CONTRIBUTING.md](CONTRIBUTING.md) · [SECURITY.md](SECURITY.md) · Apache-2.0
