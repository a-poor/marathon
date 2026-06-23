---
title: Prompting for input
description: A json mrthn=input cell collects a choice and exports it to later cells via `target`.
env:
  BASE: "https://example.test"
---

# Prompting for input

Sometimes a runbook needs a decision in the middle. Marathon expresses that with a
**special block**: an ordinary `json` block tagged `mrthn=input`. To other markdown
tools it's just highlighted JSON; to marathon it's a prompt.

## Gather the options

First, produce a list of choices and stash it under `$TMP_DIR`.

```sh
curl -s "$BASE/api/options" > "$TMP_DIR/choices.txt"
cat "$TMP_DIR/choices.txt"
```

## Ask the user to pick one

This block renders a selection prompt. The user's choice is written into the env map
under the name given by `target` (`CHOICE`), so every cell *after* this one can use
`$CHOICE`.

```json mrthn=input
{
  "type": "select",
  "prompt": "Which option do you want?",
  "target": "CHOICE",
  "option_file": "$TMP_DIR/choices.txt"
}
```

## Name the order

A `text` cell collects free-form input. Whatever the user types is written to
`target` (`LABEL`), so later cells can reference `$LABEL`.

```json mrthn=input
{
  "type": "input",
  "prompt": "Give this order a label:",
  "target": "LABEL"
}
```

## Confirm before acting

A `confirm` cell is a yes/no gate. The answer (`yes`/`no`) lands in `target`
(`PROCEED`), so a later cell can branch on it.

```json mrthn=input
{
  "type": "confirm",
  "prompt": "Submit the order now?",
  "target": "PROCEED"
}
```

## Act on the choice

```sh
echo "You picked: $CHOICE (labeled '$LABEL')"
if [ "$PROCEED" = "yes" ]; then
  curl -s -X POST "$BASE/api/order/$CHOICE"
else
  echo "Skipped — PROCEED was '$PROCEED'."
fi
```

---

What about just a select option?

```json mrthn=input
{
  "type": "select",
  "prompt": "Which option do you want?",
  "target": "CHOICE",
  "options": ["foo","bar","baz"]
}
```

The end.
