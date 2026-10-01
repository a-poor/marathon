# Sample runbooks

These are ordinary Markdown files and local examples of the implemented format.
Open one with `marathon run samples/<file>.md`, or execute it sequentially with
`marathon exec samples/<file>.md`. Add `--yes` for unattended execution.

| File | Demonstrates |
| --- | --- |
| [hello.md](hello.md) | Frontmatter environment, shell cells, `skip=true` |
| [tmpdir.md](tmpdir.md) | State shared through `$TMP_DIR`; requires `bc` |
| [interactive.md](interactive.md) | Generated options, text, confirmation, explicit defaults |
| [shell-override.md](shell-override.md) | `interpreters.sh.path`; requires `zsh` |
| [output-spooling.md](output-spooling.md) | Large paged output, partial lines, Unicode, and progress rewrites |
| [demo.md](demo.md) | TUI demonstration including an intentional failure and long output |

`demo.md` is intended for manually stepping through the TUI. Its input has no
unattended default and its failure cell intentionally stops `exec`.
