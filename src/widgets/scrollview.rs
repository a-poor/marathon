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

use crate::book::{
    BookBlock, Cancel, CodeBlock, CodeBlockState, Draft, InputCell, InputState, Runbook,
};
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
    /// The selected block's end line at the previous render — used to tail-follow a
    /// running cell as its output grows (without yanking the view if you scrolled up).
    last_selected_end: Option<usize>,
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

    /// Jump the selection to a specific block index, clamped to content.
    pub fn select_index(&mut self, idx: usize, len: usize) {
        if len > 0 {
            self.selected = idx.min(len - 1);
        }
    }

    /// Free line scroll (e.g. mouse wheel). Clamped to content at draw time.
    pub fn scroll_down(&mut self, n: u16) {
        self.offset = self.offset.saturating_add(n);
    }

    pub fn scroll_up(&mut self, n: u16) {
        self.offset = self.offset.saturating_sub(n);
    }

    fn ensure_cache(&mut self, book: &Runbook, width: u16, revision: u64, verbose: bool) {
        let stale = match &self.cache {
            Some(c) => c.width != width || c.revision != revision,
            None => true,
        };
        if stale {
            let (lines, ranges) = build_document(book, width, verbose);
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
    /// When set, cell outputs render in full instead of the truncated tail.
    verbose: bool,
}

impl<'a> DocumentView<'a> {
    pub fn new(book: &'a Runbook, revision: u64) -> Self {
        Self {
            book,
            revision,
            active: false,
            verbose: false,
        }
    }

    /// Mark that the selected cell is being edited (changes the highlight tint).
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Expand cell outputs to their full length (vs. the default truncated tail).
    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }
}

impl StatefulWidget for DocumentView<'_> {
    type State = ScrollState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut ScrollState) {
        if area.is_empty() {
            return;
        }
        state.ensure_cache(self.book, area.width, self.revision, self.verbose);

        let h = area.height as usize;
        let (total, range, selected) = {
            let cache = state.cache.as_ref().expect("cache populated above");
            if cache.ranges.is_empty() {
                return;
            }
            let selected = state.selected.min(cache.ranges.len() - 1);
            (cache.lines.len(), cache.ranges[selected].clone(), selected)
        };

        // Is the selected cell the one we last ran? Only then do we tail-follow its
        // growing output. It's the only cell whose output can change, so its end
        // grows by its own output — never by a block above shifting it down. We key
        // on last-run rather than "still running" so a fast command that finishes
        // between frames (its last output + the finish landing together) is still
        // followed all the way to its tail. (Selection index 0 is the header, so a
        // block's index `b` sits at selection `b + 1`.)
        let following_run = self.book.last_run.map(|b| b + 1) == Some(selected);

        let changed = state.last_selected != Some(selected);
        let mut off = state.offset as usize;
        if changed {
            // Selection just moved: scroll the block into view. (Wheel scrolling is
            // left alone otherwise.)
            if range.start < off {
                off = range.start;
            } else if range.end > off + h {
                off = if range.end - range.start >= h {
                    range.start // taller than viewport: pin its top
                } else {
                    range.end - h // short: scroll just enough to reveal its end
                };
            }
        } else if following_run && let Some(prev_end) = state.last_selected_end {
            // Tail-follow: the run cell's output is growing. If its bottom was on
            // screen last frame, keep it pinned in view so the latest output stays
            // visible; if you scrolled up to read earlier output, don't yank it back.
            let was_following = prev_end <= off + h;
            if was_following && range.end > off + h {
                off = range.end - h;
            }
        }
        // Clamp to content.
        off = off.min(total.saturating_sub(h));
        state.offset = off as u16;
        state.last_selected = Some(selected);
        state.last_selected_end = Some(range.end);

        // Draw the visible window line-by-line. The selected block gets a full-width
        // highlight bar — except the header (block 0), whose selection is shown by
        // recoloring its box border blue instead.
        let cache = state.cache.as_ref().expect("cache populated above");
        // A genuinely dark gray. `Color::DarkGray` is ANSI bright-black, which many
        // terminals render *light*; an indexed 256-color gives a real dark bar.
        // While editing an input cell, tint the bar a soft, muted indigo to signal
        // focus — a saturated palette blue (e.g. `Indexed(17)`) reads too harsh.
        let hl = if self.active {
            Style::new().bg(Color::Rgb(43, 47, 79))
        } else {
            Style::new().bg(Color::Indexed(237))
        };
        let is_header = selected == 0;
        let blue = Style::new().fg(Color::Blue);
        for i in 0..h {
            let idx = off + i;
            if idx >= total {
                break;
            }
            let y = area.y + i as u16;
            buf.set_line(area.x, y, &cache.lines[idx], area.width);
            if !range.contains(&idx) {
                continue;
            }
            if is_header {
                // Recolor the box border blue (top/bottom edges full-width; the side
                // edges are just the first and last columns).
                if idx == range.start || idx + 1 == range.end {
                    buf.set_style(Rect::new(area.x, y, area.width, 1), blue);
                } else {
                    buf.set_style(Rect::new(area.x, y, 1, 1), blue);
                    if area.width > 0 {
                        buf.set_style(Rect::new(area.x + area.width - 1, y, 1, 1), blue);
                    }
                }
            } else {
                buf.set_style(Rect::new(area.x, y, area.width, 1), hl);
            }
        }
    }
}

