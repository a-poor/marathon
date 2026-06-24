use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
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
        let txt = get_frontmatter_node(&ast)?;
        let frontmatter: BookFrontmatter = serde_yaml::from_str(&txt)?;

        // Coerce the blocks. Frontmatter (YAML/TOML) is config, not content, so
        // it isn't a navigable/rendered block.
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
                    // Parse the code block
                    let b = CodeBlock::try_from(c.clone()).map_err(|err| anyhow!("{}", err))?;

                    // Is it an input block?
                    if b.lang == "json"
                        && b.meta.mrthn.as_ref().map(|s| s == "input").unwrap_or(false)
                    {
                        let mib: MagicInputBlock = serde_json::from_str(&b.content)?;
                        return Ok(BookBlock::Input(InputCell::new(mib)));
                    }

                    // Otherwise just runnable code
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
            last_run: None,
            tmp_dir: None,
            tmp_guard: None,
            source: doc.to_string(),
            cli_env: HashMap::new(),
        })
    }

    /// Mutable access to the input cell at `idx`, if that block is one.
    pub fn input_at_mut(&mut self, idx: usize) -> Option<&mut InputCell> {
        match self.blocks.get_mut(idx) {
            Some(BookBlock::Input(cell)) => Some(cell),
            _ => None,
        }
    }

    /// The active temp directory as an env-var `(name, path)` pair, if one has been
    /// created yet. The dir is made lazily on first run, so this is `None` until then.
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

    /// Resolve the shared temp directory, creating it on first use (DESIGN §4).
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

    /// Build the environment map injected into the cell at `idx` (DESIGN §4):
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

    /// The captured stdout+stderr of the code cell at `idx`, for copying its output.
    /// `None` if the index is out of range, the block isn't a code cell, or the cell
    /// has produced no output yet (nothing to copy).
    pub fn output_text(&self, idx: usize) -> Option<String> {
        match self.blocks.get(idx)? {
            BookBlock::Code(c) if !c.output.is_empty() => Some(c.output.clone()),
            _ => None,
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
    /// Combined stdout+stderr captured from the run, accumulated as it streams in.
    /// Kept separate from `state` because it changes far more frequently (see the
    /// three-tier note in TODO.md): a chunk appends here without touching `state`.
    pub output: String,
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
        self.output.clear();
        self.started_at = Some(std::time::Instant::now());
        self.elapsed = None;
        self.exit_code = None;
        self.cancel = Cancel::None;
        self.state = CodeBlockState::Running;
    }

    /// Append a streamed output chunk.
    pub fn push_output(&mut self, chunk: &str) {
        self.output.push_str(chunk);
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
        self.output.clear();
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
            lang: val.lang.unwrap_or("sh".into()),
            content: val.value,
            meta,
            state: CodeBlockState::NotRun,
            output: String::new(),
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

impl MagicInputBlock {
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
    pub config: MagicInputBlock,
    pub state: InputState,
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
    Select(usize),
}

/// A single-line text buffer with a char-indexed cursor.
#[derive(Debug, Clone, Default)]
pub struct TextDraft {
    pub value: String,
    /// Cursor position as a *character* index (0..=char count).
    pub cursor: usize,
}

impl TextDraft {
    fn seeded(value: String) -> Self {
        let cursor = value.chars().count();
        Self { value, cursor }
    }

    /// Byte offset of char index `idx` (clamped to the end).
    fn byte_at(&self, idx: usize) -> usize {
        self.value
            .char_indices()
            .nth(idx)
            .map(|(b, _)| b)
            .unwrap_or(self.value.len())
    }

    fn char_count(&self) -> usize {
        self.value.chars().count()
    }

    fn insert(&mut self, c: char) {
        let at = self.byte_at(self.cursor);
        self.value.insert(at, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_at(self.cursor - 1);
        let end = self.byte_at(self.cursor);
        self.value.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor >= self.char_count() {
            return;
        }
        let start = self.byte_at(self.cursor);
        let end = self.byte_at(self.cursor + 1);
        self.value.replace_range(start..end, "");
    }

    fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_count());
    }

    fn home(&mut self) {
        self.cursor = 0;
    }

    fn end(&mut self) {
        self.cursor = self.char_count();
    }
}

impl InputCell {
    pub fn new(config: MagicInputBlock) -> Self {
        Self {
            config,
            state: InputState::Pending,
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
        match &self.config {
            MagicInputBlock::Select {
                options: Some(o), ..
            } => o,
            _ => &[],
        }
    }

    fn option_at(&self, idx: usize) -> Option<&str> {
        self.options().get(idx).map(String::as_str)
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
    pub fn begin_edit(&mut self) {
        let prior = match &self.state {
            InputState::Answered { value } => Some(value.clone()),
            _ => None,
        };
        let draft = match &self.config {
            MagicInputBlock::Confirm { .. } => Draft::Confirm(prior.as_deref() == Some("yes")),
            MagicInputBlock::Input { .. } => {
                Draft::Text(TextDraft::seeded(prior.clone().unwrap_or_default()))
            }
            MagicInputBlock::Select { .. } => {
                let idx = prior
                    .as_deref()
                    .and_then(|v| self.options().iter().position(|o| o == v))
                    .unwrap_or(0);
                Draft::Select(idx)
            }
        };
        self.state = InputState::Editing { draft, prior };
    }

    /// Commit the current draft as the answer. No-op if not editing.
    pub fn submit(&mut self) {
        let value = match &self.state {
            InputState::Editing { draft, .. } => match draft {
                Draft::Confirm(b) => Some(if *b { "yes" } else { "no" }.to_string()),
                Draft::Text(t) => Some(t.value.clone()),
                Draft::Select(i) => Some(self.option_at(*i).unwrap_or_default().to_string()),
            },
            _ => None,
        };
        if let Some(value) = value {
            self.state = InputState::Answered { value };
        }
    }

    /// Discard any answer (or in-progress edit) and return to pending.
    pub fn clear(&mut self) {
        self.state = InputState::Pending;
    }

    /// Cancel editing, restoring a prior answer if there was one.
    pub fn cancel(&mut self) {
        if let InputState::Editing { prior, .. } = &self.state {
            self.state = match prior {
                Some(value) => InputState::Answered {
                    value: value.clone(),
                },
                None => InputState::Pending,
            };
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
            *i = if forward {
                (*i + 1).min(n - 1)
            } else {
                i.saturating_sub(1)
            };
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
        })
    }

    fn text() -> InputCell {
        InputCell::new(MagicInputBlock::Input {
            prompt: "Name?".into(),
            target: "NAME".into(),
        })
    }

    fn select() -> InputCell {
        InputCell::new(MagicInputBlock::Select {
            prompt: "Pick".into(),
            target: "CHOICE".into(),
            options: Some(vec!["a".into(), "b".into(), "c".into()]),
            option_file: None,
        })
    }

    #[test]
    fn pending_has_no_resolution() {
        assert!(confirm().resolved().is_none());
    }

    #[test]
    fn confirm_submit_writes_yes_no() {
        let mut c = confirm();
        c.begin_edit();
        // Default seed is No.
        c.submit();
        assert_eq!(c.resolved(), Some(("OK", "no")));

        c.begin_edit();
        c.set_confirm(true);
        c.submit();
        assert_eq!(c.resolved(), Some(("OK", "yes")));
    }

    #[test]
    fn confirm_toggle_flips() {
        let mut c = confirm();
        c.begin_edit();
        c.toggle_confirm();
        c.submit();
        assert_eq!(c.resolved(), Some(("OK", "yes")));
    }

    #[test]
    fn text_edit_inserts_and_deletes() {
        let mut c = text();
        c.begin_edit();
        for ch in "abc".chars() {
            c.insert_char(ch);
        }
        c.cursor_left();
        c.insert_char('X'); // ab[X]c
        c.submit();
        assert_eq!(c.resolved(), Some(("NAME", "abXc")));

        c.begin_edit(); // re-edit seeds from prior answer, cursor at end
        c.backspace();
        c.submit();
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
        c.begin_edit();
        c.select_move(false); // already at 0, stays
        c.select_move(true); // -> 1
        c.select_move(true); // -> 2
        c.select_move(true); // clamps at 2
        c.submit();
        assert_eq!(c.resolved(), Some(("CHOICE", "c")));
    }

    #[test]
    fn cancel_restores_prior_answer() {
        let mut c = text();
        c.begin_edit();
        c.insert_char('z');
        c.submit();
        assert_eq!(c.resolved(), Some(("NAME", "z")));

        c.begin_edit();
        c.insert_char('!'); // editing "z!"
        c.cancel(); // discard edit, restore "z"
        assert_eq!(c.resolved(), Some(("NAME", "z")));
        assert!(!c.is_editing());
    }

    #[test]
    fn cancel_from_pending_returns_to_pending() {
        let mut c = confirm();
        c.begin_edit();
        c.cancel();
        assert!(matches!(c.state, InputState::Pending));
    }

    #[test]
    fn select_re_edit_seeds_from_answer() {
        let mut c = select();
        c.begin_edit();
        c.select_move(true); // -> "b"
        c.submit();
        assert_eq!(c.resolved(), Some(("CHOICE", "b")));

        c.begin_edit(); // should seed index at "b" (1)
        match &c.state {
            InputState::Editing {
                draft: Draft::Select(i),
                ..
            } => assert_eq!(*i, 1),
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
            output: String::new(),
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
        cell.begin_edit();
        cell.insert_char('z');
        cell.submit();
        if let BookBlock::Code(c) = &mut rb.blocks[1] {
            c.begin_run();
            c.push_output("out\n");
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
        cell.begin_edit();
        cell.insert_char('z');
        cell.submit();
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
        cell.begin_edit();
        cell.insert_char('z');
        cell.submit();

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
