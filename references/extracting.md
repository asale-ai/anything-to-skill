# Reading the extraction

Open this after `anything-to-skill extract` has run and before you read a word
of `full_text.txt`.

---

## 1. Read the report first

`metadata.json` tells you what you are about to read and, more importantly,
what is missing from it. Start with `sources[]`: each entry carries `notes`
naming exactly where the read is incomplete — a crawl that stopped at its page
limit, pages a site's `robots.txt` put off limits, repository files dropped by
`--max-files`. Those notes are the honest limits of the skill you are about to
write, and your final report has to repeat them.

Two notes are worth acting on before you read anything:

- **The crawl hit `--max-pages`.** Raise the limit and re-run, or narrow the
  starting URL. Do not quietly build a skill from the first 50 pages of a
  400-page manual.
- **Pages were outside the starting directory.** The URL was aimed one level
  too deep — the note names the URL to start from instead. Re-run from there.

Both mean the same thing: what you have is not the documentation, it is part of
it. Fix the command rather than the report.

---

## 2. Handle pages that could not be read

If `metadata.json` contains `needs_visual_reading`, those pages produced no
usable text — scanned images, or fonts with broken encodings. They are **absent
from `full_text.txt`**. A skill built without them has a hole in it and will
not say so.

Render and read them yourself:

```bash
anything-to-skill render <path> --pages 3,17,42 --out /tmp/anything_to_skill_work/pages
```

Then read the emitted PNGs with your image-reading capability and fold what
they contain into your understanding of the book. You read pages directly far
better than any text extractor reads a scan — this is the right division of
labour, not a workaround.

If there are more than ~30 such pages, say so before starting: reading them is
slower and costlier than the text path, and the user may prefer to run OCR over
the file first (`ocrmypdf in.pdf out.pdf`) and re-extract.

---

## 3. Spot-check the extraction

Extraction failures are usually silent: text that is present but wrong reads
exactly like text that is right. Before building anything on it, check a
sample.

Pick 2–3 pages from `pages_with_columns` or `pages_with_tables` in
`metadata.json` — those are where extraction is hardest — render them, and
compare each against the corresponding passage in `full_text.txt`.

```bash
anything-to-skill render <path> --pages 9,42 --out /tmp/anything_to_skill_work/check
```

Look for: columns interleaved into each other, table rows with values in the
wrong column, dropped paragraphs, garbled characters. If a sample is wrong, say
so plainly and stop — do not build a skill on text you have reason to distrust.

Three images is a small price for not shipping a confidently wrong skill.

When a PDF's tables matter and the built-in reader flattens them, `--engine
docling` hands that file to Docling instead, if it is installed
(`pip install docling`). It is slower and stronger on layout.

**For a web or repository source**, the failure mode is different: not garbled
text but the wrong text. Read the first page of `full_text.txt` and check that
what you got is the documentation and not the site's navigation, cookie banner
and footer repeated once per page. Each crawled page carries a `source:` line
under its heading — if a page's body is a list of link labels, the extractor
did not find that site's content element, and the skill would be built from
furniture. Say so rather than proceeding.

---

## 4. Size the work

From `metadata.json`:

- `estimated_tokens` — how much text there is
- `structure.chapters_detected` — how many chapters were found
- `structure.has_toc` — whether the book carries a table of contents

Tell the user the size and how you plan to proceed. If the text is larger than
you can hold at once, work chapter by chapter and keep notes as you go rather
than trying to summarize the whole thing in one pass.

If `chapters_detected` is 0 or 1 on a book that plainly has chapters, the
headings are unusual — find the structure by reading rather than trusting the
count.

A site or a repository has no chapters; its unit is the page or the file, and
`sources[].documents` says how many there are. Each one is headed by its own
title and `source:` line in `full_text.txt`, so use those as the seams.
