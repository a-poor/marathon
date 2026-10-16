# Remaining work

Current behavior is documented in [README.md](README.md) and [DESIGN.md](DESIGN.md).

## Priority workstreams

Keep runbooks ordinary Markdown and update the README, design, samples, and
bundled authoring skill when behavior changes.
Coordinate model/API changes to `book.rs` and `tui.rs` when working in parallel.

## Proposed next-release bar

- Target supported shell execution on macOS and Linux; native Windows execution
  is deferred. Existing Windows packaging checks do not imply execution support.
- Run the existing formatting, Clippy, all-target tests, MSRV, and release archive
  checks against the release candidate. Include the macOS Homebrew install path.
- Run the packaged TUI smoke tests for input errors/recovery, prerequisites,
  concurrent manual runs, run-remaining, cancellation/quit, search, output paging,
  and resize/narrow-terminal behavior. Manually check visual theme/contrast and
  system clipboard behavior on supported desktop environments.
- Profile representative large runbooks and sustained output; fix observed
  responsiveness or resource-growth problems before adding layout complexity.
- Prioritize regression coverage for cleanup, parsing/display edge cases, and
  actionable diagnostics. Keep documented limitations explicit; broader rendering
  and navigation features need not block a solid release.
- Decide whether managed background services belong in this release after defining
  their lifecycle contract below; they are not implemented today.

### Local release verification (2026-10-16)

- macOS ARM64: formatting, Clippy, Rust 1.88 compatibility, and all-target tests
  checked; native snapshot archive/CLI checks and isolated-terminal TUI checks run.
  Unicode header alignment now uses terminal widths and preserves grapheme clusters,
  with regression coverage for wide characters, combining accents, and emoji.
- Homebrew's local quarantine/hook-order check passed. Full install/uninstall and
  the other release targets still need the hosted release-smoke matrix. The local
  GoReleaser is 2.18.3; CI remains pinned to 2.18.2. Homebrew emits a nonfatal
  deprecation warning for the generated `preflight` hook; track generator support
  for `preflight_steps` when updating release tooling.
- Initial coarse profile: 5,000 cells opened in ~0.75 s, using ~205 MiB resident
  memory; search took ~81 ms including automation overhead. Capturing 32 MiB took
  ~3.1–3.3 s, with sampled RSS growth of ~0.8 MiB in a small document and ~2.3 MiB
  alongside 1,000 cells. These are local observations, not performance guarantees.
- Initial output fuzz campaign completed 51,814 cases in 61 seconds with no failure.
  Parsing fuzzing found a Markdown dependency panic, minimized to `---\n\n>`.
  Frontmatter parsing is now enabled only with matching delimiters; unclosed
  candidates remain ordinary Markdown. Unit and CLI regression tests cover this.
- Follow-up fuzzing found a second Markdown dependency panic, minimized to
  `:\n -\n--\nt\n-`. A local markdown-rs 1.0.0 patch fixes a stale paragraph
  event in setext-heading resolution; see `vendor/markdown/PATCHES.md`. All saved
  parser crash cases replay successfully. The patched parser completed 426,188
  runs in 121 seconds; output completed 52,547 runs in 61 seconds without failure.
  All 190 regular tests, formatting, Clippy, and Rust 1.88 checks passed. Output
  properties now also cover UTF-8 characters crossing multiple page boundaries.
- Remaining acceptance: supported-target CI, full disposable Homebrew installation,
  visual theme/contrast, and source/output clipboard checks in desktop terminals.

## Managed background services — proposed

- Support starting a long-lived command in one cell, continuing subsequent tasks,
  and stopping it explicitly later or manually from the TUI.
- Initial scope: CLI `exec` with an opt-in shell-fence modifier, tentatively
  `exec=background`. Start the cell without waiting for completion, continue the
  selected run, and retain ownership of its process group until that run ends.
  The command can stay in the foreground inside the cell; shell `&` is unnecessary.
  Prefer this name over `cleanup=defer`, which describes cleanup timing but does
  not clearly express nonblocking execution. Syntax remains proposed.
- Defer process-group cleanup for these cells even if their interpreter exits;
  preserve ownership of remaining descendants until end-of-run cleanup. Ordinary
  foreground cells retain their existing immediate descendant-cleanup behavior.
