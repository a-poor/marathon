---
name: marathon
description: >-
  Author, edit, and review marathon runbooks — markdown documents whose fenced
  code blocks are runnable cells. Covers the runbook format: YAML frontmatter
  (title, description, env, before_each/after_each, interpreters, tmp_dir), shell
  cells and the `skip=true` / display-only rules, `json mrthn=input` special blocks
  (input/select/confirm), the env/state model (frontmatter env → CLI --env →
  $TMP_DIR → preceding answered inputs), and the marathon CLI (run/exec/validate/
  new). Use this whenever writing or modifying a `.md` file intended to be run with
  marathon, adding runnable steps or input prompts to a runbook, or answering
  questions about marathon's runbook syntax. The core constraint: a runbook must
  stay a valid, ordinary markdown file — marathon layers behavior on standard
  markdown, never custom syntax.
---

# Authoring marathon runbooks

`marathon` runs **runbooks**: ordinary markdown files whose fenced code blocks can
be executed as ordered cells (think *glow* + *Jupyter*). The golden rule:

> **A runbook must remain a valid, standalone markdown file.** Every marathon
> feature rides on standard markdown (frontmatter, fenced code blocks, info
> strings). Never invent non-standard syntax.

## Document shape

````markdown
---
title: Deploy the API
description: One-line summary shown in the header.
env:
  REGION: us-east-1
---

# Deploy the API

Prose is rendered as markdown. Shell blocks below become runnable cells.

```sh
echo "deploying to $REGION"
```
````

## Frontmatter (all fields optional)

YAML frontmatter configures the whole runbook:

- `title`, `description` — shown in the TUI header.
- `env` — map of variables set for **every** cell. `{ KEY: value }`.
- `before_each` / `after_each` — shell snippets injected at the start/end of each
  cell's script. `before_each` **defaults to `set -eu`** when omitted; set it to an
  empty string (`before_each: ""`) to opt out. (`pipefail` is intentionally not in
  the default — it isn't POSIX `sh`.)
- `interpreters` — remap a language to an interpreter, e.g. run `sh` cells with
  `zsh`:
  ```yaml
  interpreters:
    sh:
      path: /usr/bin/env zsh
  ```
  Defaults to `/usr/bin/env <lang>`.
- `tmp_dir` — config for the shared temp directory (see "State model"):
  - `path` — pin an explicit directory (default: a fresh `mktemp`-style dir).
  - `skip_cleanup: true` — keep the dir after the run (default removes it).
  - `var_name` — name of the env var pointing at it (default `TMP_DIR`).

## Cells

Any fenced code block is a cell. Whether it **runs**:

- **Runnable**: shell languages — `sh`, `bash`, `zsh`. These execute.
- **Display-only**: any other language (e.g. `python`, `json`, no language), or a
  shell block marked `skip=true`. Rendered and highlighted, never executed.

Per-cell options live in the **info string** as bare `key=value` pairs:

````markdown
```sh skip=true
echo "marathon won't run this; other tools still highlight it as shell"
```
````

- `skip=true` — mark a shell block display-only.
- `mrthn=input` — on a `json` block, makes it an **input cell** (below).

Each runnable cell runs as its **own process** (`before_each` + body + `after_each`).
Cells do *not* share shell state (no persisted `cd`, functions, or unexported vars)
— they share state through the env map and `$TMP_DIR` instead. stdout and stderr are
merged in written order.

## Input cells (`json mrthn=input`)

The one special block: a `json` block tagged `mrthn=input`. To other markdown tools
it's just highlighted JSON; to marathon it's a prompt whose answer is written into
the env map under `target`, visible to every **later** cell as `$target`.

Three `type`s:

````markdown
```json mrthn=input
{ "type": "input", "prompt": "Give this order a label:", "target": "LABEL" }
```

```json mrthn=input
{ "type": "confirm", "prompt": "Proceed?", "target": "PROCEED" }
```

```json mrthn=input
{
  "type": "select",
  "prompt": "Which option?",
  "target": "CHOICE",
  "options": ["foo", "bar", "baz"]
}
```
````

- `input` — free-form text → `$target`.
- `confirm` — yes/no gate → `$target` is `yes` or `no`.
- `select` — pick one; provide inline `options: [...]` **or** an `option_file`
  (a path, may reference `$TMP_DIR`, one option per line — typically produced by an
  earlier cell).

## State model — how values reach a cell

A cell's environment is layered (later wins):

1. frontmatter `env`
2. CLI `--env KEY=VAL` (repeatable)
3. `$TMP_DIR` (the shared temp dir, created on first run)
4. every **preceding** answered input cell's `target=value`, in document order

So the pattern for passing data between isolated cells is: write a file under
`$TMP_DIR` in one cell, read it in a later one; or capture a decision with an input
cell and reference `$target` downstream.

## The CLI

- `marathon run <file>` — interactive TUI, step through cell by cell (the safe
  default; you confirm each cell).
- `marathon exec <file>` — headless run, output to stdout (CI / pipes). `--yes`
  runs straight through without confirmation.
- `marathon validate <file>` (alias `check`) — parse and report; run nothing.
- `marathon new <file>` — scaffold a minimal runbook.
- `-e/--env KEY=VAL` (on `run`/`exec`, repeatable) — inject env vars.

When editing a runbook, you can sanity-check it with `marathon validate <file>`.

## Authoring checklist

- The file opens fine in any markdown renderer (no custom syntax).
- Shell cells use `sh`/`bash`/`zsh`; illustrative ones are marked `skip=true`.
- Cross-cell data goes through `$TMP_DIR` files or input-cell `target`s, never
  assumed shared shell state.
- Input cells are valid JSON with `type`, `prompt`, and `target`.
- Remember `set -eu` is on by default — a failing command or unset var fails the
  cell loudly.