/// Flatten every block into wrapped lines plus a block→line-range map. A blank
/// spacer line is emitted between blocks but kept *outside* the range, so cells
/// breathe and the highlight bar stops at the content.
fn build_document(
    book: &Runbook,
    width: u16,
    verbose: bool,
) -> (Vec<Line<'static>>, Vec<Range<usize>>) {
    let w = width as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut ranges: Vec<Range<usize>> = Vec::new();

    // The runbook header banner is selectable block index 0 (so you can scroll all
    // the way back up to it). It scrolls with the document — not sticky. Its range
    // covers the box; the trailing spacer sits outside, like the cells below.
    let header = header_lines(book, w);
    lines.extend(header);
    ranges.push(0..lines.len());
    lines.push(Line::default());

    for block in &book.blocks {
        let start = lines.len();
        match block {
            BookBlock::Md(node) => lines.extend(render_md(node, w)),
            BookBlock::Code(c) => lines.extend(code_lines(c, w, verbose)),
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

/// The idle border color — a quiet gray (matching the un-run gutter). When the header
/// is selected, the draw loop overpaints the box border [`Color::Blue`] instead.
const HEADER_BORDER: Color = Color::Indexed(240);

/// The runbook header banner: a full box (border all around, `marathon` in the top
/// edge) framing the runner ASCII art on the left and, on the right, the `marathon`
/// wordmark stacked above the title / path / description / env fields. Selectable
/// block 0; scrolls with the document.
fn header_lines(book: &Runbook, width: usize) -> Vec<Line<'static>> {
    let art = runner_art();
    let art_w = art.iter().map(|l| l.chars().count()).max().unwrap_or(0);

    // Right column: the big wordmark, a blank line, then the frontmatter fields.
    let wordmark = wordmark_lines();
    let wordmark_w = wordmark.iter().map(Line::width).max().unwrap_or(0);
    let mut right = wordmark;
    right.push(Line::default());
    right.extend(header_fields(book));

    // Drop the runner on narrow terminals so it can't crowd out the wordmark: it's
    // shown only when the box has room for both (plus borders, the column gap, and a
    // small margin). Below that, just the wordmark + fields.
    let show_art = width >= art_w + wordmark_w + 7;
    let gap = if show_art { 3 } else { 0 };

    let border = Style::new().fg(HEADER_BORDER);
    let mut lines = vec![box_top("marathon", width, border)];

    let rows = if show_art {
        art.len().max(right.len())
    } else {
        right.len()
    };
    for i in 0..rows {
        // Left border, the (optional) art column padded to its width and a gap, then
        // the right column (wordmark / fields).
        let mut inner = vec![Span::styled("│", border), Span::raw(" ")];
        if show_art {
            inner.push(Span::styled(
                format!("{:<art_w$}", art.get(i).map(String::as_str).unwrap_or("")),
                Style::new().fg(Color::Cyan),
            ));
            inner.push(Span::raw(" ".repeat(gap)));
        }
        if let Some(line) = right.get(i) {
            inner.extend(line.spans.iter().cloned());
        }
        // Fit the inner content to width-1 columns, then cap with the right border.
        let mut spans = fit_to(inner, width.saturating_sub(1));
        spans.push(Span::styled("│", border));
        lines.push(Line::from(spans));
    }

    lines.push(box_bottom(width, border));
    lines
}

/// The title / path / description / env field lines (no box), each optional field
/// shown only when present.
fn header_fields(book: &Runbook) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut field = |label: &str, value: &str| {
        lines.push(Line::from(vec![
            format!("{label}: ").bold(),
            Span::raw(value.to_string()),
        ]));
    };

    if let Some(title) = book.frontmatter.title.as_deref().filter(|s| !s.is_empty()) {
        field("Title", title);
    }
    if let Some(path) = book.path.as_ref().and_then(|p| p.to_str()) {
        field("Path", path);
    }
    if let Some(desc) = book
        .frontmatter
        .description
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        field("Desc", desc);
    }

    let env = book.base_env();
    let tmp = book.tmp_dir_env();
    if !env.is_empty() || tmp.is_some() {
        lines.push(Line::from("Env:".bold()));
        for (k, v) in &env {
            lines.push(Line::from(vec![
                Span::raw("  "),
                format!("{k}: ").cyan(),
                Span::raw(v.clone()),
            ]));
        }
        // The temp-dir var is created lazily (after the first run), so it's listed
        // here only once it exists — separately from `base_env`, which excludes it.
        if let Some((k, v)) = tmp {
            lines.push(Line::from(vec![
                Span::raw("  "),
                format!("{k}: ").cyan(),
                Span::raw(v),
            ]));
        }
    }
    lines
}

