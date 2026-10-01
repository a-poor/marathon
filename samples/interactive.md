---
title: Prompting for input
description: Local example of generated options, text, and confirmation.
---

# Prompting for input

Run this with `marathon run samples/interactive.md` or
`marathon exec samples/interactive.md`. To use the explicit defaults without any
prompts, add `--yes` to `exec`.

## Produce the options

A preceding cell can generate choices in the session's scratch directory:

```sh
printf 'east\nwest\ncentral\n' > "$TMP_DIR/choices.txt"
```

## Choose a region

Run the preceding cell before opening this input. `marathon validate` accepts the
generated file path without reading it. If you open the input too early in the
TUI, it shows a file error and keeps the input unanswered: press Esc, run the
generating cell, then reopen the input. An empty or unreadable options file also
blocks submission. The default `west` must be one of the generated choices.

```json mrthn=input
{
  "type": "select",
  "prompt": "Which region?",
  "target": "REGION",
  "option_file": "$TMP_DIR/choices.txt",
  "default": "west"
}
```

## Label the run

```json mrthn=input
{
  "type": "input",
  "prompt": "Give this run a label:",
  "target": "LABEL",
  "default": "demo"
}
```

## Confirm the action

`--yes` uses this block's `false` default; it does not turn the answer into yes.
To override it for an unattended run, pass `--yes -e PROCEED=yes`.

```json mrthn=input
{
  "type": "confirm",
  "prompt": "Proceed?",
  "target": "PROCEED",
  "default": false
}
```

## Use the answers

A confirmation exports `yes`/`no`. The shell decides what that answer means:

```sh
printf 'Run %s in %s\n' "$LABEL" "$REGION"
if [ "$PROCEED" = yes ]; then
  echo "Proceeding with the example."
else
  echo "Skipped the example action."
fi
```
