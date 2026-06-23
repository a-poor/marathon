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
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::StatefulWidget;

use crate::book::{BookBlock, CodeBlock, CodeBlockState, Draft, InputCell, InputState, Runbook};
use crate::widgets::markdown::render_md;
use crate::widgets::wrap::{hard_break, wrap};

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
    /// Whether an input cell is being actively edited — tints the highlight bar.
    active: bool,
}

impl<'a> DocumentView<'a> {
    pub fn new(book: &'a Runbook, revision: u64) -> Self {
        Self {
            book,
            revision,
            active: false,
        }
    }

    /// Mark that the selected cell is being edited (changes the highlight tint).
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
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
        // A genuinely dark gray. `Color::DarkGray` is ANSI bright-black, which many
        // terminals render *light*; an indexed 256-color gives a real dark bar.
        // While editing an input cell, tint the bar a dark blue to signal focus.
        let hl = if self.active {
            Style::new().bg(Color::Indexed(17))
        } else {
            Style::new().bg(Color::Indexed(237))
        };
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

/// Flatten every block into wrapped lines plus a block→line-range map. A blank
/// spacer line is emitted between blocks but kept *outside* the range, so cells
/// breathe and the highlight bar stops at the content.
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
        // The block's selectable/highlighted range is its content only; the
        // trailing spacer sits *outside* the range so the highlight bar doesn't
        // extend into the gap between cells.
        ranges.push(start..lines.len());
        lines.push(Line::default());
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
    lines.extend(output_lines(&c.state, width));
    lines
}

/// Maximum source lines of cell output shown inline. Fuller output (and a verbose
/// toggle) is deferred polish — see TODO.md.
const OUTPUT_MAX_LINES: usize = 10;

/// Render a finished cell's captured output, indented and dimmed, capped at
/// [`OUTPUT_MAX_LINES`] with a "… N more lines" marker when truncated.
fn output_lines(state: &CodeBlockState, width: usize) -> Vec<Line<'static>> {
    let out = match state {
        CodeBlockState::Success(o) | CodeBlockState::Error(o) => o,
        CodeBlockState::NotRun | CodeBlockState::Running => return Vec::new(),
    };
    if out.trim().is_empty() {
        return Vec::new();
    }

    let avail = width.saturating_sub(2).max(1);
    let all: Vec<&str> = out.lines().collect();
    let shown = all.len().min(OUTPUT_MAX_LINES);
    let mut lines = Vec::new();
    for line in &all[..shown] {
        for chunk in hard_break(vec![Span::raw(line.to_string())], avail) {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(chunk);
            lines.push(Line::from(spans).dim());
        }
    }
    if all.len() > shown {
        lines.push(
            Line::from(format!("  … {} more lines", all.len() - shown))
                .dim()
                .italic(),
        );
    }
    lines
}

/// Render an input cell: a prompt header, then a body that reflects the cell's
/// kind and lifecycle state (pending / editing / answered).
fn input_lines(cell: &InputCell, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Header: `[kind] prompt  → $target`.
    let header = vec![
        format!("[{}] ", cell.kind()).magenta().bold(),
        Span::raw(cell.prompt().to_string()),
        format!("  → ${}", cell.target()).dim(),
    ];
    lines.extend(wrap(&header, width));

    // Body. The answered state is uniform across kinds; otherwise dispatch on
    // the cell kind and (for editing) its draft.
    match &cell.state {
        InputState::Answered { value } => {
            lines.push(Line::from(vec![
                Span::raw("  "),
                "✔ ".green(),
                Span::styled(value.clone(), Style::new().fg(Color::Green)),
            ]));
        }
        InputState::Pending => lines.extend(pending_body(cell)),
        InputState::Editing { draft, .. } => lines.extend(editing_body(cell, draft)),
    }

    lines
}

/// A subdued preview of the controls before the cell is activated.
fn pending_body(cell: &InputCell) -> Vec<Line<'static>> {
    match cell.config {
        crate::book::MagicInputBlock::Confirm { .. } => {
            vec![Line::from("  ◦ Yes / No  (enter to answer)").dim()]
        }
        crate::book::MagicInputBlock::Input { .. } => {
            vec![Line::from("  ◦ (enter to answer)").dim()]
        }
        crate::book::MagicInputBlock::Select { .. } => {
            let mut out = Vec::new();
            for opt in cell.options() {
                out.push(Line::from(format!("    {opt}")).dim());
            }
            if out.is_empty() {
                out.push(Line::from("  ◦ (no options)").dim());
            }
            out
        }
    }
}

