use std::time::Duration;

use anyhow::Result;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers, MouseEventKind},
    layout::{Constraint, Layout},
    style::Stylize,
    text::{Line, Span},
};
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::{
    book::{BookBlock, CodeBlockState, MagicInputBlock, Runbook},
    runner::{self, RunMsg},
    widgets::{
        footer::{FooterWidget, Status},
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
    /// Bumped whenever block contents change (e.g. a cell runs), to invalidate
    /// the document's wrapped-line cache.
    revision: u64,
    /// Height of the document viewport at the last draw, for page scrolling.
    viewport_h: u16,
    exit: bool,
    start: std::time::Instant,
    /// Channel for finished cell runs, drained as a `select!` arm in [`App::run`].
    run_tx: mpsc::UnboundedSender<RunMsg>,
    run_rx: mpsc::UnboundedReceiver<RunMsg>,
}

impl App {
    pub fn new(book: Runbook) -> Self {
        let (run_tx, run_rx) = mpsc::unbounded_channel();
        Self {
            book,
            scroll: ScrollState::new(),
            mode: Mode::Navigate,
            revision: 0,
            viewport_h: 0,
            exit: false,
            start: std::time::Instant::now(),
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
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        self.viewport_h = body.height;

        let path = self
            .book
            .path
            .as_ref()
            .and_then(|p| p.to_str())
            .unwrap_or("(untitled)");
        let title = Line::from(vec![
            " marathon ".bold().on_blue(),
            Span::raw(format!(" {path}")),
        ]);
        frame.render_widget(title, header);

        frame.render_stateful_widget(
            DocumentView::new(&self.book, self.revision).active(self.mode == Mode::Active),
            body,
            &mut self.scroll,
        );

        frame.render_widget(self.footer(), footer);
    }

    /// Build the footer for this frame: an aggregate run-state badge, mode-aware
    /// key hints, and `N/M complete` progress, all derived fresh from the book.
    fn footer(&self) -> FooterWidget<'static> {
        let counts = self.book.run_counts();
        let status = if counts.running > 0 {
            Status::Running
        } else if counts.errored > 0 {
            Status::Error
        } else if counts.succeeded > 0 {
            Status::Done
        } else {
            Status::Ready
        };

        let hints = match self.mode {
            Mode::Navigate => Line::from("j/k move · enter run · q quit"),
            Mode::Active => Line::from("enter submit · esc cancel · ←/→ edit"),
        };

        FooterWidget::new(self.start)
            .status(status)
            .progress(counts.finished(), counts.runnable)
            .hints(hints)
    }

    /// Translate a single terminal event into a state change. Infallible now;
    /// returns nothing because the loop owns the draw/error path.
    fn handle_event(&mut self, event: &Event) {
        if let Some(key) = event.as_key_press_event() {
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

    /// Navigation-mode keys: move the selection, scroll, quit, or activate the
    /// selected input cell.
    fn handle_navigate_key(&mut self, key: KeyEvent) {
        let len = self.book.blocks.len();
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
            (KeyCode::Enter, _) | (KeyCode::Char('r'), _) => self.activate_or_run(),
            _ => {}
        }
    }

    /// Enter on the selected cell: edit it if it's an input cell, run it if it's a
    /// runnable code cell, otherwise nothing.
    fn activate_or_run(&mut self) {
        let idx = self.scroll.selected();
        match self.book.blocks.get(idx) {
            Some(BookBlock::Input(_)) => self.activate_selected(),
            Some(BookBlock::Code(c)) if c.is_runnable() => self.run_selected(idx),
            _ => {}
        }
    }

    /// Enter edit mode on the selected cell, if it is an input cell.
    fn activate_selected(&mut self) {
        let idx = self.scroll.selected();
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

        let (interp, script, env) = match self.book.blocks.get(idx) {
            Some(BookBlock::Code(c)) if c.is_runnable() => (
                self.book.interpreter_for(&c.lang),
                self.book.script_for(c),
                self.book.env_for(idx),
            ),
            _ => return,
        };

        if let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) {
            c.begin_run();
        }
        self.book.last_run = Some(idx);
        self.revision += 1;

        let tx = self.run_tx.clone();
        tokio::spawn(runner::run_streaming(idx, interp, script, env, tx));
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
            RunMsg::Output { idx, chunk } => {
                if let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) {
                    c.push_output(&chunk);
                }
            }
            RunMsg::Finished { idx, success, code } => {
                if let Some(BookBlock::Code(c)) = self.book.blocks.get_mut(idx) {
                    c.finish(success, code);
                }
            }
        }
        // Either way the cell's rendered lines changed; invalidate the cache. The
        // draw loop coalesces many of these into one re-wrap per frame.
        self.revision += 1;
    }

    /// Active-mode keys: route into the focused input cell's draft. Esc cancels,
    /// Enter submits; everything else is dispatched by cell kind.
    fn handle_active_key(&mut self, key: KeyEvent) {
        let idx = self.scroll.selected();
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
