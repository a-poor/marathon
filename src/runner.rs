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
use std::path::Path;
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
    /// The cell's process has spawned, carrying its OS process id so the UI can send
    /// it a signal (e.g. SIGINT to cancel). On unix the child leads its own process
    /// group, so signalling `-pid` reaches the shell *and* its descendants.
    Started { idx: usize, pid: u32 },
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

/// Whether `interp` invokes a recognized POSIX shell (`sh`/`bash`/`zsh`), matching
/// any token's basename — so `["/usr/bin/env", "sh"]`, `["/bin/bash"]`, and
/// `["/bin/zsh", "-f"]` all count, but a custom non-shell interpreter does not. Used
/// to gate the `exec 2>&1` merge, which only a shell understands.
fn is_shell(interp: &[String]) -> bool {
    interp.iter().any(|s| {
        Path::new(s)
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|name| matches!(name, "sh" | "bash" | "zsh"))
    })
}

/// Prepend `exec 2>&1` so the shell points its stderr at stdout *at the source*,
/// yielding one stream in true written order (DESIGN §7). Only for recognized shells
/// — a non-shell interpreter wouldn't understand the redirect, so it's left as-is and
/// keeps its separate streams.
fn merge_streams(interp: &[String], script: &str) -> String {
    if is_shell(interp) {
        format!("exec 2>&1\n{script}")
    } else {
        script.to_owned()
    }
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
    let script = merge_streams(interp, script);
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
/// per line, then exactly one [`RunMsg::Finished`]. This is the path the TUI uses so
/// long-running cells reveal output as it arrives.
///
/// For shell cells, stderr is merged into stdout at the source via `exec 2>&1` (see
/// [`merge_streams`]), so the single stream we read is already in true written order.
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
///
/// stderr is merged into stdout in the child (`exec 2>&1`), so we read a single
/// stream and never have to interleave two pipes. For a non-shell interpreter (no
/// merge) stderr goes to `Stdio::null` rather than being captured — acceptable since
/// the runnable cells are shells; richer non-shell capture can come with the pty work.
async fn stream_inner(
    idx: usize,
    interp: &[String],
    script: &str,
    env: &HashMap<String, String>,
    tx: &UnboundedSender<RunMsg>,
) -> Result<(bool, Option<i32>)> {
    let script = merge_streams(interp, script);
    let (program, args) = interp
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty interpreter"))?;

    let mut cmd = Command::new(program);
    cmd.args(args)
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Run in a fresh process group (leader = the child) so a cancel can signal the
    // whole job — the shell and anything it spawned — via `kill(-pid, …)`.
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd.spawn()?;

    // Hand the pid back so the UI can signal it (e.g. SIGINT to cancel).
    if let Some(pid) = child.id() {
        let _ = tx.send(RunMsg::Started { idx, pid });
    }

    // Feed the script on a separate task (see `run_script`).
    let mut stdin = child.stdin.take().expect("stdin piped");
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(script.as_bytes()).await;
    });

    let mut out_lines = BufReader::new(child.stdout.take().expect("stdout piped")).lines();
    while let Some(l) = out_lines.next_line().await? {
        let _ = tx.send(RunMsg::Output {
            idx,
            chunk: format!("{l}\n"),
        });
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
                RunMsg::Started { idx, pid } => {
                    assert_eq!(idx, 7);
                    assert!(pid > 0);
                }
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

    #[test]
    fn is_shell_keys_on_basename() {
        assert!(is_shell(&["/usr/bin/env".into(), "sh".into()]));
        assert!(is_shell(&["/bin/bash".into()]));
        assert!(is_shell(&["/bin/zsh".into(), "-f".into()])); // shell with a flag
        assert!(!is_shell(&["/usr/bin/env".into(), "python3".into()]));
        assert!(!is_shell(&[]));
    }

    #[tokio::test]
    async fn merges_stderr_into_stdout_in_written_order() {
        // stdout, stderr, stdout — `exec 2>&1` must preserve this exact order, which
        // two separate pipes could not guarantee.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        run_streaming(
            0,
            sh(),
            "echo one\necho two 1>&2\necho three".into(),
            HashMap::new(),
            tx,
        )
        .await;

        let mut chunks = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            if let RunMsg::Output { chunk, .. } = msg {
                chunks.push(chunk);
            }
        }
        assert_eq!(chunks, vec!["one\n", "two\n", "three\n"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sigint_cancels_a_running_cell() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let started = std::time::Instant::now();
        let handle = tokio::spawn(run_streaming(0, sh(), "sleep 5".into(), HashMap::new(), tx));

        // Wait for the spawned pid, then SIGINT its whole process group.
        let pid = loop {
            match rx.recv().await {
                Some(RunMsg::Started { pid, .. }) => break pid,
                Some(_) => continue,
                None => panic!("run ended before a Started message"),
            }
        };
        unsafe {
            libc::kill(-(pid as i32), libc::SIGINT);
        }

        handle.await.unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(4),
            "cancel did not interrupt the 5s sleep"
        );
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
