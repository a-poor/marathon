use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers, MouseEventKind},
    layout::{Constraint, Layout},
    style::{Color, Style, Stylize},
    text::{Line, Span},
};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::{
    book::{BookBlock, Cancel, CodeBlockState, MagicInputBlock, Runbook},
    runner::{self, RunMsg},
    widgets::{
        footer::{FooterWidget, Status},
        help::HelpModal,
        scrollview::{DocumentView, ScrollState},
    },
};

/// Interaction mode. `Navigate` moves the per-cell selection through the
/// document; `Active` routes keys into the focused input cell's draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Navigate,
    Active,
}

/// Signal a running cell's process group (negated pid → the whole group the child
/// leads), interrupting the shell and anything it spawned. `hard` escalates from
/// SIGINT (graceful) to SIGKILL. Unix only; a no-op elsewhere (the runner doesn't
/// create a process group off-unix anyway).
#[cfg(unix)]
fn cancel_pid(pid: u32, hard: bool) {
    let sig = if hard { libc::SIGKILL } else { libc::SIGINT };
    // SAFETY: a bare `kill(2)` syscall; an invalid/stale pid just returns ESRCH.
    unsafe {
        libc::kill(-(pid as i32), sig);
    }
}

#[cfg(not(unix))]
fn cancel_pid(_pid: u32, _hard: bool) {}

/// The interactive runbook viewer.
///
/// Holds the parsed [`Runbook`] and drives the draw/event loop. For now it is a
/// read-only viewer: the document renders as one scrollable, wrapped markdown
/// page (via [`DocumentView`]) and the user moves a per-cell selection through
/// it. Executing the selected cell is the next step.
pub struct App {
    book: Runbook,
    scroll: ScrollState,
    /// Navigate vs. actively editing the focused input cell.
    mode: Mode,
    /// Whether the hotkeys help modal is open (overlays any mode).
    show_help: bool,
    /// Whether cell outputs are expanded to full length (vs. the truncated tail).
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
    /// OS process ids of currently-running cells, by block index — populated on
    /// [`RunMsg::Started`], cleared on [`RunMsg::Finished`]. Lets backspace signal a
    /// run to cancel it.
    running_pids: HashMap<usize, u32>,
    /// The system clipboard handle, opened once at startup (held alive so the
    /// clipboard persists on platforms that serve it from the owning process, e.g.
    /// X11). `None` if the platform has no clipboard available.
    clipboard: Option<arboard::Clipboard>,
    /// Channel for finished cell runs, drained as a `select!` arm in [`App::run`].
    run_tx: mpsc::UnboundedSender<RunMsg>,
    run_rx: mpsc::UnboundedReceiver<RunMsg>,
}

/// How long a footer flash message stays visible.
const FLASH_DURATION: Duration = Duration::from_millis(1500);

/// How long the badge reveals a cell's just-finished state before reverting to idle.
const FINISH_BADGE_TIMEOUT: Duration = Duration::from_secs(5);

impl App {
    pub fn new(book: Runbook) -> Self {
        let (run_tx, run_rx) = mpsc::unbounded_channel();
        Self {
            book,
            scroll: ScrollState::new(),
            mode: Mode::Navigate,
            show_help: false,
            verbose: false,
            revision: 0,
            viewport_h: 0,
            exit: false,
            start: std::time::Instant::now(),
            flash: None,
            last_finish: None,
            running_pids: HashMap::new(),
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

        while !self.exit {
            tokio::select! {
                _ = frames.tick() => {
                    terminal.draw(|frame| self.draw(frame))?;
                }
                Some(Ok(event)) = events.next() => {
                    self.handle_event(&event);
                }
                Some(msg) = self.run_rx.recv() => {
                    self.apply_run_msg(msg);
                }
            }
        }
        Ok(())
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

        frame.render_widget(self.footer(), footer);

        // The help modal floats over everything when open.
        if self.show_help {
            frame.render_widget(HelpModal, frame.area());
        }
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
            Mode::Navigate => Line::from("↑/↓ move • ↵ run • q quit • ? help"),
            Mode::Active => Line::from("↵ submit • esc cancel • ←/→ edit"),
        };

        let mut footer = FooterWidget::new(self.start)
            .status(status)
            .counts(self.counts_line())
            .hints(hints);

        // A transient flash (e.g. "copied") takes over the center while it's active.
        if let Some(msg) = self.flash_active() {
            footer = footer.flash(Line::from(format!("✓ {msg}")).green().bold());
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
            // The help modal is a global overlay: while open it swallows keys and is
            // dismissed with Esc (or `?`), so the underlying mode never sees them.
            if self.show_help {
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                    self.show_help = false;
                }
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
            (KeyCode::Backspace, _) => self.cancel_selected(),
            (KeyCode::Char('y'), _) => self.copy_selected(),
            (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                self.verbose = !self.verbose;
                self.revision += 1;
            }
            (KeyCode::Char('Y'), _) => self.copy_output_selected(),
            (KeyCode::Char('x'), _) => self.clear_selected(),
            (KeyCode::Char('X'), _) => self.clear_all(),
            (KeyCode::Char('?'), _) => self.show_help = true,
            _ => {}
        }
    }

