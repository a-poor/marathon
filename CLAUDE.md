# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`marathon` is a CLI/TUI for viewing, managing, and running
markdown **runbooks** — markdown documents whose fenced code blocks can be executed.
A core design constraint is **maximum compatibility with other markdown tools**: a
runbook must remain a valid, ordinary markdown file. Marathon-specific behavior is
layered on top of standard markdown rather than introducing custom syntax.

This is a working MVP. The CLI, TUI, process runner, input cells, and rendering
are implemented. Read `DESIGN.md` for implementation contracts, `README.md` for
user-facing behavior, and `TODO.md` for remaining work.

## Commands

```sh
cargo run -- <args>
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo +1.88.0 check --all-targets --locked
```

The project uses Rust edition 2024 with MSRV 1.88.0.

## Architecture

- `main.rs` / `cli.rs`: command dispatch and clap arguments.
- `book.rs`: Markdown/YAML parsing, cells, input state, environment layers, scratch
  directories. YAML frontmatter is optional. Only explicitly labeled shell cells run.
- `runner.rs`: owned process tasks, byte chunks over bounded channels, process-group
  cancellation, cleanup, and completion/error messages.
- `exec.rs`: sequential execution, prompts on stderr, raw stdout, input resolution,
  fail-fast exit status and interruption cleanup.
- `tui.rs`: asynchronous event loop, cell selection/editing, active run ownership,
  clipboard, and shutdown cleanup.
- `widgets/markdown.rs`, `wrap.rs`, `scrollview.rs`: Markdown rendering, wrapping,
  cached layout and inline cell output. Other widgets supply footer/help chrome.
- `term.rs`: shared signal handling.
- `scaffold.rs` / `skills.rs`: starter template and embedded authoring guidance.

## Conventions

- Errors propagate via `anyhow::Result`.
- When working on the TUI, use `.claude/skills/ratatui/SKILL.md`; this project uses
  Ratatui 0.30, whose API differs from older versions.
- Keep runbooks valid standalone Markdown. Update the README, samples, design,
  and `assets/skills/marathon/SKILL.md` when their format/behavior changes.
- Keep command output as bytes until the display/text boundary. Runner failures
  must not be written into command stdout.
- Retain a `RunningCell` for every active command. On shutdown/error, await process
  cleanup before dropping the runbook's scratch-directory guard.
- Do not reset or rerun an active cell. Completion messages must belong to the
  current run; preserve this invariant when changing event handling.
- Use disposable directories and local commands in execution tests. Include CLI
  binary tests for user-visible argument, output, input, and exit-status behavior.

## Git

- **Do not make any git changes without explicit permission from the user.** This
  includes `commit`, `add`/staging, `branch`, `push`, `restore`, `reset`, and any
  other state-changing git command. Read-only inspection (`status`, `diff`, `log`)
  is fine. Ask first, then act only on an explicit go-ahead.
