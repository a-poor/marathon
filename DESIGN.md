# Marathon — Design

> Status: **draft / MVP design**. This describes the intended MVP and flags
> deliberately-deferred features. It is a direction, not a frozen spec.

## 1. What marathon is

`marathon` (binary alias `mrthn`) is a CLI/TUI for viewing, validating, and
running markdown **runbooks** — markdown documents whose fenced code blocks can be
executed in order.

The guiding constraint: **a runbook is just a markdown file.** You can write one in
any editor, and it renders cleanly in any markdown tool (GitHub, Pandoc, glow, …).
Marathon-specific behavior is layered *on top of* standard markdown, never via
syntax that breaks other renderers.

The mental model for the TUI is **glow + Jupyter**: a rendered markdown document
that is also a sequence of runnable cells.

Design value: **lean simple.** Prefer the least machinery that is still useful.
Reach for new mechanisms only when an existing one genuinely can't carry the weight.

## 2. The layering strategy (how config rides on markdown)

Marathon adds configuration in three layers, in order of preference. Always prefer
the earliest layer that works:

1. **Frontmatter** — YAML at the top of the file. Document-level config.
2. **Code-block info string** — `key=value` pairs after the language token.
   Per-cell config. Standard markdown renderers use only the first token (the
   language) for highlighting and ignore the rest, so this stays compatible.
3. **Special code blocks** (last resort) — a normal `json` block tagged with a
   marathon role. Used only when 1 and 2 can't express it (notably: prompting the
   user for input). Kept as `json` (not a custom fence) so other tools still
   highlight it.

### Info-string syntax

````
Here's some markdown prelude.

```sh skip=true foo=bar
echo "$GREETING"
```

More markdown text.
````

- The first token (`sh`) is the language → other renderers highlight it as shell.
- Everything after is `key=value` pairs, parsed with the `serde-kv` crate into
  `CodeBlockMeta`.
- Compatibility note: Pandoc has a formal `{.sh key=value}` attribute form, but
  GitHub does not understand it and would show the braces as the language. Bare
  `key=value` after the language is the more broadly compatible choice, so that is
  what marathon uses.

### Canonical naming

- The special-block role key is **`mrthn`**: `` ```json mrthn=input ``. (Matches the
  binary alias; leaves room for future roles like `mrthn=table`.)
- Ordinary per-cell options are bare keys: `skip=true`.

## 3. Cells and execution

A runbook parses (via the `markdown` crate → `mdast`) into a flat, ordered list of
**cells**. Markdown prose between code blocks is rendered as-is; fenced code blocks
are the runnable/interactive cells. There is **no container/nesting** — the document
is a flat sequence.

### What runs

- **MVP runners:** shell only — `sh` / `bash` / `zsh`.
- Recognized shell languages **default to runnable**; opt out per cell with
  `skip=true`.
- Unknown languages are **display-only** (rendered, never executed) in the MVP.
- Frontmatter may **remap a language to an actual binary**, shebang-style — e.g.
  "when you see `sh`, actually run `/usr/bin/env zsh`."

### How a cell runs

Each runnable cell is executed as its own process (`tokio::process::Command`)
against the configured shell. Cells do **not** share an in-process shell session in
the MVP (no persisted shell functions, `cd`, or unexported vars). They share state
two ways only:

1. The **environment map** marathon injects at spawn (see §4).
2. **Files**, via the shared `TMP_DIR` (see §4).

- **Working directory:** the current working directory (where the user invoked
  marathon). `TMP_DIR` is *separate* scratch space, not the run dir.

## 4. State model

Marathon owns an **environment map** that accumulates over the run and is injected
into every shell cell at spawn. There is **no stdout-into-variable capture** in the
MVP. The map is populated from exactly three sources:

1. **Frontmatter `env`** — static key/values; global, available from the first cell.
2. **`TMP_DIR`** — auto-injected, set to a fresh `mktemp -d`. Shared for the entire
   run; cells write/read files here to pass durable state. Cleaned up at the end of
   the run unless retention is requested (CLI flag or frontmatter).
3. **Input cells** — a `json mrthn=input` cell with a `target` field. When the cell
   runs, the user's choice is stored in the env map under the name given by
   `target`, and is then visible to **all subsequent cells**. Frontmatter env is
   global; input-cell values depend on execution order.

> **Deferred (not MVP):** GitHub-Actions-style explicit capture (e.g. a cell writing
> `KEY=VALUE` to `$MRTHN_ENV` to export into the env map). Worth doing later; left
> out of the MVP to keep things simple.

### Input cells (the one special block)

A `json` block tagged `mrthn=input`. Marathon renders a prompt, collects the user's
choice, and writes it into the env map under `target`. Illustrative shape (subject
to change):

```json mrthn=input
{
  "type": "select",
  "multiple": false,
  "options": "./choices.txt",
  "target": "CHOICE"
}
```

A preceding `sh` cell can produce `choices.txt` (under `TMP_DIR` or cwd); a
following `sh` cell can use `"$CHOICE"`. To other markdown tools this is just a
highlighted JSON block.

> Future input/render types (other than `input`) are possible but out of MVP scope.

## 5. CLI surface

- `marathon run <file>` — execute a runbook cell by cell.
- `marathon validate <file>` — parse + check frontmatter/cell metadata without
  running anything.
- `marathon new <file>` — scaffold a minimal runbook.
- `marathon export <file>` — **backburner.** An "eject" that lowers the runbook to a
  shell script. Explicitly best-effort: it won't be pretty, interactive input cells
  can't lower cleanly, but it should roughly run and be cleanable by hand. Not an
  MVP priority.

### Safety posture

**Run at your own peril** — running a runbook executes arbitrary code by design.

- Default `run` goes **cell by cell with enter-to-confirm** before each cell. This
  is the natural safety gate.
- `--yes` (or similar) runs straight through without per-cell confirmation.
- The TUI is inherently safer (you step through); the CLI `--yes` path is the sharp
  edge, and that's accepted.

## 6. TUI

Combination of **glow** (rendered markdown) and a **Jupyter notebook** (ordered,
runnable cells). Renders the document, lets the user move between runnable cells,
run them, see output inline, and respond to input cells. Built on
ratatui (0.30) + ratatui-textarea + crossterm.

> Implementation note: use the **ratatui** skill — the 0.30 API differs
> substantially from pre-0.30 material in model training data.

## 7. Deferred / future ideas (explicitly out of MVP)

Kept here so the MVP stays small but the door stays open:

- **Multiple kernels/runners** — Python, JS, SQL, etc. beyond shell.
- **Persistent shared session** — a pty-backed runner so cells share real shell
  state (vars, functions, `cd`), instead of separate processes + env map + files.
- **GitHub-Actions-style env capture** — cells exporting values into the env map.
- **Templating** — minijinja-style (à la dbt). Deferred; the env map already covers
  most of the need via plain `$VARS`, and templating muddies the "just shell + env"
  model.
- **Richer special blocks** — additional `mrthn=` render/input types.
- **`TMP_DIR` retention / run dir** options beyond the basic flag.

## 8. Current code state

Early scaffold. `cli::App` has no subcommands yet; `book::BookFrontmatter` is empty
and `book::CodeBlockMeta` has only `skip`; `widget_markdown::render_md_node` is a
`todo!()` match over every `mdast` node; `term.rs` and `tui.rs` are empty. This
document is the target these grow toward.