    /// Enter on the selected cell: edit it if it's an input cell, run it if it's a
    /// runnable code cell, otherwise nothing.
    fn activate_or_run(&mut self) {
        let Some(idx) = self.selected_block() else {
            return; // header selected: nothing to activate
        };
        match self.book.blocks.get(idx) {
            Some(BookBlock::Input(_)) => self.activate_selected(),
            Some(BookBlock::Code(c)) if c.is_runnable() => self.run_selected(idx),
            _ => {}
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
        let Some(idx) = self.selected_block() else {
            return;
        };
        let Some(text) = self.book.output_text(idx) else {
            return;
        };
        // Copy the cleaned output (no ANSI/control litter), matching what's shown.
        self.copy_to_clipboard(crate::ansi::sanitize(&text), "copied output");
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
        self.book.clear_all();
        let _ = self.book.ensure_tmp_dir();
        self.revision += 1;
    }

    /// Enter edit mode on the selected cell, if it is an input cell.
    fn activate_selected(&mut self) {
        let Some(idx) = self.selected_block() else {
            return;
        };
        if let Some(cell) = self.book.input_at_mut(idx) {
            cell.begin_edit();
            self.mode = Mode::Active;
            self.revision += 1;
        }
    }

    /// Spawn the code cell at `idx`: mark it Running now, build its interpreter +
    /// script + env, and run it off-thread; the result returns via `run_rx`.
    fn run_selected(&mut self, idx: usize) {
        // TMP_DIR must exist before we build the env map that references it.
        if let Err(e) = self.book.ensure_tmp_dir() {
            self.set_cell_error(idx, format!("tmp dir: {e}"));
            return;
        }

        let (interp, script, mut env) = match self.book.blocks.get(idx) {
            Some(BookBlock::Code(c)) if c.is_runnable() => (
                self.book.interpreter_for(&c.lang),
                self.book.script_for(c),
                self.book.env_for(idx),
            ),
            _ => return,
        };

        // TUI runs are color-off (we strip SGR on display anyway): hint tools to
        // emit no color at the source so there's less to sanitize. A frontmatter/CLI
        // `NO_COLOR` override still wins. CLI `exec` deliberately won't do this — the
        // real terminal there interprets color (DESIGN §7).
        env.entry("NO_COLOR".to_string())
            .or_insert_with(|| "1".to_string());

        if let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) {
            c.begin_run();
        }
        self.book.last_run = Some(idx);
        self.revision += 1;

        let tx = self.run_tx.clone();
        tokio::spawn(runner::run_streaming(idx, interp, script, env, tx));
    }

    /// Backspace: escalate a cancellation of the selected cell's run, if it's running.
    /// First press sends SIGINT ("canceling…"); a second press while still canceling
    /// escalates to SIGKILL ("killing…"); further presses re-send SIGKILL. The signal
    /// hits the whole process group, so the shell *and* anything it spawned get it. A
    /// no-op if nothing is running there.
    fn cancel_selected(&mut self) {
        let Some(idx) = self.selected_block() else {
            return;
        };
        let Some(&pid) = self.running_pids.get(&idx) else {
            return;
        };
        let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) else {
            return;
        };
        match c.cancel {
            Cancel::None => {
                c.cancel = Cancel::Interrupting;
                cancel_pid(pid, false);
            }
            Cancel::Interrupting => {
                c.cancel = Cancel::Killing;
                cancel_pid(pid, true);
            }
            Cancel::Killing => cancel_pid(pid, true),
        }
        self.revision += 1;
    }

    fn set_cell_error(&mut self, idx: usize, msg: String) {
        if let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) {
            c.output = msg;
            c.state = CodeBlockState::Error;
        }
        self.revision += 1;
    }

    /// Fold a streamed run message back into the document.
    fn apply_run_msg(&mut self, msg: RunMsg) {
        match msg {
            RunMsg::Started { idx, pid } => {
                // Track the pid so backspace can signal this run; nothing to redraw.
                self.running_pids.insert(idx, pid);
                return;
            }
            RunMsg::Output { idx, chunk } => {
                if let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) {
                    c.push_output(&chunk);
                }
            }
            RunMsg::Finished { idx, success, code } => {
                if let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) {
                    c.finish(success, code);
                }
                self.running_pids.remove(&idx);
                // Record the latest outcome so the badge can briefly reveal it.
                let state = if success { Status::Done } else { Status::Error };
                self.last_finish = Some((state, std::time::Instant::now()));
            }
        }
        // Either way the cell's rendered lines changed; invalidate the cache. The
        // draw loop coalesces many of these into one re-wrap per frame.
        self.revision += 1;
    }

    /// Active-mode keys: route into the focused input cell's draft. Esc cancels,
    /// Enter submits; everything else is dispatched by cell kind.
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
                self.mode = Mode::Navigate;
            }
            KeyCode::Enter => {
                cell.submit();
                self.mode = Mode::Navigate;
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
