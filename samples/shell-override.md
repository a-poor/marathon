---
title: Choosing the shell
description: Frontmatter can remap a language to a real interpreter, shebang-style.
shells:
  sh: /usr/bin/env zsh
env:
  NAME: "marathon"
---

# Choosing the shell

The code blocks below are tagged `sh` so that other markdown tools highlight them as
shell. But the frontmatter `shells` map tells marathon: "when you see `sh`, actually
run it with `/usr/bin/env zsh`."

This lets a runbook stay portable-looking while running under the interpreter you
actually want.

```sh
# zsh-isms are fine here because this really runs under zsh
print -l "$NAME" "$SHELL" "${(U)NAME}"
```

Without the `shells` override, this same block would run under plain `sh`.
