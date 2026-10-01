---
title: Execution controls
description: Select steps, declare prerequisites, and recover within a TUI session.
---

# Execution controls

List references without running anything:

```console
marathon exec samples/execution-controls.md --list
```

Run the whole example with `marathon exec samples/execution-controls.md --yes`, or
open it with `marathon run samples/execution-controls.md` and press `r`. The TUI
waits for each command and pauses for the input answer. Pressing `r` again skips
work already completed in that session. Backspace stops the current sequence.

## Prepare a shared artifact

This step is cell 1. Its ID stays `prepare` even if other cells are inserted.

```sh id=prepare
printf 'Artifact from prepare\n' > "$TMP_DIR/artifact"
printf 'Prepared artifact\n'
```

## Answer an input

This input is cell 2. Every later cell implicitly requires its answer.

```json mrthn=input id=label
{"type":"input","prompt":"Label for the result?","target":"LABEL","default":"demo"}
```

## Consume the artifact

This step declares its file prerequisite explicitly. The TUI blocks it until
`prepare` succeeds and `label` is answered.

```sh id=consume needs=prepare
printf '%s: ' "$LABEL"
cat "$TMP_DIR/artifact"
```

## A standalone summary

This command needs only the preceding input. Try
`marathon exec samples/execution-controls.md --cell summary --yes -e LABEL=partial`.
It resolves `label` and runs `summary`, without replaying `prepare` or `consume`.

```sh id=summary
printf 'Summary for %s\n' "$LABEL"
```

Ranges are inclusive: `--from prepare --to consume` runs cells 1 through 3.
`--from consume` omits preparation and fails with fresh scratch space; selection
does not restore a checkpoint or recreate missing artifacts. To reuse files
across invocations, explicitly configure a user-owned `tmp_dir.path` and ensure
the artifacts already exist. Answers and completion records are never persisted.
