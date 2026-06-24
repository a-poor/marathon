use anyhow::{Context, Result};
use clap::Parser;
use marathon::book::{BookBlock, Runbook};
use marathon::cli::{ExecCmd, NewCmd, RunCmd, SkillsCmd, SkillsSub, ValidateCmd};
use marathon::runner::RunMsg;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<()> {
    let args = marathon::cli::App::parse();

    match args.cmd {
        marathon::cli::RootCmd::Run(cmd) => run(cmd).await,
        marathon::cli::RootCmd::Exec(cmd) => exec(cmd).await,
        marathon::cli::RootCmd::Validate(cmd) => validate(cmd).await,
        marathon::cli::RootCmd::New(cmd) => new(cmd).await,
        marathon::cli::RootCmd::Skills(cmd) => skills(cmd).await,
    }
}

/// Load a runbook from disk and parse it, layering in CLI `--env` overrides.
async fn load(path: &Path, env: Vec<(String, String)>) -> Result<Runbook> {
    let doc = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("reading runbook {}", path.display()))?;
    let mut rb =
        Runbook::new(Some(path), &doc).with_context(|| format!("parsing {}", path.display()))?;
    rb.cli_env = env.into_iter().collect::<HashMap<_, _>>();
    Ok(rb)
}

/// `run`: open the interactive TUI and step through the runbook.
async fn run(cmd: RunCmd) -> Result<()> {
    let rb = load(&cmd.path, cmd.common.env).await?;

    // Manual init/restore (rather than `ratatui::run`) so the terminal outlives the
    // closure under the async runtime, and so we always restore before propagating a
    // render error.
    let mut terminal = ratatui::init();
    let result = marathon::tui::App::new(rb).run(&mut terminal).await;
    ratatui::restore();
    result
}

/// `exec`: run the runbook headlessly, streaming each runnable cell's combined
/// output to **stdout** while marathon's own progress chrome goes to **stderr** — so
/// `marathon exec book.md > out.txt` captures exactly the cells' output and nothing
/// else. Cells run straight through in document order (the `--yes` behavior; per-cell
/// confirmation and interactive input prompting are deferred — DESIGN §5).
///
/// Output is raw: no sanitization and no forced `NO_COLOR`, unlike the TUI (DESIGN
/// §7). Execution is fail-fast — the first cell to exit non-zero stops the run and
/// `exec` exits with that cell's code (after cleaning up the temp dir), so CI sees a
/// faithful status.
async fn exec(cmd: ExecCmd) -> Result<()> {
    let mut rb = load(&cmd.path, cmd.common.env).await?;

    // Materialize $TMP_DIR up front so the first cell already sees it, and so it's
    // cleaned on drop (unless the runbook pins it / opts out of cleanup).
    rb.ensure_tmp_dir().context("creating temp dir")?;

    let total = rb
        .blocks
        .iter()
        .filter(|b| matches!(b, BookBlock::Code(c) if c.is_runnable()))
        .count();

    let mut ran = 0usize;
    let mut stdout = std::io::stdout();
    let mut failure: Option<(String, Option<i32>)> = None;

    for idx in 0..rb.blocks.len() {
        // Resolve everything the run needs (owned) before awaiting, so no borrow of
        // `rb` is held across the cell's execution.
        let (interp, script, env, label) = match &rb.blocks[idx] {
            BookBlock::Code(c) if c.is_runnable() => {
                ran += 1;
                let label = format!("cell {ran}/{total} ({})", c.lang);
                (
                    rb.interpreter_for(&c.lang),
                    rb.script_for(c),
                    rb.env_for(idx),
                    label,
                )
            }
            // Input cells can't be answered headlessly (no prompt, no default). They
            // resolve from pre-provided env if the key is set, else stay unset — note
            // which, since `set -eu` will fail a later cell that relies on it.
            BookBlock::Input(cell) => {
                let target = cell.target();
                if rb.base_env().contains_key(target) {
                    eprintln!(
                        "› input '{target}' ({}) — using value from env",
                        cell.kind()
                    );
                } else {
                    eprintln!(
                        "› input '{target}' ({}) — not set; pass -e {target}=… (downstream cells may fail)",
                        cell.kind()
                    );
                }
                continue;
            }
            _ => continue,
        };

        eprintln!("» {label}");

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = tokio::spawn(marathon::runner::run_streaming(
            idx, interp, script, env, tx,
        ));

        let mut success = false;
        let mut code = None;
        while let Some(msg) = rx.recv().await {
            match msg {
                RunMsg::Output { chunk, .. } => {
                    stdout
                        .write_all(chunk.as_bytes())
                        .and_then(|()| stdout.flush())
                        .context("writing cell output")?;
                }
                RunMsg::Finished {
                    success: s,
                    code: c,
                    ..
                } => {
                    success = s;
                    code = c;
                }
                RunMsg::Started { .. } => {}
            }
        }
        let _ = handle.await;

        if !success {
            failure = Some((label, code));
            break;
        }
    }

    if let Some((label, code)) = failure {
        match code {
            Some(c) => eprintln!("✗ {label} failed (exit {c})"),
            None => eprintln!("✗ {label} failed (killed by signal)"),
        }
        // Drop the runbook first so its temp-dir guard runs — `process::exit` skips
        // destructors — then exit with the cell's own code for a faithful CI status.
        let code = code.unwrap_or(1);
        drop(rb);
        std::process::exit(code);
    }

    eprintln!("✓ ran {total} cell(s)");
    Ok(())
}

