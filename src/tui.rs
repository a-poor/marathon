use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers, MouseEventKind},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::{
    book::{BookBlock, Cancel, CodeBlockState, MagicInputBlock, Runbook, TextDraft},
    runner::{self, RunMsg},
    widgets::{
        footer::{FooterWidget, Status},
        help::HelpModal,
        scrollview::{DocumentView, ScrollState, SearchBookmark},
    },
};

/// Interaction mode. `Navigate` moves the per-cell selection through the
/// document; `Active` routes keys into the focused input cell's draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Navigate,
    Active,
}

struct SearchEdit {
    draft: TextDraft,
    bookmark: SearchBookmark,
}

/// Interactive runbook viewer and owner of all active cell tasks.
pub struct App {
    book: Runbook,
    scroll: ScrollState,
    /// Navigate vs. actively editing the focused input cell.
    mode: Mode,
    search_edit: Option<SearchEdit>,
    /// Whether the hotkeys help modal is open (overlays any mode).
    show_help: bool,
    /// Whether cell outputs show bounded pages instead of the live tail.
    verbose: bool,
    /// Bumped whenever block contents change (e.g. a cell runs), to invalidate
    /// the document's wrapped-line cache.
    revision: u64,
    /// Height of the document viewport at the last draw, for page scrolling.
    viewport_h: u16,
    exit: bool,
    start: std::time::Instant,
    /// A transient footer status (e.g. "copied") and when it was set. Shown for
    /// [`FLASH_DURATION`], then it fades on its own as the draw loop redraws.
    flash: Option<(String, std::time::Instant)>,
    /// The most recent cell finish (its settled state + when), so the badge can
    /// briefly reveal the latest outcome for [`FINISH_BADGE_TIMEOUT`] before idling.
    last_finish: Option<(Status, std::time::Instant)>,
    /// One owner per running cell, inserted before its task can emit output.
    runs: HashMap<usize, runner::RunningCell>,
    /// Cell currently owned by run-remaining, including a paused input editor.
    sequence: Option<usize>,
    /// The system clipboard handle, opened once at startup (held alive so the
    /// clipboard persists on platforms that serve it from the owning process, e.g.
    /// X11). `None` if the platform has no clipboard available.
    clipboard: Option<arboard::Clipboard>,
    /// Channel for finished cell runs, drained as a `select!` arm in [`App::run`].
    run_tx: mpsc::Sender<RunMsg>,
    run_rx: mpsc::Receiver<RunMsg>,
}

/// How long a footer flash message stays visible.
const FLASH_DURATION: Duration = Duration::from_millis(1500);

/// How long the badge reveals a cell's just-finished state before reverting to idle.
const FINISH_BADGE_TIMEOUT: Duration = Duration::from_secs(5);

impl App {
    pub fn new(book: Runbook) -> Self {
        let (run_tx, run_rx) = mpsc::channel(runner::CHANNEL_CAPACITY);
        Self {
            book,
            scroll: ScrollState::new(),
            mode: Mode::Navigate,
            search_edit: None,
            show_help: false,
            verbose: false,
            revision: 0,
            viewport_h: 0,
            exit: false,
            start: std::time::Instant::now(),
            flash: None,
            last_finish: None,
            runs: HashMap::new(),
            sequence: None,
            clipboard: arboard::Clipboard::new().ok(),
            run_tx,
            run_rx,
        }
    }

    /// Run the async draw/event loop until the user quits.
    ///
    /// Drawing is driven by a fixed-rate timer so time-based UI (e.g. the spinner)
    /// animates even with no input, while terminal events arrive concurrently via
    /// `EventStream`. When cells begin executing, their output channel becomes a
    /// third `select!` arm here.
    pub async fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        // Create the shared temp dir up front so its path is visible in the header
        // (under $TMP_DIR) from the first frame, rather than only after the first run.
        // Best-effort: a failure here resurfaces when a cell actually runs.
        let _ = self.book.ensure_tmp_dir();

        let mut events = EventStream::new();
        let mut frames = tokio::time::interval(Duration::from_secs_f32(1.0 / 30.0));

