use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

/// Marathon is a tool for running markdown runbooks.
#[derive(Debug, Parser)]
#[clap(version, about)]
pub struct App {
    #[clap(subcommand)]
    pub cmd: RootCmd,
}

#[derive(Debug, Subcommand)]
pub enum RootCmd {
    /// Create a new runbook
    New(NewCmd),

    /// Run a runbook via the TUI
    Run(RunCmd),

    /// Run a runbook without opening the TUI
    Exec(ExecCmd),

    /// Validate a runbook
    Check(CheckCmd),
}

#[derive(Debug, Args)]
pub struct NewCmd {
    /// Path of runbook to create
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct RunCmd {
    /// Path of runbook to run
    ///
    /// NOTE: In future could be multiple and/or glob
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct ExecCmd {
    /// Path of runbook to execute
    pub path: PathBuf,

    // Automatically run cells and skip
    // prompting for inputs
    #[arg(short, long)]
    pub yes: bool,

    // JSON map of `ENV=val` to add to
    // command environments
    #[arg(short, long)]
    pub env: Option<String>,
}

#[derive(Debug, Args)]
pub struct CheckCmd {
    /// Path of runbook to validate
    ///
    /// NOTE: In future could be multiple and/or glob
    pub path: PathBuf,
}
