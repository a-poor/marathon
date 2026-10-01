# Remaining work

Current behavior is documented in [README.md](README.md) and [DESIGN.md](DESIGN.md).

## Priority workstreams

The next priority is release checks. Keep runbooks ordinary Markdown and update
the README, design, samples, and bundled authoring skill when behavior changes.
Coordinate model/API changes to `book.rs` and `tui.rs` when working in parallel.

### 1. Release checks

- Add platform build/release smoke checks around the existing GoReleaser
  configuration.

## Reliability and portability

- Implement native Windows interpreter discovery and process-tree cleanup (for
  example, Job Objects). Current process-group guarantees apply to Unix only.
- Fuzz/property-test Markdown parsing and ANSI sanitization; add golden command
  output fixtures and more Unicode/narrow-terminal cases.
- Consider optional shell-syntax and interpreter-availability preflight checks;
  current `validate` only checks Markdown/configuration/input structure.

## Output and performance

- Profile large documents. Move from one width/revision cache to per-block layout
  only when useful: dirty-block wrapping, prefix-sum heights, and a block-relative
  scroll anchor. Selection/focus decoration should remain a draw-time overlay.
- Preserve ANSI SGR colors as Ratatui styles while stripping other terminal escapes.
- Optional stdout/stderr sinks and configurable stream separation.
- Add PTY support and input forwarding for programs that require terminal
  interaction (such as editors or password prompts). Today scripts consume stdin
  and the runner cannot provide an interactive terminal.
- Animate running-cell decoration without invalidating document layout every frame.

## Interaction and scope

- Add text search and a heading outline/jump action for navigating long runbooks.
- Multi-select inputs and a real terminal cursor for text editing.
- Improve light/dark theme contrast and richer Markdown rendering.
- Revisit interpreter command quoting and unsupported/custom interpreter behavior.

## Deliberately deferred

- Persistent cross-session checkpoints. Current recovery reuses state only in an
  open TUI; partial CLI runs resolve inputs anew and never restore completion
  records. Persistence needs document fingerprints, changed-step invalidation,
  answer/secret storage, and explicit scratch-artifact ownership.
- Python/JavaScript/SQL runners beyond shell-language remapping.
- Live runbook reload/editing, with defined handling of active runs and existing
  answers/output when the document changes.
- Explicit environment export from commands (such as a `$MRTHN_ENV` file), so
  computed values can feed later cells without manual scratch-file plumbing.
- Persistent shell sessions. Today `cd`, assignments, and functions do not carry
  across cells; environment export can improve data passing independently of
  changing that process-isolation model.
- Templating, branching/navigation cells, richer special blocks, and script export.

## Implemented

- GitHub Actions CI for formatting on stable Rust, plus Clippy, all-target tests
  (including CLI execution and process cleanup), and Rust 1.88.0 compatibility
  checks on Linux and macOS.
- Shared CLI/TUI answer validation, strict option-file errors, editable TUI errors,
  and source/cell diagnostics for invalid targets and defaults. File-dependent
  checks are deferred until the input is reached; regression tests cover missing,
  unreadable, empty, and changed options plus invalid defaults and targets.
- CLI commands, shell completions, scaffolding, and bundled skill installation.
- Text/confirm/select inputs, option files with environment expansion, explicit
  defaults, and input values passed to later cells.
- Interactive sequential `exec`, unattended `--yes`, strict input resolution,
  fail-fast exit codes, and separate command-output/diagnostic streams.
- Byte-preserving streaming (including binary output and partial lines), bounded
  output queues, owned cancellation, duplicate-run guards, reset guards, and
  cleanup on quit, terminal errors, signals, and broken output pipes.
- Inline status gutters, elapsed time/exit codes, footer progress, clipboard,
  25-line output tails, and Ctrl-O expansion.
- Per-run raw/cleaned output spools, incremental display decoding, bounded 16 KiB
  windows/pages, full-output copying on demand, disk-failure diagnostics, and
  spool cleanup on rerun/reset/quit. Large-output, split-UTF-8, partial-line,
  cancellation, write-failure, and cleanup regression coverage.
- Cell IDs and actionable ordinals, `exec --list`, `--cell`, and inclusive
  `--from`/`--to` ranges with preceding-input resolution and no implicit replay.
- Explicit `needs` prerequisites, TUI blocked/next status, and protection against
  mutating prerequisites of active commands.
- TUI `r` run-remaining with sequential cleanup, input pauses, failure/cancellation
  stops, and reuse of session answers, successes, and scratch artifacts.
- Regression coverage for partial selection, recovery, dependency guards, and
  active-run/reset/cleanup invariants. Input errors stop automatic execution while
  leaving the input editable with its inline error.
