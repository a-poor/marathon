---
title: Passing state between cells
description: Cells are separate processes — they share durable state through files under $TMP_DIR.
---

# Passing state between cells

Each cell runs as its own process, so a plain shell variable set in one cell is
**not** visible in the next. To pass state along, write it to a file under
`$TMP_DIR`.

`$TMP_DIR` is injected automatically by marathon: it's a fresh `mktemp -d` that is
shared for the whole run and cleaned up at the end (unless you ask to keep it).

## Produce some data

```sh
date +%s > "$TMP_DIR/started_at"
seq 1 5 > "$TMP_DIR/numbers.txt"
echo "wrote $TMP_DIR/numbers.txt"
```

## Consume it in a later cell

```sh
total=$(paste -sd+ "$TMP_DIR/numbers.txt" | bc)
echo "sum of numbers = $total"
echo "run started at epoch $(cat "$TMP_DIR/started_at")"
```

The shell variable `total` above will be gone by the next cell — but the files in
`$TMP_DIR` persist for the rest of the run.
