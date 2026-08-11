# Contributing

Thanks for taking an interest. This document covers building the project,
how it is put together, and what a good change looks like.

## Build

Rust 1.88 or newer (edition 2024, and the code uses let-chains).

```bash
git clone https://github.com/asale-ai/anything-to-skill
cd anything-to-skill
cargo build --release
cargo test
```

The binary lands at `target/release/anything-to-skill`.

There is nothing to install first — every parser is a Rust crate, and the
dependency tree has no C build scripts, so cross-compilation works without
`cross` or a system toolchain.

For the full check the CI runs:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Architecture

Two halves, kept apart on purpose:

| | Runs | Contains |
|---|---|---|
| `SKILL.md` | The model | Judgment: what matters in a book, what to keep, how to say it |
| `src/` | The machine | Determinism: read the file, get the text out, report what failed |

Work that has a right answer belongs in the binary. Work that needs taste
belongs in the skill. When adding something, decide which side it is on first —
most design mistakes here are putting judgment in code or mechanics in prose.

```
src/
  main.rs         CLI: extract / check / render / formats
  config.rs       Extension → route
  extract/
    mod.rs        Dispatch; HTML, Calibre, and anydoc routes
    pdf.rs        Per-page PDF routing
  clean.rs        Running headers, page numbers, dehyphenation
  sanitize.rs     Invisible code points
  structure.rs    Chapter + table-of-contents detection
  tokens.rs       CJK-aware token estimation
  report.rs       metadata.json and the run summary
```

### How PDF extraction decides

No single PDF extractor wins on every page, so `extract/pdf.rs` routes per page.
Measured on two real papers:

| | Multi-column reading order | Borderless wide tables | 12 pages |
|---|---|---|---|
| `pdf-inspector` | correct | flattened | 0.03s |
| `pdftotext -layout` | columns interleaved | alignment preserved | 0.4s |
| `pdftotext` (no flag) | correct | columns lost | 0.4s |

So `pdf-inspector` reads the document, and `pdftotext -layout` is re-run on just
the pages that need it, appended under a marker rather than replacing the page —
the Markdown still carries the correct reading order for the prose around the
table.

The `pages_with_columns` signal needs care, because it means two different
things. On a genuinely two-column paper nearly every page is flagged, and
`-layout` there is actively harmful. On a single-column book a handful of
flagged pages are almost always wide tables that the column detector saw as
columns — exactly the pages worth supplementing. The document-level ratio
separates the two, and both cases are pinned by tests in `extract/pdf.rs`.

Pages that cannot be read at all are never guessed at. They surface in
`metadata.json` as `needs_visual_reading`, and the skill renders them for the
model to read directly.

## Testing

Unit tests live beside the code they cover. Run `cargo test`.

If you change extraction behaviour, test it against a real document, not a
synthetic one. Synthetic files agree with whatever you expected; real books are
where the bugs are. Two useful sources:

- Any arXiv paper — two-column layout and borderless tables in one file
- [Project Gutenberg](https://www.gutenberg.org/) EPUBs — real chapter structure

When a bug comes from a specific document shape, add a test that encodes the
shape rather than the file, so the fix stays pinned without committing a book.
The tests in `extract/pdf.rs` are the model to follow: they carry the measured
page numbers from the papers they came from, and say so in a comment.

## Adding a format

1. Add the extension to the right list in `config.rs`
2. If an existing route handles it, that is the whole change — `anydoc` already
   covers most office and ebook formats
3. Otherwise add a `Route` variant and a branch in `extract/mod.rs`
4. Test against a real file of that format

## Adding a language to chapter detection

`structure.rs` detects chapter headings across Latin, Roman-numeral, Chinese,
Japanese, Thai, and Korean conventions. To add another:

1. Add the pattern next to the existing ones, with a comment explaining what a
   heading looks like in that language and — importantly — what a *prose
   cross-reference* looks like, so the two can be told apart
2. Add tests for both: one heading that must match, one cross-reference that
   must not

The false-positive case matters more than the true-positive case. Over-detecting
chapters silently corrupts the structure report; under-detecting is visible.

## Style

Match the code around you. Beyond that:

- Comments explain *why*, not *what*. If a constant, a threshold, or a regex has
  a reason behind it, write the reason down — several of the ones here look
  arbitrary and are not.
- Error messages tell the user what to do next. `anything-to-skill` is often run
  by an agent that cannot ask a follow-up question, so "install poppler" beats
  "extraction failed".
- Prefer failing loudly over emitting plausible-looking wrong output. A skill
  built on silently corrupted text is worse than no skill.

## Pull requests

- One change per PR
- `cargo fmt`, `cargo clippy`, and `cargo test` all clean
- Say what you tested it against, especially for extraction changes

## Releasing

Releases are cut by pushing a tag; `.github/workflows/release.yml` builds every
platform and attaches the archives.

```bash
# Update the version in Cargo.toml first, then:
git tag v0.2.0
git push origin v0.2.0
```
