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
```

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

## TUI controls

| Key | Action |
| --- | --- |
| `↑` / `↓`, `k` / `j` | Move selection |
| `g` / `G`, Home / End | First / last cell |
| Ctrl-U / Ctrl-D, Page Up / Page Down | Scroll half a page |
| Enter | Run code and advance; edit an input in place, then submit and advance |
| Backspace | Interrupt selected run; press again to force-kill it |
| Ctrl-O | Expand/collapse output (paged for large captures) |
| `[` / `]` | Previous / next output page of the selected cell while expanded |
| `y` / `Y` | Copy cell source / cleaned output |
| `x` / `X` | Clear selected cell / reset all cells and automatic scratch space |
| `?` | Show help |
| `q` / Esc | Quit navigation; Esc cancels an input edit or closes help |
| Ctrl-C | Quit from any mode |

Advancement skips prose and display-only code and stays on the last actionable
cell at the end. It happens when a run starts, without waiting for completion or
automatically running the next cell. Use the arrow keys to return to running output
or cancel it with Backspace. Canceling an input edit leaves that input selected.

A running cell cannot be started again or cleared. Reset-all is blocked while any
cell is running. Different cells may run concurrently; the TUI does not enforce
execution order. Quit and terminal errors stop active processes before cleaning
scratch space. On Unix, cancellation targets the shell and descendants in its
process group. Cells are not intended to launch persistent background services.

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

Run the same checks locally:

```sh
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo +1.88.0 check --all-targets --locked
```
