---
title: Output spooling
description: Large captures, output pages, and streamed partial lines.
---

# Browse a large capture

Run this cell, then return to it with the up arrow. The inline tail stays bounded.
Press Ctrl-O to expand, `[` to browse earlier pages, and `]` to return to the
latest page. Use Page Up/Down to scroll within a page. `Y` copies all cleaned
output, including the beginning. Rerun, clear (`x`), reset (`X`), and quit remove
the temporary captures; scratch-directory preservation does not retain them.

```sh
awk 'BEGIN { for (i = 1; i <= 10000; i++) printf "record %05d: local sample output\n", i }'
```

# Partial lines and display cleanup

Output appears without waiting for a newline. The TUI strips the color escape,
collapses the progress rewrite, and preserves the euro sign split across writes.
`marathon exec samples/output-spooling.md --yes` streams the original bytes.

```sh
printf 'working...'
sleep 1
printf '\r\033[32mdone\033[0m\r\n'
printf '\342'
printf '\202\254\n'
```

# A single long line

Even a line without newlines cannot grow the live display buffer indefinitely.
Expanded pages make its full text available.

```sh
awk 'BEGIN { for (i = 0; i < 4000; i++) printf "0123456789"; printf "\n" }'
```
