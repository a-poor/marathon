use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use marathon::book::{BookBlock, Runbook};
use marathon::cli::{
    App, CompletionsCmd, ExecCmd, NewCmd, RunCmd, SkillsCmd, SkillsSub, ValidateCmd,
};
use std::collections::HashMap;
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
        marathon::cli::RootCmd::Completions(cmd) => completions(cmd),
    }
}

/// `completions`: print a shell completion script for `shell` to stdout. Synchronous
/// — it just renders clap's command tree; the caller can pipe it to the right file
/// (e.g. `marathon completions zsh > ~/.zfunc/_marathon`).
fn completions(cmd: CompletionsCmd) -> Result<()> {
    let mut command = App::command();
    let name = command.get_name().to_string();
    clap_complete::generate(cmd.shell, &mut command, name, &mut std::io::stdout());
    Ok(())
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

/// Run sequentially, then propagate the command status after all cleanup.
async fn exec(cmd: ExecCmd) -> Result<()> {
    let book = load(&cmd.path, cmd.common.env).await?;
    if cmd.list {
        for idx in book.cells() {
            let kind = match &book.blocks[idx] {
                BookBlock::Code(c) => c.lang.as_str(),
                BookBlock::Input(c) => c.kind(),
                _ => unreachable!(),
            };
            let needs: Vec<_> = book
                .prerequisites(idx)
                .into_iter()
                .map(|i| book.cell_label(i))
                .collect();
            let suffix = if needs.is_empty() {
                String::new()
            } else {
                format!(" — needs {}", needs.join(", "))
            };
            println!("{}  {kind}{suffix}", book.cell_label(idx));
        }
        return Ok(());
    }
    let code = marathon::exec::execute_selected(book, cmd.yes, &cmd.selection).await?;
    if code != 0 {
        std::process::exit(code);
    }
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
