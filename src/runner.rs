//! Cell execution: run a script through an interpreter with an injected env map.
//!
//! Each runnable cell is its own process (DESIGN §3) — no shared in-process shell.
//! Cells communicate only through the accumulated environment map (assembled by
//! [`crate::book::Runbook::env_for`]) and files under `TMP_DIR`. This module is the
//! thin process layer: build the command, feed the script on stdin, collect the
//! combined output and exit status.
//!
//! Output is captured whole (not streamed) in this first pass; live streaming and
//! ANSI handling are deferred (DESIGN §7).

use std::collections::HashMap;
use std::process::Stdio;

use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// The outcome of running one cell.
#[derive(Debug)]
pub struct RunResult {
    /// Whether the process exited 0.
    pub success: bool,
    /// Combined stdout + stderr.
    pub output: String,
}

/// A message from a spawned cell run back to the UI loop.
#[derive(Debug)]
pub enum RunMsg {
    /// A cell finished (successfully or not).
    Done {
        idx: usize,
        success: bool,
        output: String,
    },
}

/// Run `script` through `interp` (e.g. `["/usr/bin/env", "sh"]`) with `env`
/// overlaid on the inherited environment. The script is fed on stdin so multi-line
/// bodies need no escaping. Returns the combined output and success flag.
pub async fn run_script(
    interp: &[String],
    script: &str,
    env: &HashMap<String, String>,
) -> Result<RunResult> {
    let (program, args) = interp
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty interpreter"))?;

    let mut cmd = Command::new(program);
    cmd.args(args)
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn()?;

    // Feed the script on a separate task so a child that floods stdout before
    // draining stdin can't deadlock against our write.
    let mut stdin = child.stdin.take().expect("stdin piped");
    let script = script.to_owned();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(script.as_bytes()).await;
        // stdin dropped here → EOF for the child.
    });

    let out = child.wait_with_output().await?;
    let _ = writer.await;

    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    let err = String::from_utf8_lossy(&out.stderr);
    if !err.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&err);
    }

    Ok(RunResult {
        success: out.status.success(),
        output: combined,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh() -> Vec<String> {
        vec!["/usr/bin/env".to_string(), "sh".to_string()]
    }

    #[tokio::test]
    async fn runs_and_captures_stdout() {
        let res = run_script(&sh(), "echo hello", &HashMap::new())
            .await
            .unwrap();
        assert!(res.success);
        assert_eq!(res.output.trim(), "hello");
    }

    #[tokio::test]
    async fn nonzero_exit_is_not_success() {
        let res = run_script(&sh(), "exit 3", &HashMap::new()).await.unwrap();
        assert!(!res.success);
    }

    #[tokio::test]
    async fn injects_env() {
        let mut env = HashMap::new();
        env.insert("GREETING".to_string(), "howdy".to_string());
        let res = run_script(&sh(), "echo \"$GREETING\"", &env).await.unwrap();
        assert_eq!(res.output.trim(), "howdy");
    }

    #[tokio::test]
    async fn captures_stderr_too() {
        let res = run_script(&sh(), "echo oops 1>&2", &HashMap::new())
            .await
            .unwrap();
        assert!(res.success);
        assert!(res.output.contains("oops"));
    }
}
