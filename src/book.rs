use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::util::{get_frontmatter_node, parse_markdown};

/// An active runbook primitive.
pub struct Runbook {
    /// The path to the runbook
    pub path: Option<PathBuf>,

    /// Runbook frontmatter
    pub frontmatter: BookFrontmatter,

    /// Parsed blocks in the runbook
    pub blocks: Vec<BookBlock>,

    /// Index of the last run code block
    pub last_run: Option<usize>,

    /// Active temp directory (the resolved path; see [`Runbook::ensure_tmp_dir`]).
    pub tmp_dir: Option<PathBuf>,

    /// Keep-alive for an auto-created temp dir. Dropping it removes the directory,
    /// so it must live as long as the runbook (unless `skip_cleanup` persisted it).
    tmp_guard: Option<tempfile::TempDir>,

    /// The original document text. Retained so a markdown block can be copied back
    /// out as its exact source (via the `mdast` node's byte offsets).
    source: String,

    /// Extra environment overlaid on the frontmatter `env` for every cell — the merge
    /// point for CLI `--env` (and any future sources). Empty until something sets it;
    /// it overrides frontmatter keys and is surfaced in the header via [`base_env`].
    ///
    /// [`base_env`]: Runbook::base_env
    pub cli_env: HashMap<String, String>,
}

impl Runbook {
    pub fn new<P: AsRef<Path>>(path: Option<P>, doc: &str) -> Result<Self> {
        // Make that path a path
        let path = path.map(|p| p.as_ref().to_path_buf());

        // Parse the markdown ast
        let ast = parse_markdown(doc)?;

        // Parse the frontmatter
        let frontmatter: BookFrontmatter = match get_frontmatter_node(&ast) {
            Some(txt) if !txt.trim().is_empty() => serde_yaml::from_str(&txt).map_err(|err| {
                let (line, column) = err
                    .location()
                    .map(|l| (l.line() + 1, l.column()))
                    .unwrap_or((1, 1));
                anyhow!(
                    "{}:{line}:{column}: frontmatter: {err}",
                    source_name(path.as_deref())
                )
            })?,
            _ => BookFrontmatter::default(),
        };
        frontmatter
            .validate()
            .with_context(|| format!("{}:1:1: frontmatter", source_name(path.as_deref())))?;

        // Coerce the blocks. Frontmatter (YAML/TOML) is config, not content, so
        // it isn't a navigable/rendered block.
        let mut cell_number = 0;
        let blocks = ast
            .children
            .iter()
            .filter(|n| {
                !matches!(
                    n,
                    markdown::mdast::Node::Yaml(_) | markdown::mdast::Node::Toml(_)
                )
            })
            .map(|n| match n {
                markdown::mdast::Node::Code(c) => {
                    cell_number += 1;
                    let (line, column) = c
                        .position
                        .as_ref()
                        .map(|p| (p.start.line, p.start.column))
                        .unwrap_or((1, 1));
                    let location = format!(
                        "{}:{line}:{column}: cell {cell_number}",
                        source_name(path.as_deref())
                    );
                    // Parse the code block
                    let b = CodeBlock::try_from(c.clone())
                        .map_err(|err| anyhow!("{location}: {err}"))?;

                    // Is it an input block?
                    if b.lang == "json"
                        && b.meta.mrthn.as_ref().map(|s| s == "input").unwrap_or(false)
                    {
                        let mib: MagicInputBlock = serde_json::from_str(&b.content)
                            .with_context(|| format!("{location}: input JSON"))?;
                        mib.validate()
                            .with_context(|| format!("{location}: input '{}'", mib.target()))?;
                        let mut cell = InputCell::new(mib);
                        cell.location = Some(location);
                        cell.meta = b.meta;
                        return Ok(BookBlock::Input(cell));
                    }

                    // Otherwise just runnable code
                    Ok(BookBlock::Code(b))
                }
                _ => Ok(BookBlock::Md(n.clone())),
            })
            .collect::<Result<Vec<_>>>()?;

        // Done!
        let book = Self {
            path,
            frontmatter,
            blocks,
            last_run: None,
            tmp_dir: None,
            tmp_guard: None,
            source: doc.to_string(),
            cli_env: HashMap::new(),
        };
        book.validate_execution()?;
        Ok(book)
    }

    /// Mutable access to the input cell at `idx`, if that block is one.
    pub fn input_at_mut(&mut self, idx: usize) -> Option<&mut InputCell> {
        match self.blocks.get_mut(idx) {
            Some(BookBlock::Input(cell)) => Some(cell),
            _ => None,
        }
    }

    /// Begin editing the input cell at `idx`, if that block is one. Builds the
    /// scratch directory and full environment first so a select cell's `option_file`
    /// path can reference `TMP_DIR` or an earlier answer.
    pub fn begin_edit_at(&mut self, idx: usize) -> Result<()> {
        if !matches!(self.blocks.get(idx), Some(BookBlock::Input(_))) {
            return Ok(());
        }
        self.ensure_tmp_dir()
            .context("creating temp dir for input")?;
        let env = self.input_env_for(idx);
        if let Some(BookBlock::Input(cell)) = self.blocks.get_mut(idx) {
            cell.begin_edit(&env);
        }
        Ok(())
    }

    /// The active temp directory as an env-var `(name, path)` pair, if one has been
    /// created yet. Made lazily on first run or input edit, so `None` until then.
    /// Used by the header to surface the path for the user.
    pub fn tmp_dir_env(&self) -> Option<(String, String)> {
        self.tmp_dir
            .as_ref()
            .map(|p| (self.tmp_dir_var_name(), p.display().to_string()))
    }

    /// Name of the env var pointing at the temp dir (`TMP_DIR` by default).
    fn tmp_dir_var_name(&self) -> String {
        self.frontmatter
            .tmp_dir
            .as_ref()
            .and_then(|c| c.var_name.clone())
            .unwrap_or_else(|| "TMP_DIR".to_string())
    }

    /// Resolve the shared temp directory, creating it on first use (DESIGN §2).
    /// An explicit frontmatter `tmp_dir.path` is created as-is; otherwise a fresh
    /// `mktemp`-style dir is made and (unless `skip_cleanup`) removed on drop.
    pub fn ensure_tmp_dir(&mut self) -> Result<PathBuf> {
        if let Some(p) = &self.tmp_dir {
            return Ok(p.clone());
        }

        let explicit = self
            .frontmatter
            .tmp_dir
            .as_ref()
            .and_then(|c| c.path.clone());

        let path = if let Some(p) = explicit {
            std::fs::create_dir_all(&p)?;
            p
        } else {
            let td = tempfile::TempDir::new()?;
            let skip = self
                .frontmatter
                .tmp_dir
                .as_ref()
                .and_then(|c| c.skip_cleanup)
                .unwrap_or(false);
            if skip {
                // Persist: leak the guard so the directory survives the run.
                td.keep()
            } else {
                let p = td.path().to_path_buf();
                self.tmp_guard = Some(td);
                p
            }
        };

        self.tmp_dir = Some(path.clone());
        Ok(path)
    }

    /// The interpreter argv for a language, e.g. `["/usr/bin/env", "sh"]`. A
    /// frontmatter `interpreters.<lang>.path` overrides the default (shebang-style
    /// remap, so `sh` can be run with `zsh`).
    pub fn interpreter_for(&self, lang: &str) -> Vec<String> {
        if let Some(conf) = self
            .frontmatter
            .interpreters
            .as_ref()
            .and_then(|m| m.get(lang))
            && let Some(path) = &conf.path
        {
            let parts: Vec<String> = path.split_whitespace().map(String::from).collect();
            if !parts.is_empty() {
                return parts;
            }
        }
        vec!["/usr/bin/env".to_string(), lang.to_string()]
    }

