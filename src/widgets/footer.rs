use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::Widget;

use crate::widgets::spinner::SpinnerWidget;

#[derive(Debug, Default)]
pub struct FooterWidget {
    pub start: Option<std::time::Instant>,
    pub status: Status,
}

impl FooterWidget {
    pub fn new(start: std::time::Instant) -> Self {
        Self {
            start: Some(start),
            ..Default::default()
        }
    }
}

impl Widget for FooterWidget {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer) {
        let [status, _rest, spin] =
            Layout::horizontal([Constraint::Max(9), Constraint::Min(0), Constraint::Min(1)])
                .areas(area);

        // Render the rest of the bar as blank
        buf.set_style(area, Style::new().bg(Color::Black));

        // Render the status badge
        Span::styled(
            format!(" {} ", self.status.as_str()), // 1 space L + 1 space R, both colored
            Style::new()
                .bold()
                .fg(self.status.fg())
                .bg(self.status.bg()),
        )
        .render(status, buf);

        SpinnerWidget::new(self.start).render(spin, buf);
    }
}

#[derive(Debug, Default)]
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
        match self {
            Status::Ready => Color::Black,
            Status::Running => Color::Black,
            Status::Done => Color::Black,
            Status::Error => Color::Black,
        }
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
