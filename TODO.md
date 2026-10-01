# Remaining work

Current behavior is documented in [README.md](README.md) and [DESIGN.md](DESIGN.md).

## Reliability and portability

- Add CI for formatting, Clippy, tests, and Rust 1.88 compatibility; add platform
  build/release smoke checks. GoReleaser configuration exists, but no CI workflow is
  checked in yet.
- Implement native Windows interpreter discovery and process-tree cleanup (for
  example, Job Objects). Current process-group guarantees apply to Unix only.
- Surface unreadable `option_file` errors in the TUI instead of silently retaining
  inline choices; prevent submitting an empty selection there.
- Improve validation diagnostics with cell/source locations and checks for invalid
  environment targets and input defaults. Preserve compatibility with unrelated
  Markdown frontmatter fields.
- Fuzz/property-test Markdown parsing and ANSI sanitization; add golden command
  output fixtures and more Unicode/narrow-terminal cases.

## Output and performance

- Bound or spool captured TUI output. Queued runner chunks are bounded, but the
  accumulated output buffer still grows for the lifetime of a cell run.
- Profile large documents. Move from one width/revision cache to per-block layout
  only when useful: dirty-block wrapping, prefix-sum heights, and a block-relative
  scroll anchor. Selection/focus decoration should remain a draw-time overlay.
- Preserve ANSI SGR colors as Ratatui styles while stripping other terminal escapes.
- Optional stdout/stderr sinks and configurable stream separation.
- PTY support for programs that require a terminal and better terminal fidelity.
- Animate running-cell decoration without invalidating document layout every frame.

## Interaction and scope

- Run-order ordinals and richer blocked/next status indicators. Define ordering
  and dependency semantics first; today the TUI permits arbitrary cell order.
- Multi-select inputs and a real terminal cursor for text editing.
- Improve light/dark theme contrast and richer Markdown rendering.
- Revisit interpreter command quoting and unsupported/custom interpreter behavior.

## Deliberately deferred

- Python/JavaScript/SQL runners beyond shell-language remapping.
- Persistent shell sessions and live runbook reload/editing.
- Explicit environment export from commands (such as a `$MRTHN_ENV` file).
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
