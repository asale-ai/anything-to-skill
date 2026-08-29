# @asale/anything-to-skill

Turn a book, a paper, a documentation site, or a repository into an agent
skill — and keep it worth loading.

```bash
npx @asale/anything-to-skill audit
```

| | |
|---|---|
| **`build`** | a source in, a finished skill out |
| **`audit`** | what a skill costs in every session, and whether it will ever fire |
| **`eval`** | ask the source's own questions of the skill, and see which it answers |
| **`refresh`** | the docs moved; rebuild, and say what changed |

Install it once instead of fetching it each time:

```bash
npm install -g @asale/anything-to-skill
```

## What this package is

A thin launcher. The tool itself is a Rust binary; this package downloads the
build for your platform from the project's GitHub release on install, checks it
against the release's published SHA256, and runs it.

Nothing is downloaded twice, and nothing is unpacked before its checksum
matches. `HTTP_PROXY`, `HTTPS_PROXY` and `NO_PROXY` are honoured.

Prebuilt for macOS and Linux on x64 and arm64 (glibc and musl), and Windows on
x64. Anywhere else: `cargo install anything-to-skill`.

## The rest

Full documentation, the skill itself, and the source:
<https://github.com/asale-ai/anything-to-skill>

Apache-2.0.