/// The interactive controls while the cell is focused.
fn editing_body(cell: &InputCell, draft: &Draft) -> Vec<Line<'static>> {
    match draft {
        Draft::Confirm(yes) => vec![confirm_line(*yes)],
        Draft::Select(idx) => select_lines(cell, *idx),
        Draft::Text(t) => vec![text_field_line(&t.value, Some(t.cursor))],
    }
}

/// `[ Yes ]   No ` with the chosen option reversed/bold.
fn confirm_line(yes: bool) -> Line<'static> {
    let on = Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD);
    let off = Style::new().dim();
    let (yes_style, no_style) = if yes { (on, off) } else { (off, on) };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(" Yes ", yes_style),
        Span::raw("  "),
        Span::styled(" No ", no_style),
    ])
}

/// The option list with a `▶` marker and bold on the highlighted row.
fn select_lines(cell: &InputCell, idx: usize) -> Vec<Line<'static>> {
    let opts = cell.options();
    if opts.is_empty() {
        return vec![Line::from("  ◦ (no options)").dim()];
    }
    opts.iter()
        .enumerate()
        .map(|(i, opt)| {
            if i == idx {
                Line::from(vec![
                    "  ▶ ".cyan(),
                    Span::styled(opt.clone(), Style::new().add_modifier(Modifier::BOLD)),
                ])
            } else {
                Line::from(format!("    {opt}")).dim()
            }
        })
        .collect()
}

/// A single-line text field with a synthetic block caret (the scrollview's flat
/// line model makes the real terminal cursor impractical to place here).
fn text_field_line(value: &str, cursor: Option<usize>) -> Line<'static> {
    let mut spans = vec![Span::raw("  "), "❯ ".cyan()];
    match cursor {
        None if value.is_empty() => spans.push("(empty)".dim()),
        None => spans.push(Span::raw(value.to_string())),
        Some(cur) => {
            let caret = Style::new().add_modifier(Modifier::REVERSED);
            let chars: Vec<char> = value.chars().collect();
            let before: String = chars[..cur.min(chars.len())].iter().collect();
            if !before.is_empty() {
                spans.push(Span::raw(before));
            }
            match chars.get(cur) {
                Some(c) => spans.push(Span::styled(c.to_string(), caret)),
                None => spans.push(Span::styled(" ", caret)),
            }
            let after: String = chars
                .get(cur + 1..)
                .map(|s| s.iter().collect())
                .unwrap_or_default();
            if !after.is_empty() {
                spans.push(Span::raw(after));
            }
        }
    }
    Line::from(spans)
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
        // First range starts at the top; the last ends just before the trailing
        // spacer line (spacers live outside the highlighted ranges).
        assert_eq!(ranges[0].start, 0);
        assert_eq!(ranges.last().unwrap().end, lines.len() - 1);
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

    use crate::book::{InputCell, MagicInputBlock};

    fn confirm_cell() -> InputCell {
        InputCell::new(MagicInputBlock::Confirm {
            prompt: "Proceed?".into(),
            target: "OK".into(),
        })
    }

    fn select_cell() -> InputCell {
        InputCell::new(MagicInputBlock::Select {
            prompt: "Pick".into(),
            target: "CHOICE".into(),
            options: Some(vec!["alpha".into(), "beta".into()]),
            option_file: None,
        })
    }

    #[test]
    fn input_header_shows_kind_prompt_and_target() {
        let lines = input_lines(&confirm_cell(), 80);
        let head = line_text(&lines[0]);
        assert!(head.contains("[confirm]"), "got: {head}");
        assert!(head.contains("Proceed?"), "got: {head}");
        assert!(head.contains("→ $OK"), "got: {head}");
    }

    #[test]
    fn confirm_editing_marks_chosen_option() {
        let mut c = confirm_cell();
        c.begin_edit();
        c.set_confirm(true);
        let lines = input_lines(&c, 80);
        // The "Yes" span carries the reversed+bold "chosen" style.
        let yes = lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.content.contains("Yes"))
            .unwrap();
        assert!(yes.style.add_modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn select_editing_marks_current_row() {
        let mut c = select_cell();
        c.begin_edit();
        c.select_move(true); // highlight "beta"
        let lines = input_lines(&c, 80);
        let marked = lines
            .iter()
            .map(line_text)
            .find(|t| t.contains('▶'))
            .unwrap();
        assert!(
            marked.contains("beta"),
            "marker not on current row: {marked}"
        );
    }

    #[test]
    fn answered_cell_shows_value() {
        let mut c = confirm_cell();
        c.begin_edit();
        c.set_confirm(true);
        c.submit();
        let lines = input_lines(&c, 80);
        assert!(
            lines.iter().any(|l| line_text(l).contains("yes")),
            "answered value not shown"
        );
    }
}
