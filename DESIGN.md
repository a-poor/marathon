# Marathon — implementation contracts

Marathon is a working CLI/TUI MVP for Markdown runbooks. This document describes
current behavior; [TODO.md](TODO.md) tracks deferred work. User-facing examples and
commands live in [README.md](README.md).

## 1. Format and compatibility

A runbook is ordinary Markdown. Marathon-specific configuration uses, in order:

1. Optional YAML frontmatter for document settings.
2. Bare `key=value` pairs after a fence's language for per-cell settings.
3. `json mrthn=input` blocks for structured prompts.

The Markdown parser uses GFM plus frontmatter. Only top-level `sh`, `bash`, and
`zsh` code blocks are runnable; `skip=true` disables execution. Unknown languages,
unlabeled fences, and indented code are display-only. Nested Markdown is prose,
not a nested execution graph. Frontmatter fields are documented in the README.
Unknown frontmatter fields are tolerated for compatibility with other Markdown tools.

`validate` parses Markdown, YAML, fence metadata, and input JSON. It checks shell
environment names (`[A-Za-z_][A-Za-z0-9_]*`) for input targets, frontmatter `env` keys,
and `tmp_dir.var_name`; CLI `--env` uses the same rule. Environment values cannot
contain NUL. Inline-only selections must have options and a default, if present,
must match an option. Errors include source locations and code-block ordinals.
It does not execute commands, validate shell syntax, check installed interpreters,
or read option files. File-dependent choices/defaults are checked on reaching the
input, even when a literal file path already exists during parsing.

## 2. Processes and shared state

Each code cell runs in a separate interpreter process, with its script supplied on
stdin. The working directory is Marathon's invocation directory. The default
interpreter is `/usr/bin/env <language>`; `interpreters.<language>.path` overrides it
using whitespace-separated argv, without shell quoting/expansion.

The script is `before_each` + cell body + `after_each`. Omitted `before_each`
defaults to `set -eu`; an empty value opts out. Custom hooks replace the default.
`after_each` is appended shell code, not an unconditional cleanup handler.
`pipefail` is not part of the portable default.

Cells do not share `cd`, functions, or shell-local variables. Their environments
inherit Marathon's process environment, then layer:

1. frontmatter `env`;
2. CLI `--env KEY=VALUE` overrides;
3. the scratch-directory variable (`TMP_DIR` by default);
4. answered input cells preceding this cell in document order.

The scratch directory is created for the session and held by a `TempDir` guard.
It is removed only after active processes have stopped. `skip_cleanup: true`
preserves automatic scratch directories. Explicit `tmp_dir.path` directories are
always user-owned and are never removed, including on reset.

## 3. Input cells

`json mrthn=input` has `type`, `prompt`, and `target`, plus type-specific fields:

| Type | Answer | Optional `default` |
| --- | --- | --- |
| `input` | Arbitrary text, including empty text | String |
| `confirm` | `yes` or `no` | Boolean |
| `select` | A value from the available options | String option value |

Selections combine inline `options` and lines read from `option_file`. File lines
are trimmed and blank lines omitted. `$NAME`/`${NAME}` in paths expand against the
cell environment; unknown references remain literal, `$$` yields `$`, and no shell
substitution is performed. Options are refreshed when an input is reached/edited,
against the full cell environment including inherited variables. The scratch
directory is initialized before input editing as well as code execution. Reads
are strict in both CLI and TUI: unreadable files discard cached choices and block
answers even if inline choices exist. Empty combined choices also block answers.

Input state is pending, editing (draft plus prior answer), or answered. In the TUI,
explicit defaults seed the editor; cancellation restores the prior answer only if
it remains valid after options are refreshed. CLI answers and TUI submissions use
one validation boundary. Failed submissions retain the draft and focus, display
an inline error, and do not advance selection or export an answer. A default or
prior answer absent from refreshed choices leaves no highlighted choice until the
user selects one. Esc followed by reopening retries option loading. Interactive
CLI execution reports invalid file-dependent defaults and prompts without a
default; unattended execution requires a valid default or supplied value.
A confirmation is data, not a control-flow gate: shell code must branch on the
exported `yes`/`no` value.

## 4. Owning a run

`runner::spawn_run` returns a `RunningCell` owner immediately, before scheduling
can race with another key event. It owns a task and a cancellation channel. The
TUI stores one owner per block index; a block cannot restart or clear until its
finished message is consumed. Reset-all is blocked while any run exists.
Different blocks can execute concurrently in the TUI.

Enter starts the selected code cell and immediately selects the next runnable or
input cell, skipping prose and display-only code. It does not start that next cell.
Opening an input editor keeps focus in place; submitting advances, while canceling
stays put. The last actionable cell remains selected (no wrapping). Rejected starts
do not advance, and asynchronous completion never changes selection.

On Unix each cell leads a new process group. Backspace requests SIGINT; a second
press requests SIGKILL. A stop requested before spawn is retained. Quit, terminal
errors, CLI interruption, and output failures force cleanup and await the task
before releasing scratch files. A drop guard kills the process group if the task
is aborted/unwinds. The shell is reaped; descendants remaining in its group are
terminated when the shell completes. This runner is not a background-service
supervisor or a sandbox: deliberately detached processes can escape its group.

On non-Unix systems cancellation kills the direct child; descendant cleanup needs
a native implementation. The default interpreter command also assumes POSIX tools.

## 5. Output and completion