/// Top box edge with a label embedded near the left: `┌─ marathon ─────────┐`.
fn box_top(label: &str, width: usize, border: Style) -> Line<'static> {
    let head = format!("┌─ {label} ");
    let used = head.chars().count();
    let trail = width.saturating_sub(used + 1); // +1 for the closing corner
    Line::from(vec![
        Span::styled(head, border),
        Span::styled("─".repeat(trail), border),
        Span::styled("┐", border),
    ])
}

/// Bottom box edge: `└──────────────────┘`.
fn box_bottom(width: usize, border: Style) -> Line<'static> {
    let mid = "─".repeat(width.saturating_sub(2));
    Line::from(Span::styled(format!("└{mid}┘"), border))
}

/// Truncate or pad a row of spans to exactly `n` display columns (ASCII/box glyphs
/// are width 1), so each boxed line lands its right border in the same column.
fn fit_to(spans: Vec<Span<'static>>, n: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for s in spans {
        if used >= n {
            break;
        }
        let w = s.content.chars().count();
        if used + w <= n {
            used += w;
            out.push(s);
        } else {
            let take = n - used;
            let truncated: String = s.content.chars().take(take).collect();
            out.push(Span::styled(truncated, s.style));
            used = n;
        }
    }
    if used < n {
        out.push(Span::raw(" ".repeat(n - used)));
    }
    out
}

/// The big `marathon` wordmark for the header (block-glyph art), styled bold. Leading
/// whitespace is preserved (it shapes the glyphs); only blank edge lines are trimmed.
fn wordmark_lines() -> Vec<Line<'static>> {
    const TEXT: &str = r#"
                         ▗▖
                     ▐▌  ▐▌
▐█▙█▖ ▟██▖ █▟█▌ ▟██▖▐███ ▐▙██▖ ▟█▙ ▐▙██▖
▐▌█▐▌ ▘▄▟▌ █▘   ▘▄▟▌ ▐▌  ▐▛ ▐▌▐▛ ▜▌▐▛ ▐▌
▐▌█▐▌▗█▀▜▌ █   ▗█▀▜▌ ▐▌  ▐▌ ▐▌▐▌ ▐▌▐▌ ▐▌
▐▌█▐▌▐▙▄█▌ █   ▐▙▄█▌ ▐▙▄ ▐▌ ▐▌▝█▄█▘▐▌ ▐▌
▝▘▀▝▘ ▀▀▝▘ ▀    ▀▀▝▘  ▀▀ ▝▘ ▝▘ ▝▀▘ ▝▘ ▝▘
"#;
    let raw: Vec<&str> = TEXT.lines().collect();
    let start = raw.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
    let end = raw
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    raw[start..end]
        .iter()
        .map(|l| Line::from(l.trim_end().to_string().bold()))
        .collect()
}

/// The runner ASCII art for the header, with surrounding blank lines stripped and the
/// common left indent removed.
fn runner_art() -> Vec<String> {
    const ART: &str = r#"
                        kfv,
                       c'>C|8
                       p0&C;\
                      }hndkM
                 j>{]MX@!wL8W
              fY]iCmjWCiZ[1*I
             Xkv   CW'Mxo[^t
             t&>   !v}j*X<M;<]O@)&
             +^    .i>Yn%b'0~},_
             Ud   }+Wa:~b
             1jb ~mCBlU&1
                 [Iz^]uzh
                 Z(}l+8#mqZ
      ,I8        +X'u]~Xn"_C
    {j~`f~_ai    \~ZWI`>Uj~X{
         QC;t^adcI]`@O  n&;Cp(8
             qaqJm{#       C`mB
                 c+        *+Cf
                           vBzt/
                             [r)
                              .!~
                              "Yok,
                               C]koM<
