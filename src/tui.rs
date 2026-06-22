use std::time::Duration;

use anyhow::Result;
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{Event, EventStream, KeyCode, KeyModifiers, MouseEventKind},
    layout::{Constraint, Layout},
    style::Stylize,
    text::{Line, Span},
};
use tokio_stream::StreamExt;

use crate::{
    book::Runbook,
    widgets::{
        footer::FooterWidget,
        scrollview::{DocumentView, ScrollState},
    },
};

/// The interactive runbook viewer.
///
/// Holds the parsed [`Runbook`] and drives the draw/event loop. For now it is a
/// read-only viewer: the document renders as one scrollable, wrapped markdown
/// page (via [`DocumentView`]) and the user moves a per-cell selection through
/// it. Executing the selected cell is the next step.
pub struct App {
    book: Runbook,
    scroll: ScrollState,
    /// Bumped whenever block contents change (e.g. a cell runs), to invalidate
    /// the document's wrapped-line cache.
    revision: u64,
    /// Height of the document viewport at the last draw, for page scrolling.
    viewport_h: u16,
    exit: bool,
    start: std::time::Instant,
}

impl App {
    pub fn new(book: Runbook) -> Self {
        Self {
            book,
            scroll: ScrollState::new(),
            revision: 0,
            viewport_h: 0,
            exit: false,
            start: std::time::Instant::now(),
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
            DocumentView::new(&self.book, self.revision),
            body,
            &mut self.scroll,
        );

        frame.render_widget(FooterWidget::new(self.start), footer);
    }

    /// Translate a single terminal event into a state change. Infallible now;
    /// returns nothing because the loop owns the draw/error path.
    fn handle_event(&mut self, event: &Event) {
        let len = self.book.blocks.len();
        let page = (self.viewport_h / 2).max(1);

        if let Some(key) = event.as_key_press_event() {
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
                _ => {}
            }
        } else if let Event::Mouse(m) = event {
            // Only delivered when mouse capture is enabled; harmless otherwise.
            match m.kind {
                MouseEventKind::ScrollDown => self.scroll.scroll_down(3),
                MouseEventKind::ScrollUp => self.scroll.scroll_up(3),
                _ => {}
            }
        }
    }
}
