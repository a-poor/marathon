use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let args = marathon::cli::App::parse();

    match args.cmd {
        marathon::cli::RootCmd::Run(cmd) => {
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