    /// The full script for a cell: frontmatter `before_each`, the cell body, then
    /// `after_each`, joined with newlines.
    ///
    /// When `before_each` is omitted it defaults to `set -eu` (errexit + nounset), so
    /// a failing command or an unset variable fails the cell loudly instead of
    /// limping on. An explicit `before_each: ""` opts out; any custom value replaces
    /// the default. `pipefail` is intentionally *not* in the default — it isn't POSIX
    /// `sh`, so it would break non-bash shells. Stream merging (`exec 2>&1`) is a
    /// separate, always-on concern handled by the runner, not this default.
    pub fn script_for(&self, c: &CodeBlock) -> String {
        let mut s = String::new();
        let before = self.frontmatter.before_each.as_deref().unwrap_or("set -eu");
        if !before.is_empty() {
            s.push_str(before);
            if !before.ends_with('\n') {
                s.push('\n');
            }
        }
        s.push_str(&c.content);
        if let Some(a) = &self.frontmatter.after_each {
            if !s.is_empty() && !s.ends_with('\n') {
                s.push('\n');
            }
            s.push_str(a);
        }
        s
    }

    /// Build the environment map injected into the cell at `idx` (DESIGN §2):
    /// frontmatter `env`, then CLI `--env`, then `TMP_DIR`, then every *preceding*
    /// answered input cell's `target=value` in document order (later layers win).
    pub fn env_for(&self, idx: usize) -> HashMap<String, String> {
        let mut map = HashMap::new();

        if let Some(env) = &self.frontmatter.env {
            map.extend(env.clone());
        }
        map.extend(self.cli_env.clone());
        if let Some(tmp) = &self.tmp_dir {
            map.insert(self.tmp_dir_var_name(), tmp.display().to_string());
        }
        for block in self.blocks.iter().take(idx) {
            if let BookBlock::Input(cell) = block
                && let Some((target, value)) = cell.resolved()
            {
                map.insert(target.to_string(), value.to_string());
            }
        }

        map
    }

    /// The full environment for input values and option-file expansion, including
    /// inherited variables with the same precedence used by command execution.
    pub fn input_env_for(&self, idx: usize) -> HashMap<String, String> {
        let mut env: HashMap<_, _> = std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
            .collect();
        env.extend(self.env_for(idx));
        env
    }

    /// The base environment shown in the header: frontmatter `env` overlaid with CLI
    /// `--env`, sorted by key. Per-cell additions — `TMP_DIR`, answered inputs — are
    /// layered on only at run time by [`env_for`], so they're deliberately excluded.
    pub fn base_env(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        if let Some(env) = &self.frontmatter.env {
            map.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        map.extend(self.cli_env.iter().map(|(k, v)| (k.clone(), v.clone())));
        map
    }

    /// Tally code-cell run states for the footer's aggregate badge + progress.
    /// One pass over the blocks; only [`CodeBlock`]s contribute.
    pub fn run_counts(&self) -> RunCounts {
        let mut c = RunCounts::default();
        for block in &self.blocks {
            if let BookBlock::Code(cb) = block {
                if cb.is_runnable() {
                    c.runnable += 1;
                }
                match cb.state {
                    CodeBlockState::Running => c.running += 1,
                    CodeBlockState::Success => c.succeeded += 1,
                    CodeBlockState::Error => c.errored += 1,
                    CodeBlockState::NotRun => {}
                }
            }
        }
        c
    }

    /// The text to copy for the block at `idx`, by kind:
    /// - **code** → the raw cell body (not the fenced ```` ``` ```` block);
    /// - **markdown** → its exact source, sliced from the original document via the
    ///   `mdast` node's byte offsets;
    /// - **input** → not copyable (`None`).
    ///
    /// `None` if the index is out of range, the block is an input cell, or a markdown
    /// node lacks position info.
    pub fn copy_text(&self, idx: usize) -> Option<String> {
        match self.blocks.get(idx)? {
            BookBlock::Code(c) => Some(c.content.clone()),
            BookBlock::Input(_) => None,
            BookBlock::Md(node) => {
                let pos = node.position()?;
                self.source
                    .get(pos.start.offset..pos.end.offset)
                    .map(str::to_string)
            }
        }
    }

    /// The cleaned stdout+stderr of the code cell at `idx`, read on demand for copying.
    /// `None` if the index is out of range, the block isn't a code cell, or the cell
    /// has produced no output yet (nothing to copy).
    pub fn output_text(&self, idx: usize) -> std::io::Result<Option<String>> {
        match self.blocks.get(idx) {
            Some(BookBlock::Code(c)) => c
                .output
                .text()
                .map(|text| (!text.is_empty()).then_some(text)),
            _ => Ok(None),
        }
    }

    /// Reset every cell to its initial state: code outputs cleared and un-run, input
    /// answers discarded. Prose is untouched. Also forgets `last_run` and discards the
    /// auto-created temp directory (see [`Runbook::reset_tmp_dir`]) so the next run
    /// starts fresh.
    pub fn clear_all(&mut self) {
        for block in &mut self.blocks {
            match block {
                BookBlock::Code(c) => c.clear(),
                BookBlock::Input(i) => i.clear(),
                BookBlock::Md(_) => {}
            }
        }
        self.last_run = None;
        self.reset_tmp_dir();
    }

    /// Discard the auto-created temp directory so the next [`ensure_tmp_dir`] mints a
    /// fresh one. Dropping `tmp_guard` removes the old directory from disk; clearing
    /// `tmp_dir` forces re-creation on next use.
    ///
    /// A user-configured `tmp_dir.path` or a `skip_cleanup` directory has no guard and
    /// is deliberately left untouched — it is the user's directory to manage, not ours
    /// to delete out from under them.
    ///
    /// [`ensure_tmp_dir`]: Runbook::ensure_tmp_dir
    pub fn reset_tmp_dir(&mut self) {
        if self.tmp_guard.is_some() {
            self.tmp_guard = None; // Drop removes the old directory.
            self.tmp_dir = None; // Next ensure_tmp_dir creates a fresh one.
        }
    }
}

fn source_name(path: Option<&Path>) -> String {
    path.map(|p| p.display().to_string())
        .unwrap_or_else(|| "<runbook>".into())
}

/// Names exported by runbooks must be usable as shell variables.
pub(crate) fn validate_env_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    if !chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        bail!("invalid environment target {name:?}; use [A-Za-z_][A-Za-z0-9_]*");
    }
    Ok(())
}

fn validate_env_value(value: &str) -> Result<()> {
    if value.contains('\0') {
        bail!("environment values cannot contain NUL characters");
    }
    Ok(())
}

/// Aggregate run state across all code cells, derived fresh each draw from the
/// blocks' current [`CodeBlockState`]s (re-running a cell flips its state, so these
/// reflect *now*, not history). Drives the footer badge and `N/M` progress.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunCounts {
    /// Cells currently executing.
    pub running: usize,
    /// Cells whose last run exited 0.
    pub succeeded: usize,
    /// Cells whose last run failed.
    pub errored: usize,
    /// Total cells marathon would execute (recognized shell, not `skip`).
    pub runnable: usize,
}

impl RunCounts {
    /// Cells that have finished a run (succeeded or errored) — the `N` in `N/M`.
    pub fn finished(&self) -> usize {
        self.succeeded + self.errored
    }
}

#[derive(Debug)]
pub enum BookBlock {
    Code(CodeBlock),
    Input(InputCell),
    Md(markdown::mdast::Node),
}

