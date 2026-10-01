//! Sequential CLI execution with prompts on stderr and raw cell bytes on stdout.

use std::io::{self, BufRead, Write};

use anyhow::{Context, Result, bail};
use tokio::sync::{mpsc, oneshot};

use crate::book::{BookBlock, InputCell, MagicInputBlock, Runbook};
use crate::runner::{self, RunMsg, RunningCell};

/// A detached reader keeps a blocking terminal read out of Tokio's blocking pool:
/// Ctrl-C can shut down the runtime even while the terminal is awaiting a line.
/// The thread owns no runbook/process state and ends on EOF or receiver closure.
#[derive(Default)]
struct Answers {
    rx: Option<mpsc::Receiver<io::Result<String>>>,
}

impl Answers {
    async fn read(&mut self, prompt: &str) -> Result<String> {
        eprint!("{prompt}");
        io::stderr().flush()?;
        let rx = self.rx.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel(1);
            std::thread::spawn(move || {
                for line in io::stdin().lock().lines() {
                    let failed = line.is_err();
                    if tx.blocking_send(line).is_err() || failed {
                        break;
                    }
                }
            });
            rx
        });
        rx.recv().await.context("input ended before an answer; use --yes with -e KEY=VALUE or input defaults for unattended runs")?
            .context("reading answer")
    }
}

/// A full downstream pipe must not block the async task that handles signals.
/// Acknowledgements ensure all bytes are written before normal completion. Like
/// the stdin reader, this thread owns no child or scratch-directory state.
#[derive(Default)]
struct Output {
    tx: Option<mpsc::Sender<WriteRequest>>,
}

struct WriteRequest {
    bytes: Vec<u8>,
    done: oneshot::Sender<io::Result<()>>,
}

impl Output {
    async fn write(&mut self, bytes: Vec<u8>) -> Result<()> {
        let tx = self.tx.get_or_insert_with(|| {
            let (tx, mut rx) = mpsc::channel::<WriteRequest>(1);
            std::thread::spawn(move || {
                let mut stdout = io::stdout().lock();
                while let Some(request) = rx.blocking_recv() {
                    let result = stdout
                        .write_all(&request.bytes)
                        .and_then(|()| stdout.flush());
                    let failed = result.is_err();
                    if request.done.send(result).is_err() || failed {
                        break;
                    }
                }
            });
            tx
        });
        let (done, result) = oneshot::channel();
        tx.send(WriteRequest { bytes, done })
            .await
            .context("output writer closed")?;
        result.await.context("output writer stopped")??;
        Ok(())
    }
}

/// Execute and fully clean up before returning an exit status to main.
pub async fn execute(mut book: Runbook, yes: bool) -> Result<i32> {
    let mut active = None;
    let result = tokio::select! {
        biased;
        signal = crate::term::termination() => {
            eprintln!("✗ run interrupted");
            signal
        }
        result = run_book(&mut book, yes, &mut active) => result,
    };
    if let Some(run) = active.take() {
        // run_book's receiver is dropped first, unblocking any output sends.
        run.shutdown().await;
    }
    // book's temp-dir guard drops only after the child has stopped.
    result
}

