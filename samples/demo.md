---
title: Hello, Marathon
description: The smallest useful runbook — frontmatter env, a runnable cell, a skipped cell.
env:
  GREETING: "Hello"
  WHO: "world"
---

# Hello, Marathon

This is an ordinary markdown file. Open it in any editor or renderer and it reads
fine. Open it in `marathon` and the shell blocks below become runnable cells.

The values `GREETING` and `WHO` come from the frontmatter `env`, so they're
available to every cell from the start.

```sh
echo "$GREETING, $WHO!"
```

Ask for a new name:

```json mrthn=input
{
  "type": "select",
  "prompt": "Which option do you want?",
  "target": "NAME",
  "options": ["foo","bar","baz"]
}
```

Greet that person:

```sh
echo "Let me think..."
sleep 1
echo "$GREETING, $NAME!"
```

How about a fail?

```sh
echo "Oh no!"
echo $PWD
exit 1
```

## A cell you can opt out of

This block is illustrative, not something to run. `skip=true` keeps marathon from
executing it, while other markdown tools still highlight it as shell.

```sh skip=true
echo "this block is skipped — marathon won't run it"
```

## Shell over multiple lines

A cell is just a normal fenced code block — it can be as long as you like.

```sh
for n in 1 2 3; do
  echo "step $n"
done
echo "done"
```
