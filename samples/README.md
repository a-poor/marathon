# Sample runbooks

Hand-written examples that double as living documentation for the features in
[`../DESIGN.md`](../DESIGN.md). Each file is valid standalone markdown — it renders
fine in any markdown tool, and runs as a runbook under `marathon`.

| File | Demonstrates |
| --- | --- |
| [`hello.md`](hello.md) | Frontmatter `env`, a runnable cell, `skip=true` opt-out |
| [`tmpdir.md`](tmpdir.md) | Passing state between cells via files under `$TMP_DIR` |
| [`interactive.md`](interactive.md) | A `json mrthn=input` cell that exports a choice via `target` |
| [`shell-override.md`](shell-override.md) | Remapping a language to a real interpreter in frontmatter |

> These are illustrative. Frontmatter/cell field shapes here are the *intended*
> design (see `DESIGN.md`); they may shift as the implementation lands.