"#;
    let raw: Vec<&str> = ART.lines().collect();
    // Trim fully-blank leading/trailing lines.
    let start = raw.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
    let end = raw
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    let body = &raw[start..end];
    // Remove the common left indent.
    let indent = body
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    body.iter()
        .map(|l| {
            let dedented = if l.len() >= indent { &l[indent..] } else { l };
            dedented.trim_end().to_string()
        })
        .collect()
}

/// Render a code cell. A **runnable** cell (a recognized shell, not `skip`) gets the
/// run gutter — a corner + language label, the body on a **heavy** bar tinted by run
/// state, then the result section (output + status line) on a **light dotted** bar.
/// A **display-only** cell (`skip=true`, or a non-shell language) never executes, so
/// it renders as a plain code block instead (see [`display_code_lines`]).
fn code_lines(c: &CodeBlock, width: usize, verbose: bool) -> Vec<Line<'static>> {
    if !c.is_runnable() {
        return display_code_lines(c, width);
    }

    let mut lines = Vec::new();
    let color = gutter_color(c.state);

    // Header: a corner anchoring the gutter and the language label. No backticks —
    // the gutter conveys "code".
    let head = vec![
        Span::styled("┏ ", Style::new().fg(color)),
        c.lang.clone().cyan(),
    ];
    lines.push(Line::from(head));

    // Body: each wrapped line carries the heavy bar; only the content is green so
    // the bar keeps its state tint.
    let avail = width.saturating_sub(2).max(1);
    for line in c.content.lines() {
        for chunk in hard_break(vec![Span::raw(line.to_string())], avail) {
            let mut spans = vec![Span::styled("┃ ", Style::new().fg(color))];
            spans.extend(chunk.into_iter().map(Stylize::green));
            lines.push(Line::from(spans));
        }
    }

    // Result section: streamed output, then the status line as the run's
    // conclusion (so "running…" sits beneath the live output tail).
    lines.extend(output_lines(&c.output, width, verbose));
    lines.push(status_line(c));
    lines
}

/// Render a display-only code block (`skip=true`, or a non-shell language) as an
/// ordinary fenced code block: the same cyan language label and green body as a
/// runnable cell, just indented two columns. No run gutter, status line, or "not
/// run" footer — it never executes, so run chrome would only mislead.
fn display_code_lines(c: &CodeBlock, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // The language label (omitted for an unlabelled fence) sets the block apart from
    // prose; the colors match a runnable cell so code reads consistently.
    if !c.lang.is_empty() {
        lines.push(Line::from(c.lang.clone().cyan()));
    }

    let avail = width.saturating_sub(2).max(1);
    for line in c.content.lines() {
        for chunk in hard_break(vec![Span::raw(line.to_string())], avail) {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(chunk.into_iter().map(Stylize::green));
            lines.push(Line::from(spans));
        }
    }
    lines
}

/// The left-gutter tint for a code cell's run state: grey when idle, then yellow /
/// green / red as it runs / succeeds / fails. Shared by the bar and the status glyph.
fn gutter_color(state: CodeBlockState) -> Color {
    match state {
        CodeBlockState::NotRun => Color::Indexed(240),
        CodeBlockState::Running => Color::Yellow,
        CodeBlockState::Success => Color::Green,
        CodeBlockState::Error => Color::Red,
    }
}

/// Maximum source lines of cell output shown inline. Fuller output (and a verbose
/// toggle) is deferred polish — see TODO.md.
const OUTPUT_MAX_LINES: usize = 25;

