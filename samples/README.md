# Sample runbooks

These are ordinary Markdown files demonstrating the implemented format.
Open one with `marathon run samples/<file>.md`, or execute it sequentially with
`marathon exec samples/<file>.md`. Add `--yes` for unattended execution.

| File | Demonstrates |
| --- | --- |
| [hello.md](hello.md) | Frontmatter environment, shell cells, `skip=true` |
| [tmpdir.md](tmpdir.md) | State shared through `$TMP_DIR`; requires `bc` |
| [interactive.md](interactive.md) | Generated options, text, confirmation, explicit defaults |
| [execution-controls.md](execution-controls.md) | Stable IDs, prerequisites, partial CLI runs, TUI recovery |
| [shell-override.md](shell-override.md) | `interpreters.sh.path`; requires `zsh` |
| [output-spooling.md](output-spooling.md) | Large paged output, partial lines, Unicode, and progress rewrites |
| [demo.md](demo.md) | JSONPlaceholder request, name selection, and greeting; requires `curl` and internet access |

The README recording uses `demo.tape`: `r` fetches a small JSONPlaceholder response,
pauses at the name input, then uses the answer in the next command. The input has
no unattended default; to run it with `exec --yes`, supply `--env NAME=Bob` (or
`NAME=Alice`). The request has a ten-second timeout and fails on HTTP errors.