/// Runnable markdown code block
#[derive(Debug)]
pub struct CodeBlock {
    pub lang: String,
    pub meta: CodeBlockMeta,
    pub content: String,
    /// Lifecycle status of the cell (idle / running / ok / error).
    pub state: CodeBlockState,
    /// Disk-backed raw and cleaned output, owned by this run.
    pub output: crate::output::OutputCapture,
    /// Persistent Marathon diagnostic, separate from command output.
    pub error: Option<String>,
    /// Expanded output page; None follows the latest page.
    pub output_page: Option<u64>,
    /// When the current run began. Set in [`CodeBlock::begin_run`], used to compute
    /// [`CodeBlock::elapsed`] on finish. A live ticking timer is the footer's job
    /// (it redraws every frame); this only yields the final duration.
    pub started_at: Option<std::time::Instant>,
    /// Wall-clock duration of the last finished run, shown on the status line.
    pub elapsed: Option<std::time::Duration>,
    /// Exit code of the last finished run, if the process exited normally (`None`
    /// if killed by a signal). Surfaced on the status line only when non-zero.
    pub exit_code: Option<i32>,
    /// How far the user has escalated a cancellation of this run. While `Running` it
    /// reads as "canceling…/killing…"; once finished it labels the outcome
    /// "canceled/killed" instead of a plain error. Reset by [`CodeBlock::begin_run`].
    pub cancel: Cancel,
}

/// A cell's cancellation phase — the strongest stop signal sent to its run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Cancel {
    /// No stop requested.
    #[default]
    None,
    /// SIGINT sent, awaiting exit (a graceful cancel).
    Interrupting,
    /// SIGKILL sent, awaiting exit (escalated after a cancel didn't take).
    Killing,
}

impl CodeBlock {
    /// Whether marathon will execute this cell: a recognized shell language and
    /// not opted out via `skip=true`. Unknown languages are display-only (MVP).
    pub fn is_runnable(&self) -> bool {
        !self.meta.skip.unwrap_or(false) && matches!(self.lang.as_str(), "sh" | "bash" | "zsh")
    }

    /// Reset for a fresh run: clear prior output, start the clock, mark it running.
    pub fn begin_run(&mut self) {
        self.output = crate::output::OutputCapture::default();
        self.error = None;
        self.output_page = None;
        self.started_at = Some(std::time::Instant::now());
        self.elapsed = None;
        self.exit_code = None;
        self.cancel = Cancel::None;
        self.state = CodeBlockState::Running;
    }

    /// Append a streamed output chunk.
    pub fn push_output(&mut self, chunk: impl AsRef<[u8]>) -> std::io::Result<()> {
        self.output.append(chunk.as_ref())
    }

    /// Mark the run finished, recording how long it ran and its exit code.
    pub fn finish(&mut self, success: bool, code: Option<i32>) {
        self.elapsed = self.started_at.map(|s| s.elapsed());
        self.exit_code = code;
        self.state = if success {
            CodeBlockState::Success
        } else {
            CodeBlockState::Error
        };
    }

    /// Discard any prior run: clear captured output and return to the un-run state.
    pub fn clear(&mut self) {
        self.output = crate::output::OutputCapture::default();
        self.error = None;
        self.output_page = None;
        self.started_at = None;
        self.elapsed = None;
        self.exit_code = None;
        self.cancel = Cancel::None;
        self.state = CodeBlockState::NotRun;
    }
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
            lang: val.lang.unwrap_or_default(),
            content: val.value,
            meta,
            state: CodeBlockState::NotRun,
            output: crate::output::OutputCapture::default(),
            error: None,
            output_page: None,
            started_at: None,
            elapsed: None,
            exit_code: None,
            cancel: Cancel::None,
        })
    }
}

/// Lifecycle status of a runnable cell. The captured output lives separately on
/// [`CodeBlock::output`], so a streamed chunk never has to reconstruct this.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CodeBlockState {
    #[default]
    NotRun,
    Running,
    Success,
    Error,
}

/// The frontmatter from a runbook
///
/// Expected to be deserialized from yaml.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct BookFrontmatter {
    /// Human-readable runbook title (shown in the header).
    pub title: Option<String>,

    /// One-line description of the runbook (shown in the header).
    pub description: Option<String>,

    /// Configuration for code block interpreters
    pub interpreters: Option<HashMap<String, InterpreterConf>>,

    /// Code to inject at the start of each code block
    pub before_each: Option<String>,

    /// Code to inject at the end of each code block
    pub after_each: Option<String>,

    /// Environment variables to set for each code block
    pub env: Option<HashMap<String, String>>,

    /// Config options for temp dir to be shared across
    /// code block runs.
    ///
    /// Since code blocks are isolated, this can be a way
    /// to pass messages between steps.
    ///
    /// An environment variable `$TMP_DIR` will be injected
    pub tmp_dir: Option<TmpDirConf>,
}

impl BookFrontmatter {
    fn validate(&self) -> Result<()> {
        if let Some(env) = &self.env {
            for (key, value) in env {
                validate_env_name(key).with_context(|| format!("env key {key:?}"))?;
                validate_env_value(value).with_context(|| format!("env.{key}"))?;
            }
        }
        if let Some(name) = self.tmp_dir.as_ref().and_then(|c| c.var_name.as_deref()) {
            validate_env_name(name).context("tmp_dir.var_name")?;
        }
        Ok(())
    }
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
    /// Stable reference for a runnable or input cell.
    pub id: Option<String>,

    /// Comma-separated references to earlier prerequisite cells.
    pub needs: Option<String>,

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

        /// Explicit unattended default. A confirmation exports `yes` or `no`.
        default: Option<bool>,
    },

    /// Prompt the user for some input text
    Input {
        /// Prompt to display for user
        prompt: String,

        /// Environment variable to store
        /// output for subsequent commands
        target: String,

        /// Explicit default used by `exec --yes` and to seed interactive editing.
        default: Option<String>,
    },

    /// Prompt the user to select
    Select {
        /// Prompt to display for user
        prompt: String,

        /// Environment variable to store
        /// output for subsequent commands
        target: String,

        /// The default option's value, not its numeric index.
        default: Option<String>,

        /// List of options from which the
        /// user can choose
        options: Option<Vec<String>>,

        /// Path to file whose lines will
        /// be used as options
        option_file: Option<String>,
    },
}

impl MagicInputBlock {
    /// Static checks only: never open an option file while parsing a runbook.
    fn validate(&self) -> Result<()> {
        validate_env_name(self.target())?;
        if let Some(default) = self.default_value() {
            validate_env_value(&default).context("invalid default")?;
        }
        if let Self::Select {
            options,
            option_file,
            default,
            ..
        } = self
        {
            let options = options.as_deref().unwrap_or_default();
            for value in options {
                validate_env_value(value).context("invalid option")?;
            }
            if let Some(path) = option_file {
                if path.is_empty() || path.contains('\0') {
                    bail!("option_file must be a nonempty path without NUL characters");
                }
            } else {
                if options.is_empty() {
                    bail!("no options available; provide nonempty options or an option_file");
                }
                if default
                    .as_ref()
                    .is_some_and(|value| !options.contains(value))
                {
                    bail!("invalid default: value is not one of the available options");
                }
            }
        }
        Ok(())
    }

    pub fn default_value(&self) -> Option<String> {
        match self {
            Self::Confirm { default, .. } => default.map(|v| if v { "yes" } else { "no" }.into()),
            Self::Input { default, .. } | Self::Select { default, .. } => default.clone(),
        }
    }

    pub fn prompt(&self) -> &str {
        match self {
            Self::Confirm { prompt, .. }
            | Self::Input { prompt, .. }
            | Self::Select { prompt, .. } => prompt,
        }
    }

    pub fn target(&self) -> &str {
        match self {
            Self::Confirm { target, .. }
            | Self::Input { target, .. }
            | Self::Select { target, .. } => target,
        }
    }

    /// Short label for the cell kind, e.g. `confirm`/`input`/`select`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Confirm { .. } => "confirm",
            Self::Input { .. } => "input",
            Self::Select { .. } => "select",
        }
    }
}

