# marathon

![Demo of the marathon TUI](./assets/demo.gif)

`marathon` is a CLI/TUI for viewing, validating, and running Markdown runbooks.
Write ordinary Markdown, add shell code blocks, and run them one at a time in the
TUI or sequentially from the CLI. Runbooks remain readable in other Markdown tools.

## Installation

On macOS, use Homebrew:

```sh
brew install a-poor/tap/marathon
```

Or build from source with Rust 1.88 or newer:

```sh
cargo install --git https://github.com/a-poor/marathon
```

Precompiled binaries are available on the [releases page](https://github.com/a-poor/marathon/releases).
Execution currently assumes a POSIX shell environment; process-group cancellation
is supported on macOS and Linux. Native Windows process-tree cleanup is not yet implemented.

## Quick start

```sh
marathon new hello.md
marathon validate hello.md
marathon run hello.md
```

In the TUI, select a cell with the arrow keys and press Enter to run it and select
the next runnable or input cell. Input cells advance after you submit an answer.
For sequential execution without the TUI:

```sh
marathon exec hello.md          # confirm each shell cell
marathon exec hello.md --yes    # unattended execution
```

Runbooks execute commands with your permissions, from the directory where you
invoke Marathon. Review a runbook before running it.

## Writing runbooks

A minimal runbook needs only a shell fence. YAML frontmatter is optional:

````markdown
---
title: My first runbook
env:
  ANSWER: "42"
---

Add some context, then add some code.

```sh
printf 'The answer is %s\n' "$ANSWER"
```
````

Top-level `sh`, `bash`, and `zsh` blocks run by default. Other languages, unlabeled
blocks, and indented code are display-only. Add `skip=true` to keep a shell example
from running:

````markdown
```sh skip=true
echo "illustrative only"
```
````

Each cell gets its own subprocess. A `cd`, variable assignment, or shell function
does not carry over to the next cell. Cells share environment values and files in
`$TMP_DIR`, a fresh directory that is removed when the run ends.

### Configuration

All frontmatter fields are optional:

| Field | Behavior |
| --- | --- |
| `title`, `description` | Text shown in the TUI header |
| `env` | Environment values for every cell; CLI `-e KEY=VALUE` overrides them |
| `interpreters.<language>.path` | Interpreter command, such as `/usr/bin/env zsh` for `sh` cells |
| `before_each` | Prepended to every cell; defaults to `set -eu`; `""` disables it |
| `after_each` | Appended to every cell; skipped if the shell exits earlier |
| `tmp_dir.path` | Use an explicit directory, which Marathon never removes |
| `tmp_dir.skip_cleanup` | Keep an automatically created directory after the run |
| `tmp_dir.var_name` | Override the scratch-directory variable name (`TMP_DIR`) |

Interpreter commands are split on whitespace, without shell quoting or expansion.
The default is `/usr/bin/env <language>`. For example:

```yaml
interpreters:
  sh:
    path: /usr/bin/env zsh
```

Command environments inherit the parent environment, then layer frontmatter `env`,
CLI `--env`, the scratch-directory variable, and preceding answered input cells
(in that order; later values win). Input answers affect only subsequent cells.
`set -eu` stops on failed commands and unset variables; `pipefail` is not enabled
by default because it is not supported by every POSIX shell.

### Cell references and prerequisites

Runnable shell cells and input cells have 1-based ordinals in document order.
Prose, skipped fences, and display-only code do not count. Use
`marathon exec book.md --list` to see the ordinals, IDs, and prerequisites without
running commands or creating scratch space. The TUI shows the same references,
plus `next` and `blocked by` status.

Give important steps an `id` so their references survive inserted or moved cells:

````markdown
```sh id=prepare
printf 'ready\n' > "$TMP_DIR/state"
```

```sh id=consume needs=prepare
cat "$TMP_DIR/state"
```
````

IDs must be unique across runnable and input cells, start with an ASCII letter,
and contain only letters, digits, `_`, or `-`. Ordinals can change when actionable
cells are added or removed; IDs stay stable as long as you preserve them.
`needs=prepare,2` declares prerequisites by ID or ordinal. Each must reference an
earlier actionable cell; unknown, self, and forward references are errors.
Input fences also accept `id` and `needs`, for example
`json mrthn=input id=region needs=prepare` for generated choices.

Every earlier input is an implicit prerequisite because it supplies environment
values. In the TUI, prerequisites and their ancestors must have succeeded or been
answered before a cell can start. Independent shell cells have no implicit code
dependencies and can still be run concurrently. Declare `needs` when commands
share artifacts or require a particular execution order.

### Input cells

Use a normal JSON fence with `mrthn=input`. Answers become environment variables:

````markdown
```json mrthn=input
{"type":"input","prompt":"Give this run a label:","target":"LABEL","default":"demo"}
```

```json mrthn=input
{"type":"select","prompt":"Which region?","target":"REGION","options":["east","west"],"default":"east"}
```

```json mrthn=input
{"type":"confirm","prompt":"Proceed?","target":"PROCEED","default":false}
```

```sh
if [ "$PROCEED" = yes ]; then
  printf '%s: proceeding in %s\n' "$LABEL" "$REGION"
fi
```
````

`default` is optional: a string for text/selection, a boolean for confirmation.
A selection default is an option's value, not its index. Confirmations export
`yes` or `no`; a `no` answer does not stop execution by itself—branch on it in shell.
Input targets, frontmatter `env` keys, `tmp_dir.var_name`, and CLI `--env` keys must
match `[A-Za-z_][A-Za-z0-9_]*`. Answers, defaults, and options cannot contain NUL
characters, which cannot be passed in an environment variable.

Selections can also use `"option_file":"$TMP_DIR/choices.txt"`. File lines are
trimmed, blank lines omitted, and entries appended to any inline `options`.
`$NAME` and `${NAME}` in the path resolve against the cell's environment; there is
no command substitution. Files are re-read when the input is reached/edited, so an
earlier cell can generate them. Both CLI and TUI report unreadable files, even
when inline choices are present, and reject selections with no available options.
In the TUI, an error stays with the input and Enter cannot advance until the answer
is valid. Press Esc, fix the file or run its generating cell, then reopen the input
to retry. If a refreshed file removes a previous answer or default, choose an
available option explicitly; Marathon does not silently substitute the first one.
Canceling restores a previous answer only if it is still valid after the refresh.

## CLI execution

`marathon exec <file>` walks cells in document order. It shows each shell script
(including hooks) on stderr and asks `Run this cell? [y/N]`. Answering no or pressing
Enter stops the run with status 1. Input cells collect text, a numbered selection,
or yes/no. EOF before an answer is an error. Prompts read stdin, which also permits
piping a known sequence of answers.

`--yes` disables all prompts. Each input must have a supplied environment value or
an explicit `default`; a missing or invalid answer stops the run at that input,
before any subsequent cells execute. Earlier cells may already have run.
`--yes` never turns confirmation answers into `yes` automatically.

```sh
marathon exec deploy.md --yes -e REGION=west -e PROCEED=yes > run.log
marathon exec deploy.md --list
marathon exec deploy.md --cell deploy --yes       # one ID (or ordinal)
marathon exec deploy.md --from deploy --yes       # chosen cell through the end
marathon exec deploy.md --from 3 --to 5 --yes      # inclusive range
```

Repeat `--cell` to select several cells; they execute once each in document order,
regardless of argument order. `--cell` cannot combine with `--from` or `--to`.
`--to` alone starts at the beginning. All references are checked before execution.

Partial runs resolve **all inputs preceding the last selected cell**, in document
order, using the same environment/default/prompt rules as a full run. Inputs after
that cell are ignored. Skipped shell commands are never replayed, even if declared
as prerequisites: Marathon reports omitted prerequisites, and you are responsible
for providing their effects. A missing generated option file still fails, even
with a supplied answer. Select its producer too, or explicitly reuse an existing
directory through `tmp_dir.path`.

Supplied input values come from the environment available at that cell, including
inherited variables, frontmatter, CLI overrides, and earlier answers. They are
validated and used without prompting in either mode. Otherwise interactive mode
prompts, using the explicit default when you press Enter. With no explicit default,
interactive confirmation starts at `no`; text may be empty; selection requires a
choice. The TUI seeds editors from defaults but lets you answer each input directly.
If a generated file makes a selection default invalid, interactive CLI execution
reports it and asks for a choice; `--yes` requires a valid supplied value instead.

Cell stdout and stderr are combined on **stdout**. Shell streams preserve written
order, and CLI output preserves bytes exactly—including CRLF, binary data, and a
missing final newline. Output streams before a newline arrives. Marathon prompts,
progress, and runner errors go to **stderr**.

Execution stops at the first failed cell and preserves its numeric exit code.
A signal-terminated cell or runner/input error yields status 1. Interrupting Marathon
with Ctrl-C yields 130 (SIGTERM: 143 on Unix), after stopping the active command and
cleaning scratch files. Closing the output pipe also stops the active command.

Commands receive their script over stdin, not the prompt input stream. Programs
that require an interactive terminal are not supported by the current runner.
This is a deliberate constraint for now: use input cells, environment values,
files, and unattended command flags to keep execution predictable.

## TUI controls

| Key | Action |
| --- | --- |
| `↑` / `↓`, `k` / `j` | Move selection |
| `g` / `G`, Home / End | First / last cell |
| Ctrl-U / Ctrl-D, Page Up / Page Down | Scroll half a page |
| `/` | Search the rendered document; Enter accepts, Esc cancels the preview |
| `n` / `N` | Next / previous search match (wraps around) |
| Enter | Run code and advance; edit an input in place, then submit and advance |
| `r` | Run remaining unfinished cells sequentially, from the first unfinished cell |
| Backspace | Stop run remaining / interrupt selected manual run; press again to force-kill it |
| Ctrl-O | Expand/collapse output (paged for large captures) |
| `[` / `]` | Previous / next output page of the selected cell while expanded |
| `y` / `Y` | Copy cell source / cleaned output |
| `x` / `X` | Clear selected cell / reset all cells and automatic scratch space |
| `?` | Show help |
| `q` / Esc | Quit navigation; Esc first clears search, cancels an edit, or closes help |
| Ctrl-C | Quit from any mode |

Search is literal and case-insensitive, with live highlighting and a match count.
It searches each rendered line of prose, cell source, inputs, and the current
output tails/pages. Matches do not span wrapped lines; expand/page output to search
other parts of a capture. `/` edits the previous query; Ctrl-U clears it. Canceling
the preview restores the previous query and position. Stop run-remaining before
opening search; cell input editors continue to treat `/`, `n`, and `N` as input.

Advancement skips prose and display-only code and stays on the last actionable
cell at the end. It happens when a run starts, without waiting for completion or
automatically running the next cell. Use the arrow keys to return to running output
or cancel it with Backspace. Canceling an input edit leaves that input selected.

A running cell cannot be started again or cleared, and its prerequisites cannot
be edited, rerun, or cleared until it finishes. Reset-all is blocked while any
cell is running. Different independent cells may run concurrently. Quit and
terminal errors stop active processes before cleaning
scratch space. On Unix, cancellation targets the shell and descendants in its
process group. Cells are not intended to launch persistent background services.

`r` runs unfinished cells one at a time, waiting for command cleanup before
starting the next. It pauses in the input editor when an answer is needed, then
continues on submission. A failure, input loading/validation error, Backspace, or canceled
input edit stops the sequence. Input errors stay editable inline; after correcting
an answer, press `r` explicitly to continue. Backspace targets the sequence's active command
even if you moved the selection; Esc cancels an input edit. A canceled command
remains unfinished even if it handles the interrupt and exits successfully.
Finish or cancel manual runs before using `r`. While a sequence is active, manual
starts, edits, and resets are blocked; navigation and copying remain available.

### Recovery and session state

Within an open TUI session, `r` reuses answers, successful steps, and the same
scratch directory. After a failure, pressing `r` explicitly retries the first
unfinished cell and continues; it never automatically replays successful work.
Enter explicitly reruns a selected cell. Clearing a cell with `x` makes it
unfinished again; `X` clears all answers/results and renews automatic scratch
space. Explicit `tmp_dir.path` directories remain user-owned on reset and quit.

Completed results are history, not proof that external effects remain valid.
Changing an earlier answer or rerunning a prerequisite does not erase completed
downstream results. Clear or explicitly rerun those steps when their effects need
updating. Marathon does not roll back failed or canceled commands.

There are no persistent checkpoints: closing the TUI or starting another CLI
invocation forgets answers and completion state. `--from` is a fresh partial run,
with fresh automatic scratch space and newly resolved answers. An explicitly
preserved directory can retain files, but does not restore completion records or
answers. Runbooks are loaded once per session; edits on disk take effect on the
next invocation, which revalidates references and starts with no completion state.

TUI output replaces invalid UTF-8 for display, strips ANSI escapes, normalizes
progress rewrites, and expands tabs. Full raw and cleaned captures are spooled to
private temporary files. The inline view shows the last 25 lines within a 16 KiB
window; even a single very long line stays bounded. Ctrl-O expands to 16 KiB pages.
Use `[` / `]` on the selected cell to browse earlier/later pages, and the usual
scroll keys within a page. The latest page follows new output; earlier pages stay
put. Pages can split lines, but preserve UTF-8 characters. `Y` copies the full
cleaned capture, allocating its text only when requested. CLI output remains raw
and streaming.

Spools are removed on rerun, clear/reset, and quit, after active processes and
writes finish. They are separate from `$TMP_DIR` and are removed even when scratch
cleanup is disabled. Spool creation/write failures fail the cell and stop its
command; read/copy failures are shown in the TUI. Diagnostics are separate from
captured output, and an incomplete capture cannot be copied as if it were complete.
Disk use grows with output; spools are session captures, not persistent logs.

## Other commands

```sh
marathon validate book.md          # alias: check; parse without executing
marathon new book.md               # refuses to overwrite an existing file
marathon skills install --project  # install bundled runbook-authoring guidance
marathon completions zsh           # also bash, fish, elvish, powershell
```

Validation checks Markdown configuration, JSON input shapes, environment targets,
and defaults. Inline-only selections need at least one option and any default must
match an option. Diagnostics identify the source file, fence line/column, and cell
number (counting top-level code blocks). Unrelated frontmatter fields are accepted.
Validation does not read option files or check defaults against their contents;
those checks wait until the input is reached. It also does not verify shell syntax
or interpreter availability.

The Homebrew cask installs completions automatically. For other installations:

```sh
marathon completions zsh > ~/.zfunc/_marathon  # directory must exist and be on $fpath
marathon completions fish > ~/.config/fish/completions/marathon.fish
```

See [samples](samples/README.md) for runnable examples, [DESIGN.md](DESIGN.md) for
implementation contracts, and [TODO.md](TODO.md) for remaining work.

## Development

[CI](.github/workflows/ci.yml) runs on pushes and pull requests, and can also be
started manually. It checks formatting on stable Rust and runs Clippy, the full
test suite (including CLI execution and process cleanup), and the Rust 1.88.0
minimum-version build check on both Linux and macOS.

[Release smoke tests](.github/workflows/release-smoke.yml) validate the GoReleaser
configuration and build snapshot archives for every configured release target.
They verify checksums and archive contents, then test the extracted binaries:
startup, completions, scaffolding, and validation on all targets; execution,
partial selection, raw output, exit status, and scratch cleanup on Linux/macOS.
On macOS, they also install the generated Homebrew cask from the local snapshot,
verify that the download is quarantined and the install hook clears quarantine
before completion generation, run the installed executable, verify shell
completions, and uninstall it. Hook ordering is checked independently of whether
Gatekeeper is enabled on the runner.
Windows checks do not imply native shell execution support. These checks do not
publish releases, update the Homebrew tap, or automate the interactive TUI.

Run the same checks locally:

```sh
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo +1.88.0 check --all-targets --locked
```

To smoke-test a native release archive locally, install GoReleaser v2.18.2 and use a
Python virtual environment for the release tools. From the repository root:

```sh
python3 -m venv target/release-tools
. target/release-tools/bin/activate
python -m pip install -r scripts/release-smoke-requirements.txt
export CARGO_ZIGBUILD_PYTHON_PATH=python
goreleaser check
python scripts/release-config.py aarch64-apple-darwin # use your native release target
goreleaser release --snapshot --clean --skip=before --config target/release-smoke.yaml
python scripts/release-smoke.py target/release-smoke
```

Run `python scripts/release-config.py` without a target to list the CI matrix.
Generated configuration and archives live under `target/`. The global GoReleaser
installation hooks are skipped because the toolchain is already provisioned.
macOS artifacts use native `cargo build` and Apple's linker to avoid duplicate
framework links in Zig-built binaries. Full releases therefore need a macOS build
host; Linux/Windows targets continue to use `cargo zigbuild`.
Windows-only snapshots omit Homebrew cask generation because they contain only a
Windows ZIP. Linux/macOS snapshots retain the release cask configuration.

The Homebrew installation test runs only on disposable GitHub-hosted macOS runners
and refuses to replace an existing installation. To inspect its generated local
cask without installing it:

```sh
python scripts/homebrew-smoke.py target/release-smoke --prepare-only target/marathon.rb
```

On macOS, test the generated hooks against a quarantined copy of the packaged
binary without changing your Homebrew installation:

```sh
python scripts/homebrew-smoke.py target/release-smoke --quarantine-only
```

To refresh the README demo, install [VHS](https://github.com/charmbracelet/vhs), then
record the current local build from the repository root:

```sh
cargo build --locked
PATH="$PWD/target/debug:$PATH" vhs demo.tape
```

The tape writes `assets/demo.gif` and demonstrates a JSONPlaceholder request with
`curl`, input selection, and the chosen name passed to the next command in a short,
successful run. Recording requires `curl` and internet access.
Review the recording after changing the TUI or sample.
