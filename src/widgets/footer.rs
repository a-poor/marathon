use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::widgets::spinner::SpinnerWidget;

/// The bottom status bar: a run-state badge (left), run counts (center), and context
/// key hints (right).
///
/// The footer is rebuilt and redrawn every frame (unlike the cached document), so
/// anything live — the spinner animation here, the badge's timed reveal — is free.
/// The spinner lives *inside* the badge, to the left of the text, and only animates
/// while a cell is running; otherwise the badge shows a static glyph for its state.
pub struct FooterWidget<'a> {
    start: Option<std::time::Instant>,
    status: Status,
    /// Run-state counts (pending / succeeded / errored), pre-styled by the caller.
    counts: Line<'a>,
    /// Context-sensitive key hints for the current mode.
    hints: Line<'a>,
    /// A transient status flash (e.g. "copied") shown in the center, over the counts,
    /// while it's active.
    flash: Option<Line<'a>>,
}

impl<'a> FooterWidget<'a> {
    pub fn new(start: std::time::Instant) -> Self {
        Self {
            start: Some(start),
            status: Status::Ready,
            counts: Line::default(),
            hints: Line::default(),
            flash: None,
        }
    }

    pub fn status(mut self, status: Status) -> Self {
        self.status = status;
        self
    }

    pub fn counts(mut self, counts: Line<'a>) -> Self {
        self.counts = counts;
        self
    }

    pub fn hints(mut self, hints: Line<'a>) -> Self {
        self.hints = hints;
        self
    }

    /// Set a transient flash message, shown centered over the counts while active.
    pub fn flash(mut self, flash: Line<'a>) -> Self {
        self.flash = Some(flash);
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
        // Reserve a *fixed* badge cell (sized to the widest status) so the centered
        // counts don't shift as the status text changes width. The badge itself
        // renders left-aligned within it; the rest of the cell stays blank.
        let badge_w = Status::badge_width();

        // Hints sit flush right, with a one-column margin from the edge.
        let hints_w = self.hints.width() as u16 + 1;

        let [badge, mid, right] = Layout::horizontal([
            Constraint::Length(badge_w),
            Constraint::Min(0),
            Constraint::Length(hints_w),
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

        // Center shows a transient flash if set, else the run counts.
        match self.flash {
            Some(flash) => flash.centered().render(mid, buf),
            None => self.counts.centered().render(mid, buf),
        }
        // Key hints, right-aligned.
        self.hints.right_aligned().dim().render(right, buf);
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

    /// The fixed badge-cell width: the widest status text plus the glyph and the
    /// three surrounding spaces (`" <glyph> running "`). Fixing this keeps the
    /// centered counts from shifting as the status changes.
    fn badge_width() -> u16 {
        let widest = [Status::Ready, Status::Running, Status::Done, Status::Error]
            .iter()
            .map(|s| s.as_str().chars().count())
            .max()
            .unwrap_or(0);
        // " {glyph} {text} " — one space, the 1-col glyph, a space, the text, a space.
        widest as u16 + 4
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(footer: FooterWidget<'_>) -> String {
        let mut term = Terminal::new(TestBackend::new(60, 1)).unwrap();
        term.draw(|f| f.render_widget(footer, f.area())).unwrap();
        format!("{:?}", term.backend().buffer())
    }

    #[test]
    fn flash_takes_over_the_center_from_counts() {
        let start = std::time::Instant::now();

        // Without a flash, the center shows the counts; hints sit on the right.
        let plain = render(
            FooterWidget::new(start)
                .counts(Line::from("COUNTS"))
                .hints(Line::from("HINTS")),
        );
        assert!(plain.contains("COUNTS"), "counts not shown: {plain}");
        assert!(plain.contains("HINTS"), "hints not shown: {plain}");

        // With a flash, it replaces the counts; the hints stay put on the right.
        let flashed = render(
            FooterWidget::new(start)
                .counts(Line::from("COUNTS"))
                .hints(Line::from("HINTS"))
                .flash(Line::from("copied")),
        );
        assert!(flashed.contains("copied"), "flash not shown: {flashed}");
        assert!(
            !flashed.contains("COUNTS"),
            "counts should be hidden while flashing: {flashed}"
        );
        assert!(flashed.contains("HINTS"), "hints stay visible: {flashed}");
    }
}
