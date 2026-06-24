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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;

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
    /// A line of output (stdout or stderr) streamed from a running cell. The chunk
    /// already includes its trailing newline.
    Output { idx: usize, chunk: String },
    /// The cell's process exited. `code` is the exit status if it exited normally
    /// (`None` if killed by a signal); surfaced on the cell when non-zero.
    Finished {
        idx: usize,
        success: bool,
        code: Option<i32>,
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

/// Run a cell and stream its output line-by-line back over `tx`: a [`RunMsg::Output`]
/// per line (stdout and stderr merged), then exactly one [`RunMsg::Finished`]. This
/// is the path the TUI uses so long-running cells reveal output as it arrives.
///
/// stdout/stderr are interleaved by arrival, not strictly ordered — true ordering
/// needs a pty and is deferred (DESIGN §7).
pub async fn run_streaming(
    idx: usize,
    interp: Vec<String>,
    script: String,
    env: HashMap<String, String>,
    tx: UnboundedSender<RunMsg>,
) {
    match stream_inner(idx, &interp, &script, &env, &tx).await {
        Ok((success, code)) => {
            let _ = tx.send(RunMsg::Finished { idx, success, code });
        }
        Err(e) => {
            let _ = tx.send(RunMsg::Output {
                idx,
                chunk: format!("failed to run: {e}\n"),
            });
            let _ = tx.send(RunMsg::Finished {
                idx,
                success: false,
                code: None,
            });
        }
    }
}

/// Spawn the process, pump output lines to `tx`, and return the success flag. Sends
/// no `Finished` — the caller does, so spawn errors get a uniform path.
async fn stream_inner(
    idx: usize,
    interp: &[String],
    script: &str,
    env: &HashMap<String, String>,
    tx: &UnboundedSender<RunMsg>,
) -> Result<(bool, Option<i32>)> {
    let (program, args) = interp
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty interpreter"))?;

    let mut child = Command::new(program)
        .args(args)
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Feed the script on a separate task (see `run_script`).
    let mut stdin = child.stdin.take().expect("stdin piped");
    let script = script.to_owned();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(script.as_bytes()).await;
    });

    let mut out_lines = BufReader::new(child.stdout.take().expect("stdout piped")).lines();
    let mut err_lines = BufReader::new(child.stderr.take().expect("stderr piped")).lines();
    let (mut out_done, mut err_done) = (false, false);

    while !(out_done && err_done) {
        tokio::select! {
            r = out_lines.next_line(), if !out_done => match r {
                Ok(Some(l)) => { let _ = tx.send(RunMsg::Output { idx, chunk: format!("{l}\n") }); }
                _ => out_done = true,
            },
            r = err_lines.next_line(), if !err_done => match r {
                Ok(Some(l)) => { let _ = tx.send(RunMsg::Output { idx, chunk: format!("{l}\n") }); }
                _ => err_done = true,
            },
        }
    }

    let status = child.wait().await?;
    let _ = writer.await;
    Ok((status.success(), status.code()))
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

    #[tokio::test]
    async fn streams_each_line_then_finishes() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        run_streaming(7, sh(), "printf 'a\\nb\\nc\\n'".into(), HashMap::new(), tx).await;

        let mut chunks = Vec::new();
        let mut finished = None;
        while let Ok(msg) = rx.try_recv() {
            match msg {
                RunMsg::Output { idx, chunk } => {
                    assert_eq!(idx, 7);
                    chunks.push(chunk);
                }
                RunMsg::Finished { idx, success, code } => {
                    assert_eq!(idx, 7);
                    finished = Some((success, code));
                }
            }
        }
        assert_eq!(chunks, vec!["a\n", "b\n", "c\n"]);
        assert_eq!(finished, Some((true, Some(0))));
    }

    #[tokio::test]
    async fn streaming_reports_failure() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        run_streaming(0, sh(), "exit 3".into(), HashMap::new(), tx).await;
        let mut finished = None;
        while let Ok(msg) = rx.try_recv() {
            if let RunMsg::Finished { success, code, .. } = msg {
                finished = Some((success, code));
            }
        }
        assert_eq!(finished, Some((false, Some(3))));
    }
}
