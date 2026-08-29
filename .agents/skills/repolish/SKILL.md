---
name: repolish
description: >-
  Diagnose and improve what an open-source repository looks like to a first-time
  visitor. Use when asked to review, score, or improve a README, a repository's
  documentation, its "first impression", its discoverability, or its
  open-source hygiene (license, CI, contributing guide, issue templates) — and
  before rewriting any README by hand. Also use to generate the SVG overview
  card, the report card, SVG tables, or a VHS terminal recording for a CLI.
---

# repolish

A command-line tool that scores 22 concrete signals about a repository and, for
every point it deducts, names the file and line and says what to write instead.

**No model is in the scoring path.** The same commit always produces the same
score. You may suggest wording; you must never claim a number the tool did not
produce.

## The rule that matters most

**Do not rewrite the README.** Your instinct will be to replace it with a
well-structured one. That destroys the author's voice, their layout, their
examples, and usually their accuracy. Instead:

1. **Measure.** `repolish` — one command. It scores the repository and prints
   every file it would touch, and it writes **nothing**.
2. **Show the user that plan, then apply it.** `repolish --apply` — badge, table
   of contents, issue/PR templates, `CONTRIBUTING.md`, the CI workflow. It only
   ever *inserts*; the diff is new lines and nothing else.
3. **Hand back what needs judgement.** A missing quickstart, a vague tagline, a
   README claim whose command no longer exists — those need the author's
   knowledge, or a targeted edit you can justify from evidence.
4. **Measure again** and report the delta — `repolish --stages check --base <ref>`
   does the arithmetic for you and lists only the checks that moved.

Only edit prose directly when a finding names a specific line and the fix is
unambiguous, and say which finding you were acting on.

## Install

**Check first, then install.** Every command below calls `repolish` by name, so
start by finding out whether it is already there:

```bash
repolish --version
```

If that prints a version, use `repolish` as written throughout this document. If
it says "command not found", reach for npx — it needs nothing installed and
works wherever Node does:

```bash
npx -y @asale/repolish
```

Then read every `repolish …` below as `npx -y @asale/repolish …`, **for the rest
of the session**. The `-y` matters: without it npx stops to ask, and you are not
at a terminal to answer.

**npx does not install anything onto PATH.** It downloads into a cache and runs
from there, so `repolish` still will not exist after you have used it once — and
the tool's own output says `npx @asale/repolish …` back to you for exactly that
reason. Never drop the prefix partway through, and never tell the user to run
bare `repolish` unless `repolish --version` worked above.

That package is a launcher — it downloads the release binary, verifies its
`.sha256` and execs it, forwarding the exit code, which is what `--min-score`
depends on. The current version is 0.4.0.

If the user would rather have it on PATH permanently, this installs the binary
into `~/.local/bin` and drops this skill into whichever agents it finds:

```bash
curl -fsSL https://raw.githubusercontent.com/asale-ai/repolish/main/install.sh | sh
```

`cargo install repolish` works too, as does a release binary from
`https://github.com/asale-ai/repolish/releases` (five targets, each with a
`.sha256`).

## Commands

**There are no subcommands.** `repolish` is the whole surface; `--stages` picks
which parts of the pipeline run. The default is `check,polish,artifacts,ci`.

| Stage | What it does |
|---|---|
| `check` | Score the repository and print the report |
| `polish` | The mechanical fixes: badge, table of contents, issue/PR templates, `CONTRIBUTING.md` |
| `artifacts` | `.repolish/badge.json`, the banner, the overview and report cards, and every SVG the README already references |
| `ci` | `.github/workflows/repolish.yml` |
| `skill` | `SKILL.md` — opt-in, not in the default run |
| `demo` | Record the CLI — opt-in, and with `--apply` it **executes** the commands. A run that skipped it says so at the end |

### Run it

```bash
repolish                            # everything, and nothing is written
repolish --apply                    # write it
repolish --apply -v                 # also print every new file in full
repolish --stages check             # score only, no network
repolish --stages check --remote    # also read description / topics / homepage from GitHub
repolish --stages check --format json      # machine-readable, schema frozen at version 1
repolish --stages check --min-score 70     # exit 1 below the threshold, for CI
repolish --stages check --base origin/main # also score that ref, report only what moved
repolish --stages check --sarif out.sarif  # SARIF 2.1.0, for GitHub code scanning
```

`--remote` reads `GITHUB_TOKEN` or `GH_TOKEN`. Without a token it falls back to
60 anonymous requests per hour.