/// A navigable input cell: the parsed [`MagicInputBlock`] config plus the live
/// interaction state. Config is immutable for the session; `state` advances as
/// the user activates, edits, and answers the cell.
///
/// Per the architecture notes, the *answered value* is model state (it belongs to
/// the document and feeds later cells), while activation/draft is transient edit
/// state that lives here only while the cell is focused.
#[derive(Debug)]
pub struct InputCell {
    pub meta: CodeBlockMeta,
    pub config: MagicInputBlock,
    pub state: InputState,
    /// Resolved select options: the inline `options` list followed by any lines
    /// read from `option_file`. Recomputed on `begin_edit` (and seeded at
    /// construction with inline choices only) so a file produced by a preceding
    /// cell at runtime is picked up. Always empty for non-select cells.
    loaded_options: Vec<String>,
    /// A failed/incomplete refresh blocks submission until editing is reopened.
    options_error: Option<String>,
    /// Last error shown inline; invalid drafts stay editable.
    error: Option<String>,
    location: Option<String>,
}

/// Where an input cell is in its lifecycle.
#[derive(Debug, Clone, Default)]
pub enum InputState {
    /// Not yet answered and not currently focused.
    #[default]
    Pending,
    /// Focused and being edited. `prior` remembers a previous answer (if any) so
    /// a cancelled re-edit can restore it.
    Editing { draft: Draft, prior: Option<String> },
    /// Answered; `value` is what gets written to the cell's target env var.
    Answered { value: String },
}

/// The in-progress edit value, shaped by the cell kind.
#[derive(Debug, Clone)]
pub enum Draft {
    /// Yes (`true`) / No (`false`) toggle.
    Confirm(bool),
    /// Free text with a cursor.
    Text(TextDraft),
    /// Highlighted option index into the cell's options.
    Select(Option<usize>),
}

/// A single-line text buffer with a char-indexed cursor.
#[derive(Debug, Clone, Default)]
pub struct TextDraft {
    pub value: String,
    /// Cursor position as a *character* index (0..=char count).
    pub cursor: usize,
}

impl TextDraft {
    pub(crate) fn seeded(value: String) -> Self {
        let cursor = value.chars().count();
        Self { value, cursor }
    }

    /// Byte offset of char index `idx` (clamped to the end).
    pub(crate) fn byte_at(&self, idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(idx)
            .map(|(b, _)| b)
            .unwrap_or(self.value.len())
    }

    fn char_count(&self) -> usize {
        self.value.chars().count()
    }

    pub(crate) fn insert(&mut self, c: char) {
        let at = self.byte_at(self.cursor);
        self.value.insert(at, c);
        self.cursor += 1;
    }

    pub(crate) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_at(self.cursor - 1);
        let end = self.byte_at(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    pub(crate) fn delete(&mut self) {
        if self.cursor >= self.char_count() {
            return;
        }
        let start = self.byte_at(self.cursor);
        let end = self.byte_at(self.cursor + 1);
        self.value.replace_range(start..end, "");
    }

    pub(crate) fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub(crate) fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_count());
    }

    pub(crate) fn home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn end(&mut self) {
        self.cursor = self.char_count();
    }
}

/// Expand `$NAME` / `${NAME}` references in `input` against `env`, where a name
/// is `[A-Za-z_][A-Za-z0-9_]*`. This is a *lookup only* — deliberately not a
/// shell: there is no command substitution, no `${VAR:-default}`, no arithmetic,
/// and no tilde expansion. A `$` that doesn't begin a valid reference, or one
/// naming an unknown variable, is left untouched; `$$` is a literal `$`.
fn expand_env(input: &str, env: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            // `$$` -> literal `$`
            Some('$') => {
                chars.next();
                out.push('$');
            }
            // `${NAME}`
            Some('{') => {
                chars.next();
                let mut name = String::new();
                let mut closed = false;
                for ch in chars.by_ref() {
                    if ch == '}' {
                        closed = true;
                        break;
                    }
                    name.push(ch);
                }
                match env.get(&name) {
                    Some(v) if closed => out.push_str(v),
                    // Unknown or unterminated: leave the original text in place.
                    _ => {
                        out.push_str("${");
                        out.push_str(&name);
                        if closed {
                            out.push('}');
                        }
                    }
                }
            }
            // `$NAME`
            Some(ch) if ch.is_ascii_alphabetic() || ch == '_' => {
                let mut name = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch.is_ascii_alphanumeric() || ch == '_' {
                        name.push(ch);
                        chars.next();
                    } else {
                        break;
                    }
                }
                match env.get(&name) {
                    Some(v) => out.push_str(v),
                    None => {
                        out.push('$');
                        out.push_str(&name);
                    }
                }
            }
            // Trailing or loose `$`.
            _ => out.push('$'),
        }
    }
    out
}

impl InputCell {
    pub fn new(config: MagicInputBlock) -> Self {
        let (loaded_options, options_error) = match &config {
            MagicInputBlock::Select {
                options,
                option_file,
                ..
            } => (
                options.clone().unwrap_or_default(),
                option_file
                    .as_ref()
                    .map(|_| "options have not been loaded; reopen this input to load them".into()),
            ),
            _ => (Vec::new(), None),
        };
        Self {
            meta: CodeBlockMeta::default(),
            config,
            state: InputState::Pending,
            loaded_options,
            options_error,
            error: None,
            location: None,
        }
    }

    pub fn prompt(&self) -> &str {
        self.config.prompt()
    }

    pub fn target(&self) -> &str {
        self.config.target()
    }

