//! Owned cell processes with byte-preserving output and cancellation.

use std::collections::HashMap;
use std::path::Path;
use std::process::{ExitStatus, Stdio};

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// Bound queued output to 32 chunks (at most 8 KiB each), applying backpressure.
pub const CHANNEL_CAPACITY: usize = 32;

#[derive(Debug)]
pub enum RunMsg {
    /// A TUI capture changed on disk; carries no accumulated output.
    Captured {
        idx: usize,
    },
    Output {
        idx: usize,
        chunk: Vec<u8>,
    },
    /// Sent after cleanup. Runner errors are separate from command output and
    /// from a command exiting nonzero or by signal.
    Finished {
        idx: usize,
        success: bool,
        code: Option<i32>,
        error: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    None,
    Interrupt,
    Kill,
}

/// Owns a run from scheduling through cleanup. Dropping requests a kill; use
/// `shutdown` to also wait for cleanup before exiting or removing scratch files.
pub struct RunningCell {
    stop: Option<watch::Sender<Stop>>,
    task: Option<JoinHandle<()>>,
}

impl RunningCell {
    pub fn cancel(&self, hard: bool) {
        if let Some(sender) = &self.stop {
            sender.send_modify(|stop| {
                *stop = if hard || *stop == Stop::Kill {
                    Stop::Kill
                } else {
                    Stop::Interrupt
                };
            });
        }
    }

    pub async fn wait(mut self) {
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }

    pub async fn shutdown(mut self) {
        self.cancel(true);
        self.stop.take(); // cleanup must not block on a full output queue
        self.wait().await;
    }
}

impl Drop for RunningCell {
    fn drop(&mut self) {
        self.cancel(true);
    }
}

pub fn spawn_run(
    idx: usize,
    interp: Vec<String>,
    script: String,
    env: HashMap<String, String>,
    tx: mpsc::Sender<RunMsg>,
) -> RunningCell {
    spawn_run_inner(idx, interp, script, env, tx, None)
}

/// TUI variant: spool on blocking workers and report bounded change notices.
pub fn spawn_captured_run(
    idx: usize,
    interp: Vec<String>,
    script: String,
    env: HashMap<String, String>,
    tx: mpsc::Sender<RunMsg>,
    capture: crate::output::OutputCapture,
) -> RunningCell {
    spawn_run_inner(idx, interp, script, env, tx, Some(capture))
}

fn spawn_run_inner(
    idx: usize,
    interp: Vec<String>,
    script: String,
    env: HashMap<String, String>,
    tx: mpsc::Sender<RunMsg>,
    capture: Option<crate::output::OutputCapture>,
) -> RunningCell {
    let (stop, mut rx) = watch::channel(Stop::None);
    let task = tokio::spawn(async move {
        let spool = capture.map(SpoolWorkers::new);
        let result = stream_inner(idx, &interp, &script, &env, &tx, &mut rx, spool.as_ref()).await;
        let result = if let Some(spool) = spool {
            // A canceled pipe future can leave a blocking disk write in flight.
            // Join these before finishing the capture or sending Finished.
            let capture = spool.join().await;
            let finish = tokio::task::spawn_blocking(move || capture.finish()).await;
            match finish {
                Ok(Ok(())) => result,
                Ok(Err(e)) if result.is_ok() => {
                    Err(anyhow::Error::from(e).context("finishing output spool"))
                }
                Err(e) => Err(anyhow::Error::from(e).context("output spool worker")),
                _ => result,
            }
        } else {
            result
        };
        let (success, code, error) = match result {
            Ok(Some(status)) => (status.success(), status.code(), None),
            Ok(None) => (false, None, None), // canceled before spawning
            Err(e) => (false, None, Some(format!("{e:#}"))),
        };
        tokio::select! {
            _ = tx.send(RunMsg::Finished { idx, success, code, error }) => {}
            _ = rx.wait_for(|_| false) => {} // owner dropped/shut down
        }
    });
    RunningCell {
        stop: Some(stop),
        task: Some(task),
    }
}

/// Own all disk jobs, including those whose awaiting pipe future was canceled.
/// At most one write per pipe is outstanding; completed handles are pruned.
struct SpoolWorkers {
    capture: crate::output::OutputCapture,
    jobs: std::sync::Mutex<Vec<JoinHandle<()>>>,
}

impl SpoolWorkers {
    fn new(capture: crate::output::OutputCapture) -> Self {
        Self {
            capture,
            jobs: std::sync::Mutex::new(Vec::new()),
        }
    }

