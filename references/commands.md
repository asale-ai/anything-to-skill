# Command reference

Every command and the flags that matter. `--help` on any of them is
authoritative; this is the version with the reasoning attached.

Only `build` and `eval` use a model. They read `ANTHROPIC_API_KEY` from the
environment, never from an argument — a key on a command line ends up in the
shell history of whoever ran it. `ANTHROPIC_BASE_URL` points at a proxy;
`ANYTHING_TO_SKILL_MODEL` sets the default model.

---

## extract — source in, text out

```bash
anything-to-skill extract <source> [<source> ...] [--out DIR]
```

Writes `full_text.txt` and `metadata.json`. Defaults to
`$ANYTHING_TO_SKILL_WORKDIR`, else a temp directory.

| Flag | Applies to | Default | Notes |
|---|---|---|---|
| `--crawl` | URLs | off | Follow links on the same site at or below the directory named. Without it, only the page you named is read. |
| `--max-pages N` | crawls | 50 | Where a crawl stops. The report says when it did. |
| `--depth N` | crawls | 3 | How many links deep to follow. |
| `--delay-ms N` | crawls | 250 | Pause between requests. Being fast is not worth being blocked. |
| `--no-llms-txt` | crawls | off | Crawl even when the site publishes an `llms.txt`, which is otherwise read instead — one request for text the site curated itself. |
| `--branch REF` | repos | default branch | Overrides one named in the source. |
| `--max-files N` | repos | 200 | Where a repository read stops. |
| `--include GLOB` | repos | prose formats | Repeatable. Pass it when the code *is* the documentation: `--include 'src/**/*.py'`. |
| `--exclude GLOB` | repos | — | Repeatable. |
| `--engine E` | files | `builtin` | `docling` hands PDFs and Office formats to Docling instead (`pip install docling`): slower, stronger on tables and layout. |

Crawls run one request at a time, stay on the same host at or below the
directory named, and honour `robots.txt`.

---

## build — source in, skill out

```bash
anything-to-skill build <source> ... [--out DIR] [--name NAME] [--purpose P]
```

Takes every `extract` flag, plus:

| Flag | Default | Notes |
|---|---|---|
| `--purpose` | `working-guide` | `reference`, `working-guide`, or `deep-study`. Not `--depth`, which is the crawler's and means something else. |
| `--name` | derived from the source | The skill directory's name. |
| `--out` | `.` | The skill is created at `<out>/<name>/`. |
| `--model` | `$ANYTHING_TO_SKILL_MODEL` | |
| `--work DIR` | temp | Where the intermediate extraction is kept, so a failed build can be inspected. |
| `--dry-run` | off | Extract, print the plan, make no requests, write nothing. Needs no API key. |

It reads in two passes — notes per section, then the skill from the notes — so
nothing has to hold the whole book and the whole answer at once. It writes
`.a2s.lock` recording the sources and how they were read, then audits what it
wrote.

---

## audit — grade a skill. No key, no network.

```bash
anything-to-skill audit [PATH ...] [--body-budget N] [--json] [--strict]
```

With no path, it finds the agent skills directories on the machine
(`~/.claude/skills`, `~/.agents/skills`, and the project-local equivalents) and
grades everything in them, counting a skill symlinked into several of them once.

It reports three separate costs, because they are not paid at the same time:

- **always-loaded** — `name` and `description`, in every session, fired or not
- **on trigger** — the `SKILL.md` body, when the skill fires
- **on demand** — `references/`, only if the body sends you there

| Rule | Severity | What it means |
|---|---|---|
| `no-frontmatter` | error | Nothing will load it. |
| `no-description` | error | Nothing to match against. |
| `description-too-long` | error | Over 1024 characters; the loader rejects it. |
| `description-too-short` | warning | Too little to route on. |
| `description-not-routable` | warning | Names the subject, never the situation. |
| `name-mismatch` | warning | Frontmatter and directory disagree; tools disagree about which wins. |
| `name-not-kebab-case` | warning | Some loaders reject anything else. |
| `body-over-budget` | warning / error | Past `--body-budget` (2000), or past twice it. |
| `broken-reference-link` | error | The body links a file that does not exist. |
| `orphan-reference` | note | A reference file nothing links to or names. |
| `empty-reference` | warning | A reference file with nothing in it. |
| `duplicate-name` | error | Two skills with one name; only one can load. |
| `trigger-overlap` | warning | Two descriptions built from the same words. Routing between them is a coin flip. |
| `no-progressive-disclosure` | note | One file, so firing the skill pays for all of it. |

Exits non-zero on any error, and on warnings too under `--strict`.

---

## eval — test the skill against the source it came from

```bash
anything-to-skill eval <skill-dir> [--against full_text.txt] [--questions N]
                                   [--min-pass PCT] [--json]
```

Questions are set from the source, the skill answers with nothing but itself,
and the answers are graded against the source. Without `--against`, the sources
in `.a2s.lock` are re-read.

The pass rate is the headline; the failures are the point. Each names the
section its question came from, so the report ends with the parts of the source
the skill did not carry. `--min-pass` exits non-zero below that percentage.

---

## refresh — rebuild when the source moves

```bash
anything-to-skill refresh <skill-dir> [--check] [--model M]
```

Re-reads the sources from `.a2s.lock`, replaying the flags the original build
used — a crawl re-run with different limits is a different document, and the
diff would be about the flags rather than about the docs.

`--check` reports and changes nothing, exiting 1 when the source moved. That is
what makes it usable as a scheduled job that opens a pull request. Without it,
the skill is rebuilt and the change is recorded in the skill's own
`CHANGELOG.md`.

Every document carries its own fingerprint, so the report names which pages
moved rather than only that something did.

---

## mcp — the same tools, for agents with no shell

```bash
anything-to-skill mcp [--work DIR]
```

Speaks JSON-RPC over stdin and stdout, exposing `extract`, `read_text`,
`audit` and `sources`. `read_text` pages through the extraction so the text
reaches an agent that cannot open a file.

`build` and `eval` are deliberately absent: an agent connected over MCP already
is the model, and handing it a second one to pay for would be nonsense.

---

## render, check, formats, sources

```bash
anything-to-skill render <pdf> --pages 3,17,42 --out DIR [--dpi 150]
anything-to-skill check          # which optional external tools are present
anything-to-skill formats        # every extension accepted
anything-to-skill sources        # every kind of source accepted, with examples
```

`render` needs `pdftoppm` (poppler). 150 dpi keeps dense body text legible while
staying inside a model's per-image budget.
