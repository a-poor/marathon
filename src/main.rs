use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let _ = marathon::cli::App::parse();
    Ok(())
}