    async fn append(&self, chunk: Vec<u8>) -> Result<()> {
        let capture = self.capture.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let job = tokio::task::spawn_blocking(move || {
            let _ = tx.send(capture.append(&chunk));
        });
        {
            let mut jobs = self.jobs.lock().unwrap();
            jobs.retain(|job| !job.is_finished());
            jobs.push(job);
        }
        rx.await.context("output spool worker stopped")??;
        Ok(())
    }

    async fn join(self) -> crate::output::OutputCapture {
        for job in self.jobs.into_inner().unwrap() {
            let _ = job.await;
        }
        self.capture
    }
}

fn is_shell(interp: &[String]) -> bool {
    interp.iter().any(|s| {
        Path::new(s)
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|name| matches!(name, "sh" | "bash" | "zsh"))
    })
}

fn merge_streams(interp: &[String], script: &str) -> String {
    if is_shell(interp) {
        format!("exec 2>&1\n{script}")
    } else {
        script.to_owned()
    }
}

/// Also covers task abortion/unwinding. Normal paths kill/reap explicitly.
struct Process {
    child: Child,
    pid: Option<u32>,
}

impl Process {
    fn signal(&self, hard: bool) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            // SAFETY: pid belongs to the process group created for this run.
            unsafe {
                libc::kill(
                    -(pid as i32),
                    if hard { libc::SIGKILL } else { libc::SIGINT },
                );
            }
        }
        #[cfg(not(unix))]
        let _ = hard;
    }

    async fn kill_and_wait(&mut self) -> Result<ExitStatus> {
        self.signal(true);
        let _ = self.child.start_kill();
        let status = self
            .child
            .wait()
            .await
            .context("waiting for canceled process")?;
        self.pid = None;
        Ok(status)
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.pid.is_some() {
            self.signal(true);
            let _ = self.child.start_kill();
        }
    }
}

async fn pump(
    mut reader: impl AsyncRead + Unpin,
    idx: usize,
    tx: &mpsc::Sender<RunMsg>,
    capture: Option<&SpoolWorkers>,
) -> Result<()> {
    let mut bytes = [0; 8192];
    loop {
        let n = reader
            .read(&mut bytes)
            .await
            .context("reading cell output")?;
        if n == 0 {
            return Ok(());
        }
        let chunk = bytes[..n].to_vec();
        let msg = if let Some(capture) = capture {
            capture
                .append(chunk)
                .await
                .context("writing output spool")?;
            RunMsg::Captured { idx }
        } else {
            RunMsg::Output { idx, chunk }
        };
        tx.send(msg).await.context("output receiver closed")?;
    }
}

