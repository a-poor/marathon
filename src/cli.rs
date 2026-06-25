use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[clap(
    name = "marathon",
    version,
    about,
    long_about = "
                         ▗▖
                     ▐▌  ▐▌
▐█▙█▖ ▟██▖ █▟█▌ ▟██▖▐███ ▐▙██▖ ▟█▙ ▐▙██▖
▐▌█▐▌ ▘▄▟▌ █▘   ▘▄▟▌ ▐▌  ▐▛ ▐▌▐▛ ▜▌▐▛ ▐▌
▐▌█▐▌▗█▀▜▌ █   ▗█▀▜▌ ▐▌  ▐▌ ▐▌▐▌ ▐▌▐▌ ▐▌
▐▌█▐▌▐▙▄█▌ █   ▐▙▄█▌ ▐▙▄ ▐▌ ▐▌▝█▄█▘▐▌ ▐▌
▝▘▀▝▘ ▀▀▝▘ ▀    ▀▀▝▘  ▀▀ ▝▘ ▝▘ ▝▀▘ ▝▘ ▝▘

A CLI and TUI for running markdown runbooks.
"
)]
pub struct App {
    #[clap(subcommand)]
    pub cmd: RootCmd,
}

#[derive(Debug, Subcommand)]
pub enum RootCmd {
    /// Run a runbook interactively in the TUI, cell by cell
    Run(RunCmd),

    /// Run a runbook headlessly, streaming output to stdout (no TUI)
    Exec(ExecCmd),

    /// Parse and check a runbook without running anything
    #[command(visible_alias = "check")]
    Validate(ValidateCmd),

    /// Scaffold a minimal new runbook
    New(NewCmd),

    /// Manage marathon's Claude Code agent skills
    Skills(SkillsCmd),

    /// Print a shell completion script to stdout
    Completions(CompletionsCmd),
}

#[derive(Debug, Args)]
pub struct CompletionsCmd {
    /// Shell to generate completions for (e.g. bash, zsh, fish, elvish, powershell)
    pub shell: clap_complete::Shell,
}

#[derive(Debug, Args)]
pub struct SkillsCmd {
    #[command(subcommand)]
    pub cmd: SkillsSub,
}

#[derive(Debug, Subcommand)]
pub enum SkillsSub {
    /// Install marathon's runbook-authoring skill for Claude Code
    Install(SkillsInstallCmd),
}

#[derive(Debug, Args)]
pub struct SkillsInstallCmd {
    /// Which skills directory to install into: `claude` (.claude/skills), `agents`
    /// (.agents/skills), or `both` — write to .agents and symlink .claude to it.
    #[arg(long, value_enum, default_value_t = crate::skills::Target::default())]
    pub target: crate::skills::Target,

    /// Install into the current project (`./.claude` / `./.agents`) instead of the
    /// user-level directories under `$HOME`.
    #[arg(long)]
    pub project: bool,

    /// Overwrite an existing install instead of refusing.
    #[arg(short, long)]
    pub force: bool,
}

/// Flags shared by the run paths (`run` and `exec`) for shaping the cells'
/// execution environment. Flattened into each so they read as first-class args.
#[derive(Debug, Args)]
pub struct CommonArgs {
    /// Set an environment variable for every cell (repeatable): `-e KEY=VAL`.
    ///
    /// Layered over the runbook's frontmatter `env` (CLI wins), beneath per-cell
    /// additions like answered inputs and `$TMP_DIR` (see DESIGN §4).
    #[arg(short, long = "env", value_parser = parse_key_val, value_name = "KEY=VAL")]
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Args)]
pub struct RunCmd {
    /// Path of the runbook to run
    ///
    /// NOTE: In future could be multiple and/or glob
    pub path: PathBuf,

    #[command(flatten)]
    pub common: CommonArgs,
}

#[derive(Debug, Args)]
pub struct ExecCmd {
    /// Path of the runbook to execute
    pub path: PathBuf,

    /// Run straight through without per-cell confirmation, and answer input cells
    /// with their defaults instead of prompting. The sharp edge — runs arbitrary
    /// code unattended (DESIGN §5).
    #[arg(short, long)]
    pub yes: bool,

    #[command(flatten)]
    pub common: CommonArgs,
}

#[derive(Debug, Args)]
pub struct ValidateCmd {
    /// Path of the runbook to validate
    ///
    /// NOTE: In future could be multiple and/or glob
    pub path: PathBuf,
}

#[derive(Debug, Args)]
pub struct NewCmd {
    /// Path of the runbook to create
    pub path: PathBuf,
}

/// Parse a `KEY=VAL` pair for `--env`. The value may contain `=`; only the first
/// splits. An empty key (e.g. `=val`) is rejected.
fn parse_key_val(s: &str) -> Result<(String, String), String> {
    let (key, val) = s
        .split_once('=')
        .ok_or_else(|| format!("expected `KEY=VAL`, got `{s}` (no `=`)"))?;
    if key.is_empty() {
        return Err(format!("empty variable name in `{s}`"));
    }
    Ok((key.to_string(), val.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_key_val_splits_on_first_equals() {
        assert_eq!(
            parse_key_val("FOO=bar").unwrap(),
            ("FOO".to_string(), "bar".to_string())
        );
        // The value keeps later `=` (e.g. base64, query strings).
        assert_eq!(
            parse_key_val("URL=a=b=c").unwrap(),
            ("URL".to_string(), "a=b=c".to_string())
        );
        // An empty value is fine (unset-to-empty).
        assert_eq!(
            parse_key_val("EMPTY=").unwrap(),
            ("EMPTY".to_string(), String::new())
        );
    }

    #[test]
    fn parse_key_val_rejects_bad_input() {
        assert!(parse_key_val("NOEQUALS").is_err());
        assert!(parse_key_val("=novalue").is_err());
    }

    #[test]
    fn cli_parses_the_command_tree() {
        use clap::Parser;

        // run with repeated --env
        let app = App::parse_from(["marathon", "run", "book.md", "-e", "A=1", "--env", "B=2"]);
        match app.cmd {
            RootCmd::Run(c) => {
                assert_eq!(c.path.to_str(), Some("book.md"));
                assert_eq!(
                    c.common.env,
                    vec![
                        ("A".to_string(), "1".to_string()),
                        ("B".to_string(), "2".to_string())
                    ]
                );
            }
            _ => panic!("expected Run"),
        }

        // exec --yes
        match App::parse_from(["marathon", "exec", "book.md", "--yes"]).cmd {
            RootCmd::Exec(c) => assert!(c.yes),
            _ => panic!("expected Exec"),
        }

        // `check` is a visible alias for `validate`
        match App::parse_from(["marathon", "check", "book.md"]).cmd {
            RootCmd::Validate(c) => assert_eq!(c.path.to_str(), Some("book.md")),
            _ => panic!("expected Validate via `check` alias"),
        }
    }
}
