# Remaining work

Current behavior is documented in [README.md](README.md) and [DESIGN.md](DESIGN.md).

## Priority workstreams

The next priorities, in order, are input correctness, output spooling, execution
controls, and CI. Each is a separate workstream; input, output, and execution work
share parts of `book.rs` and `tui.rs`, so coordinate model/API changes when working
in parallel. Keep runbooks ordinary Markdown and update the README, design,
samples, and bundled authoring skill when behavior changes.

### 1. Input correctness and validation

- Surface unreadable `option_file` errors in the TUI instead of silently retaining
  inline choices; prevent submitting a selection when no options are available.
- Align CLI and TUI answer validation so invalid selections/defaults cannot become
  answered inputs. Keep an invalid TUI input editable and show an actionable error.
- Improve validation diagnostics with cell/source locations and checks for invalid
  environment targets and input defaults. Preserve compatibility with unrelated
  Markdown frontmatter fields, and defer checks that require generated option files
  until the input is reached.
- Add regression coverage for missing/unreadable option files, empty choices,
  invalid defaults/targets, and consistent CLI/TUI validation.

### 2. Output spooling and bounded memory

- Bound in-memory TUI output and spool full output to disk. Queued runner chunks
  are bounded today, but accumulated cell output grows without a limit; the
  25-line display tail does not bound memory use.
- Preserve access to full output for expansion/copying without loading or
  sanitizing the entire capture on every update. Define spool ownership, cleanup
  on rerun/reset/quit, and how disk/write failures are surfaced.
- Keep CLI output byte-preserving and streaming. Verify large-output behavior,
  partial lines, split UTF-8, cancellation, and spool cleanup with local tests.

### 3. Execution controls and recovery

- Add CLI cell/range selection and a way to start at a chosen cell, so recovering
  from a late failure does not require replaying every earlier command. Define
  stable cell references and how preceding inputs are resolved for partial runs.
- Add a sequential TUI "run remaining" mode that waits for each cell, handles
  inputs, and stops on failure or cancellation. Define how it interacts with
  manually started concurrent runs.
- Define prerequisite/order semantics and show blocked/next status plus run-order
  ordinals. Today the TUI allows arbitrary order, including starting a downstream
  cell while its prerequisite is still running.
- Define resume/checkpoint semantics separately from merely starting at a cell:
  which answers, completed steps, and scratch artifacts can be reused, and how
  runbook changes affect that state. Avoid automatically replaying completed work.
- Cover partial-run input resolution, sequential execution, failure/cancellation,
  and existing active-run/reset/cleanup invariants with regression tests.

### 4. CI and release checks

- Add checked-in CI for `cargo fmt --all -- --check`,
  `cargo clippy --all-targets --locked -- -D warnings`,
  `cargo test --all-targets --locked`, and
  `cargo +1.88.0 check --all-targets --locked`.
- Exercise supported Unix platforms (macOS and Linux), including CLI execution
  and process-cleanup tests; add platform build/release smoke checks around the
  existing GoReleaser configuration. No CI workflow is checked in yet.

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
