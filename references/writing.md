# Writing the skill

Open this once you trust the extraction.

---

## 1. Ask what the skill is for

One question, and it genuinely changes the output:

> "What do you want this skill to do for you?
>
> 1. **Reference** — look things up fast; you already know the material
> 2. **Working guide** — apply the book's methods to real tasks
> 3. **Deep study** — learn it thoroughly, including the reasoning"

| Answer | Per chapter | Keep | Cut |
|---|---|---|---|
| Reference | ~400 tokens | Definitions, tables, commands, syntax | Reasoning, anecdotes, worked examples |
| Working guide | ~1,000 tokens | Procedures, worked examples, decision rules | History, digressions, proofs |
| Deep study | ~2,500 tokens | Reasoning, worked examples, counter-arguments | Little — preserve the argument |

If the user does not answer, default to **working guide**. It is the most
generally useful and the easiest to trim later. (`build --purpose` takes the
same three values.)

---

## 2. Read the source and take notes

Read `full_text.txt` a section at a time — chapter by chapter for a book, page
by page for a site, file by file for a repository. For each section, capture:

- **What it claims** — the actual position, not the topic it covers
- **Why** — the reasoning or evidence, at whatever depth step 1 set
- **How to apply it** — the procedure, rule, or checklist a reader would use
- **One worked example** — the single highest-value thing to carry over, and
  what readers return to a book for

Use the source's own vocabulary and its own examples. Keep exact names:
commands, flags, functions, parameters, defaults, versions. Record caveats and
disagreements — a skill that carries only conclusions cannot tell its reader
when they stop applying.

---

## 3. Write it

```
<skill-name>/
  SKILL.md          # what it is, when to use it, and a map of the rest
  references/       # tables, syntax, commands — anything looked up, not read
```

The frontmatter needs `name` and a `description` that says **when to use it**,
not just what it is. The description is the only part an agent sees before
deciding whether to load the skill; one that names the subject but never the
situation will not fire. Put the words a user would actually type in it.

Keep `SKILL.md` under about 2,000 tokens. It is a map, not the territory:
everything looked up rather than read belongs in `references/`, where it costs
nothing until it is opened. Link every reference file from the body with a
sentence saying when to open it — a file nothing links to will never be read.

Where a claim came from a source with a URL, cite it inline. Where you can name
the chapter, name it. A reader who cannot trace a claim cannot check it.

Write for an agent that has not read the book. Spell out the author's shorthand.
Prefer the source's own examples over invented ones.

Then grade what you wrote:

```bash
anything-to-skill audit ./<skill-name>
```

---

## 4. Report honestly

When you hand the skill over, say plainly:

- What it covers and what it deliberately leaves out
- Any pages that could not be read, and whether you read them as images
- Every note from `sources[]` — a crawl that stopped at its page limit, pages
  robots.txt withheld, files a limit dropped. These are the difference between
  "this skill knows the manual" and "this skill knows part of the manual"
- Anything the spot-check flagged
- Where the source's own coverage was thin
- For a site or repository, when it was read. Documentation moves; a skill
  built from it is a snapshot and should say so. `anything-to-skill refresh
  --check` answers "has it moved since" later.

A skill whose limits are stated is usable. One that hides them is a trap for
whoever loads it next.