async fn stream_inner(
    idx: usize,
    interp: &[String],
    script: &str,
    env: &HashMap<String, String>,
    tx: &mpsc::Sender<RunMsg>,
    stop: &mut watch::Receiver<Stop>,
    capture: Option<&SpoolWorkers>,
) -> Result<Option<ExitStatus>> {
    if *stop.borrow() == Stop::Kill || tx.is_closed() {
        return Ok(None);
    }
    let (program, args) = interp.split_first().context("empty interpreter")?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .envs(env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd.spawn().with_context(|| format!("starting {program}"))?;
    let mut process = Process {
        pid: child.id(),
        child,
    };
    let mut stdin = process.child.stdin.take().expect("stdin piped");
    let stdout = process.child.stdout.take().expect("stdout piped");
    let stderr = process.child.stderr.take().expect("stderr piped");
    let script = merge_streams(interp, script);

    // Poll script writing and both output pipes concurrently so neither a large
    // script nor a noisy child can deadlock the other side of the pipes.
    let io = async {
        let write = async {
            let result = stdin.write_all(script.as_bytes()).await;
            drop(stdin);
            match result {
                Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
                _ => Ok(()), // an early exit may intentionally stop reading stdin
            }
        };
        tokio::try_join!(
            write,
            pump(stdout, idx, tx, capture),
            pump(stderr, idx, tx, capture)
        )?;
        Ok::<_, anyhow::Error>(())
    };
    tokio::pin!(io);
    let mut io_done = false;
    let mut exit_status = None;
    let outcome = loop {
        if io_done && let Some(status) = exit_status {
            break Some(Ok(status));
        }
        let request = *stop.borrow_and_update();
        match request {
            Stop::Kill => break None,
            Stop::Interrupt => {
                process.signal(false);
                #[cfg(not(unix))]
                break None;
            }
            Stop::None => {}
        }
        tokio::select! {
            biased;
            _ = tx.closed() => break None,
            changed = stop.changed() => {
                if changed.is_err() { break None; }
            }
            result = &mut io, if !io_done => {
                match result {
                    Ok(()) => io_done = true,
                    Err(e) => break Some(Err(e)),
                }
            }
            status = process.child.wait(), if exit_status.is_none() => {
                match status.context("waiting for cell") {
                    Ok(status) => {
                        // Stop background descendants as soon as the shell exits,
                        // then drain the remaining pipe bytes before Finished.
                        process.signal(true);
                        process.pid = None;
                        exit_status = Some(status);
                    }
                    Err(e) => break Some(Err(e)),
                }
            }
        }
    };
    match outcome {
        Some(Ok(status)) => Ok(Some(status)),
        Some(Err(e)) => {
            let _ = process.kill_and_wait().await;
            Err(e)
        }
        None => process.kill_and_wait().await.map(Some),
    }
}

/// Text convenience API; streaming itself preserves arbitrary bytes.
pub struct RunResult {
    pub success: bool,
    pub output: String,
}

pub async fn run_script(
    interp: &[String],
    script: &str,
    env: &HashMap<String, String>,
) -> Result<RunResult> {
    let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
    let run = spawn_run(0, interp.to_vec(), script.to_owned(), env.clone(), tx);
    let mut output = Vec::new();
    let mut result = Err(anyhow::anyhow!("runner ended without a result"));
    while let Some(msg) = rx.recv().await {
        match msg {
            RunMsg::Captured { .. } => unreachable!("text adapter does not spool"),
            RunMsg::Output { chunk, .. } => output.extend(chunk),
            RunMsg::Finished { success, error, .. } => {
                result = match error {
                    Some(e) => Err(anyhow::anyhow!(e)),
                    None => Ok(success),
                };
            }
        }
    }
    run.wait().await;
    Ok(RunResult {
        success: result?,
        output: String::from_utf8_lossy(&output).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sh() -> Vec<String> {
        vec!["/usr/bin/env".into(), "sh".into()]
    }

    async fn collect(script: &str) -> (Vec<u8>, bool, Option<i32>) {
        let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
        let run = spawn_run(7, sh(), script.into(), HashMap::new(), tx);
        let mut output = Vec::new();
        let mut finished = None;
        while let Some(msg) = rx.recv().await {
            match msg {
                RunMsg::Captured { .. } => unreachable!("test runner does not spool"),
                RunMsg::Output { idx, chunk } => {
                    assert_eq!(idx, 7);
                    output.extend(chunk);
                }
                RunMsg::Finished {
                    idx,
                    success,
                    code,
                    error,
                } => {
                    assert_eq!(idx, 7);
                    assert!(error.is_none(), "{error:?}");
                    assert!(finished.is_none(), "duplicate finish");
                    finished = Some((success, code));
                }
            }
        }
        run.wait().await;
        let (success, code) = finished.expect("finished message");
        (output, success, code)
    }

    #[tokio::test]
    async fn preserves_bytes_and_exit_status() {
        let (bytes, success, code) = collect("printf 'a\\r\\nb\\377\\000'; exit 7").await;
        assert_eq!(bytes, b"a\r\nb\xff\0");
        assert!(!success);
        assert_eq!(code, Some(7));
    }

    #[tokio::test]
    async fn merges_stderr_in_written_order() {
        let (bytes, success, _) = collect("printf one; printf two >&2; printf three").await;
        assert_eq!(bytes, b"onetwothree");
        assert!(success);
    }

    #[tokio::test]
    async fn injects_env_and_collects_text() {
        let result = run_script(
            &sh(),
            "printf \"$GREETING\"",
            &HashMap::from([("GREETING".into(), "hello".into())]),
        )
        .await
        .unwrap();
        assert!(result.success);
        assert_eq!(result.output, "hello");
    }

    #[tokio::test]
    async fn streams_without_waiting_for_a_newline() {
        let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
        let run = spawn_run(0, sh(), "printf ready; sleep 10".into(), HashMap::new(), tx);
        let message = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(message, RunMsg::Output { chunk, .. } if chunk == b"ready"));
        tokio::time::timeout(Duration::from_secs(3), run.shutdown())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn shutdown_does_not_block_on_full_output_queue() {
        let (tx, mut rx) = mpsc::channel(1);
        let run = spawn_run(
            0,
            sh(),
            "while :; do printf 'output\\n'; done".into(),
            HashMap::new(),
            tx,
        );
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), run.shutdown())
            .await
            .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_stops_descendants_and_escalates_ignored_interrupts() {
        let dir = tempfile::TempDir::new().unwrap();
        let marker = dir.path().join("survived");
        let release = dir.path().join("release");
        let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
        let env = HashMap::from([
            ("MARKER".into(), marker.display().to_string()),
            ("RELEASE".into(), release.display().to_string()),
        ]);
        let run = spawn_run(0, sh(), "trap '' INT\n(while [ ! -e \"$RELEASE\" ]; do sleep 0.05; done; touch \"$MARKER\") &\nprintf ready\nwait".into(), env, tx);
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        run.cancel(false);
        run.cancel(true);
        tokio::time::timeout(Duration::from_secs(3), run.shutdown())
            .await
            .unwrap();
        std::fs::write(release, "go").unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!marker.exists(), "descendant survived cancellation");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn interrupt_finishes_a_running_cell() {
        let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
        let run = spawn_run(
            0,
            sh(),
            "trap 'exit 130' INT; printf ready; while :; do sleep 0.05; done".into(),
            HashMap::new(),
            tx,
        );
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        run.cancel(false);
        let msg = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            msg,
            RunMsg::Finished {
                success: false,
                error: None,
                ..
            }
        ));
        run.wait().await;
    }

    #[tokio::test]
    async fn spawn_errors_are_not_command_output() {
        let (tx, mut rx) = mpsc::channel(CHANNEL_CAPACITY);
        let run = spawn_run(
            0,
            vec!["/no/such/marathon-interpreter".into()],
            String::new(),
            HashMap::new(),
            tx,
        );
        assert!(matches!(
            rx.recv().await,
            Some(RunMsg::Finished {
                success: false,
                error: Some(_),
                ..
            })
        ));
        assert!(rx.recv().await.is_none());
        run.wait().await;
    }

    #[tokio::test]
    async fn large_script_and_output_do_not_deadlock() {
        let script = format!(
            "head -c 100000 /dev/zero\n# {}\nprintf done",
            "x".repeat(100000)
        );
        let (bytes, success, _) = tokio::time::timeout(Duration::from_secs(5), collect(&script))
            .await
            .unwrap();
        assert!(success);
        assert_eq!(bytes.len(), 100004);
        assert!(bytes.ends_with(b"done"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_exit_stops_background_jobs_without_waiting_for_pipe_eof() {
        let (bytes, success, _) = tokio::time::timeout(
            Duration::from_secs(3),
            collect("sleep 10 &\nprintf complete"),
        )
        .await
        .unwrap();
        assert!(success);
        assert_eq!(bytes, b"complete");
    }

    #[tokio::test]
    async fn dropping_an_owner_cleans_up_even_when_the_receiver_stays_open() {
        let (tx, mut rx) = mpsc::channel(1);
        let run = spawn_run(0, sh(), "printf ready; sleep 10".into(), HashMap::new(), tx);
        tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .unwrap()
            .unwrap();
        drop(run);
        tokio::time::timeout(Duration::from_secs(3), async {
            while rx.recv().await.is_some() {}
        })
        .await
        .unwrap();
    }

    #[test]
    fn recognizes_shell_interpreters() {
        assert!(is_shell(&sh()));
        assert!(is_shell(&["/bin/bash".into(), "-e".into()]));
        assert!(!is_shell(&["/usr/bin/env".into(), "python3".into()]));
    }
}
