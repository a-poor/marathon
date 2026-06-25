//! End-to-end execution flow without the TUI: parse a runbook, answer an input
//! cell, assemble the env map, and run a shell cell — exercising the same
//! `ensure_tmp_dir` → `interpreter_for` / `script_for` / `env_for` → `run_script`
//! chain the TUI drives.

use marathon::book::{BookBlock, Runbook};
use marathon::runner::run_script;

const DOC: &str = "---
env:
  GREETING: hi
---

```json mrthn=input
{\"type\":\"input\",\"prompt\":\"Name?\",\"target\":\"WHO\"}
```

```sh
echo \"$GREETING $WHO\" > \"$TMP_DIR/out.txt\"
cat \"$TMP_DIR/out.txt\"
```
";

#[tokio::test]
async fn answered_input_feeds_a_shell_cell() {
    let mut rb = Runbook::new(None::<&str>, DOC).unwrap();

    // Answer the input cell (block 0): WHO = "world".
    rb.begin_edit_at(0);
    let cell = rb.input_at_mut(0).unwrap();
    for ch in "world".chars() {
        cell.insert_char(ch);
    }
    cell.submit();

    // Assemble + run the shell cell (block 1), as the TUI would.
    rb.ensure_tmp_dir().unwrap();
    let code_idx = 1;
    let (interp, script, env) = match &rb.blocks[code_idx] {
        BookBlock::Code(c) => (
            rb.interpreter_for(&c.lang),
            rb.script_for(c),
            rb.env_for(code_idx),
        ),
        other => panic!("expected code cell, got {other:?}"),
    };

    let result = run_script(&interp, &script, &env).await.unwrap();
    assert!(result.success, "cell failed: {}", result.output);
    assert_eq!(result.output.trim(), "hi world");
}