**Always run it once without `--apply` and show the user the plan.** The dry run
is free, and it is the whole safety story: nothing lands in the author's
repository that they have not seen first.

**Prefer `--format json` when you are going to act on the result.** The text
output is laid out for a human reading a terminal; the JSON is stable and tells
you, per check, the score, the evidence (file and line) and the fixes.

Shape of the JSON, abridged:

```json
{
  "score": 82,
  "coverage": 0.86,
  "mode": "remote",
  "categories": [{ "category": "credibility", "score": 90 }],
  "checks": [
    {
      "id": "license",
      "category": "credibility",
      "risk": "critical",
      "outcome": {
        "kind": "scored",
        "score": 0,
        "evidence": [{ "file": ".", "line": null, "note": "no LICENSE file" }],
        "fixes": [{ "severity": "P1", "message": "Add a LICENSE file" }]
      }
    }
  ],
  "coverageLimits": ["repo-topics: requires --remote"]
}
```

Read `score: null` as **"no score"**, not zero: it means fewer than half the
registered checks could run. Report that honestly rather than picking a number.

An outcome can also be `notApplicable`, `inconclusive` or `skipped`. Those are
excluded from the score on purpose. Never present them as passes.

### Fix what can be fixed

The `polish` stage is in the default run, so `repolish --apply` already does
this. To do it *without* the badge JSON and the CI workflow:

```bash
repolish --stages check,polish            # dry run: print the changes it would make
repolish --stages check,polish --apply    # write them
```

What it will do: the repolish badge (plus the `.repolish/badge.json` it points
at), a table of contents built from the author's own headings, GitHub issue and
PR templates, and a `CONTRIBUTING.md` whose build and test commands come from the
**detected package manifest**.

What it will **not** do: rewrite a single existing line, invent a build command
when there is no manifest, or write a code of conduct. Where it cannot know, it
does not write.

`--apply` refuses to run outside a git repository unless you pass `--force`,
because `git checkout` is the undo button. Never pass `--force` on the user's
behalf without saying so.

### Wording you are allowed to suggest

```bash
repolish --suggest                  # needs REPOLISH_LLM_API_KEY
```

This asks a model for the three pieces no mechanical rule can write: the
tagline, the quick start, the usage example. It **prints and never writes**, not
even with `--apply`.

You usually do not need it — you are a model, and you are already here. It
exists for the author running the CLI without you. If you are drafting those
sections yourself, hold to the same three rules it does: fill the gap, never
rewrite what is there, and never invent a command that is not in the manifest.

### The visuals

```bash
repolish --stages artifacts --apply                     # redraw everything already referenced
repolish --stages artifacts --apply --artifact overview # just .repolish/overview.svg
repolish --stages artifacts --apply --artifact score    # just .repolish/card.svg
repolish --stages artifacts --apply --theme porcelain   # light palette, for a light README
repolish --stages artifacts --apply --lang zh-CN        # en / zh-CN / ja; follows the README
repolish --stages artifacts --apply --remote --stars    # star history curve (~12 extra API calls)
```

The **overview card** goes at the top of the README: languages, file
composition, commit activity, licence. The **score card** goes at the bottom,
under a "Polished with repolish" heading. Do not swap them — the top of a README
belongs to the project, not to our tool.

To have `polish` insert them, and render README tables as SVG with the original
folded into `<details>`:

The cards and the SVG tables are **on by default**, so a plain `--apply` inserts
them. `--no-visuals` leaves the README's visuals alone — reach for it when the
author has a README they clearly art-directed themselves.

```bash
repolish --apply                    # cards and SVG tables included
repolish --apply --no-visuals       # leave them alone
repolish --apply --logo assets/hero.svg --logo-width full --align center
```

Every SVG is self-contained and deterministic: no external fonts, no scripts,
nothing hosted by a third party, and the same commit renders a byte-identical
file.

### Record a CLI

```bash
repolish --stages demo                  # list what it would run, run nothing
repolish --stages demo --apply          # run them, and write .repolish/demo.svg
repolish --stages demo --apply --cmd "tool build" --cmd "tool run"
repolish --stages demo --apply --tape   # also write a VHS tape, for a GIF instead
```

**With `--apply` this executes the commands.** That is the point — the output in the
recording is real — but it means you must not run it against a repository whose commands
you have not looked at. Run it without `--apply` first and show the user the list. Never
pass `--cmd` with anything destructive.