    pub fn kind(&self) -> &'static str {
        self.config.kind()
    }

    /// The select options (empty for non-select cells, or if none configured).
    pub fn options(&self) -> &[String] {
        &self.loaded_options
    }

    /// Recompute the cached select options from the inline `options` list plus
    /// any lines in `option_file`. The path has `$NAME`/`${NAME}` expanded
    /// against `env` (see [`expand_env`]), so it can reference `TMP_DIR` or an
    /// earlier answer. Read lazily so a file produced by a preceding cell at
    /// runtime is picked up; blank lines are skipped and surrounding whitespace
    /// trimmed. A failed read discards all cached choices and blocks submission.
    pub fn try_refresh_options(&mut self, env: &HashMap<String, String>) -> Result<()> {
        self.loaded_options.clear();
        let result = self.load_options(env);
        self.options_error = result.as_ref().err().map(|e| format!("{e:#}"));
        self.error = self
            .options_error
            .as_ref()
            .map(|e| self.input_error(e).to_string());
        result.map_err(|e| self.input_error(format!("{e:#}")))
    }

    fn load_options(&mut self, env: &HashMap<String, String>) -> Result<()> {
        let MagicInputBlock::Select {
            options,
            option_file,
            ..
        } = &self.config
        else {
            return Ok(());
        };
        let mut loaded = options.clone().unwrap_or_default();
        if let Some(raw) = option_file {
            let path = expand_env(raw, env);
            let text = std::fs::read_to_string(&path)
                .map_err(|e| anyhow!("reading option file {path:?}: {e}; fix the file or run its generating cell, then reopen this input"))?;
            loaded.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned),
            );
        }
        if loaded.is_empty() {
            bail!(
                "no options available; add choices to options or option_file, then reopen this input"
            );
        }
        for value in &loaded {
            validate_env_value(value).context("invalid option")?;
        }
        self.loaded_options = loaded;
        Ok(())
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(crate) fn input_error(&self, message: impl std::fmt::Display) -> anyhow::Error {
        let location = self
            .location
            .as_ref()
            .map(|l| format!("{l}: "))
            .unwrap_or_default();
        anyhow!("{location}input '{}': {message}", self.target())
    }

    /// Shared validation boundary for CLI answers and TUI drafts. A failed
    /// answer never replaces the current state.
    pub fn answer(&mut self, value: String) -> Result<()> {
        let result = self
            .validate_answer(value)
            .map_err(|e| self.input_error(format!("{e:#}")));
        self.error = result.as_ref().err().map(ToString::to_string);
        let value = result?;
        self.state = InputState::Answered { value };
        Ok(())
    }

    fn validate_answer(&self, value: String) -> Result<String> {
        validate_env_name(self.target())?;
        validate_env_value(&value)?;
        if let Some(error) = &self.options_error {
            bail!("{error}");
        }
        let value = match &self.config {
            MagicInputBlock::Confirm { .. } => match value.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" | "true" => "yes".to_owned(),
                "n" | "no" | "false" => "no".to_owned(),
                _ => bail!("expected yes or no"),
            },
            MagicInputBlock::Select { .. } if self.options().is_empty() => {
                bail!("no options available; add choices and reopen this input");
            }
            MagicInputBlock::Select { .. } if !self.options().contains(&value) => {
                bail!("value is not one of the available options; choose an available option");
            }
            _ => value,
        };
        Ok(value)
    }

    pub(crate) fn validated_default(&self) -> Result<Option<String>> {
        self.config
            .default_value()
            .map(|value| {
                self.validate_answer(value)
                    .map_err(|e| self.input_error(format!("invalid default: {e:#}")))
            })
            .transpose()
    }

    /// True if the cell is currently focused for editing.
    pub fn is_editing(&self) -> bool {
        matches!(self.state, InputState::Editing { .. })
    }

    /// The resolved `(target, value)` once answered — the seam later wired into
    /// the env map. `None` until the cell has been answered.
    pub fn resolved(&self) -> Option<(&str, &str)> {
        match &self.state {
            InputState::Answered { value } => Some((self.target(), value)),
            _ => None,
        }
    }

    /// Begin editing, seeding a draft from any prior answer or sensible default.
    /// `env` is the cell's environment (from [`Runbook::input_env_for`]), used to
    /// expand a select cell's `option_file` path.
    pub fn begin_edit(&mut self, env: &HashMap<String, String>) {
        // Re-read `option_file` in case a preceding cell just produced it.
        // Refresh errors are retained on the cell for display and submission.
        let _ = self.try_refresh_options(env);
        let prior = match &self.state {
            InputState::Answered { value } => Some(value.clone()),
            _ => None,
        };
        let seed = prior.clone().or_else(|| self.config.default_value());
        if self.error.is_none()
            && let Some(value) = &seed
            && let Err(error) = self.validate_answer(value.clone())
        {
            let label = if prior.is_some() {
                "previous answer"
            } else {
                "default"
            };
            self.error = Some(
                self.input_error(format!(
                    "invalid {label}: {error:#}; edit the answer before submitting"
                ))
                .to_string(),
            );
        }
        let draft = match &self.config {
            MagicInputBlock::Confirm { .. } => Draft::Confirm(seed.as_deref() == Some("yes")),
            MagicInputBlock::Input { .. } => {
                Draft::Text(TextDraft::seeded(seed.clone().unwrap_or_default()))
            }
            MagicInputBlock::Select { .. } => {
                let idx = match seed.as_deref() {
                    Some(value) => self.options().iter().position(|o| o == value),
                    None => (!self.options().is_empty()).then_some(0),
                };
                Draft::Select(idx)
            }
        };
        self.state = InputState::Editing { draft, prior };
    }

    /// Commit the current draft as the answer. No-op if not editing.
    pub fn submit(&mut self) -> Result<()> {
        let value = match &self.state {
            InputState::Editing { draft, .. } => match draft {
                Draft::Confirm(b) => Some(if *b { "yes" } else { "no" }.to_string()),
                Draft::Text(t) => Some(t.value.clone()),
                Draft::Select(i) => match i.and_then(|idx| self.options().get(idx)).cloned() {
                    Some(value) => Some(value),
                    None => {
                        let message =
                            self.options_error
                                .as_deref()
                                .unwrap_or(if self.options().is_empty() {
                                    "no options available; add choices and reopen this input"
                                } else {
                                    "no option selected; use Up/Down to choose an available option"
                                });
                        let error = self.input_error(message);
                        self.error = Some(error.to_string());
                        return Err(error);
                    }
                },
            },
            _ => None,
        };
        if let Some(value) = value {
            self.answer(value)?;
        }
        Ok(())
    }

    /// Discard any answer (or in-progress edit) and return to pending.
    pub fn clear(&mut self) {
        self.state = InputState::Pending;
        self.error = None;
    }

    /// Cancel editing, restoring a prior answer if still valid after the refresh.
    pub fn cancel(&mut self) {
        if let InputState::Editing { prior, .. } = &self.state {
            let prior = prior.clone();
            self.state = InputState::Pending;
            self.error = None;
            if let Some(value) = prior {
                // Changed options must not resurrect an answer that is no longer valid.
                let _ = self.answer(value);
            }
        }
    }

    fn draft_mut(&mut self) -> Option<&mut Draft> {
        match &mut self.state {
            InputState::Editing { draft, .. } => Some(draft),
            _ => None,
        }
    }

    // --- confirm ---

    pub fn toggle_confirm(&mut self) {
        if let Some(Draft::Confirm(b)) = self.draft_mut() {
            *b = !*b;
        }
    }

    pub fn set_confirm(&mut self, yes: bool) {
        if let Some(Draft::Confirm(b)) = self.draft_mut() {
            *b = yes;
        }
    }

    // --- select ---

    pub fn select_move(&mut self, forward: bool) {
        let n = self.options().len();
        if n == 0 {
            return;
        }
        if let Some(Draft::Select(i)) = self.draft_mut() {
            *i = Some(match *i {
                Some(idx) if forward => idx.saturating_add(1).min(n - 1),
                Some(idx) => idx.saturating_sub(1).min(n - 1),
                None => 0,
            });
            self.error = None;
        }
    }

    // --- text ---

    fn text_mut(&mut self) -> Option<&mut TextDraft> {
        match self.draft_mut() {
            Some(Draft::Text(t)) => Some(t),
            _ => None,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        if let Some(t) = self.text_mut() {
            t.insert(c);
        }
    }

    pub fn backspace(&mut self) {
        if let Some(t) = self.text_mut() {
            t.backspace();
        }
    }

    pub fn delete(&mut self) {
        if let Some(t) = self.text_mut() {
            t.delete();
        }
    }

    pub fn cursor_left(&mut self) {
        if let Some(t) = self.text_mut() {
            t.left();
        }
    }

    pub fn cursor_right(&mut self) {
        if let Some(t) = self.text_mut() {
            t.right();
        }
    }

    pub fn cursor_home(&mut self) {
        if let Some(t) = self.text_mut() {
            t.home();
        }
    }

    pub fn cursor_end(&mut self) {
        if let Some(t) = self.text_mut() {
            t.end();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirm() -> InputCell {
        InputCell::new(MagicInputBlock::Confirm {
            prompt: "Proceed?".into(),
            target: "OK".into(),
            default: None,
        })
    }

    fn text() -> InputCell {
        InputCell::new(MagicInputBlock::Input {
            prompt: "Name?".into(),
            target: "NAME".into(),
            default: None,
        })
    }

    fn select() -> InputCell {
        InputCell::new(MagicInputBlock::Select {
            prompt: "Pick".into(),
            target: "CHOICE".into(),
            default: None,
            options: Some(vec!["a".into(), "b".into(), "c".into()]),
            option_file: None,
        })
    }

    fn file_select(path: &Path) -> InputCell {
        InputCell::new(MagicInputBlock::Select {
            prompt: "Pick".into(),
            target: "CHOICE".into(),
            default: Some("inline".into()),
            options: Some(vec!["inline".into()]),
            option_file: Some(path.display().to_string()),
        })
    }

    #[test]
    fn failed_option_reads_block_cli_and_tui_even_with_inline_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let invalid_utf8 = dir.path().join("invalid-utf8");
        std::fs::write(&invalid_utf8, [0xff]).unwrap();
        for path in [
            dir.path().join("missing"),
            dir.path().to_owned(),
            invalid_utf8,
        ] {
            let mut cell = file_select(&path);
            assert!(
                cell.answer("inline".into()).is_err(),
                "file must be loaded first"
            );
            let cli_error = cell
                .try_refresh_options(&HashMap::new())
                .unwrap_err()
                .to_string();
            assert!(cli_error.contains("reading option file"));
            assert!(cli_error.contains(&path.display().to_string()));
            assert!(cell.answer("inline".into()).is_err());
            cell.begin_edit(&HashMap::new());
            assert_eq!(cell.error(), Some(cli_error.as_str()));
            assert!(cell.submit().is_err());
            assert!(cell.is_editing());
            assert!(cell.resolved().is_none());
            assert!(cell.options().is_empty());
        }
    }

    #[test]
    fn changed_option_files_do_not_reuse_stale_choices_or_prior_answers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("options");
        std::fs::write(&path, "old\n").unwrap();
        let mut cell = file_select(&path);
        cell.try_refresh_options(&HashMap::new()).unwrap();
        cell.answer("old".into()).unwrap();

        std::fs::write(&path, "new\n").unwrap();
        cell.begin_edit(&HashMap::new());
        assert!(cell.error().unwrap().contains("previous answer"));
        assert!(cell.submit().is_err());
        cell.cancel();
        assert!(cell.resolved().is_none());
        cell.begin_edit(&HashMap::new());
        cell.select_move(true);
        cell.submit().unwrap();
        assert_eq!(cell.resolved(), Some(("CHOICE", "new")));
        assert!(cell.error().is_none());
    }

    #[test]
    fn empty_choices_and_out_of_range_drafts_cannot_be_submitted() {
        let mut cell = select();
        cell.begin_edit(&HashMap::new());
        if let InputState::Editing { draft, .. } = &mut cell.state {
            *draft = Draft::Select(Some(99));
        }
        assert!(cell.submit().is_err());
        assert!(cell.is_editing());
        assert!(cell.resolved().is_none());
        cell.select_move(true);
        cell.submit().unwrap();
        assert_eq!(cell.resolved(), Some(("CHOICE", "c")));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty");
        std::fs::write(&path, " \n\t\n").unwrap();
        let mut cell = file_select(&path);
        if let MagicInputBlock::Select { options, .. } = &mut cell.config {
            *options = None;
        }
        cell.begin_edit(&HashMap::new());
        assert!(cell.error().unwrap().contains("no options available"));
        assert!(cell.answer(String::new()).is_err());
        assert!(cell.submit().is_err());
        assert!(cell.resolved().is_none());
    }

    #[test]
    fn cli_and_tui_reject_nul_values_and_allow_empty_text() {
        let mut cli = text();
        let mut tui = text();
        tui.begin_edit(&HashMap::new());
        tui.insert_char('\0');
        let error = cli.answer("\0".into()).unwrap_err().to_string();
        assert_eq!(tui.submit().unwrap_err().to_string(), error);
        assert!(tui.is_editing());
        assert!(tui.resolved().is_none());
        tui.backspace();
        tui.submit().unwrap();
        cli.answer(String::new()).unwrap();
        assert_eq!(tui.resolved(), cli.resolved());
        assert!(tui.error().is_none());
    }

    #[test]
    fn input_edit_initializes_scratch_and_uses_layered_environment() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("choices"), "from-file\n").unwrap();
        let mut book = Runbook::new(
            None::<&str>,
            r#"---
env:
  OPTIONS_NAME: overridden
---
```json mrthn=input
{"type":"select","prompt":"Pick","target":"CHOICE","option_file":"${SCRATCH}/$OPTIONS_NAME"}
```"#,
        )
        .unwrap();
        book.frontmatter.tmp_dir = Some(TmpDirConf {
            path: Some(dir.path().to_owned()),
            var_name: Some("SCRATCH".into()),
            skip_cleanup: None,
        });
        book.cli_env.insert("OPTIONS_NAME".into(), "choices".into());
        assert!(book.tmp_dir.is_none());
        book.begin_edit_at(0).unwrap();
        let cell = book.input_at_mut(0).unwrap();
        assert_eq!(cell.options(), ["from-file"]);
        cell.submit().unwrap();
        assert_eq!(
            book.env_for(1).get("CHOICE").map(String::as_str),
            Some("from-file")
        );
    }

    #[test]
    fn split_utf8_output_is_decoded_only_at_the_text_boundary() {
        let mut book = Runbook::new(None::<&str>, "```sh\necho test\n```").unwrap();
        let BookBlock::Code(cell) = &mut book.blocks[0] else {
            panic!("code");
        };
        cell.push_output([0xe2]).unwrap();
        cell.push_output([0x82, 0xac, 0xff]).unwrap();
        assert_eq!(book.output_text(0).unwrap().as_deref(), Some("€�"));
    }

    #[test]
    fn explicit_input_defaults_seed_editors_without_becoming_answers_on_cancel() {
        for config in [
            r#"{"type":"input","prompt":"Name?","target":"NAME","default":"hello"}"#,
            r#"{"type":"confirm","prompt":"Proceed?","target":"OK","default":true}"#,
            r#"{"type":"select","prompt":"Pick?","target":"CHOICE","options":["a","b"],"default":"b"}"#,
        ] {
            let mut cell = InputCell::new(serde_json::from_str(config).unwrap());
            let expected = cell.config.default_value().unwrap();
            cell.begin_edit(&HashMap::new());
            cell.cancel();
            assert!(cell.resolved().is_none());
            cell.begin_edit(&HashMap::new());
            cell.submit().unwrap();
            assert_eq!(cell.resolved().unwrap().1, expected);
        }
    }

    #[test]
    fn pending_has_no_resolution() {
        assert!(confirm().resolved().is_none());
    }

    #[test]
    fn empty_selection_submission_keeps_the_editor_pending() {
        let mut cell = InputCell::new(
            serde_json::from_str(
                r#"{"type":"select","prompt":"?","target":"CHOICE","options":[]}"#,
            )
            .unwrap(),
        );
        cell.begin_edit(&HashMap::new());
        assert!(cell.submit().is_err());
        assert!(cell.is_editing());
        assert!(cell.resolved().is_none());
    }

    #[test]
    fn select_reads_option_file_lines() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("choices.txt");
        std::fs::write(&path, "alpha\n  beta  \n\ngamma\n").unwrap();

        let mut cell = InputCell::new(MagicInputBlock::Select {
            prompt: "Pick".into(),
            target: "CHOICE".into(),
            default: None,
            options: None,
            option_file: Some(path.display().to_string()),
        });
        // Picking the second option resolves to its value.
        cell.begin_edit(&HashMap::new());
        // Trimmed, blank lines skipped; files are only read on activation.
        assert_eq!(cell.options(), ["alpha", "beta", "gamma"]);
        cell.select_move(true);
        cell.submit().unwrap();
        assert_eq!(cell.resolved(), Some(("CHOICE", "beta")));
    }

    #[test]
    fn select_inline_options_precede_file_lines() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("choices.txt");
        std::fs::write(&path, "from_file\n").unwrap();

        let mut cell = InputCell::new(MagicInputBlock::Select {
            prompt: "Pick".into(),
            target: "CHOICE".into(),
            default: None,
            options: Some(vec!["inline".into()]),
            option_file: Some(path.display().to_string()),
        });
        assert_eq!(cell.options(), ["inline"]);
        cell.begin_edit(&HashMap::new());
        assert_eq!(cell.options(), ["inline", "from_file"]);
    }

    #[test]
    fn select_picks_up_file_produced_after_construction() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("choices.txt");

        // Files are not read at construction (a preceding cell may create it).
        let mut cell = InputCell::new(MagicInputBlock::Select {
            prompt: "Pick".into(),
            target: "CHOICE".into(),
            default: None,
            options: None,
            option_file: Some(path.display().to_string()),
        });
        assert!(cell.options().is_empty());

        // The file appears, then the user activates the cell.
        std::fs::write(&path, "late\n").unwrap();
        cell.begin_edit(&HashMap::new());
        assert_eq!(cell.options(), ["late"]);
    }

    #[test]
    fn select_expands_vars_in_option_file_path() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("choices.txt"), "one\ntwo\n").unwrap();

        let mut cell = InputCell::new(MagicInputBlock::Select {
            prompt: "Pick".into(),
            target: "CHOICE".into(),
            default: None,
            options: None,
            option_file: Some("${TMP_DIR}/choices.txt".into()),
        });
        // Unresolved at construction (no env): the literal `${TMP_DIR}` path
        // doesn't exist, so no options yet.
        assert!(cell.options().is_empty());

        // On edit, the env supplies TMP_DIR and the path resolves.
        let env = HashMap::from([("TMP_DIR".to_string(), dir.path().display().to_string())]);
        cell.begin_edit(&env);
        assert_eq!(cell.options(), ["one", "two"]);
    }

    #[test]
    fn expand_env_lookup_forms() {
        let env = HashMap::from([
            ("TMP_DIR".to_string(), "/tmp/x".to_string()),
            ("WHO".to_string(), "ann".to_string()),
        ]);
        // `$NAME` and `${NAME}`.
        assert_eq!(expand_env("$TMP_DIR/f.txt", &env), "/tmp/x/f.txt");
        assert_eq!(expand_env("${TMP_DIR}/f.txt", &env), "/tmp/x/f.txt");
        // `${NAME}` lets a name butt up against following word chars.
        assert_eq!(expand_env("${WHO}_file", &env), "ann_file");
        // `$NAME` stops at the first non-word char.
        assert_eq!(expand_env("$WHO-x", &env), "ann-x");
    }

    #[test]
    fn expand_env_leaves_unknown_and_loose_dollars_literal() {
        let env = HashMap::from([("WHO".to_string(), "ann".to_string())]);
        // Unknown var: left untouched (both forms).
        assert_eq!(expand_env("$NOPE/x", &env), "$NOPE/x");
        assert_eq!(expand_env("${NOPE}/x", &env), "${NOPE}/x");
        // Unterminated `${`: left untouched.
        assert_eq!(expand_env("${WHO", &env), "${WHO");
        // A loose `$` (not a reference) survives.
        assert_eq!(expand_env("cost is $5", &env), "cost is $5");
        assert_eq!(expand_env("trailing $", &env), "trailing $");
        // `$$` is a literal `$` — not a recursive expansion.
        assert_eq!(expand_env("$$WHO", &env), "$WHO");
    }

    #[test]
    fn confirm_submit_writes_yes_no() {
        let mut c = confirm();
        c.begin_edit(&HashMap::new());
        // Default seed is No.
        c.submit().unwrap();
        assert_eq!(c.resolved(), Some(("OK", "no")));

        c.begin_edit(&HashMap::new());
        c.set_confirm(true);
        c.submit().unwrap();
        assert_eq!(c.resolved(), Some(("OK", "yes")));
    }

    #[test]
    fn confirm_toggle_flips() {
        let mut c = confirm();
        c.begin_edit(&HashMap::new());
        c.toggle_confirm();
        c.submit().unwrap();
        assert_eq!(c.resolved(), Some(("OK", "yes")));
    }

    #[test]
    fn text_edit_inserts_and_deletes() {
        let mut c = text();
        c.begin_edit(&HashMap::new());
        for ch in "abc".chars() {
            c.insert_char(ch);
        }
        c.cursor_left();
        c.insert_char('X'); // ab[X]c
        c.submit().unwrap();
        assert_eq!(c.resolved(), Some(("NAME", "abXc")));

        c.begin_edit(&HashMap::new()); // re-edit seeds from prior answer, cursor at end
        c.backspace();
        c.submit().unwrap();
        assert_eq!(c.resolved(), Some(("NAME", "abX")));
    }

    #[test]
    fn text_cursor_clamps() {
        let mut t = TextDraft::default();
        t.left(); // no panic at 0
        t.insert('é'); // multi-byte
        t.insert('x');
        assert_eq!(t.cursor, 2);
        t.home();
        t.delete(); // removes 'é'
        assert_eq!(t.value, "x");
        assert_eq!(t.cursor, 0);
    }

    #[test]
    fn select_moves_and_clamps() {
        let mut c = select();
        c.begin_edit(&HashMap::new());
        c.select_move(false); // already at 0, stays
        c.select_move(true); // -> 1
        c.select_move(true); // -> 2
        c.select_move(true); // clamps at 2
        c.submit().unwrap();
        assert_eq!(c.resolved(), Some(("CHOICE", "c")));
    }

    #[test]
    fn cancel_restores_prior_answer() {
        let mut c = text();
        c.begin_edit(&HashMap::new());
        c.insert_char('z');
        c.submit().unwrap();
        assert_eq!(c.resolved(), Some(("NAME", "z")));

        c.begin_edit(&HashMap::new());
        c.insert_char('!'); // editing "z!"
        c.cancel(); // discard edit, restore "z"
        assert_eq!(c.resolved(), Some(("NAME", "z")));
        assert!(!c.is_editing());
    }

    #[test]
    fn cancel_from_pending_returns_to_pending() {
        let mut c = confirm();
        c.begin_edit(&HashMap::new());
        c.cancel();
        assert!(matches!(c.state, InputState::Pending));
    }

    #[test]
    fn select_re_edit_seeds_from_answer() {
        let mut c = select();
        c.begin_edit(&HashMap::new());
        c.select_move(true); // -> "b"
        c.submit().unwrap();
        assert_eq!(c.resolved(), Some(("CHOICE", "b")));

        c.begin_edit(&HashMap::new()); // should seed index at "b" (1)
        match &c.state {
            InputState::Editing {
                draft: Draft::Select(i),
                ..
            } => assert_eq!(*i, Some(1)),
            other => panic!("expected select draft, got {other:?}"),
        }
    }

    // --- execution layer ---

    #[test]
    fn is_runnable_recognizes_shells_and_skip() {
        let mk = |lang: &str, skip: Option<bool>| CodeBlock {
            lang: lang.into(),
            meta: CodeBlockMeta {
                skip,
                ..Default::default()
            },
            content: String::new(),
            state: CodeBlockState::NotRun,
            output: crate::output::OutputCapture::default(),
            error: None,
            output_page: None,
            started_at: None,
            elapsed: None,
            exit_code: None,
            cancel: Cancel::None,
        };
        assert!(mk("sh", None).is_runnable());
        assert!(mk("bash", None).is_runnable());
        assert!(mk("zsh", Some(false)).is_runnable());
        assert!(!mk("sh", Some(true)).is_runnable()); // opted out
        assert!(!mk("python", None).is_runnable()); // unknown lang
    }

    #[test]
    fn run_counts_tally_runnable_and_states() {
        // Two runnable shell cells and one display-only python cell.
        let doc = "---\ntitle: t\n---\n\n```sh\necho a\n```\n\n\
                   ```sh\necho b\n```\n\n```python\nprint(1)\n```\n";
        let mut rb = Runbook::new(None::<&str>, doc).unwrap();
        assert_eq!(
            rb.run_counts(),
            RunCounts {
                runnable: 2,
                ..Default::default()
            }
        );

        // Drive the first shell cell to success, the second to error.
        let mut seen = 0;
        for block in rb.blocks.iter_mut() {
            if let BookBlock::Code(c) = block
                && c.lang == "sh"
            {
                c.state = if seen == 0 {
                    CodeBlockState::Success
                } else {
                    CodeBlockState::Error
                };
                seen += 1;
            }
        }

        let counts = rb.run_counts();
        assert_eq!(counts.runnable, 2);
        assert_eq!(counts.succeeded, 1);
        assert_eq!(counts.errored, 1);
        assert_eq!(counts.finished(), 2);
    }

    #[test]
    fn clear_all_resets_cells_and_last_run() {
        let doc = "---\ntitle: t\n---\n\n\
            ```json mrthn=input\n{\"type\":\"input\",\"prompt\":\"p\",\"target\":\"T\"}\n```\n\n\
            ```sh\necho hi\n```\n";
        let mut rb = Runbook::new(None::<&str>, doc).unwrap();

        // Answer the input and drive the code cell through a finished run.
        let cell = rb.input_at_mut(0).unwrap();
        cell.begin_edit(&HashMap::new());
        cell.insert_char('z');
        cell.submit().unwrap();
        if let BookBlock::Code(c) = &mut rb.blocks[1] {
            c.begin_run();
            c.push_output("out\n").unwrap();
            c.finish(false, Some(2));
        }
        rb.last_run = Some(1);

        rb.clear_all();

        match &rb.blocks[0] {
            BookBlock::Input(i) => assert!(matches!(i.state, InputState::Pending)),
            other => panic!("expected input, got {other:?}"),
        }
        match &rb.blocks[1] {
            BookBlock::Code(c) => {
                assert!(c.output.is_empty(), "output not cleared");
                assert_eq!(c.state, CodeBlockState::NotRun);
                assert!(c.elapsed.is_none() && c.exit_code.is_none());
            }
            other => panic!("expected code, got {other:?}"),
        }
        assert_eq!(rb.last_run, None);
    }

    #[test]
    fn copy_text_yields_source_code_and_value() {
        let doc = "---\ntitle: t\n---\n\n# Heading\n\n```sh\necho hi\n```\n\n\
            ```json mrthn=input\n{\"type\":\"input\",\"prompt\":\"Name?\",\"target\":\"WHO\"}\n```\n";
        let mut rb = Runbook::new(None::<&str>, doc).unwrap();

        // Markdown → exact source; code → raw body (no fence).
        assert_eq!(rb.copy_text(0).as_deref(), Some("# Heading"));
        assert_eq!(rb.copy_text(1).as_deref(), Some("echo hi"));

        // Input blocks are not copyable, answered or not.
        assert_eq!(rb.copy_text(2), None);
        let cell = rb.input_at_mut(2).unwrap();
        cell.begin_edit(&HashMap::new());
        cell.insert_char('z');
        cell.submit().unwrap();
        assert_eq!(rb.copy_text(2), None);

        // Out of range.
        assert_eq!(rb.copy_text(99), None);
    }

    #[test]
    fn interpreter_defaults_and_remaps() {
        let rb = Runbook::new(None::<&str>, "---\ntitle: t\n---\n\n```sh\n:\n```\n").unwrap();
        assert_eq!(rb.interpreter_for("sh"), vec!["/usr/bin/env", "sh"]);

        let doc = "---\ninterpreters:\n  sh:\n    path: /bin/zsh -f\n---\n\n```sh\n:\n```\n";
        let rb = Runbook::new(None::<&str>, doc).unwrap();
        assert_eq!(rb.interpreter_for("sh"), vec!["/bin/zsh", "-f"]);
    }

    #[test]
    fn script_wraps_with_before_and_after_each() {
        let doc = "---\nbefore_each: set -e\nafter_each: echo done\n---\n\n```sh\necho body\n```\n";
        let rb = Runbook::new(None::<&str>, doc).unwrap();
        let c = match &rb.blocks[0] {
            BookBlock::Code(c) => c,
            other => panic!("expected code, got {other:?}"),
        };
        assert_eq!(rb.script_for(c), "set -e\necho body\necho done");
    }

    #[test]
    fn script_defaults_before_each_to_strict_mode() {
        let code = |doc: &str| {
            let rb = Runbook::new(None::<&str>, doc).unwrap();
            match &rb.blocks[0] {
                BookBlock::Code(c) => rb.script_for(c),
                other => panic!("expected code, got {other:?}"),
            }
        };

        // Omitted `before_each` → defaults to `set -eu`.
        assert_eq!(
            code("---\ntitle: t\n---\n\n```sh\necho body\n```\n"),
            "set -eu\necho body"
        );
        // Explicit empty string opts out entirely.
        assert_eq!(
            code("---\nbefore_each: \"\"\n---\n\n```sh\necho body\n```\n"),
            "echo body"
        );
    }

    #[test]
    fn env_for_layers_frontmatter_then_preceding_inputs() {
        let doc = "---\nenv:\n  BASE: x\n---\n\n\
            ```json mrthn=input\n{\"type\":\"input\",\"prompt\":\"p\",\"target\":\"NAME\"}\n```\n\n\
            ```sh\necho hi\n```\n";
        let mut rb = Runbook::new(None::<&str>, doc).unwrap();

        // Before answering: the sh cell (block 1) sees BASE but not NAME.
        let env = rb.env_for(1);
        assert_eq!(env.get("BASE").map(String::as_str), Some("x"));
        assert!(!env.contains_key("NAME"));

        // Answer the input cell (block 0).
        let cell = rb.input_at_mut(0).unwrap();
        cell.begin_edit(&HashMap::new());
        cell.insert_char('z');
        cell.submit().unwrap();

        // Now block 1 sees the answer; block 0 (the input itself) does not see
        // its own forthcoming value (only *preceding* cells count).
        let env = rb.env_for(1);
        assert_eq!(env.get("NAME").map(String::as_str), Some("z"));
        assert!(!rb.env_for(0).contains_key("NAME"));
    }

    #[test]
    fn ensure_tmp_dir_is_created_and_injected() {
        let mut rb = Runbook::new(None::<&str>, "---\ntitle: t\n---\n\n```sh\n:\n```\n").unwrap();
        let dir = rb.ensure_tmp_dir().unwrap();
        assert!(dir.is_dir());
        // Idempotent: second call returns the same path.
        assert_eq!(rb.ensure_tmp_dir().unwrap(), dir);
        // And it shows up in the env map under TMP_DIR.
        let env = rb.env_for(0);
        assert_eq!(
            env.get("TMP_DIR").map(String::as_str),
            Some(dir.to_str().unwrap())
        );
    }

    #[test]
    fn clear_all_recreates_the_temp_dir() {
        let mut rb = Runbook::new(None::<&str>, "---\ntitle: t\n---\n\n```sh\n:\n```\n").unwrap();
        let first = rb.ensure_tmp_dir().unwrap();
        assert!(first.is_dir());

        // A full clear removes the old auto-created dir and mints a fresh one.
        rb.clear_all();
        assert!(!first.exists(), "old temp dir should be removed on clear");

        let second = rb.ensure_tmp_dir().unwrap();
        assert!(second.is_dir());
        assert_ne!(first, second, "a new temp dir should be created");
    }

    #[test]
    fn clear_all_leaves_an_explicit_temp_dir_untouched() {
        let scratch = tempfile::TempDir::new().unwrap();
        let path = scratch.path().join("explicit");
        let src = format!(
            "---\ntmp_dir:\n  path: {}\n---\n\n```sh\n:\n```\n",
            path.display()
        );
        let mut rb = Runbook::new(None::<&str>, &src).unwrap();
        let dir = rb.ensure_tmp_dir().unwrap();
        assert_eq!(dir, path);

        // A user-configured dir has no guard, so a clear must not delete it.
        rb.clear_all();
        assert!(path.is_dir(), "explicit temp dir must survive a clear");
        assert_eq!(rb.ensure_tmp_dir().unwrap(), path);
    }
}