async fn run_book(book: &mut Runbook, yes: bool, active: &mut Option<RunningCell>) -> Result<i32> {
    book.ensure_tmp_dir().context("creating temp dir")?;
    let total = book
        .blocks
        .iter()
        .filter(|b| matches!(b, BookBlock::Code(c) if c.is_runnable()))
        .count();
    let mut ran = 0;
    let mut answers = Answers::default();
    let mut stdout = Output::default();

    for idx in 0..book.blocks.len() {
        let env = book.env_for(idx);
        let input_env = if matches!(book.blocks[idx], BookBlock::Input(_)) {
            book.input_env_for(idx)
        } else {
            Default::default()
        };
        if let BookBlock::Input(cell) = &mut book.blocks[idx] {
            cell.try_refresh_options(&input_env)?;
            if let Some(value) = input_env.get(cell.target()) {
                cell.answer(value.clone())?;
                eprintln!("› input '{}' — using supplied value", cell.target());
            } else if yes {
                let value = cell.validated_default()?.ok_or_else(|| {
                    cell.input_error(format!(
                        "needs a value; pass -e {}=VALUE or set its default",
                        cell.target()
                    ))
                })?;
                cell.answer(value)?;
                eprintln!("› input '{}' — using default", cell.target());
            } else {
                prompt_input(cell, &mut answers)
                    .await
                    .map_err(|e| cell.input_error(e))?;
            }
            continue;
        }

        let BookBlock::Code(cell) = &book.blocks[idx] else {
            continue;
        };
        if !cell.is_runnable() {
            continue;
        }
        let label = format!("cell {}/{total} ({})", ran + 1, cell.lang);
        let script = book.script_for(cell);
        if !yes {
            // Show the actual script, including hooks, before requesting consent.
            eprintln!("\n» {label}\n{}", crate::ansi::sanitize(&script));
            loop {
                let answer = answers.read("Run this cell? [y/N] ").await?;
                match answer.trim().to_ascii_lowercase().as_str() {
                    "y" | "yes" => break,
                    "" | "n" | "no" => {
                        eprintln!("✗ stopped before {label}");
                        return Ok(1);
                    }
                    _ => eprintln!("Enter yes or no."),
                }
            }
        } else {
            eprintln!("» {label}");
        }

        let (tx, mut rx) = mpsc::channel(runner::CHANNEL_CAPACITY);
        *active = Some(runner::spawn_run(
            idx,
            book.interpreter_for(&cell.lang),
            script,
            env,
            tx,
        ));
        let mut outcome = None;
        while let Some(msg) = rx.recv().await {
            match msg {
                RunMsg::Output { chunk, .. } => {
                    stdout.write(chunk).await.context("writing cell output")?;
                }
                RunMsg::Finished {
                    success,
                    code,
                    error,
                    ..
                } => {
                    outcome = Some((success, code, error));
                }
            }
        }
        if let Some(run) = active.take() {
            run.wait().await;
        }
        let (success, code, error) = outcome.context("runner ended without a result")?;
        if let Some(error) = error {
            bail!("{label}: {error}");
        }
        if !success {
            match code {
                Some(code) => eprintln!("✗ {label} failed (exit {code})"),
                None => eprintln!("✗ {label} terminated by signal"),
            }
            return Ok(code.unwrap_or(1));
        }
        ran += 1;
    }
    eprintln!("✓ ran {ran} cell(s)");
    Ok(0)
}

async fn prompt_input(cell: &mut InputCell, answers: &mut Answers) -> Result<()> {
    eprintln!(
        "\n{} → ${}",
        crate::ansi::sanitize(cell.prompt()),
        cell.target()
    );
    if matches!(cell.config, MagicInputBlock::Select { .. }) {
        for (i, option) in cell.options().iter().enumerate() {
            eprintln!("  {}. {}", i + 1, crate::ansi::sanitize(option));
        }
    }
    let default = match cell.validated_default() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}; enter an answer instead.");
            None
        }
    };
    let hint = match &cell.config {
        MagicInputBlock::Confirm { .. } => "yes/no",
        MagicInputBlock::Select { .. } => "option number",
        MagicInputBlock::Input { .. } => "value",
    };
    let prompt = match &default {
        Some(value) => format!("{hint} [{}]: ", crate::ansi::sanitize(value)),
        None if matches!(cell.config, MagicInputBlock::Confirm { .. }) => "yes/no [no]: ".into(),
        None => format!("{hint}: "),
    };
    loop {
        let answer = answers.read(&prompt).await?;
        if answer.is_empty()
            && default.is_none()
            && matches!(cell.config, MagicInputBlock::Select { .. })
        {
            eprintln!(
                "{}",
                cell.input_error(format!(
                    "Enter an option number from 1 to {}.",
                    cell.options().len()
                ))
            );
            continue;
        }
        let value = if answer.is_empty() {
            default.clone().unwrap_or_else(|| {
                if matches!(cell.config, MagicInputBlock::Confirm { .. }) {
                    "no".into()
                } else {
                    String::new()
                }
            })
        } else if matches!(cell.config, MagicInputBlock::Select { .. }) {
            match answer
                .trim()
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|i| cell.options().get(i))
            {
                Some(value) => value.clone(),
                None => {
                    eprintln!(
                        "{}",
                        cell.input_error(format!(
                            "Enter an option number from 1 to {}.",
                            cell.options().len()
                        ))
                    );
                    continue;
                }
            }
        } else {
            answer
        };
        match cell.answer(value) {
            Ok(()) => return Ok(()),
            Err(error) => eprintln!("{error}"),
        }
    }
}