The output is an animated SVG with a real text layer, not a GIF, and it needs no external
tools. Only meaningful when the project actually has a binary; the tape is a plain text
file the author is expected to edit.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Score below `--min-score` |
| 2 | Bad arguments |
| 3 | Not a valid repository |
| 4 | `--remote` failed (API error, rate limit, private repo) |
| 5 | Less than half the total weight could be scored — no total score is reported |
| 7 | `--base` could not be resolved: shallow clone, unknown ref, no git |

Codes 4 and 7 are deliberately distinct from code 1. A rate limit and a shallow
clone are not quality regressions, and must never be reported as one. **Say which
happened**; do not summarise either of them as "the check failed".

## Making the calls repolish cannot

This is the part of the job that is actually yours, so it is worth being precise about
where the tool stops.

repolish decides three different ways, and only two of them are strong:

1. **Facts.** Does a LICENSE file exist, is there a workflow, how many headings are
   there. Filesystem and Markdown AST. Not arguable.
2. **Cross-references.** `claim-consistency` takes the commands out of the README and
   checks them against the manifest and the filesystem. `readme-install-consistency`
   checks that the install command installs *this* package. These are joins between two
   sources of truth, not opinions — and they are the checks worth acting on first.
3. **Graded heuristics.** `readme-quickstart` scores 0/4/6/8/10 from hand-curated
   substring lists. Most of the README checks have a list like that somewhere.

So the score is honestly a measure of **whether the machinery a reader needs is present
and whether the promises are true.** It is not a measure of whether the writing is any
good. repolish will happily give 10/10 to a quickstart made of the right keywords in the
wrong order.

That is the gap you are here to close. **Do not try to close it by rewriting.** Work
finding by finding:

| Finding | What a good fix looks like | The failure mode to avoid |
|---|---|---|
| `claim-consistency` | Make the claim true (restore the script, add the npm script) or correct the text to what actually works | **Deleting the line.** That turns the check green and leaves the reader with no instructions at all |
| `readme-title-tagline` | One line saying what it does and who it is for, in the author's register | Replacing a specific tagline with "A blazingly fast, modern toolkit for…" |
| `readme-quickstart` | The shortest path from zero to one working result, with the prerequisite named | Turning it into a feature tour, or inventing a command you have not run |
| `readme-usage-example` | A real example lifted from tests or `examples/` | Inventing an API that does not exist. Check it compiles or runs |
| `readme-length` | Move reference detail into `docs/` and link it | Deleting the detail |
| `license` | Tell the author their options and let them choose | Picking one for them. It is a legal decision, not a formatting one |
| `code-of-conduct` | Ask for a real reporting address | A Contributor Covenant with a placeholder email promises a channel that does not exist |
| `contributing` | Take build and test commands from the detected manifest | `<your build command here>` |
| `repo-description` / `repo-topics` / `repo-homepage` | Draft the text and hand it to the author | Changing repository settings yourself |

Three rules that hold across all of them:

- **Cite the finding.** Every edit you make should be traceable to an id and a
  file:line that repolish reported. If you cannot name one, you are redecorating.
- **Leave the voice alone.** Match the surrounding register, list markers, heading
  depth and line width. A README that suddenly reads like documentation-as-a-service is
  a worse README even when it scores higher.
- **A higher score is not the goal.** The goal is a repository a stranger can use. If a
  change would raise the number without helping that stranger, do not make it, and say
  why.

### Why there is no model inside repolish

If you are wondering whether to suggest wiring an LLM into it: the scoring path is
deliberately model-free, and that is not conservatism. A badge whose number moves because
a model answered differently this morning is worth nothing, and the same commit has to
produce the same score for the number to be comparable between repositories at all.

The intended arrangement is the one you are already in: **repolish supplies evidence,
you supply judgment.** You have context it structurally cannot have — the codebase, the
user's intent, this conversation. It has determinism you cannot have. Neither half is
improved by moving it into the other.

## Things to get right

- **Local and remote scores are not comparable.** Without `--remote`, three
  discoverability checks drop out of the denominator. Say which one you ran.
- **Do not tune thresholds.** `.repolish.toml` deliberately does not expose
  per-check thresholds; the check set and weights are frozen for v1 so scores
  stay comparable between repositories.
- **`claim-consistency` is the check to take seriously.** It verifies that the
  commands the README promises actually exist — `npm run build` in
  `package.json`, `make test` as a real target, `./scripts/setup.sh` as a real
  file. A README that fails on its first command is where readers leave. Fix
  those first, and fix them by making the claim true or by correcting it — never
  by deleting the line to make the check pass.
- **Report the numbers the tool gave you.** If it says `not scored`, say
  `not scored`.
