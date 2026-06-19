use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::util::{get_frontmatter_node, parse_markdown};

pub struct Runbook {
    pub path: Option<PathBuf>,
    pub frontmatter: BookFrontmatter,
    pub blocks: Vec<BookBlock>,
}

impl Runbook {
    pub fn new<P: AsRef<Path>>(path: Option<P>, doc: &str) -> Result<Self> {
        // Make that path a path
        let path = path.map(|p| p.as_ref().to_path_buf());

        // Parse the markdown ast
        let ast = parse_markdown(doc)?;

        // Parse the frontmatter
        let txt = get_frontmatter_node(&ast)?;
        let frontmatter: BookFrontmatter = serde_yaml::from_str(&txt)?;

        // Coerce the blocks
        let blocks = ast
            .children
            .iter()
            .map(|n| match n {
                markdown::mdast::Node::Code(c) => {
                    let b = CodeBlock::try_from(c.clone()).map_err(|err| anyhow!("{}", err))?;
                    Ok(BookBlock::Code(b))
                }
                _ => Ok(BookBlock::Md(n.clone())),
            })
            .collect::<Result<Vec<_>>>()?;

        // Done!
        Ok(Self {
            path,
            frontmatter,
            blocks,
        })
    }
}

#[derive(Debug)]
pub enum BookBlock {
    Code(CodeBlock),
    Md(markdown::mdast::Node),
}

/// Runnable markdown code block
#[derive(Debug)]
pub struct CodeBlock {
    pub lang: String,
    pub meta: CodeBlockMeta,
    pub content: String,
    pub state: CodeBlockState,
    // more state?
}

impl TryFrom<markdown::mdast::Code> for CodeBlock {
    type Error = String;

    fn try_from(val: markdown::mdast::Code) -> Result<Self, Self::Error> {
        // Parse the meta fields
        let meta: CodeBlockMeta = if let Some(meta) = val.meta {
            serde_kv::from_str(&meta)
                .map_err(|err| format!("failed to parse block meta: {}", err))?
        } else {
            CodeBlockMeta::default()
        };

        // Format and return
        Ok(Self {
            lang: val.lang.unwrap_or("sh".into()),
            content: val.value,
            meta,
            state: CodeBlockState::NotRun,
        })
    }
}

#[derive(Debug, Default)]
pub enum CodeBlockState {
    #[default]
    NotRun,
    Running,
    Success(String),
    Error(String),
}

/// The frontmatter from a runbook
///
/// Expected to be deserialized from yaml.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct BookFrontmatter {
    /// Configuration for code block interpreters
    pub interpreters: HashMap<String, InterpreterConf>,

    /// Code to inject at the start of each code block
    pub before_each: Option<String>,

    /// Code to inject at the end of each code block
    pub after_each: Option<String>,

    /// Environment variables to set for each code block
    pub env: HashMap<String, String>,

    /// Config options for temp dir to be shared across
    /// code block runs.
    ///
    /// Since code blocks are isolated, this can be a way
    /// to pass messages between steps.
    ///
    /// An environment variable `$TMP_DIR` will be injected
    pub tmp_dir: TmpDirConf,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct InterpreterConf {
    /// Path to the interpreter
    ///
    /// Defaults to `/usr/bin/env {lang}`
    ///
    /// Could also be used to run `sh` codeblocks
    /// with `zsh`, for example.
    pub path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct TmpDirConf {
    /// Explicitly set the path to the temporary
    /// directory to be used during the run.
    ///
    /// Defaults to a random dir (e.g. `/tmp/{{random_name}}`)
    ///
    /// If you explicitly set a temp dir you *may* not want
    /// it to be cleaned up afterwards (via `.skip_cleanup`).
    ///
    /// NOTE: This might want to take some config
    /// (e.g. prefix, suffix, etc.).
    pub path: Option<PathBuf>,

    /// If not `true`, the temp directory will be
    /// removed after the run is finished
    pub skip_cleanup: Option<bool>,

    /// Name of the environment variable pointing
    /// to the temp dir.
    ///
    /// Defaults to `TMP_DIR`.
    pub var_name: Option<String>,
}

/// The key/value data stored in a md code block
///
/// Expected to be deserialized from `serde-kv`
///
/// TODO: Maybe allow redirecting stdout/stderr
/// rather than just combining them.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct CodeBlockMeta {
    /// Special config field
    ///
    /// For now, just used with `lang=json`
    /// where `mrthn=input` so we know to
    /// deserialize into a `MagicInputBlock`.
    pub mrthn: Option<String>,

    /// Don't run the codeblock
    pub skip: Option<bool>,
}

/// Structure for *magic* json code blocks
/// to prompt the user for input.
///
/// TODO: Should these be split into their own
/// sub-structs?
///
/// NOTE: Future additions could include "edit"
/// (aka open a given file in a text editor,
/// like git commit) or "branch"/"goto" (for
/// logic that says "if X condition is met, do
/// Y, else Z"). These might require us adding
/// jinja templating to some parts (eg reference
/// `TMP_DIR` in the edito file) or add IDs to
/// cells (eg allow branch to reference a cell
/// to goto).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MagicInputBlock {
    /// Prompt the user with a yes/no option
    Confirm {
        /// Prompt to display for user
        prompt: String,

        /// Environment variable to store
        /// output for subsequent commands
        target: String,
    },

    /// Prompt the user for some input text
    Input {
        /// Prompt to display for user
        prompt: String,

        /// Environment variable to store
        /// output for subsequent commands
        target: String,
    },

    /// Prompt the user to select
    Select {
        /// Prompt to display for user
        prompt: String,

        /// Environment variable to store
        /// output for subsequent commands
        target: String,

        /// List of options from which the
        /// user can choose
        options: Option<Vec<String>>,

        /// Path to file whose lines will
        /// be used as options
        option_file: Option<String>,
    },
}