The runner reads fixed-size byte chunks without waiting for newline or decoding
UTF-8. Script writing and both output pipes are polled concurrently. A bounded
channel (32 × at most 8 KiB) applies backpressure. Recognized shell interpreters get
`exec 2>&1` prepended, preserving stdout/stderr written order. For other interpreter
remaps both pipes are captured, but their relative ordering is best-effort.

`RunMsg::Output` contains bytes. `Finished` is sent after cleanup with success,
exit code, and a separate optional runner error. Spawn/read failures do not masquerade
as command output or signal termination. `run_script` is a collect-as-text library
adapter over the same runner, converting invalid UTF-8 lossily only at that boundary.

The CLI writes command bytes unchanged to stdout. All Marathon prompts, progress,
and errors go to stderr. CLI execution never creates output spools. The TUI
requests `NO_COLOR=1` unless overridden; color rendering and PTYs are deferred.

Each TUI run owns an `OutputCapture`: two private `NamedTempFile`s in the OS temp
directory, one byte-exact raw capture and one incrementally cleaned UTF-8 capture.
These are independent of scratch-directory settings, including `skip_cleanup` and
explicit paths. The cell and active runner share ownership. Rerun/reset replaces
the capture, and quit drops it after process cleanup and outstanding disk jobs
have been joined. Normal final-owner drop deletes both files. As with scratch
directories, uncatchable termination cannot guarantee named-file cleanup.

The runner writes each chunk on a blocking worker, then sends a bounded
`RunMsg::Captured` change notice. At most one write per pipe is in flight. Those
jobs are tracked even when a cancellation drops the awaiting pipe future; cleanup
joins them before finalizing the decoder and emitting `Finished`. Creation
failure prevents spawn; write/finalization failure fails the run and triggers
process cleanup. Errors remain separate from captured bytes and appear on the
cell. Read failures appear inline or as copy diagnostics. Failed captures retain
the available prefix for viewing, but full-text copying reports an error.

The incremental display decoder retains only an incomplete UTF-8 character and
constant escape/CR/tab state. Escape payloads are discarded without accumulation.
CRLF becomes LF; bare CR rewinds the cleaned file to the current line's start.
Incomplete UTF-8 waits for subsequent chunks and becomes a replacement character
on completion. Tabs use eight-character stops after escape removal. Disk writes
and decoding do not depend on the accumulated capture size, including long lines
and unterminated escape sequences.

Rendering reads at most 16 KiB plus three UTF-8 boundary bytes per cell. The
collapsed window shows at most the last 25 lines; Ctrl-O shows disk pages with
`[`/`]` navigating the selected cell. Pages may split lines but never characters,
and need no growing in-memory line index. The latest page follows new output;
browsing an earlier page pins its offset. `Y` reads the full cleaned capture only
on explicit request; the clipboard API requires a full string, so copying is an
intentional memory exception. No full raw read or repeat sanitization is needed.
Raw bytes are also available to library callers through `raw_reader`.

A width/revision cache avoids rewrapping on selection changes. Content changes
still rebuild the document, but output contributed by each cell is bounded by
its window. Large-document layout work remains separate from output spooling.

## 6. CLI execution

- `run <file>`: interactive TUI; execution order is selected by the user.
- `exec <file>`: sequential execution, prompting before each runnable cell.
- `exec <file> --yes`: unattended execution with supplied/default input values.
- `validate <file>` / `check`: parse and summarize.
- `new <file>`: scaffold, refusing to overwrite an existing path.
- `completions <shell>`: generate shell completion code.
- `skills install`: install bundled authoring guidance.

Interactive `exec` shows the full script, including hooks, before confirming.
No/blank confirmation stops the run; EOF is an error. Input values already present
in the cell environment are validated and used in either mode. Otherwise,
interactive input prompts use explicit defaults on Enter (`confirm` falls back to
no, text accepts empty, select requires a choice). `--yes` requires an explicit
default or supplied value and never automatically approves a confirmation.
Selections must match available options. Missing/invalid values stop at that input;
previous code cells may already have executed.

The CLI is fail-fast. Normal child exit codes are preserved; signaled children,
input errors, and runner errors return 1. Ctrl-C returns 130 and Unix SIGTERM returns
143 after cleanup, including when awaiting a prompt or writing to a full pipe.
Separate stdin/stdout threads keep blocking standard I/O out of the async signal
path and Tokio's shutdown; output acknowledgements prevent truncation on success.

Compatibility changes from the earlier prototype: `exec` now actually honors
`--yes`; scripts relying on unattended execution must pass it. Unlabeled blocks no
longer execute as `sh`, and frontmatter is no longer mandatory.

## 7. Architecture and verification

- `book.rs`: parse/model, environment composition, input state, scratch lifetime.
- `runner.rs`: process ownership, cancellation, byte streams, completion.
- `output.rs`: disk capture ownership, incremental display decoding, bounded pages.
- `exec.rs`: sequential CLI orchestration, prompting, input resolution.
- `tui.rs`: event loop, active run owners, navigation/editing, clipboard.
- `widgets/`: Markdown rendering, wrapping, document cache, footer/help.
- `term.rs`: shared termination-signal handling.
- `main.rs` / `cli.rs`: command dispatch and arguments.
- `scaffold.rs` / `skills.rs`: runbook template and bundled skill installation.

Tests exercise model and rendering behavior, real shell execution, process
cancellation, and the actual CLI binary. Keep regression cases for byte fidelity,
partial output, duplicate starts, reset/quit, output backpressure, failed writes,
signals, confirmation, input defaults/validation, exit codes, and scratch cleanup.
Tests must use disposable directories and deterministic local commands.
