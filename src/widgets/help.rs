use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Widget};

/// A centered "Hotkeys" modal listing the key bindings, dismissed with `Esc`.
///
/// Rendered over the whole frame: it clears a centered box and draws a bordered
/// panel on top, so it floats above the document. It's drawn every frame while open
/// (cheap), like the footer — no document invalidation involved.
pub struct HelpModal;

/// A keybinding section: a heading and its `(keys, description)` rows.
struct Section {
    heading: &'static str,
    rows: &'static [(&'static str, &'static str)],
}

const SECTIONS: &[Section] = &[
    Section {
        heading: "Navigate",
        rows: &[
            ("j / k   ↓ / ↑", "move selection"),
            ("g / G", "first / last cell"),
            ("Ctrl-d / Ctrl-u", "half-page down / up"),
            ("Enter", "run cell · edit input"),
            ("Backspace", "cancel run (×2 to kill)"),
            ("Ctrl-o", "expand / collapse output"),
            ("y", "copy cell to clipboard"),
            ("Y", "copy cell output to clipboard"),
            ("x / X", "clear cell / clear all"),
            ("?", "toggle this help"),
            ("q   Esc   Ctrl-c", "quit"),
        ],
    },
    Section {
        heading: "Editing an input",
        rows: &[
            ("Enter", "submit"),
            ("Esc", "cancel"),
            ("← / →   y / n   ▲ / ▼", "adjust value"),
        ],
    },
];

impl HelpModal {
    /// Width to pad the key column to, so descriptions align in a second column.
    fn key_col_width() -> usize {
        SECTIONS
            .iter()
            .flat_map(|s| s.rows.iter())
            .map(|(keys, _)| keys.chars().count())
            .max()
            .unwrap_or(0)
    }

    /// The body lines: each section's heading then its padded key→description rows,
    /// blank-line separated, with a closing hint.
    fn lines() -> Vec<Line<'static>> {
        let key_w = Self::key_col_width();
        let mut lines = Vec::new();

        for (i, section) in SECTIONS.iter().enumerate() {
            if i > 0 {
                lines.push(Line::default());
            }
            lines.push(Line::from(section.heading.bold().underlined()));
            for (keys, desc) in section.rows {
                let pad = " ".repeat(key_w - keys.chars().count());
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    keys.cyan(),
                    Span::raw(format!("{pad}  ")),
                    Span::raw(*desc),
                ]));
            }
        }

        lines.push(Line::default());
        lines.push(Line::from("esc to close").dim().italic().centered());
        lines
    }
}

impl Widget for HelpModal {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        let lines = Self::lines();

        // Size the panel to its content (plus border + padding), clamped to the
        // frame, then center it.
        let inner_w = lines
            .iter()
            .map(Line::width)
            .max()
            .unwrap_or(0)
            .max("Hotkeys".len()) as u16;
        let w = (inner_w + 4).min(area.width); // 2 border + 2 padding
        let h = (lines.len() as u16 + 2).min(area.height); // 2 border
        let x = area.x + (area.width.saturating_sub(w)) / 2;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let popup = Rect::new(x, y, w, h);

        Clear.render(popup, buf);

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().cyan())
            .padding(Padding::horizontal(1))
            .title(Line::from(" Hotkeys ").bold().centered());
        let inner = block.inner(popup);
        block.render(popup, buf);

        for (i, line) in lines.into_iter().enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.bottom() {
                break;
            }
            buf.set_line(inner.x, y, &line, inner.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn renders_keys_centered_without_panic() {
        let mut term = Terminal::new(TestBackend::new(60, 24)).unwrap();
        term.draw(|f| f.render_widget(HelpModal, f.area())).unwrap();
        let dump = format!("{:?}", term.backend().buffer());
        assert!(dump.contains("Hotkeys"), "title missing");
        assert!(dump.contains("move selection"), "a binding row missing");
        assert!(dump.contains("esc to close"), "dismiss hint missing");
    }

    #[test]
    fn fits_in_a_small_viewport() {
        // Smaller than the panel's natural size: it clamps instead of panicking.
        let mut term = Terminal::new(TestBackend::new(20, 6)).unwrap();
        term.draw(|f| f.render_widget(HelpModal, f.area())).unwrap();
    }
}
