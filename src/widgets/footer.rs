use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::widgets::spinner::SpinnerWidget;

/// The bottom status bar: a run-state badge (left), context key hints (center),
/// and `N/M complete` run progress (right).
///
/// The footer is rebuilt and redrawn every frame (unlike the cached document), so
/// anything live — the spinner animation here — is free. The spinner lives *inside*
/// the badge, to the left of the text, and only animates while a cell is running;
/// otherwise the badge shows a static glyph for its state.
pub struct FooterWidget<'a> {
    start: Option<std::time::Instant>,
    status: Status,
    /// `(finished, runnable)` — rendered as `N/M complete`.
    progress: (usize, usize),
    /// Context-sensitive key hints for the current mode.
    hints: Line<'a>,
}

impl<'a> FooterWidget<'a> {
    pub fn new(start: std::time::Instant) -> Self {
        Self {
            start: Some(start),
            status: Status::Ready,
            progress: (0, 0),
            hints: Line::default(),
        }
    }

    pub fn status(mut self, status: Status) -> Self {
        self.status = status;
        self
    }

    pub fn progress(mut self, finished: usize, runnable: usize) -> Self {
        self.progress = (finished, runnable);
        self
    }

    pub fn hints(mut self, hints: Line<'a>) -> Self {
        self.hints = hints;
        self
    }

    /// The badge glyph: the live spinner frame while running, else a static glyph
    /// for the state.
    fn glyph(&self) -> char {
        match self.status {
            Status::Running => SpinnerWidget::new(self.start).current(),
            Status::Ready => '◦',
            Status::Done => '✔',
            Status::Error => '✗',
        }
    }
}

impl Widget for FooterWidget<'_> {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer) {
        // Blank the whole bar first.
        buf.set_style(area, Style::new().bg(Color::Black));

        let badge_text = format!(" {} {} ", self.glyph(), self.status.as_str());
        let badge_w = badge_text.chars().count() as u16;

        let (finished, runnable) = self.progress;
        let prog_text = if runnable > 0 {
            format!("{finished}/{runnable} complete ")
        } else {
            String::new()
        };
        let prog_w = prog_text.chars().count() as u16;

        let [badge, mid, right] = Layout::horizontal([
            Constraint::Length(badge_w),
            Constraint::Min(0),
            Constraint::Length(prog_w),
        ])
        .areas(area);

        Span::styled(
            badge_text,
            Style::new()
                .bold()
                .fg(self.status.fg())
                .bg(self.status.bg()),
        )
        .render(badge, buf);

        // Hints centered in the open middle; progress right-aligned.
        self.hints.centered().dim().render(mid, buf);
        Line::from(prog_text).dim().render(right, buf);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    #[default]
    Ready,
    Running,
    Done,
    Error,
}

impl Status {
    fn as_str(&self) -> &'static str {
        match self {
            Status::Ready => "ready",
            Status::Running => "running",
            Status::Done => "done",
            Status::Error => "error",
        }
    }

    fn fg(&self) -> Color {
        Color::Black
    }

    fn bg(&self) -> Color {
        match self {
            Status::Ready => Color::Blue,
            Status::Running => Color::Yellow,
            Status::Done => Color::Green,
            Status::Error => Color::Red,
        }
    }
}