- At normal completion (including a partial run's selected endpoint), stop all
  retained background process groups and await cleanup before returning. Do the
  same on failure, declined execution, broken output pipes, or interruption, before
  releasing scratch space. A separate stop/control cell can follow later.
- Starting a background cell does not imply service readiness. Initially, a later
  ordinary shell cell can perform a bounded readiness poll. Define how `needs`
  interacts with background cells rather than treating spawn as successful completion.
- Before implementation, settle background exit/failure propagation, concurrent
  CLI output ordering/interleaving and draining, and graceful-stop escalation.
- Current manual TUI runs can overlap, but `exec` and run-remaining wait for each
  command to finish. `needs` requires successful completion, not service readiness.
  Shell-backgrounded descendants are terminated when their cell's shell exits.
- Follow-up: design a structured stop/control cell referencing a stable cell ID,
  analogous to input cells, and integrate background execution with TUI sequencing.
  Keep it ordinary Markdown and preserve existing foreground behavior.
- Define readiness versus merely spawned, bounded readiness waits, dependencies,
  unexpected service exits, and how failures stop dependent/sequential work.
- Retain owned processes and bounded output capture while later cells execute.
  Define intentional-stop success, graceful stop/forced-kill escalation, repeated
  starts/stops, and active-service reset/rerun guards.
- Clean up all owned services on normal run completion, failure, interruption,
  and quit before releasing scratch space. Define partial-run behavior when a
  service's start or stop cell is omitted; do not silently replay omitted starts.
- Cover CLI and TUI lifecycle behavior with local disposable-server tests, including
  readiness failure, early exit, cancellation, and descendant cleanup.

## Reliability and portability

- Extend the bounded parsing/output fuzz campaigns and curated regression corpus;
  add golden command output fixtures and more Unicode/narrow-terminal cases.
- Consider optional shell-syntax and interpreter-availability preflight checks;
  current `validate` only checks Markdown/configuration/input structure.

## Output and performance

- Profile large documents. Move from one width/revision cache to per-block layout
  only when useful: dirty-block wrapping, prefix-sum heights, and a block-relative
  scroll anchor. Selection/focus decoration should remain a draw-time overlay.
- Preserve ANSI SGR colors as Ratatui styles while stripping other terminal escapes.
- Optional stdout/stderr sinks and configurable stream separation.
- Animate running-cell decoration without invalidating document layout every frame.

## Interaction and scope

- Add a heading outline/jump action for navigating long runbooks.
- Consider search across wrapped lines and unloaded output pages if needed; current
  search covers individual rendered lines and bounded output tails/pages.
- Multi-select inputs and a real terminal cursor for text editing.
- Improve light/dark theme contrast and richer Markdown rendering.
- Revisit interpreter command quoting and unsupported/custom interpreter behavior.

## Deliberately deferred

- Formal microbenchmark baselines and mutation-testing campaigns. Current release
  work uses coarse terminal performance probes plus targeted fuzz/property tests.
- Native Windows execution support, including interpreter discovery and process-tree
  cleanup (for example, Job Objects). Current process-group guarantees apply to
  Unix only; Windows support is not a next-release requirement.
- PTY support and command input forwarding. Keep cell execution noninteractive for
  now to preserve a simple, predictable runner. Runbooks should use input cells,
  environment values, files, and commands' unattended flags. This does not remove
  Marathon's own input prompts or TUI. Revisit only when concrete workflows justify
  the added terminal ownership, input routing, and cancellation complexity; it is
  not a planned requirement for the next release.
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

- Coverage-guided fuzz targets for runbook parsing and output sanitization/capture,
  with curated seeds/dictionaries and bounded weekly/manual CI campaigns. A shared
  fixed-seed generative test runs in ordinary CI and verifies chunk-boundary
  invariance, raw-byte preservation, and bounded UTF-8 page reconstruction.
- Packaged TUI smoke tests through an isolated tmux server on Linux/macOS, covering
  input error/reload, sequential failure/retry, retained answers/results,
  prerequisite guards, concurrent manual runs, process-group cancellation, quit
  cleanup, output paging, Unicode search, help, and narrow-terminal resizing.
  A separate optional profile script measures large-document startup/search and
  sampled resident memory during sustained output; clipboard/visual acceptance
  remains manual.
- GitHub Actions CI for formatting on stable Rust, plus Clippy, all-target tests
  (including CLI execution and process cleanup), and Rust 1.88.0 compatibility
  checks on Linux and macOS.
- GoReleaser configuration validation and per-target snapshot archive smoke tests
  for every configured release target. Linux/macOS exercise CLI execution, partial
  selection, raw output, failure status, and scratch cleanup; Windows checks archive
  contents, startup, completions, scaffolding, and validation only. Snapshots never
  publish releases or update the Homebrew tap.
- macOS snapshot cask installation from quarantined downloads, quarantine removal
  before completion generation, installed CLI/completion checks, and uninstall
  cleanup on disposable Homebrew runners. Hook ordering also has a local check
  that does not depend on Gatekeeper enforcement or replace installed files.
- TUI `/` search with live highlights, match counts, `n`/`N` wraparound, and
  cancel/restore; README demo regenerated through `demo.tape`.
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
