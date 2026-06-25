# marathon

![Demo gif of the marathon tui](./assets/demo.gif)

`marathon` is a CLI/TUI for viewing, managing, and running markdown runbooks.

Marathon's goal is to be as compatable as possible with other markdown tools.

It's all just markdown! But marathon lets you run your code blocks.

## Installing

On mac, you can install `marathon` with Homebrew:

```sh
brew install a-poor/tap/marathon
```

You can also install it with cargo:

```sh
cargo install --git https://github.com/a-poor/marathon
```

Or you can download a pre-compiled binary from the [releases](https://github.com/a-poor/marathon/releases).

## Writing Runbooks

Marathon tries to get out of your way. All you have to do is open up a markdown file and start to add some code blocks.

````md
---
title: My First Runbook
env:
  ANSWER: 42
---

Add some context. Then add some code.

```sh
echo "The answer is... $ANSWER"
```

That's pretty much it!
````

But, sometimes you need to be able to make a selection or pass in some information.

To solve that, you can use a JSON code block with `mrthn=input` in the code block meta field.

````md
Get input from the user:

```json mrthn=input
{
  "type": "input",
  "prompt": "Give this order a label:",
  "target": "LABEL"
}
```

After this, the user's input value will be passed in as the environment variable `LABEL`.
````
In addition to input text, there are also `choice` and `confirm` blocks.

````md
Multiple choice:

```json mrthn=input
{
  "type": "select",
  "prompt": "Which option do you want?",
  "target": "CHOICE",
  "options": ["foo","bar","baz"]
}
```

Confirm:

```json mrthn=input
{
  "type": "confirm",
  "prompt": "Submit the order now?",
  "target": "PROCEED"
}
```
````

Note that each code block is executed in a separate subprocess so they can't *directly* communicate.

For that reason, marathon also provides a temp directory, that gets cleaned up at the end of each run,
and can be used to pass state or information between cells.

The path to the temp directory is passed in as an environment variable (`TMP_DIR`).


## Running Runbooks

```
$ marathon --help
                         ▗▖
                     ▐▌  ▐▌
▐█▙█▖ ▟██▖ █▟█▌ ▟██▖▐███ ▐▙██▖ ▟█▙ ▐▙██▖
▐▌█▐▌ ▘▄▟▌ █▘   ▘▄▟▌ ▐▌  ▐▛ ▐▌▐▛ ▜▌▐▛ ▐▌
▐▌█▐▌▗█▀▜▌ █   ▗█▀▜▌ ▐▌  ▐▌ ▐▌▐▌ ▐▌▐▌ ▐▌
▐▌█▐▌▐▙▄█▌ █   ▐▙▄█▌ ▐▙▄ ▐▌ ▐▌▝█▄█▘▐▌ ▐▌
▝▘▀▝▘ ▀▀▝▘ ▀    ▀▀▝▘  ▀▀ ▝▘ ▝▘ ▝▀▘ ▝▘ ▝▘

A CLI and TUI for running markdown runbooks.


Usage: marathon <COMMAND>

Commands:
  run       Run a runbook interactively in the TUI, cell by cell
  exec      Run a runbook headlessly, streaming output to stdout (no TUI)
  validate  Parse and check a runbook without running anything [aliases: check]
  new       Scaffold a minimal new runbook
  skills    Manage marathon's agent skills
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version

$ marathon run path/to/myrunbook.md
````


