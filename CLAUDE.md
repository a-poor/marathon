# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`marathon` (binary alias `mrthn`) is a CLI/TUI for viewing, managing, and running
markdown **runbooks** — markdown documents whose fenced code blocks can be executed.
A core design constraint is **maximum compatibility with other markdown tools**: a
runbook must remain a valid, ordinary markdown file. Marathon-specific behavior is
layered on top of standard markdown rather than introducing custom syntax.

This is an early-stage scaffold. Most modules (`term.rs`, `tui.rs`) are empty, and
`widget_markdown.rs` is a `todo!()` skeleton. Expect to implement, not just extend.

## Commands

```sh
cargo run -- <args>      # run the CLI
cargo build              # build
cargo test               # run all tests
cargo test <name>        # run a single test by name substring
cargo clippy             # lint
cargo fmt                # format
```

Note: edition is **2024**, which requires a recent stable toolchain.

## Architecture

Entry point is `main.rs`, which parses `cli::App` (clap derive) and runs under a
`#[tokio::main]` async runtime. Library code lives behind `lib.rs` (crate
`marathon`); `main.rs` is a thin binary over it.

Module responsibilities:
- `cli.rs` — clap argument parsing (`App`). The top-level command surface.
- `book.rs` — the runbook data model. `BookFrontmatter` is YAML frontmatter
  (parsed with `serde_yaml`); `CodeBlockMeta` is per-code-block config parsed from
  a code block's info string via `serde-kv` (e.g. controlling whether a block is
  runnable via `skip`).
- `widget_markdown.rs` — renders a parsed markdown AST. The `markdown` crate
  produces an `mdast::Node` tree; `render_md_node` matches over every node variant.
  This is the rendering core that ties parsing to display.
- `term.rs` / `tui.rs` — terminal and TUI layers (ratatui + ratatui-textarea +
  crossterm), currently unimplemented.

Data flow (intended): markdown file → `markdown` crate parses to `mdast` →
frontmatter + per-block `CodeBlockMeta` extracted into the `book` model →
`widget_markdown` renders the tree → TUI (`tui`/`term`) drives interaction and
executes non-skipped code blocks.

## Conventions

- Errors propagate via `anyhow::Result`.
- When working on the TUI, use the **ratatui** skill — the project pins ratatui
  0.30, whose API differs significantly from pre-0.30 versions in training data.
- Keep runbook files valid standalone markdown; do not invent non-standard syntax.
