//! Scrollable runbook document view.
//!
//! Flattens the whole runbook into one `Vec<Line>` (prose via [`render_md`], code
//! and input cells rendered here) plus a per-block line-range map, then draws a
//! line-level scrolling window with a full-width highlight bar over the selected
//! block. Unlike a `List`, scrolling is by line — so a cell taller than the
//! viewport scrolls *within* the viewport rather than being clipped.
//!
//! The flattened lines are cached and only rebuilt when the width or a revision
//! counter changes (the 30fps spinner redraw must not rewrap every frame).

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::StatefulWidget;

use crate::book::{BookBlock, CodeBlock, CodeBlockState, MagicInputBlock, Runbook};
use crate::widgets::markdown::render_md;
use crate::widgets::wrap::hard_break;

/// Persisted scroll/selection state for the document view.
#[derive(Default)]
pub struct ScrollState {
    /// Index of the selected block (cell).
    selected: usize,
    /// Top visible line of the flattened document.
    offset: u16,
    /// Selection at the previous render — used to auto-scroll only when the
    /// selection *changes*, leaving free (wheel) scrolling alone otherwise.
    last_selected: Option<usize>,
    /// Wrapped-document cache, keyed on width + revision.
    cache: Option<Cache>,
}

struct Cache {
    width: u16,
    revision: u64,
    lines: Vec<Line<'static>>,
    ranges: Vec<Range<usize>>,
}

impl ScrollState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn select_next(&mut self, len: usize) {
        if len > 0 {
            self.selected = (self.selected + 1).min(len - 1);
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self, len: usize) {
        self.selected = len.saturating_sub(1);
    }

    /// Free line scroll (e.g. mouse wheel). Clamped to content at draw time.
    pub fn scroll_down(&mut self, n: u16) {
        self.offset = self.offset.saturating_add(n);
    }

    pub fn scroll_up(&mut self, n: u16) {
        self.offset = self.offset.saturating_sub(n);
    }

    fn ensure_cache(&mut self, book: &Runbook, width: u16, revision: u64) {
        let stale = match &self.cache {
            Some(c) => c.width != width || c.revision != revision,
            None => true,
        };
        if stale {
            let (lines, ranges) = build_document(book, width);
            self.cache = Some(Cache {
                width,
                revision,
                lines,
                ranges,
            });
        }
    }
}

/// The document widget: borrows the runbook for the duration of a render.
pub struct DocumentView<'a> {
    book: &'a Runbook,
    revision: u64,
}

impl<'a> DocumentView<'a> {
    pub fn new(book: &'a Runbook, revision: u64) -> Self {
        Self { book, revision }
    }
}

impl StatefulWidget for DocumentView<'_> {
    type State = ScrollState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut ScrollState) {
        if area.is_empty() {
            return;
        }
        state.ensure_cache(self.book, area.width, self.revision);

        let h = area.height as usize;
        let (total, range, selected) = {
            let cache = state.cache.as_ref().expect("cache populated above");
            if cache.ranges.is_empty() {
                return;
            }
            let selected = state.selected.min(cache.ranges.len() - 1);
            (cache.lines.len(), cache.ranges[selected].clone(), selected)
        };

        // Scroll the selected block into view, but only when the selection just
        // changed — so wheel scrolling isn't fought every frame.
        let changed = state.last_selected != Some(selected);
        let mut off = state.offset as usize;
        if changed {
            if range.start < off {
                off = range.start;
            } else if range.end > off + h {
                off = if range.end - range.start >= h {
                    range.start // taller than viewport: pin its top
                } else {
                    range.end - h // short: scroll just enough to reveal its end
                };
            }
        }
        // Clamp to content.
        off = off.min(total.saturating_sub(h));
        state.offset = off as u16;
        state.last_selected = Some(selected);

        // Draw the visible window line-by-line, painting a full-width highlight
        // bar over rows belonging to the selected block.
        let cache = state.cache.as_ref().expect("cache populated above");
        let hl = Style::new().bg(Color::DarkGray);
        for i in 0..h {
            let idx = off + i;
            if idx >= total {
                break;
            }
            let y = area.y + i as u16;
            buf.set_line(area.x, y, &cache.lines[idx], area.width);
            if range.contains(&idx) {
                buf.set_style(Rect::new(area.x, y, area.width, 1), hl);
            }
        }
    }
}

