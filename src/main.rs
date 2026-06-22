use anyhow::Result;
use clap::Parser;
use marathon::book::Runbook;

#[tokio::main]
async fn main() -> Result<()> {
    let args = marathon::cli::App::parse();

    match args.cmd {
        marathon::cli::RootCmd::Run(cmd) => {
            // Read in the file
            let doc = tokio::fs::read_to_string(&cmd.path).await?;

            // Parse it as a runbook
            let rb = Runbook::new(Some(&cmd.path), &doc)?;

            // Launch the TUI. Use manual init/restore (rather than `ratatui::run`)
            // so the terminal outlives the closure under the async runtime, and so
            // we always restore before propagating a render error.
            let mut terminal = ratatui::init();
            let result = marathon::tui::App::new(rb).run(&mut terminal).await;
            ratatui::restore();
            result?;
        }
        marathon::cli::RootCmd::Check(cmd) => {
            let txt = tokio::fs::read_to_string(&cmd.path).await?;
            let mut po = markdown::ParseOptions::gfm();
            po.constructs.frontmatter = true;
            let ast = markdown::to_mdast(&txt, &po).unwrap();
            for n in ast.children().unwrap() {
                println!("{:?}\n", n);
            }
        }
        _ => {}
    }

    Ok(())
}
