# anything-to-skill

Turn a book, paper, or manual into an agent skill — for Claude Code, Codex,
Gemini CLI, opencode, and anything else that loads skills from a directory.

Point it at a PDF and you get back a skill your agent can load later.

---

## Install

Download the binary for your platform from the
[latest release](https://github.com/asale-ai/anything-to-skill/releases/latest),
or install with one line:

```bash
curl -fsSL https://raw.githubusercontent.com/asale-ai/anything-to-skill/main/install.sh | sh
```

There is no runtime to install — no Python, no Node, no virtualenv. One binary,
about 9 MB, with every document parser built in.

<details>
<summary>Other ways to install</summary>

```bash
# From crates.io
cargo install anything-to-skill

# From source
git clone https://github.com/asale-ai/anything-to-skill
cd anything-to-skill && cargo install --path .
```
</details>

### Install the skill

Copy `SKILL.md` into wherever your agent reads skills from:

```bash
mkdir -p ~/.claude/skills/anything-to-skill
curl -fsSL -o ~/.claude/skills/anything-to-skill/SKILL.md \
  https://raw.githubusercontent.com/asale-ai/anything-to-skill/main/SKILL.md
```

Then just ask: *"turn this book into a skill"* and point at the file.

---

## Use it directly

You don't need an agent — the CLI is useful on its own.

```bash
# Pull the text out of anything
anything-to-skill extract book.pdf --out ./work
```

Two files land in `./work`:

- **`full_text.txt`** — the text
- **`metadata.json`** — how many chapters, how many tokens, and which pages
  could not be read

```bash
# What's installed?
anything-to-skill check

# Turn specific pages into images (for pages that have no text layer)
anything-to-skill render book.pdf --pages 3,17 --out ./work/pages

# What can it read?
anything-to-skill formats
```

---

## Formats

Nothing extra to install:

| | |
|---|---|
| **Documents** | PDF · DOCX · DOC · RTF · ODT |
| **Books** | EPUB |
| **Slides & sheets** | PPTX · PPT · XLSX · XLS · ODS · ODP · CSV |
| **Text** | TXT · Markdown · reStructuredText · AsciiDoc · HTML |

**Kindle** (MOBI · AZW · AZW3) needs [Calibre](https://calibre-ebook.com/download).

Two optional extras make PDFs better — install
[poppler](https://poppler.freedesktop.org/) to recover tables from tricky pages
and to render pages as images:

```bash
brew install poppler              # macOS
sudo apt install poppler-utils    # Debian / Ubuntu
```

Run `anything-to-skill check` to see what you have.

---

## Why PDFs come out right

PDFs are the hard case, and most tools pick one strategy for the whole file.
This one decides page by page:

- **Two-column papers** keep their reading order — the left column is not
  interleaved into the right one.
- **Tables** survive, including the borderless kind that academic papers use.
- **Superscripts** stay superscripts: `O(n² · d)`, not `O(n2 · d)`.
- **Scanned pages** are not guessed at. They are reported, and your agent reads
  them as images — which it does far better than any text extractor.

A 12-page paper takes about 0.03 seconds.

---

## Docs

- [SKILL.md](SKILL.md) — the skill itself
- [CONTRIBUTING.md](CONTRIBUTING.md) — building, testing, and how it works inside
- [SECURITY.md](SECURITY.md) — reporting a vulnerability, and what the tool does
  with untrusted documents

## Credits

Text extraction rests on two MIT-licensed Rust crates from
[Firecrawl](https://github.com/firecrawl):
[`anydoc`](https://github.com/firecrawl/anydoc) and
[`pdf-inspector`](https://github.com/firecrawl/pdf-inspector).

## License

MIT