/// `validate`: parse the runbook and report a summary; non-zero exit on parse error.
async fn validate(cmd: ValidateCmd) -> Result<()> {
    // No CLI env needed just to parse.
    let rb = load(&cmd.path, Vec::new()).await?;

    let mut runnable = 0usize;
    let mut display_only = 0usize;
    let mut inputs = 0usize;
    let mut prose = 0usize;
    for block in &rb.blocks {
        match block {
            BookBlock::Code(c) if c.is_runnable() => runnable += 1,
            BookBlock::Code(_) => display_only += 1,
            BookBlock::Input(_) => inputs += 1,
            BookBlock::Md(_) => prose += 1,
        }
    }

    println!("✓ {}: valid", cmd.path.display());
    if let Some(title) = rb.frontmatter.title.as_deref().filter(|s| !s.is_empty()) {
        println!("  title: {title}");
    }
    if let Some(desc) = rb
        .frontmatter
        .description
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        println!("  description: {desc}");
    }
    println!(
        "  {runnable} runnable cell(s), {display_only} display-only, {inputs} input(s), {prose} prose block(s)"
    );
    Ok(())
}

/// `skills`: manage marathon's bundled Claude Code agent skills.
async fn skills(cmd: SkillsCmd) -> Result<()> {
    match cmd.cmd {
        SkillsSub::Install(c) => {
            // The base is the project working tree (cwd) or the user's $HOME; the
            // skills module joins `.claude`/`.agents` beneath it.
            let base = if c.project {
                std::path::PathBuf::from(".")
            } else {
                std::env::home_dir().context("could not determine home directory")?
            };
            let report = marathon::skills::install(&base, c.target, c.force)?;
            println!("✓ installed marathon skill → {}", report.written.display());
            if let Some(link) = &report.linked {
                println!("  linked {} → it", link.display());
            }
            println!("  restart Claude Code (or reload skills) to pick it up");
            Ok(())
        }
    }
}

/// `new`: scaffold a minimal runbook at `path`, refusing to clobber an existing file.
async fn new(cmd: NewCmd) -> Result<()> {
    let path = &cmd.path;
    if path.exists() {
        anyhow::bail!("{} already exists — refusing to overwrite", path.display());
    }
    // Create parent dirs so `marathon new runbooks/deploy.md` just works.
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("runbook");
    let title = marathon::scaffold::title_from_stem(stem);
    let content = marathon::scaffold::runbook_template(&title);
    tokio::fs::write(path, content)
        .await
        .with_context(|| format!("writing {}", path.display()))?;

    println!("✓ created {}", path.display());
    println!("  run it:  marathon run {}", path.display());
    Ok(())
}