        let signal = crate::term::termination();
        tokio::pin!(signal);
        let result = async {
            while !self.exit {
                tokio::select! {
                    result = &mut signal => { result?; self.exit = true; }
                    _ = frames.tick() => {
                        terminal.draw(|frame| self.draw(frame))?;
                    }
                    event = events.next() => {
                        match event {
                            Some(Ok(event)) => self.handle_event(&event),
                            Some(Err(e)) => return Err(e.into()),
                            None => break,
                        }
                    }
                    Some(msg) = self.run_rx.recv() => self.apply_run_msg(msg),
                }
            }
            Ok(())
        }
        .await;
        // This also runs on draw/input errors, before the runbook drops TMP_DIR.
        self.shutdown_runs().await;
        result
    }

    async fn shutdown_runs(&mut self) {
        self.sequence = None;
        self.run_rx.close();
        for run in self.runs.values() {
            run.cancel(true);
        }
        for (_, run) in self.runs.drain() {
            run.shutdown().await;
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        // No sticky header — the runbook's header banner scrolls inside the document
        // (see `scrollview::header_lines`). Just the body and the footer bar.
        let [body, footer] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
        self.viewport_h = body.height;

        frame.render_stateful_widget(
            DocumentView::new(&self.book, self.revision)
                .active(self.mode == Mode::Active)
                .verbose(self.verbose),
            body,
            &mut self.scroll,
        );

        if let Some(search) = &self.search_edit {
            self.draw_search(frame, footer, &search.draft);
        } else {
            frame.render_widget(self.footer(), footer);
        }

        // The help modal floats over everything when open.
        if self.show_help {
            frame.render_widget(HelpModal, frame.area());
        }
    }

    fn draw_search(&self, frame: &mut Frame, area: Rect, draft: &TextDraft) {
        if area.is_empty() {
            return;
        }
        let hint = format!(
            " {} • Enter accept • Esc cancel",
            self.scroll.search_count()
        );
        let hint_width = if area.width > 50 {
            Line::raw(&hint).width() as u16
        } else {
            0
        };
        let [prompt, status] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(hint_width)]).areas(area);
        let cursor = draft.byte_at(draft.cursor);
        let mut start = 0;
        // Keep the insertion point visible, measuring terminal columns rather
        // than bytes or character count (wide and accented text can differ).
        let available = prompt.width.saturating_sub(2) as usize;
        while start < cursor && Line::raw(&draft.value[start..cursor]).width() > available {
            start += draft.value[start..].chars().next().unwrap().len_utf8();
        }
        frame.render_widget(
            Line::from(format!("/{}", &draft.value[start..])).cyan(),
            prompt,
        );
        frame.render_widget(Line::from(hint).dim(), status);
        let x = 1 + Line::raw(&draft.value[start..cursor]).width() as u16;
        frame.set_cursor_position((prompt.x + x.min(prompt.width - 1), prompt.y));
    }

    /// Build the footer for this frame: a run-state badge (left), run counts
    /// (center), and mode-aware key hints (right), all derived fresh from the book.
    ///
    /// The badge shows the *latest* activity, not a persistent aggregate: a running
    /// cell wins; otherwise the most recent finish is revealed for
    /// [`FINISH_BADGE_TIMEOUT`]; otherwise it idles at `ready`.
    fn footer(&self) -> FooterWidget<'static> {
        let counts = self.book.run_counts();
        let status = if counts.running > 0 {
            Status::Running
        } else {
            self.last_finish
                .filter(|(_, at)| at.elapsed() < FINISH_BADGE_TIMEOUT)
                .map(|(state, _)| state)
                .unwrap_or(Status::Ready)
        };

        let hints = match self.mode {
            Mode::Navigate if self.sequence.is_some() => {
                Line::from("running remaining • backspace stop • q quit")
            }
            Mode::Navigate if !self.scroll.search_query().is_empty() => {
                Line::from("n/N match • esc clear • ↵ run • ? help")
            }
            Mode::Navigate => Line::from("↑/↓ move • ↵ run • r remaining • / search • ? help"),
            Mode::Active => Line::from("↵ submit • esc cancel • ←/→ edit"),
        };

        let mut footer = FooterWidget::new(self.start)
            .status(status)
            .counts(self.counts_line())
            .hints(hints);

        // A transient flash (e.g. "copied") takes over the center while it's active.
        if let Some(msg) = self.flash_active() {
            footer = footer.flash(Line::from(msg.to_owned()));
        } else if !self.scroll.search_query().is_empty() {
            footer = footer.flash(Line::from(format!(
                "/{} · {}",
                self.scroll.search_query(),
                self.scroll.search_count(),
            )));
        }
        footer
    }

    /// The run-count tally for the footer center: pending / succeeded / errored, by
    /// symbol. A group is dimmed when its count is zero so `✗ 0` doesn't read as an
    /// alarm. Pending counts every runnable cell not yet finished (running included).
    fn counts_line(&self) -> Line<'static> {
        let c = self.book.run_counts();
        let pending = c.runnable.saturating_sub(c.succeeded + c.errored);

        let group = |glyph: char, n: usize, color: Color| {
            let text = format!("{glyph} {n}");
            if n > 0 {
                Span::styled(text, Style::new().fg(color))
            } else {
                text.dim()
            }
        };

        Line::from(vec![
            group('◦', pending, Color::Gray).dim(),
            Span::raw("   "),
            group('✔', c.succeeded, Color::Green),
            Span::raw("   "),
            group('✗', c.errored, Color::Red),
        ])
    }

    /// Translate a single terminal event into a state change. Infallible now;
    /// returns nothing because the loop owns the draw/error path.
    fn handle_event(&mut self, event: &Event) {
        if let Some(key) = event.as_key_press_event() {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                self.exit = true;
                return;
            }
            // The help modal is a global overlay: while open it swallows keys and is
            // dismissed with Esc (or `?`), so the underlying mode never sees them.
            if self.show_help {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                    self.show_help = false;
                }
                return;
            }
            if self.search_edit.is_some() {
                self.handle_search_key(key);
                return;
            }
            match self.mode {
                Mode::Navigate => self.handle_navigate_key(key),
                Mode::Active => self.handle_active_key(key),
            }
        } else if let Event::Mouse(m) = event {
            // Wheel scroll works in either mode. Only delivered when mouse
            // capture is enabled; harmless otherwise.
            match m.kind {
                MouseEventKind::ScrollDown => self.scroll.scroll_down(3),
                MouseEventKind::ScrollUp => self.scroll.scroll_up(3),
                _ => {}
            }
        }
    }

    /// Total selectable items: the header banner (index 0) plus every block.
    fn selectable_count(&self) -> usize {
        self.book.blocks.len() + 1
    }

    /// The block index the selection points at, or `None` when the header (index 0)
    /// is selected. Selection space is `[header, block 0, block 1, …]`.
    fn selected_block(&self) -> Option<usize> {
        self.scroll.selected().checked_sub(1)
    }

    /// Navigation-mode keys: move the selection, scroll, quit, or activate the
    /// selected input cell.
    fn handle_navigate_key(&mut self, key: KeyEvent) {
        let len = self.selectable_count();
        let page = (self.viewport_h / 2).max(1);

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) if !self.scroll.search_query().is_empty() => {
                self.scroll.search_for(String::new());
            }
            (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => self.exit = true,
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.exit = true,
            (KeyCode::Char('j'), _) | (KeyCode::Down, _) => self.scroll.select_next(len),
            (KeyCode::Char('k'), _) | (KeyCode::Up, _) => self.scroll.select_prev(),
            (KeyCode::Char('g'), _) | (KeyCode::Home, _) => self.scroll.select_first(),
            (KeyCode::Char('G'), _) | (KeyCode::End, _) => self.scroll.select_last(len),
            (KeyCode::Char('d'), KeyModifiers::CONTROL) | (KeyCode::PageDown, _) => {
                self.scroll.scroll_down(page)
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) | (KeyCode::PageUp, _) => {
                self.scroll.scroll_up(page)
            }
            (KeyCode::Enter, _) => self.activate_or_run(),
            (KeyCode::Char('r'), _) => self.run_remaining(),
            (KeyCode::Backspace, _) => self.cancel_selected(),
            (KeyCode::Char('y'), _) => self.copy_selected(),
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                self.verbose = !self.verbose;
                self.revision += 1;
            }
            (KeyCode::Char('['), _) => self.page_output(false),
            (KeyCode::Char(']'), _) => self.page_output(true),
            (KeyCode::Char('Y'), _) => self.copy_output_selected(),
            (KeyCode::Char('x'), _) => self.clear_selected(),
            (KeyCode::Char('X'), _) => self.clear_all(),
            (KeyCode::Char('?'), _) => self.show_help = true,
            (KeyCode::Char('/'), _) => self.start_search(),
            (KeyCode::Char('n' | 'N'), _) => {
                if self.scroll.search_query().is_empty() {
                    self.notice("press / to search");
                } else {
                    self.scroll.search_next(key.code == KeyCode::Char('N'));
                    self.scroll
                        .resolve_search(&self.book, self.revision, self.verbose);
                }
            }
            _ => {}
        }
    }

    fn start_search(&mut self) {
        // Run-remaining owns selection and can activate an input when a command
        // finishes. Do not let a search preview change that editor's target.
        if self.sequence.is_some() {
            self.notice("stop run remaining before searching");
            return;
        }
        self.flash = None;
        self.search_edit = Some(SearchEdit {
            draft: TextDraft::seeded(self.scroll.search_query().to_owned()),
            bookmark: self.scroll.search_bookmark(),
        });
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            let edit = self.search_edit.take().expect("search editor is open");
            self.scroll.restore_search(edit.bookmark);
            return;
        }
        if key.code == KeyCode::Enter {
            self.search_edit = None;
            return;
        }
        let draft = &mut self
            .search_edit
            .as_mut()
            .expect("search editor is open")
            .draft;
        match (key.code, key.modifiers) {
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => *draft = TextDraft::default(),
            (KeyCode::Char(ch), KeyModifiers::NONE | KeyModifiers::SHIFT) if !ch.is_control() => {
                draft.insert(ch);
            }
            (KeyCode::Backspace, _) => draft.backspace(),
            (KeyCode::Delete, _) => draft.delete(),
            (KeyCode::Left, _) => draft.left(),
            (KeyCode::Right, _) => draft.right(),
            (KeyCode::Home, _) => draft.home(),
            (KeyCode::End, _) => draft.end(),
            _ => {}
        }
        self.scroll.search_for(draft.value.clone());
        self.scroll
            .resolve_search(&self.book, self.revision, self.verbose);
    }

    /// Enter edits an input in place, or starts code and advances to the next
    /// actionable cell. Merely selecting the next cell does not activate it.
    fn activate_or_run(&mut self) {
        let Some(idx) = self.selected_block() else {
            return; // header selected: nothing to activate
        };
        if !self.can_change(idx) {
            return;
        }
        let started = match self.book.blocks.get(idx) {
            Some(BookBlock::Input(_)) => {
                self.activate_selected();
                false
            }
            Some(BookBlock::Code(c)) if c.is_runnable() => self.run_selected(idx),
            _ => false,
        };
        if started {
            self.select_next_cell(idx);
        }
    }

    fn notice(&mut self, message: impl Into<String>) {
        self.flash = Some((message.into(), std::time::Instant::now()));
    }

    fn can_change(&mut self, idx: usize) -> bool {
        if self.sequence.is_some() {
            self.notice("stop run remaining before changing cells");
            return false;
        }
        if self.runs.contains_key(&idx) {
            self.notice("cancel the running cell before changing it");
            return false;
        }
        if let Some(&dependent) = self
            .runs
            .keys()
            .find(|&&run| self.book.depends_on(run, idx))
        {
            self.notice(format!(
                "cell {} is using this prerequisite",
                self.book.cell_label(dependent)
            ));
            return false;
        }
        true
    }

    fn is_blocked(&mut self, idx: usize) -> bool {
        if let Some(prerequisite) = self.book.blocked_by(idx) {
            self.notice(format!(
                "blocked: complete cell {} first",
                self.book.cell_label(prerequisite)
            ));
            return true;
        }
        false
    }

    /// Exclusive sequential ownership: never adopt or overlap manually started runs.
    fn run_remaining(&mut self) {
        if self.sequence.is_some() || !self.runs.is_empty() {
            self.notice("wait for or cancel active runs before running remaining");
            return;
        }
        self.advance_remaining();
    }

    fn advance_remaining(&mut self) {
        if self.exit {
            return;
        }
        let Some(idx) = self.book.next_remaining() else {
            self.notice("all cells complete");
            return;
        };
        self.scroll.select_index(idx + 1, self.selectable_count());
        self.sequence = Some(idx);
        let started = match &self.book.blocks[idx] {
            BookBlock::Input(_) => self.activate_selected(),
            BookBlock::Code(_) => self.run_selected(idx),
            _ => false,
        };
        if !started {
            self.sequence = None;
        }
    }

    /// Skip prose and display-only code, staying put at the end of the runbook.
    fn select_next_cell(&mut self, idx: usize) {
        let next =
            self.book.blocks.iter().enumerate().skip(idx + 1).find_map(
                |(idx, block)| match block {
                    BookBlock::Input(_) => Some(idx),
                    BookBlock::Code(cell) if cell.is_runnable() => Some(idx),
                    _ => None,
                },
            );
        if let Some(next) = next {
            self.scroll.select_index(next + 1, self.selectable_count());
        }
    }

    /// `y`: copy the selected cell to the system clipboard — a code cell's raw body
    /// or a markdown block's source (input cells are not copyable), never the fenced
    /// ```` ``` ```` wrapper. Flashes "copied" on success; a no-op when there's
    /// nothing to copy or no clipboard is available.
    fn copy_selected(&mut self) {
        let Some(idx) = self.selected_block() else {
            return;
        };
        let Some(text) = self.book.copy_text(idx) else {
            return;
        };
        self.copy_to_clipboard(text, "copied");
    }

    /// `Y` (Shift-y): copy the selected code cell's captured output (stdout+stderr) to
    /// the system clipboard. A no-op for markdown/input cells or a cell with no output.
    fn copy_output_selected(&mut self) {
        if self.clipboard.is_none() {
            return;
        }
        let Some(idx) = self.selected_block() else {
            return;
        };
        match self.book.output_text(idx) {
            Ok(Some(text)) => self.copy_to_clipboard(text, "copied output"),
            Ok(None) => {}
            Err(e) => self.set_output_error(idx, format!("copying output: {e}")),
        }
    }

    fn page_output(&mut self, forward: bool) {
        if !self.verbose {
            return;
        }
        let Some(idx) = self.selected_block() else {
            return;
        };
        let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) else {
            return;
        };
        match c.output.window(c.output_page, true) {
            Ok(window) => {
                let last = window.total.saturating_sub(1) / crate::output::PAGE_BYTES;
                let page = c.output_page.unwrap_or(last).min(last);
                c.output_page = if forward {
                    (page + 1 < last).then_some(page + 1)
                } else {
                    Some(page.saturating_sub(1))
                };
                self.scroll.reveal_selected();
                self.revision += 1;
            }
            Err(e) => self.set_output_error(idx, format!("reading output: {e}")),
        }
    }

    fn set_output_error(&mut self, idx: usize, msg: String) {
        if let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) {
            c.error = Some(msg);
        }
        self.revision += 1;
    }

    /// Place `text` on the system clipboard, flashing `label` on success. A no-op when
    /// no clipboard is available.
    fn copy_to_clipboard(&mut self, text: String, label: &str) {
        let Some(clipboard) = self.clipboard.as_mut() else {
            return;
        };
        if clipboard.set_text(text).is_ok() {
            self.flash = Some((label.to_string(), std::time::Instant::now()));
        }
    }

    /// The active footer flash message, if one was set within [`FLASH_DURATION`].
    fn flash_active(&self) -> Option<&str> {
        self.flash
            .as_ref()
            .filter(|(_, set)| set.elapsed() < FLASH_DURATION)
            .map(|(msg, _)| msg.as_str())
    }

    /// `x`: reset the selected cell (code output → un-run, input answer → pending).
    fn clear_selected(&mut self) {
        let Some(idx) = self.selected_block() else {
            return;
        };
        if !self.can_change(idx) {
            return;
        }
        match self.book.blocks.get_mut(idx) {
            Some(BookBlock::Code(c)) => c.clear(),
            Some(BookBlock::Input(i)) => i.clear(),
            _ => return,
        }
        if self.book.last_run == Some(idx) {
            self.book.last_run = None;
        }
        self.revision += 1;
    }

    /// `X`: reset every cell (all code outputs and input answers). Also discards the
    /// temp dir and mints a fresh one, kept visible in the header from the next frame.
    fn clear_all(&mut self) {
        if !self.runs.is_empty() || self.sequence.is_some() {
            self.flash = Some((
                "cancel running cells before resetting".into(),
                std::time::Instant::now(),
            ));
            return;
        }
        self.book.clear_all();
        let _ = self.book.ensure_tmp_dir();
        self.revision += 1;
    }

    /// Enter edit mode on the selected cell, if it is an input cell.
    fn activate_selected(&mut self) -> bool {
        let Some(idx) = self.selected_block() else {
            return false;
        };
        if self.is_blocked(idx) {
            return false;
        }
        if self.book.input_at_mut(idx).is_some() {
            if let Err(error) = self.book.begin_edit_at(idx) {
                self.flash = Some((format!("{error:#}"), std::time::Instant::now()));
                return false;
            }
            self.mode = Mode::Active;
            self.revision += 1;
            // Keep validation errors editable inline, but release sequence ownership
            // so fixing an input cannot silently restart automatic execution.
            return self.sequence != Some(idx)
                || self
                    .book
                    .input_at_mut(idx)
                    .is_some_and(|cell| cell.error().is_none());
        }
        false
    }

    /// Spawn the code cell at `idx`: mark it Running now, build its interpreter +
    /// script + env, and run it off-thread; the result returns via `run_rx`.
    /// Return whether a new run was scheduled, so rejected runs do not advance.
    fn run_selected(&mut self, idx: usize) -> bool {
        if self.runs.contains_key(&idx) {
            return false;
        }
        if self.is_blocked(idx) {
            return false;
        }
        // TMP_DIR must exist before we build the env map that references it.
        if let Err(e) = self.book.ensure_tmp_dir() {
            self.set_cell_error(idx, format!("tmp dir: {e}"));
            return false;
        }

        let (interp, script, mut env) = match self.book.blocks.get(idx) {
            Some(BookBlock::Code(c)) if c.is_runnable() => (
                self.book.interpreter_for(&c.lang),
                self.book.script_for(c),
                self.book.env_for(idx),
            ),
            _ => return false,
        };

        // TUI runs are color-off (we strip SGR on display anyway): hint tools to
        // emit no color at the source so there's less to sanitize. A frontmatter/CLI
        // `NO_COLOR` override still wins. CLI `exec` deliberately won't do this — the
        // real terminal there interprets color (DESIGN §5).
        env.entry("NO_COLOR".to_string())
            .or_insert_with(|| "1".to_string());

        let capture = match crate::output::OutputCapture::create() {
            Ok(capture) => capture,
            Err(e) => {
                self.set_cell_error(idx, format!("creating output spool: {e}"));
                return false;
            }
        };
        if let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) {
            c.begin_run();
            c.output = capture.clone();
        }
        self.book.last_run = Some(idx);
        self.revision += 1;

        let run =
            runner::spawn_captured_run(idx, interp, script, env, self.run_tx.clone(), capture);
        self.runs.insert(idx, run);
        true
    }

    /// Backspace: escalate a cancellation of the selected cell's run, if it's running.
    /// First press sends SIGINT ("canceling…"); a second press while still canceling
    /// escalates to SIGKILL ("killing…"); further presses re-send SIGKILL. The signal
    /// hits the whole process group, so the shell *and* anything it spawned get it. A
    /// no-op if nothing is running there.
    fn cancel_selected(&mut self) {
        // Stop the sequence regardless of where the user has scrolled/selected.
        let current = self.sequence.take();
        if let Some(idx) = current {
            self.scroll.select_index(idx + 1, self.selectable_count());
            self.notice("run remaining stopped");
        }
        let Some(idx) = current.or_else(|| self.selected_block()) else {
            return;
        };
        let Some(run) = self.runs.get(&idx) else {
            return;
        };
        let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) else {
            return;
        };
        match c.cancel {
            Cancel::None => {
                c.cancel = Cancel::Interrupting;
                run.cancel(false);
            }
            Cancel::Interrupting => {
                c.cancel = Cancel::Killing;
                run.cancel(true);
            }
            Cancel::Killing => run.cancel(true),
        }
        self.revision += 1;
    }

    fn set_cell_error(&mut self, idx: usize, msg: String) {
        if let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) {
            c.clear();
            c.error = Some(msg);
            c.state = CodeBlockState::Error;
        }
        self.revision += 1;
    }

    /// Fold a streamed run message back into the document.
    fn apply_run_msg(&mut self, msg: RunMsg) {
        let idx = match &msg {
            RunMsg::Output { idx, .. }
            | RunMsg::Captured { idx }
            | RunMsg::Finished { idx, .. } => *idx,
        };
        // Owners remain installed through Finished, so stale output cannot attach
        // to a cleared cell or trigger another step.
        if !self.runs.contains_key(&idx) {
            return;
        }
        match msg {
            RunMsg::Captured { .. } => {}
            RunMsg::Output { .. } => unreachable!("TUI output is spooled by the runner"),
            RunMsg::Finished {
                idx,
                success,
                code,
                error,
            } => {
                let canceled = matches!(&self.book.blocks[idx], BookBlock::Code(c) if c.cancel != Cancel::None);
                let success = success && !canceled && error.is_none();
                if let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) {
                    if let Some(error) = error {
                        c.error = Some(format!("failed to run: {error}"));
                    }
                    c.finish(success, code);
                }
                self.runs.remove(&idx);
                // Record the latest outcome so the badge can briefly reveal it.
                let state = if success { Status::Done } else { Status::Error };
                self.last_finish = Some((state, std::time::Instant::now()));
                if self.sequence == Some(idx) {
                    self.sequence = None;
                    if self.book.cell_complete(idx) {
                        self.advance_remaining();
                    } else {
                        self.scroll.select_index(idx + 1, self.selectable_count());
                        self.notice(format!(
                            "run remaining stopped at {}",
                            self.book.cell_label(idx)
                        ));
                    }
                }
            }
        }
        // Either way the cell's rendered lines changed; invalidate the cache. The
        // draw loop coalesces many of these into one re-wrap per frame.
        self.revision += 1;
    }

    /// Active-mode keys: route into the focused input cell's draft. Esc cancels,
    /// Enter submits and advances; everything else is dispatched by cell kind.
    fn handle_active_key(&mut self, key: KeyEvent) {
        let Some(idx) = self.selected_block() else {
            self.mode = Mode::Navigate;
            return;
        };
        let Some(cell) = self.book.input_at_mut(idx) else {
            // Selection somehow isn't an input cell; bail back to navigate.
            self.mode = Mode::Navigate;
            return;
        };

        match key.code {
            KeyCode::Esc => {
                cell.cancel();
                self.sequence = None;
                self.mode = Mode::Navigate;
            }
            KeyCode::Enter => {
                if cell.submit().is_ok() {
                    self.mode = Mode::Navigate;
                    if self.sequence.take().is_some() {
                        self.advance_remaining();
                    } else {
                        self.select_next_cell(idx);
                    }
                } else {
                    self.sequence = None;
                }
            }
            code => match &cell.config {
                MagicInputBlock::Confirm { .. } => match code {
                    KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Char('h')
                    | KeyCode::Char('l')
                    | KeyCode::Tab => cell.toggle_confirm(),
                    KeyCode::Char('y') | KeyCode::Char('Y') => cell.set_confirm(true),
                    KeyCode::Char('n') | KeyCode::Char('N') => cell.set_confirm(false),
                    _ => {}
                },
                MagicInputBlock::Select { .. } => match code {
                    KeyCode::Up | KeyCode::Char('k') => cell.select_move(false),
                    KeyCode::Down | KeyCode::Char('j') => cell.select_move(true),
                    _ => {}
                },
                MagicInputBlock::Input { .. } => match code {
                    KeyCode::Char(c) => cell.insert_char(c),
                    KeyCode::Backspace => cell.backspace(),
                    KeyCode::Delete => cell.delete(),
                    KeyCode::Left => cell.cursor_left(),
                    KeyCode::Right => cell.cursor_right(),
                    KeyCode::Home => cell.cursor_home(),
                    KeyCode::End => cell.cursor_end(),
                    _ => {}
                },
            },
        }

        // Any active-mode key may have changed what the cell renders.
        self.revision += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(script: &str) -> App {
        let book = Runbook::new(None::<&str>, &format!("```sh\n{script}\n```\n")).unwrap();
        let mut app = App::new(book);
        app.scroll.select_index(1, 2);
        app
    }

    async fn next(app: &mut App) {
        let msg = tokio::time::timeout(Duration::from_secs(3), app.run_rx.recv())
            .await
            .unwrap()
            .unwrap();
        app.apply_run_msg(msg);
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    #[test]
    fn search_keys_preview_accept_cancel_and_never_execute_cells() {
        let mut app = app("echo qrX界\necho qrX界");
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(70, 12)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        press(&mut app, KeyCode::Char('/'));
        for ch in "qrX界".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(app.scroll.search_count(), "1/2");
        assert!(!app.exit);
        assert!(app.runs.is_empty());
        assert!(app.sequence.is_none());
        press(&mut app, KeyCode::Enter);
        assert!(app.search_edit.is_none());
        assert!(
            app.runs.is_empty(),
            "accepting a search does not run the match"
        );
        press(&mut app, KeyCode::Char('n'));
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(app.scroll.search_count(), "2/2");
        press(&mut app, KeyCode::Char('N'));
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(app.scroll.search_count(), "1/2");

        press(&mut app, KeyCode::Char('/'));
        app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('u'),
            KeyModifiers::CONTROL,
        )));
        press(&mut app, KeyCode::Char('z'));
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(app.scroll.search_count(), "no matches");
        press(&mut app, KeyCode::Esc);
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(app.scroll.search_query(), "qrX界");
        assert_eq!(app.scroll.search_count(), "1/2");
        assert!(!app.exit);
        press(&mut app, KeyCode::Esc);
        assert!(app.scroll.search_query().is_empty());
        assert!(!app.exit);
        press(&mut app, KeyCode::Esc);
        assert!(app.exit);
    }

    #[test]
    fn accepting_search_then_editing_without_a_frame_uses_the_matching_input() {
        let doc = "```json mrthn=input\n{\"type\":\"input\",\"prompt\":\"Unique label\",\"target\":\"NAME\"}\n```";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        // Deliberately do not draw between key events, like a burst of terminal
        // input between timer ticks. Search must resolve selection synchronously.
        press(&mut app, KeyCode::Char('/'));
        for ch in "Unique label".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, Mode::Active);
        assert_eq!(app.selected_block(), Some(0));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 12)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(app.selected_block(), Some(0));
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.book.input_at_mut(0).unwrap().resolved().unwrap().1, "A");
    }

    #[test]
    fn search_editor_handles_unicode_and_tiny_terminals_and_global_quit() {
        let mut app = app("echo test");
        press(&mut app, KeyCode::Char('/'));
        for ch in "界éabc".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.scroll.search_query(), "abc");
        press(&mut app, KeyCode::End);
        for width in [1, 2, 4, 12, 80] {
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 3)).unwrap();
            terminal.draw(|frame| app.draw(frame)).unwrap();
        }
        app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(app.exit);
    }

    #[test]
    fn search_does_not_take_over_an_input_or_running_sequence() {
        let doc = "```json mrthn=input\n{\"type\":\"input\",\"prompt\":\"Name?\",\"target\":\"NAME\"}\n```";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        app.scroll.select_index(1, 2);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('n'));
        assert!(app.search_edit.is_none());
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.book.input_at_mut(0).unwrap().resolved().unwrap().1,
            "/n"
        );
        app.sequence = Some(0);
        press(&mut app, KeyCode::Char('/'));
        assert!(app.search_edit.is_none());
        assert!(app.flash_active().unwrap().contains("stop run remaining"));
    }

    #[tokio::test]
    async fn remaining_waits_for_completion_pauses_for_input_and_reuses_session() {
        let doc = r#"
```sh id=prepare
printf 'west\n' > "$TMP_DIR/options"
printf prepared
```
```json mrthn=input id=region needs=prepare
{"type":"select","prompt":"Region?","target":"REGION","option_file":"$TMP_DIR/options"}
```
```sh id=use needs=prepare,region
printf '%s' "$REGION"
```
"#;
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.sequence, Some(0));
        assert_eq!(app.runs.len(), 1);
        let scratch = app.book.tmp_dir.clone().unwrap();
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.runs.len(),
            1,
            "manual execution cannot overlap the queue"
        );
        app.clear_selected();
        app.clear_all();
        assert_eq!(app.book.tmp_dir.as_ref(), Some(&scratch));
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        assert_eq!(app.sequence, Some(1));
        assert_eq!(app.mode, Mode::Active);
        assert_eq!(app.selected_block(), Some(1));
        assert_eq!(app.book.input_at_mut(1).unwrap().options(), ["west"]);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.sequence, Some(2));
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        assert!(app.sequence.is_none());
        assert_eq!(app.book.output_text(2).unwrap().as_deref(), Some("west"));
        // Pressing r again preserves completed work, answers, and scratch artifacts.
        press(&mut app, KeyCode::Char('r'));
        assert!(app.runs.is_empty());
        assert_eq!(app.book.tmp_dir.as_ref(), Some(&scratch));
        assert_eq!(
            app.book.output_text(0).unwrap().as_deref(),
            Some("prepared")
        );
        assert!(scratch.join("options").exists());
        app.shutdown_runs().await;
        drop(app);
        assert!(!scratch.exists());
    }

    #[tokio::test]
    async fn remaining_stops_on_failure_and_retries_only_unfinished_work() {
        let doc = "```sh\necho once >> \"$TMP_DIR/log\"\n```\n\
                   ```sh\nif [ ! -e \"$TMP_DIR/attempt\" ]; then touch \"$TMP_DIR/attempt\"; exit 9; fi\n```\n\
                   ```sh\ncat \"$TMP_DIR/log\"\n```";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        press(&mut app, KeyCode::Char('r'));
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        assert_eq!(app.selected_block(), Some(1));
        assert!(app.sequence.is_none());
        assert_eq!(app.book.run_counts().succeeded, 1);
        assert!(app.book.output_text(2).unwrap().is_none());
        press(&mut app, KeyCode::Char('r'));
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        assert_eq!(app.book.run_counts().succeeded, 3);
        assert_eq!(app.book.output_text(2).unwrap().as_deref(), Some("once\n"));
        app.shutdown_runs().await;
    }

    #[tokio::test]
    async fn remaining_cannot_adopt_manual_runs_and_cancellation_stops_the_queue() {
        let doc = "```sh\nsleep 10\n```\n```sh\nprintf wrong\n```";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        app.scroll.select_index(1, app.selectable_count());
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('r'));
        assert!(app.sequence.is_none());
        app.scroll.select_index(1, app.selectable_count());
        press(&mut app, KeyCode::Backspace);
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        press(&mut app, KeyCode::Char('r'));
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.selected_block(), Some(0));
        assert!(app.sequence.is_none());
        press(&mut app, KeyCode::Char('r'));
        assert!(
            app.sequence.is_none(),
            "must drain cancellation before restarting"
        );
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        assert!(!app.book.cell_complete(0));
        assert!(app.book.output_text(1).unwrap().is_none());
        app.shutdown_runs().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn canceled_command_exiting_zero_does_not_count_as_complete() {
        let doc = "```sh\ntrap 'exit 0' INT\nprintf ready\nwhile :; do sleep 0.05; done\n```\n```sh\nprintf wrong\n```";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        press(&mut app, KeyCode::Char('r'));
        next(&mut app).await;
        press(&mut app, KeyCode::Backspace);
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        assert!(!app.book.cell_complete(0));
        assert!(app.sequence.is_none());
        assert!(app.book.output_text(1).unwrap().is_none());
        app.shutdown_runs().await;
    }

    #[tokio::test]
    async fn prerequisites_block_downstream_runs_and_active_consumers_protect_ancestors() {
        let doc = "```sh id=prepare\nprintf ready; sleep 10\n```\n\
                   ```sh id=use needs=prepare\nsleep 10\n```\n\
                   ```sh id=independent\nsleep 10\n```";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        app.scroll.select_index(2, app.selectable_count());
        press(&mut app, KeyCode::Enter);
        assert!(app.runs.is_empty());
        assert_eq!(app.selected_block(), Some(1));
        app.scroll.select_index(1, app.selectable_count());
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.runs.len(), 1);
        app.scroll.select_index(3, app.selectable_count());
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.runs.len(), 2, "independent manual runs can overlap");
        app.shutdown_runs().await;

        let doc = "```sh id=prepare\nprintf ready\n```\n```sh needs=prepare\nprintf started; sleep 10\n```";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        app.scroll.select_index(1, app.selectable_count());
        press(&mut app, KeyCode::Enter);
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        press(&mut app, KeyCode::Enter);
        app.scroll.select_index(1, app.selectable_count());
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.runs.len(), 1);
        assert!(app.book.cell_complete(0));
        assert_eq!(app.book.output_text(0).unwrap().as_deref(), Some("ready"));
        app.shutdown_runs().await;
    }

    #[test]
    fn remaining_input_cancel_and_load_errors_never_advance() {
        for config in [
            r#"{"type":"select","prompt":"?","target":"CHOICE","option_file":"$TMP_DIR/empty"}"#,
            r#"{"type":"select","prompt":"?","target":"CHOICE","options":["a"],"option_file":"$TMP_DIR/missing"}"#,
            r#"{"type":"select","prompt":"?","target":"CHOICE","default":"missing","option_file":"$TMP_DIR/choices"}"#,
        ] {
            let doc = format!("```json mrthn=input\n{config}\n```\n```sh\necho wrong\n```");
            let mut app = App::new(Runbook::new(None::<&str>, &doc).unwrap());
            let scratch = app.book.ensure_tmp_dir().unwrap();
            std::fs::write(scratch.join("empty"), "").unwrap();
            std::fs::write(scratch.join("choices"), "available\n").unwrap();
            press(&mut app, KeyCode::Char('r'));
            assert!(app.sequence.is_none());
            assert!(app.runs.is_empty());
            assert_eq!(app.selected_block(), Some(0));
            assert!(app.book.input_at_mut(0).unwrap().resolved().is_none());
            assert_eq!(app.mode, Mode::Active);
            assert!(app.book.input_at_mut(0).unwrap().error().is_some());
            if config.contains("default") {
                press(&mut app, KeyCode::Down);
                press(&mut app, KeyCode::Enter);
                assert!(app.book.cell_complete(0));
                assert_eq!(app.mode, Mode::Navigate);
                assert!(
                    app.runs.is_empty(),
                    "fixing an input must not restart the stopped sequence"
                );
            }
        }
        let doc = "```json mrthn=input\n{\"type\":\"input\",\"prompt\":\"?\",\"target\":\"ANSWER\"}\n```\n```sh\necho wrong\n```";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.sequence, Some(0));
        press(&mut app, KeyCode::Esc);
        assert!(app.sequence.is_none());
        assert_eq!(app.mode, Mode::Navigate);
        assert!(app.runs.is_empty());
        assert!(app.book.input_at_mut(0).unwrap().resolved().is_none());
    }

    #[tokio::test]
    async fn enter_runs_and_advances_through_actionable_cells() {
        let doc = "```sh\nprintf first\n```\n\nProse\n\n\
                   ```sh skip=true\necho skipped\n```\n\n\
                   ```python\nprint('display only')\n```\n\n\
                   ```json mrthn=input\n\
                   {\"type\":\"input\",\"prompt\":\"Name?\",\"target\":\"NAME\"}\n\
                   ```\n\n```sh\nprintf '%s' \"$NAME\"\n```\n\nThe end.\n";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        app.scroll.select_index(1, app.selectable_count());

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_block(), Some(4));
        assert_eq!(app.mode, Mode::Navigate);
        assert_eq!(app.runs.len(), 1);
        assert!(!app.book.input_at_mut(4).unwrap().is_editing());
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        // Completion leaves the next cell selected without running it.
        assert_eq!(app.selected_block(), Some(4));
        assert_eq!(app.book.run_counts().succeeded, 1);

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_block(), Some(4));
        assert_eq!(app.mode, Mode::Active);
        press(&mut app, KeyCode::Char('A'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_block(), Some(5));
        assert_eq!(app.mode, Mode::Navigate);
        assert!(app.runs.is_empty());

        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.selected_block(),
            Some(5),
            "do not move to trailing prose"
        );
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        assert_eq!(app.book.output_text(5).unwrap().as_deref(), Some("A"));
        app.shutdown_runs().await;
    }

    #[test]
    fn input_cancel_stays_put_and_final_input_does_not_wrap() {
        let doc = "```json mrthn=input\n\
                   {\"type\":\"confirm\",\"prompt\":\"Proceed?\",\"target\":\"OK\"}\n\
                   ```\n\nThe end.\n";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        app.scroll.select_index(1, app.selectable_count());
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.selected_block(), Some(0));
        assert!(app.book.input_at_mut(0).unwrap().resolved().is_none());
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_block(), Some(0));
        assert_eq!(app.mode, Mode::Navigate);
        assert_eq!(
            app.book.input_at_mut(0).unwrap().resolved(),
            Some(("OK", "no"))
        );
    }

    #[test]
    fn failed_input_stays_editable_and_reopening_retries_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("choices");
        let config = serde_json::json!({
            "type": "select", "prompt": "Pick", "target": "CHOICE",
            "options": ["inline"], "default": "inline", "option_file": path,
        });
        let doc = format!("```json mrthn=input\n{config}\n```\n\n```sh\necho downstream\n```");
        let mut app = App::new(Runbook::new(Some("book.md"), &doc).unwrap());
        app.scroll.select_index(1, app.selectable_count());
        press(&mut app, KeyCode::Enter);
        let error = app.book.input_at_mut(0).unwrap().error().unwrap();
        assert!(error.contains("book.md:1:1: cell 1"));
        assert!(error.contains("reading option file"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, Mode::Active);
        assert_eq!(app.selected_block(), Some(0));
        assert!(!app.book.env_for(1).contains_key("CHOICE"));
        assert!(app.runs.is_empty());

        press(&mut app, KeyCode::Esc);
        std::fs::write(path, "from-file\n").unwrap();
        press(&mut app, KeyCode::Enter);
        assert!(app.book.input_at_mut(0).unwrap().error().is_none());
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, Mode::Navigate);
        assert_eq!(app.selected_block(), Some(1));
        assert_eq!(
            app.book.env_for(1).get("CHOICE").map(String::as_str),
            Some("from-file")
        );
    }

    #[test]
    fn invalid_generated_default_requires_an_explicit_selection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("choices");
        std::fs::write(&path, "available\n").unwrap();
        let config = serde_json::json!({
            "type": "select", "prompt": "Pick", "target": "CHOICE",
            "default": "missing", "option_file": path,
        });
        let doc = format!("```json mrthn=input\n{config}\n```");
        let mut app = App::new(Runbook::new(None::<&str>, &doc).unwrap());
        app.scroll.select_index(1, app.selectable_count());
        press(&mut app, KeyCode::Enter);
        assert!(
            app.book
                .input_at_mut(0)
                .unwrap()
                .error()
                .unwrap()
                .contains("invalid default")
        );
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, Mode::Active);
        assert!(app.book.input_at_mut(0).unwrap().resolved().is_none());
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, Mode::Navigate);
        assert_eq!(
            app.book.input_at_mut(0).unwrap().resolved(),
            Some(("CHOICE", "available"))
        );
    }

    #[tokio::test]
    async fn rejected_rerun_does_not_advance() {
        let doc = "```sh\nsleep 10\n```\n\n```sh\nprintf second\n```";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        app.scroll.select_index(1, app.selectable_count());
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_block(), Some(1));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_block(), Some(0));
        assert_eq!(app.runs.len(), 1);
        app.shutdown_runs().await;
    }

    #[test]
    fn failed_run_setup_does_not_advance() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("not-a-directory");
        std::fs::write(&file, "occupied").unwrap();
        let doc = "```sh\nprintf first\n```\n\n```sh\nprintf second\n```";
        let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
        app.book.frontmatter.tmp_dir = Some(crate::book::TmpDirConf {
            path: Some(file),
            ..Default::default()
        });
        app.scroll.select_index(1, app.selectable_count());
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected_block(), Some(0));
        assert!(app.runs.is_empty());
        assert_eq!(app.book.run_counts().errored, 1);
    }

    #[tokio::test]
    async fn duplicate_enter_runs_once_and_reset_preserves_active_scratch() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join("runs");
        let mut app = app("echo start >> \"$LOG\"; printf ready; sleep 10");
        app.book
            .cli_env
            .insert("LOG".into(), log.display().to_string());
        app.activate_or_run();
        app.activate_or_run();
        assert_eq!(app.runs.len(), 1);
        next(&mut app).await;
        assert_eq!(std::fs::read_to_string(log).unwrap(), "start\n");
        let tmp = app.book.tmp_dir.clone().unwrap();
        app.clear_selected();
        app.clear_all();
        assert_eq!(app.book.tmp_dir.as_ref(), Some(&tmp));
        assert!(tmp.exists());
        assert_eq!(app.book.run_counts().running, 1);
        app.shutdown_runs().await;
        drop(app);
        assert!(!tmp.exists());
    }

    #[tokio::test]
    async fn completed_cell_can_run_again_without_mixed_output() {
        let mut app = app("printf done");
        for _ in 0..2 {
            app.activate_or_run();
            while !app.runs.is_empty() {
                next(&mut app).await;
            }
            assert_eq!(app.book.output_text(0).unwrap().as_deref(), Some("done"));
            assert_eq!(app.book.run_counts().succeeded, 1);
        }
        app.shutdown_runs().await;
    }

    #[tokio::test]
    async fn quit_waits_for_process_group_cleanup() {
        let dir = tempfile::TempDir::new().unwrap();
        let marker = dir.path().join("survived");
        let release = dir.path().join("release");
        let mut app = app(
            "printf ready; while [ ! -e \"$RELEASE\" ]; do sleep 0.05; done; touch \"$MARKER\"",
        );
        app.book
            .cli_env
            .insert("MARKER".into(), marker.display().to_string());
        app.book
            .cli_env
            .insert("RELEASE".into(), release.display().to_string());
        app.activate_or_run();
        next(&mut app).await;
        app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::NONE,
        )));
        assert!(app.exit);
        app.shutdown_runs().await;
        std::fs::write(release, "go").unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn cancel_before_spawn_and_ctrl_c_in_edit_mode() {
        let mut app = app("sleep 10");
        app.activate_or_run();
        app.cancel_selected();
        app.cancel_selected();
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        assert_eq!(app.book.run_counts().errored, 1);
        app.mode = Mode::Active;
        app.show_help = true;
        app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(app.exit);
        app.shutdown_runs().await;
    }

    fn capture(app: &App) -> crate::output::OutputCapture {
        let BookBlock::Code(c) = &app.book.blocks[0] else {
            panic!("code");
        };
        c.output.clone()
    }

    #[tokio::test]
    async fn large_output_is_paged_and_spools_are_removed_on_rerun_clear_and_reset() {
        let mut app = app("awk 'BEGIN { for(i=0;i<20000;i++) print i }'");
        let mut previous = Vec::<std::path::PathBuf>::new();
        for _ in 0..2 {
            app.activate_or_run();
            assert!(previous.iter().all(|p| !p.exists()));
            while !app.runs.is_empty() {
                next(&mut app).await;
            }
            let output = capture(&app);
            previous = output.paths();
            assert!(previous.iter().all(|p| p.exists()));
            assert!(
                output
                    .window(None, false)
                    .unwrap()
                    .text
                    .ends_with("19999\n")
            );
            assert!(output.window(None, true).unwrap().start > 0);
            app.verbose = true;
            while !matches!(&app.book.blocks[0], BookBlock::Code(c) if c.output_page == Some(0)) {
                app.page_output(false);
            }
            let BookBlock::Code(c) = &app.book.blocks[0] else {
                panic!();
            };
            assert!(
                c.output
                    .window(c.output_page, true)
                    .unwrap()
                    .text
                    .starts_with("0\n")
            );
            assert_eq!(
                app.book.output_text(0).unwrap().unwrap().lines().count(),
                20000
            );
        }
        app.clear_selected();
        assert!(previous.iter().all(|p| !p.exists()));
        app.activate_or_run();
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        let paths = capture(&app).paths();
        app.clear_all();
        assert!(paths.iter().all(|p| !p.exists()));
        app.shutdown_runs().await;
    }

    #[tokio::test]
    async fn spool_write_failure_stops_run_and_keeps_diagnostic_out_of_output() {
        for remaining in [false, true] {
            let dir = tempfile::TempDir::new().unwrap();
            let release = dir.path().join("release");
            let doc = "```sh\nprintf ready; while [ ! -e \"$RELEASE\" ]; do sleep 0.01; done; while :; do printf more; done\n```\n```sh\nprintf wrong\n```";
            let mut app = App::new(Runbook::new(None::<&str>, doc).unwrap());
            app.scroll.select_index(1, app.selectable_count());
            app.book
                .cli_env
                .insert("RELEASE".into(), release.display().to_string());
            if remaining {
                app.run_remaining();
            } else {
                app.activate_or_run();
            }
            next(&mut app).await;
            let capture = capture(&app);
            assert_eq!(capture.text().unwrap(), "ready");
            capture.fail_writes();
            std::fs::write(release, "go").unwrap();
            while !app.runs.is_empty() {
                next(&mut app).await;
            }
            let BookBlock::Code(c) = &app.book.blocks[0] else {
                panic!();
            };
            assert_eq!(c.state, CodeBlockState::Error);
            assert!(c.error.as_deref().unwrap().contains("writing output spool"));
            assert_eq!(capture.window(None, false).unwrap().text, "ready");
            assert!(app.book.output_text(0).is_err());
            assert!(app.sequence.is_none());
            assert!(
                matches!(&app.book.blocks[1], BookBlock::Code(c) if c.state == CodeBlockState::NotRun)
            );
            app.shutdown_runs().await;
        }
    }

    #[tokio::test]
    async fn cancellation_preserves_partial_capture_until_clear_and_quit_cleans_up() {
        let mut app = app("printf partial; sleep 10");
        app.activate_or_run();
        next(&mut app).await;
        let paths = capture(&app).paths();
        app.cancel_selected();
        app.cancel_selected();
        while !app.runs.is_empty() {
            next(&mut app).await;
        }
        assert_eq!(app.book.output_text(0).unwrap().as_deref(), Some("partial"));
        assert!(paths.iter().all(|p| p.exists()));
        app.clear_selected();
        assert!(paths.iter().all(|p| !p.exists()));
        app.activate_or_run();
        next(&mut app).await;
        let paths = capture(&app).paths();
        app.shutdown_runs().await;
        drop(app);
        assert!(paths.iter().all(|p| !p.exists()));
    }
}
