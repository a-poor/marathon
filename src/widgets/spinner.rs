use ratatui::{text::Span, widgets::Widget};

static FPS: u128 = 15;

pub struct SpinnerWidget {
    chars: Vec<char>,
    start: Option<std::time::Instant>,
}

impl SpinnerWidget {
    pub fn new(start: Option<std::time::Instant>) -> Self {
        Self {
            chars: vec!['⣷', '⣯', '⣟', '⡿', '⢿', '⣻', '⣽', '⣾'],
            start,
        }
    }

    /// The braille frame for the current instant, or a space if no start time is
    /// set. Public so the footer can embed the spinner inside its status badge
    /// rather than rendering it as a standalone widget.
    pub fn current(&self) -> char {
        let dur = if let Some(s) = self.start {
            s.elapsed()
        } else {
            return ' ';
        };
        let frame_ms = 1000 / FPS; // ms per frame at FPS frames/sec
        let i = (dur.as_millis() / frame_ms) as usize % self.chars.len();
        self.chars[i]
    }
}

impl Widget for SpinnerWidget {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer) {
        Span::raw(format!("{}", self.current())).render(area, buf);
    }
}