/// Render a cell's captured output on the light dotted "result" gutter, dimmed.
/// Shows the *tail* (last [`OUTPUT_MAX_LINES`] lines) so a streaming run reveals its
/// latest output, with a "… N earlier lines" marker when there's more above. When
/// `verbose` (Ctrl+O), the full output is shown with no truncation or marker.
fn output_lines(output: &str, width: usize, verbose: bool) -> Vec<Line<'static>> {
    if output.trim().is_empty() {
        return Vec::new();
    }

    // Sanitize at the TUI boundary: strip ANSI/control bytes ratatui would render
    // literally and corrupt the display with (DESIGN §7). Output that was *only*
    // escapes is now empty.
    let clean = crate::ansi::sanitize(output);
    if clean.trim().is_empty() {
        return Vec::new();
    }

    let avail = width.saturating_sub(2).max(1);
    let all: Vec<&str> = clean.lines().collect();
    let hidden = if verbose {
        0
    } else {
        all.len().saturating_sub(OUTPUT_MAX_LINES)
    };

    // A neutral dim bar — output is data, not a verdict, so it stays uncolored
    // (the status line below carries the run-state tint).
    let bar = || Span::raw("┊ ").dim();

    let mut lines = Vec::new();
    if hidden > 0 {
        lines.push(Line::from(vec![
            bar(),
            format!("… {hidden} earlier lines").dim().italic(),
        ]));
    }
    for line in &all[hidden..] {
        for chunk in hard_break(vec![Span::raw(line.to_string())], avail) {
            let mut spans = vec![bar()];
            spans.extend(chunk.into_iter().map(Stylize::dim));
            lines.push(Line::from(spans));
        }
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

/// The status line closing a code cell. It caps the gutter with a `┗` bottom corner
/// (matching the `┏` header) tinted by run state, so the verdict reads as the cell's
/// closing bracket rather than another output row. Once finished, the cell's elapsed
/// run time is appended (`✔ ok · 1.2s`); the live timer while *running* is the
/// footer's job, since updating it here would force a per-frame re-wrap of the document.
///
/// A pending cancellation re-labels the line: while running it reads "canceling…" /
/// "killing…"; once the interrupted run finishes it reads "canceled" / "killed".
fn status_line(c: &CodeBlock) -> Line<'static> {
    let bar = Span::styled("┗ ", Style::new().fg(gutter_color(c.state)));
    match (c.state, c.cancel) {
        (CodeBlockState::NotRun, _) => Line::from(vec![bar, "◦ not run".dim()]),
        (CodeBlockState::Running, Cancel::None) => Line::from(vec![bar, "● running…".yellow()]),
        (CodeBlockState::Running, Cancel::Interrupting) => {
            Line::from(vec![bar, "● canceling…".yellow()])
        }
        (CodeBlockState::Running, Cancel::Killing) => Line::from(vec![bar, "● killing…".red()]),
        (CodeBlockState::Success, _) => finished_line(bar, "✔ ok".green(), c.elapsed, c.exit_code),
        (CodeBlockState::Error, Cancel::Interrupting) => {
            finished_line(bar, "✗ canceled".red(), c.elapsed, c.exit_code)
        }
        (CodeBlockState::Error, Cancel::Killing) => {
            finished_line(bar, "✗ killed".red(), c.elapsed, c.exit_code)
        }
        (CodeBlockState::Error, Cancel::None) => {
            finished_line(bar, "✗ error".red(), c.elapsed, c.exit_code)
        }
    }
}

/// A finished-run status line: the gutter bar, the styled label, a dim ` · {elapsed}`
/// suffix, and a dim ` · exit N` suffix when the process exited non-zero (a clean
/// exit 0 is already conveyed by the ✔, so it's omitted).
fn finished_line(
    bar: Span<'static>,
    label: Span<'static>,
    elapsed: Option<std::time::Duration>,
    code: Option<i32>,
) -> Line<'static> {
    let mut spans = vec![bar, label];
    if let Some(d) = elapsed {
        spans.push(format!(" · {}", fmt_elapsed(d)).dim());
    }
    if let Some(n) = code
        && n != 0
    {
        spans.push(format!(" · exit {n}").dim());
    }
    Line::from(spans)
}