/// Flatten every block into wrapped lines plus a block→line-range map. Each
/// block's range includes a trailing blank line so cells breathe and the
/// highlight bar has a gap between them.
fn build_document(book: &Runbook, width: u16) -> (Vec<Line<'static>>, Vec<Range<usize>>) {
    let w = width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut ranges: Vec<Range<usize>> = Vec::new();

    for block in &book.blocks {
        let start = lines.len();
        match block {
            BookBlock::Md(node) => lines.extend(render_md(node, w)),
            BookBlock::Code(c) => lines.extend(code_lines(c, w)),
            BookBlock::Input(i) => lines.extend(input_lines(i, w)),
        }
        lines.push(Line::default());
        ranges.push(start..lines.len());
    }

    (lines, ranges)
}

/// Render a runnable code cell: a fenced header, the (char-wrapped) body indented
/// two columns, and a status line.
fn code_lines(c: &CodeBlock, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    let mut head = vec![format!("```{}", c.lang).cyan()];
    if c.meta.skip.unwrap_or(false) {
        head.push(Span::raw(" "));
        head.push("skip".italic().yellow());
    }
    lines.push(Line::from(head));

    let avail = width.saturating_sub(2).max(1);
    for line in c.content.lines() {
        for chunk in hard_break(vec![Span::raw(line.to_string())], avail) {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(chunk);
            lines.push(Line::from(spans).green());
        }
    }

    lines.push(status_line(&c.state));
    lines
}

fn input_lines(i: &MagicInputBlock, width: usize) -> Vec<Line<'static>> {
    let (kind, prompt, target) = match i {
        MagicInputBlock::Confirm { prompt, target } => ("confirm", prompt, target),
        MagicInputBlock::Input { prompt, target } => ("input", prompt, target),
        MagicInputBlock::Select { prompt, target, .. } => ("select", prompt, target),
    };
    let spans = vec![
        format!("[{kind}] ").magenta().bold(),
        Span::raw(prompt.clone()),
        format!("  → ${target}").dim(),
    ];
    crate::widgets::wrap::wrap(&spans, width)
}

fn status_line(state: &CodeBlockState) -> Line<'static> {
    match state {
        CodeBlockState::NotRun => Line::from("  ◦ not run").dim(),
        CodeBlockState::Running => Line::from("  ● running…").yellow(),
        CodeBlockState::Success(_) => Line::from("  ✔ ok").green(),
        CodeBlockState::Error(_) => Line::from("  ✗ error").red(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::wrap::display_width;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const SRC: &str = "---\ntitle: t\n---\n\n# A heading that is somewhat long here\n\n\
This is a long paragraph of prose that should wrap across several lines when \
rendered into a narrow viewport, instead of being truncated at the edge.\n\n\
```sh\necho \"a fairly long shell command that also needs to be wrapped somehow\"\n```\n";

    fn book() -> Runbook {
        Runbook::new(None::<&str>, SRC).unwrap()
    }

    fn line_text(l: &Line<'static>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn every_line_fits_the_width() {
        let book = book();
        let (lines, _) = build_document(&book, 20);
        for l in &lines {
            assert!(
                display_width(&line_text(l)) <= 20,
                "line too wide: {:?}",
                line_text(l)
            );
        }
    }

    #[test]
    fn long_content_wraps_to_multiple_lines() {
        let book = book();
        let (lines, ranges) = build_document(&book, 20);
        // 3 blocks (heading, paragraph, code) but many more lines once wrapped.
        assert_eq!(ranges.len(), 3);
        assert!(
            lines.len() > ranges.len() * 2,
            "expected wrapping to expand the document, got {} lines",
            lines.len()
        );
        // Ranges are contiguous and cover the whole document.
        assert_eq!(ranges[0].start, 0);
        assert_eq!(ranges.last().unwrap().end, lines.len());
    }

    #[test]
    fn renders_into_a_test_backend_without_panic() {
        let book = book();
        let mut term = Terminal::new(TestBackend::new(24, 12)).unwrap();
        let mut state = ScrollState::new();
        state.select_next(3); // move selection so the highlight path runs
        term.draw(|f| {
            f.render_stateful_widget(DocumentView::new(&book, 0), f.area(), &mut state);
        })
        .unwrap();
        // Heading text should appear somewhere in the rendered buffer.
        let dump = format!("{:?}", term.backend().buffer());
        assert!(dump.contains("heading"), "heading not rendered");
    }
}