/// Human-readable run duration: `0.4s`, `1.2s`, `1m05.0s`.
fn fmt_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let mins = (secs / 60.0).floor();
        let rem = secs - mins * 60.0;
        format!("{mins:.0}m{rem:04.1}s")
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
        let (lines, _) = build_document(&book, 20, false);
        for l in &lines {
            assert!(
                display_width(&line_text(l)) <= 20,
                "line too wide: {:?}",
                line_text(l)
            );
        }
    }

    #[test]
    fn header_shows_frontmatter_fields() {
        let doc = "---\ntitle: My Run\ndescription: does things\nenv:\n  FOO: bar\n  BAZ: qux\n---\n\n# Hi\n";
        let book = Runbook::new(Some("book.md"), doc).unwrap();
        let lines = header_lines(&book, 60);
        let text: Vec<String> = lines.iter().map(line_text).collect();
        let joined = text.join("\n");

        assert!(joined.contains("marathon"), "missing top rule label");
        assert!(joined.contains("Title: My Run"), "missing title");
        assert!(joined.contains("Path: book.md"), "missing path");
        assert!(joined.contains("Desc: does things"), "missing description");
        // Env keys are listed, sorted.
        let baz = text.iter().position(|l| l.contains("BAZ: qux")).unwrap();
        let foo = text.iter().position(|l| l.contains("FOO: bar")).unwrap();
        assert!(baz < foo, "env should be sorted by key");
    }

    #[test]
    fn header_shows_temp_dir_once_created() {
        let mut book =
            Runbook::new(Some("book.md"), "---\ntitle: T\n---\n\n```sh\n:\n```\n").unwrap();
        // `header_fields` is the un-truncated source; the box layout fits it to width.
        let join = |b: &Runbook| {
            header_fields(b)
                .iter()
                .map(line_text)
                .collect::<Vec<_>>()
                .join("\n")
        };
        // Before any run the dir doesn't exist, so it isn't listed.
        assert!(!join(&book).contains("TMP_DIR"), "temp dir shown too early");

        // Once created (as a run would), it appears under the Env section.
        let dir = book.ensure_tmp_dir().unwrap();
        let shown = join(&book);
        assert!(shown.contains("Env:"), "env section missing: {shown}");
        assert!(
            shown.contains(&format!("TMP_DIR: {}", dir.display())),
            "temp dir path missing: {shown}"
        );
    }

    #[test]
    fn header_hides_runner_on_narrow_terminals() {
        let book = Runbook::new(Some("book.md"), "---\ntitle: T\n---\n\n# Hi\n").unwrap();
        let join = |w| {
            header_lines(&book, w)
                .iter()
                .map(line_text)
                .collect::<Vec<_>>()
                .join("\n")
        };
        // '@' is a distinctive runner glyph, absent from the wordmark and fields.
        assert!(
            join(100).contains('@'),
            "runner should show on a wide terminal"
        );

        let narrow = join(50);
        assert!(
            !narrow.contains('@'),
            "runner should be hidden when narrow: {narrow}"
        );
        assert!(
            narrow.contains("Title: T"),
            "title should remain when narrow"
        );
        assert!(narrow.contains("marathon"), "wordmark label should remain");
    }

    #[test]
    fn header_omits_absent_optional_fields() {
        // Minimal frontmatter: no title/description, empty env, and no path.
        let book = Runbook::new(None::<&str>, "---\nenv: {}\n---\n\n# Just a heading\n").unwrap();
        let joined = header_lines(&book, 40)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !joined.contains("Title:"),
            "title should be omitted: {joined}"
        );
        assert!(
            !joined.contains("Path:"),
            "path should be omitted: {joined}"
        );
        assert!(!joined.contains("Env:"), "env should be omitted: {joined}");
    }

    #[test]
    fn long_content_wraps_to_multiple_lines() {
        let book = book();
        let (lines, ranges) = build_document(&book, 20, false);
        // The header (range 0) plus 3 blocks (heading, paragraph, code).
        assert_eq!(ranges.len(), 4);
        assert!(
            lines.len() > ranges.len() * 2,
            "expected wrapping to expand the document, got {} lines",
            lines.len()
        );
        // The header range starts at the very top; the first *block* follows it; the
        // last range ends just before the trailing spacer (spacers live outside ranges).
        assert_eq!(ranges[0].start, 0, "header is the first selectable range");
        assert!(ranges[1].start > 0, "first block follows the header");
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

    #[test]
    fn running_cell_tail_follows_as_output_grows() {
        let mut book =
            Runbook::new(None::<&str>, "---\ntitle: t\n---\n\n```sh\necho hi\n```\n").unwrap();
        // The lone code cell, marked running with no output yet, and recorded as the
        // last-run cell (what drives tail-follow).
        let BookBlock::Code(c) = &mut book.blocks[0] else {
            panic!("expected a code cell");
        };
        c.state = CodeBlockState::Running;
        book.last_run = Some(0);

        let mut term = Terminal::new(TestBackend::new(24, 6)).unwrap();
        let mut state = ScrollState::new();
        state.select_index(1, 2); // select the cell (index 0 is the header)

        // Frame 1: the (short) cell and its bottom are on screen.
        term.draw(|f| f.render_stateful_widget(DocumentView::new(&book, 0), f.area(), &mut state))
            .unwrap();
        let first = state.offset;

        // Stream more output than the viewport is tall, then redraw.
        let BookBlock::Code(c) = &mut book.blocks[0] else {
            unreachable!()
        };
        for i in 0..30 {
            c.push_output(&format!("line {i}\n"));
        }
        term.draw(|f| f.render_stateful_widget(DocumentView::new(&book, 1), f.area(), &mut state))
            .unwrap();

        // The view followed the tail: it scrolled further, and the newest line shows.
        assert!(
            state.offset > first,
            "view should follow the growing output"
        );
        let dump = format!("{:?}", term.backend().buffer());
        assert!(
            dump.contains("line 29"),
            "newest output not visible: {dump}"
        );
    }

    #[test]
    fn fast_finished_run_still_follows_tail() {
        // A command that finishes between frames: by the time we draw, its full
        // output is in and its state is already Success. Tail-follow keys on
        // last-run, not "still running", so its tail is still revealed.
        let mut book =
            Runbook::new(None::<&str>, "---\ntitle: t\n---\n\n```sh\necho hi\n```\n").unwrap();

        let mut term = Terminal::new(TestBackend::new(24, 6)).unwrap();
        let mut state = ScrollState::new();
        state.select_index(1, 2); // select the cell (index 0 is the header)
        // Frame 1: idle cell, nothing scrolled.
        term.draw(|f| f.render_stateful_widget(DocumentView::new(&book, 0), f.area(), &mut state))
            .unwrap();

        // The whole run lands at once: output + finish, before the next draw.
        let BookBlock::Code(c) = &mut book.blocks[0] else {
            panic!("expected a code cell");
        };
        for i in 0..30 {
            c.push_output(&format!("line {i}\n"));
        }
        c.finish(true, Some(0));
        book.last_run = Some(0);

        term.draw(|f| f.render_stateful_widget(DocumentView::new(&book, 1), f.area(), &mut state))
            .unwrap();
        assert!(
            state.offset > 0,
            "finished run should still follow its tail"
        );
        let dump = format!("{:?}", term.backend().buffer());
        assert!(
            dump.contains("line 29"),
            "newest output not visible: {dump}"
        );
    }

    #[test]
    fn scrolled_up_running_cell_is_not_yanked_down() {
        let mut book =
            Runbook::new(None::<&str>, "---\ntitle: t\n---\n\n```sh\necho hi\n```\n").unwrap();
        let BookBlock::Code(c) = &mut book.blocks[0] else {
            panic!("expected a code cell");
        };
        c.state = CodeBlockState::Running;
        for i in 0..30 {
            c.push_output(&format!("line {i}\n"));
        }
        book.last_run = Some(0);

        let mut term = Terminal::new(TestBackend::new(24, 6)).unwrap();
        let mut state = ScrollState::new();
        state.select_index(1, 2); // select the cell (index 0 is the header)
        term.draw(|f| f.render_stateful_widget(DocumentView::new(&book, 0), f.area(), &mut state))
            .unwrap();

        // User scrolls up to read earlier output, away from the tail.
        state.scroll_up(4);
        let parked = state.offset;
        // More output streams in; the view must stay where the user parked it.
        let BookBlock::Code(c) = &mut book.blocks[0] else {
            unreachable!()
        };
        for i in 30..40 {
            c.push_output(&format!("line {i}\n"));
        }
        term.draw(|f| f.render_stateful_widget(DocumentView::new(&book, 1), f.area(), &mut state))
            .unwrap();
        assert_eq!(
            state.offset, parked,
            "scrolled-up view should not be yanked down"
        );
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

    fn code_cell() -> CodeBlock {
        code_cell_from("---\ntitle: t\n---\n\n```sh\necho hi\n```\n")
    }

    fn code_cell_from(src: &str) -> CodeBlock {
        Runbook::new(None::<&str>, src)
            .unwrap()
            .blocks
            .into_iter()
            .find_map(|b| match b {
                BookBlock::Code(c) => Some(c),
                _ => None,
            })
            .expect("one code cell")
    }

    #[test]
    fn code_chrome_uses_gutter_not_backticks() {
        let c = code_cell();
        let lines = code_lines(&c, 40, false);

        // Header carries the language label and a gutter corner, no raw fence.
        let head = line_text(&lines[0]);
        assert!(head.contains("sh"), "language label missing: {head}");
        assert!(head.contains('┏'), "gutter corner missing: {head}");
        assert!(
            !head.contains("```"),
            "raw fence leaked into chrome: {head}"
        );

        // The body line is prefixed by the heavy gutter bar.
        let body = line_text(&lines[1]);
        assert!(body.contains('┃'), "code body lacks gutter bar: {body}");
        assert!(body.contains("echo"), "code body missing: {body}");
    }

    #[test]
    fn display_only_code_has_no_gutter_or_status() {
        // Both a `skip=true` shell block and a non-shell block are display-only.
        for src in [
            "---\ntitle: t\n---\n\n```sh skip=true\necho hi\n```\n",
            "---\ntitle: t\n---\n\n```python\nprint(\"hi\")\n```\n",
        ] {
            let c = code_cell_from(src);
            assert!(!c.is_runnable(), "expected display-only: {src}");

            let joined = code_lines(&c, 40, false)
                .iter()
                .map(line_text)
                .collect::<Vec<_>>()
                .join("\n");

            // No run gutter glyphs, no status footer.
            for g in ['┏', '┃', '┗', '┊'] {
                assert!(!joined.contains(g), "run gutter leaked ({g}): {joined}");
            }
            assert!(
                !joined.contains("not run"),
                "display-only block should have no status line: {joined}"
            );
            // Still recognizable as a code block: a language label and the body.
            assert!(
                joined.contains("echo") || joined.contains("print"),
                "body missing: {joined}"
            );
        }
    }

    #[test]
    fn output_rides_the_dotted_gutter() {
        let lines = output_lines("hello world\n", 40, false);
        let only = line_text(&lines[0]);
        assert!(only.contains('┊'), "output lacks dotted gutter: {only}");
        assert!(only.contains("hello world"), "output text missing: {only}");
    }

    #[test]
    fn verbose_output_shows_all_lines_no_marker() {
        // More lines than the tail cap, so collapsed view truncates with a marker.
        let output: String = (0..OUTPUT_MAX_LINES + 5)
            .map(|i| format!("line {i}\n"))
            .collect();

        let collapsed = output_lines(&output, 40, false);
        let collapsed_text: String = collapsed.iter().map(line_text).collect();
        assert!(
            collapsed_text.contains("earlier lines"),
            "collapsed view should mark hidden lines"
        );
        assert!(
            !collapsed_text.contains("line 0"),
            "tail should hide the head"
        );

        let expanded = output_lines(&output, 40, true);
        let expanded_text: String = expanded.iter().map(line_text).collect();
        assert!(
            !expanded_text.contains("earlier lines"),
            "verbose view should have no truncation marker"
        );
        assert!(
            expanded_text.contains("line 0") && expanded_text.contains("line 29"),
            "verbose view should show the whole output"
        );
    }

    #[test]
    fn status_line_appends_elapsed_and_nonzero_exit() {
        let mut c = code_cell();

        // Success: shows elapsed, but not "exit 0".
        c.state = CodeBlockState::Success;
        c.elapsed = Some(std::time::Duration::from_millis(1200));
        c.exit_code = Some(0);
        let t = line_text(&status_line(&c));
        assert!(t.contains("ok") && t.contains("1.2s"), "got: {t}");
        assert!(!t.contains("exit"), "exit 0 should be omitted: {t}");

        // Error: shows elapsed and the non-zero exit code.
        c.state = CodeBlockState::Error;
        c.elapsed = Some(std::time::Duration::from_millis(400));
        c.exit_code = Some(2);
        let t = line_text(&status_line(&c));
        assert!(t.contains("error") && t.contains("0.4s"), "got: {t}");
        assert!(t.contains("exit 2"), "got: {t}");
    }

    #[test]
    fn status_line_reflects_cancel_phase() {
        let mut c = code_cell();

        // While running, the label tracks the escalation.
        c.state = CodeBlockState::Running;
        c.cancel = Cancel::Interrupting;
        assert!(line_text(&status_line(&c)).contains("canceling"));
        c.cancel = Cancel::Killing;
        assert!(line_text(&status_line(&c)).contains("killing"));

        // Once the interrupted run finishes (signal → error), it names the outcome.
        c.state = CodeBlockState::Error;
        c.cancel = Cancel::Interrupting;
        let t = line_text(&status_line(&c));
        assert!(t.contains("canceled") && !t.contains("error"), "got: {t}");
        c.cancel = Cancel::Killing;
        assert!(line_text(&status_line(&c)).contains("killed"));
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
