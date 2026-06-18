# Text Editing

Ratatui ships **no text-input widget** — text editing is state you own: a string (or lines), a cursor position, and key handlers. This file builds that up from a single-line input to a multi-line editor, a markdown editor with live preview, and an LLM-agent chat input. Targets ratatui 0.30 / crossterm 0.29.

```toml
[dependencies]
ratatui = "0.30.1"
crossterm = "0.29"
unicode-width = "0.2"     # display-width math (ratatui uses it internally too)
```

## The three coordinate systems (read this first)

For a `String` you are editing, one "position" has three different numeric values:

1. **Byte index** — what `String::insert`/`remove`/slicing need. Multi-byte UTF-8 means byte ≠ char.
2. **Char index** — what your cursor should count (`chars().count()`); stable under typing.
3. **Display column** — what the terminal shows; CJK/emoji are width 2, combining marks 0. Compute with `unicode_width::UnicodeWidthStr::width(s)` / `UnicodeWidthChar`.

Rules: store the cursor as a **char index**; convert to a **byte index** to mutate the string; convert to a **display column** to position the terminal cursor. The conversion helpers:

```rust
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Byte index of the `char_idx`-th character (== s.len() when at end).
fn byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(i, _)| i)
}

/// Display width (terminal columns) of the first `char_idx` characters.
fn display_col(s: &str, char_idx: usize) -> usize {
    s.chars().take(char_idx).map(|c| c.width().unwrap_or(0)).sum()
}
```

(For full grapheme-cluster correctness — emoji ZWJ sequences, flags — step the cursor with `unicode-segmentation`'s `graphemes()` instead of `chars()`. Char-stepping is what most TUI inputs ship and is fine in practice.)

**The cursor is yours to draw.** Ratatui hides the hardware cursor unless you call `frame.set_cursor_position(Position)` — *every frame* while an input is focused. Don't fake a cursor with a styled block: the real cursor gets IME placement, terminal cursor styling, and screen-reader behavior right.

## 1. Single-line input from scratch

The complete pattern (condensed from ratatui's own `user-input` example):

```rust
use crossterm::event::{self, KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

#[derive(Default)]
struct LineInput {
    value: String,
    cursor: usize,          // char index, 0..=value.chars().count()
}

impl LineInput {
    fn insert(&mut self, c: char) {
        self.value.insert(byte_idx(&self.value, self.cursor), c);
        self.cursor += 1;
    }
    fn insert_str(&mut self, s: &str) {                     // for Event::Paste
        self.value.insert_str(byte_idx(&self.value, self.cursor), s);
        self.cursor += s.chars().count();
    }
    fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.value.remove(byte_idx(&self.value, self.cursor));
        }
    }
    fn delete(&mut self) {
        if self.cursor < self.value.chars().count() {
            self.value.remove(byte_idx(&self.value, self.cursor));
        }
    }
    fn left(&mut self)  { self.cursor = self.cursor.saturating_sub(1); }
    fn right(&mut self) { self.cursor = (self.cursor + 1).min(self.value.chars().count()); }
    fn home(&mut self)  { self.cursor = 0; }
    fn end(&mut self)   { self.cursor = self.value.chars().count(); }

    /// Delete the word before the cursor (Ctrl+W / Alt+Backspace).
    fn delete_word(&mut self) {
        let mut new_cursor = self.cursor;
        let chars: Vec<char> = self.value.chars().collect();
        while new_cursor > 0 && chars[new_cursor - 1].is_whitespace() { new_cursor -= 1; }
        while new_cursor > 0 && !chars[new_cursor - 1].is_whitespace() { new_cursor -= 1; }
        let (a, b) = (byte_idx(&self.value, new_cursor), byte_idx(&self.value, self.cursor));
        self.value.replace_range(a..b, "");
        self.cursor = new_cursor;
    }

    fn on_key(&mut self, key: event::KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => self.delete_word(),
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => { self.value.clear(); self.cursor = 0; }
            (KeyCode::Char('a'), KeyModifiers::CONTROL) | (KeyCode::Home, _) => self.home(),
            (KeyCode::Char('e'), KeyModifiers::CONTROL) | (KeyCode::End, _) => self.end(),
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => self.insert(c),
            (KeyCode::Backspace, KeyModifiers::ALT) => self.delete_word(),
            (KeyCode::Backspace, _) => self.backspace(),
            (KeyCode::Delete, _) => self.delete(),
            (KeyCode::Left, _) => self.left(),
            (KeyCode::Right, _) => self.right(),
            _ => {}
        }
    }

    /// Render inside a bordered block and place the cursor. Horizontal-scrolls
    /// when the value is wider than the visible area.
    fn render(&self, frame: &mut Frame, area: ratatui::layout::Rect, focused: bool) {
        let inner_width = area.width.saturating_sub(2) as usize;   // borders
        let cursor_col = display_col(&self.value, self.cursor);
        let scroll = cursor_col.saturating_sub(inner_width.saturating_sub(1));  // keep cursor visible
        let input = Paragraph::new(self.value.as_str())
            .scroll((0, scroll as u16))
            .block(Block::bordered().title("Input"));
        frame.render_widget(input, area);
        if focused {
            frame.set_cursor_position(Position::new(
                area.x + 1 + (cursor_col - scroll) as u16,
                area.y + 1,
            ));
        }
    }
}
```

Wire it with the mode pattern from 05 (printable chars go to the input only while focused), and handle `Event::Paste` → `insert_str` with bracketed paste enabled.

## 2. Or use `tui-input` (ratatui 0.30 ✓)

[`tui-input`](https://crates.io/crates/tui-input) `0.15` is exactly the struct above, maintained, with emacs-style bindings built in:

```rust
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;   // adds input.handle_event(&Event)

struct App { input: Input, focused: bool }

// event handling: feed it any crossterm event; it returns Some(StateChanged) if it consumed it
if app.focused {
    app.input.handle_event(&event);
}

// submit:
let value: String = app.input.value_and_reset();

// render (scroll-aware; reserve 2 cols for borders + 1 for the cursor):
let width = area.width.saturating_sub(3) as usize;
let scroll = app.input.visual_scroll(width);
let p = Paragraph::new(app.input.value())
    .scroll((0, scroll as u16))
    .block(Block::bordered());
frame.render_widget(p, area);
frame.set_cursor_position(Position::new(
    area.x + 1 + (app.input.visual_cursor().saturating_sub(scroll)) as u16,
    area.y + 1,
));
```

## 3. Multi-line editor from scratch

State = `Vec<String>` (one entry per logical line, no `\n` stored) + 2-D cursor + scroll offsets. No soft-wrap (horizontal scroll instead) — that's the right call for *editors* (predictable cursor math, like nano/vim defaults); soft-wrap is covered in the chat input (§5).

```rust
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthChar;

pub struct Editor {
    pub lines: Vec<String>,
    pub cursor: (usize, usize),   // (row, char col)
    scroll: (usize, usize),       // (top row, left display col)
    desired_col: usize,           // remembered col for up/down over short lines
}

impl Default for Editor {
    fn default() -> Self {
        Self { lines: vec![String::new()], cursor: (0, 0), scroll: (0, 0), desired_col: 0 }
    }
}

impl Editor {
    pub fn text(&self) -> String { self.lines.join("\n") }

    pub fn set_text(&mut self, s: &str) {
        self.lines = s.split('\n').map(str::to_string).collect();
        if self.lines.is_empty() { self.lines.push(String::new()); }
        self.cursor = (0, 0);
        self.scroll = (0, 0);
    }

    fn line(&self) -> &str { &self.lines[self.cursor.0] }
    fn line_chars(&self) -> usize { self.line().chars().count() }

    // ---- edits -------------------------------------------------------------

    pub fn insert_char(&mut self, c: char) {
        let (row, col) = self.cursor;
        let at = byte_idx(&self.lines[row], col);
        self.lines[row].insert(at, c);
        self.cursor.1 += 1;
        self.desired_col = self.cursor.1;
    }

    pub fn insert_str(&mut self, s: &str) {              // paste (may contain newlines)
        for (i, part) in s.split('\n').enumerate() {
            if i > 0 { self.insert_newline(); }
            let (row, col) = self.cursor;
            let at = byte_idx(&self.lines[row], col);
            self.lines[row].insert_str(at, part);
            self.cursor.1 += part.chars().count();
        }
        self.desired_col = self.cursor.1;
    }

    pub fn insert_newline(&mut self) {
        let (row, col) = self.cursor;
        let at = byte_idx(&self.lines[row], col);
        let rest = self.lines[row].split_off(at);
        self.lines.insert(row + 1, rest);
        self.cursor = (row + 1, 0);
        self.desired_col = 0;
    }

    pub fn backspace(&mut self) {
        let (row, col) = self.cursor;
        if col > 0 {
            let at = byte_idx(&self.lines[row], col - 1);
            self.lines[row].remove(at);
            self.cursor.1 -= 1;
        } else if row > 0 {
            // join with previous line
            let removed = self.lines.remove(row);
            let prev_chars = self.lines[row - 1].chars().count();
            self.lines[row - 1].push_str(&removed);
            self.cursor = (row - 1, prev_chars);
        }
        self.desired_col = self.cursor.1;
    }

    pub fn delete(&mut self) {
        let (row, col) = self.cursor;
        if col < self.line_chars() {
            let at = byte_idx(&self.lines[row], col);
            self.lines[row].remove(at);
        } else if row + 1 < self.lines.len() {
            let next = self.lines.remove(row + 1);
            self.lines[row].push_str(&next);
        }
    }

    // ---- movement ----------------------------------------------------------

    pub fn left(&mut self) {
        if self.cursor.1 > 0 { self.cursor.1 -= 1; }
        else if self.cursor.0 > 0 { self.cursor.0 -= 1; self.cursor.1 = self.line_chars(); }
        self.desired_col = self.cursor.1;
    }
    pub fn right(&mut self) {
        if self.cursor.1 < self.line_chars() { self.cursor.1 += 1; }
        else if self.cursor.0 + 1 < self.lines.len() { self.cursor = (self.cursor.0 + 1, 0); }
        self.desired_col = self.cursor.1;
    }
    pub fn up(&mut self)   { self.vertical(-1); }
    pub fn down(&mut self) { self.vertical(1); }
    fn vertical(&mut self, delta: isize) {
        let row = self.cursor.0.saturating_add_signed(delta).min(self.lines.len() - 1);
        self.cursor.0 = row;
        self.cursor.1 = self.desired_col.min(self.line_chars());
    }
    pub fn home(&mut self) { self.cursor.1 = 0; self.desired_col = 0; }
    pub fn end(&mut self)  { self.cursor.1 = self.line_chars(); self.desired_col = self.cursor.1; }
    pub fn page(&mut self, dir: isize, page: usize) {
        self.vertical(dir * page as isize);
    }

    // ---- key dispatch --------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent, viewport_rows: usize) {
        match (key.code, key.modifiers) {
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => self.insert_char(c),
            (KeyCode::Enter, _) => self.insert_newline(),
            (KeyCode::Backspace, _) => self.backspace(),
            (KeyCode::Delete, _) => self.delete(),
            (KeyCode::Left, _) => self.left(),
            (KeyCode::Right, _) => self.right(),
            (KeyCode::Up, _) => self.up(),
            (KeyCode::Down, _) => self.down(),
            (KeyCode::Home, _) => self.home(),
            (KeyCode::End, _) => self.end(),
            (KeyCode::PageUp, _) => self.page(-1, viewport_rows),
            (KeyCode::PageDown, _) => self.page(1, viewport_rows),
            _ => {}
        }
    }

    // ---- rendering -----------------------------------------------------------

    /// Render with line numbers; keeps the cursor inside the viewport by
    /// adjusting scroll. Call set_cursor_position only when focused.
    pub fn render(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let block = Block::bordered().title("Editor");
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let gutter = (self.lines.len().max(1).ilog10() as u16) + 2;   // " 42 "
        let text_w = inner.width.saturating_sub(gutter + 1) as usize;
        let text_h = inner.height as usize;

        // -- keep cursor visible (vertical: rows, horizontal: display cols)
        let cur_col = display_col(self.line(), self.cursor.1);
        if self.cursor.0 < self.scroll.0 { self.scroll.0 = self.cursor.0; }
        if self.cursor.0 >= self.scroll.0 + text_h { self.scroll.0 = self.cursor.0 + 1 - text_h; }
        if cur_col < self.scroll.1 { self.scroll.1 = cur_col; }
        if text_w > 0 && cur_col >= self.scroll.1 + text_w { self.scroll.1 = cur_col + 1 - text_w; }

        // -- build visible lines: "{n:>w} {sliced line}"
        let mut rows: Vec<Line> = Vec::with_capacity(text_h);
        for (i, line) in self.lines.iter().enumerate().skip(self.scroll.0).take(text_h) {
            let num = format!("{:>w$} ", i + 1, w = gutter as usize - 1);
            let body = slice_columns(line, self.scroll.1, text_w);
            rows.push(Line::from(vec![num.dim(), body.into()]));
        }
        frame.render_widget(Paragraph::new(rows), inner);

        if focused {
            frame.set_cursor_position(Position::new(
                inner.x + gutter + (cur_col - self.scroll.1) as u16,
                inner.y + (self.cursor.0 - self.scroll.0) as u16,
            ));
        }
    }
}

/// The substring of `s` covering display columns [skip, skip+take).
/// Wide chars straddling a boundary are replaced conservatively with a space.
fn slice_columns(s: &str, skip: usize, take: usize) -> String {
    let mut out = String::new();
    let (mut col, end) = (0usize, skip + take);
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if col + w > end { break; }
        if col >= skip { out.push(c); }
        else if col + w > skip { out.push(' '); }    // wide char cut by left edge
        col += w;
    }
    out
}
```

Notes on the design:

- `desired_col` reproduces the editor nicety where moving through a short line and back restores your column.
- Scroll-into-view runs in `render` (needs the viewport size), so `render` takes `&mut self` — that's fine inside `terminal.draw(|frame| ...)`.
- Tabs: either expand to spaces on insert (`KeyCode::Tab => editor.insert_str("    ")`) or handle width-aware rendering of `\t` (much harder; expanding is what most TUIs do).
- **Undo**: keep `undo_stack: Vec<(Vec<String>, (usize, usize))>` — push a snapshot before each *edit group* (first edit after a movement, newline, or deletion burst), pop on Ctrl+Z. Snapshots are fine for human-scale documents; switch to operation-based undo only if you have data showing you need it.
- For files: `editor.set_text(&std::fs::read_to_string(path)?)`, save with `std::fs::write(path, editor.text() + "\n")`.

## 4. Markdown editor with live preview

Embed `Editor` on the left, render a styled preview on the right. The interesting part is `markdown_to_text` — mapping markdown to `Text`/`Line`/`Span` styling:

```rust
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span, Text};

fn markdown_to_text(src: &str) -> Text<'static> {
    let mut out: Vec<Line> = Vec::new();
    let mut in_code = false;
    for raw in src.split('\n') {
        let line = raw.to_string();
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            out.push(Line::from(line.dim()));
            continue;
        }
        if in_code {
            out.push(Line::from(Span::styled(line, Style::new().fg(Color::Yellow).on_black())));
        } else if let Some(rest) = line.strip_prefix("### ") {
            out.push(Line::from(rest.to_string().bold().cyan()));
        } else if let Some(rest) = line.strip_prefix("## ") {
            out.push(Line::from(rest.to_string().bold().underlined().cyan()));
        } else if let Some(rest) = line.strip_prefix("# ") {
            out.push(Line::from(rest.to_string().bold().reversed().cyan()));
        } else if let Some(rest) = line.strip_prefix("> ") {
            out.push(Line::from(vec!["┃ ".cyan(), rest.to_string().italic().dim()]));
        } else if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            out.push(Line::from(vec!["• ".cyan(), inline_md(rest)]));
        } else {
            out.push(Line::from(spans_md(&line)));
        }
    }
    Text::from(out)
}

/// Minimal inline styling: `code` and **bold**. (Toy parser — fine for previews.)
fn spans_md(s: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, chunk) in s.split('`').enumerate() {
        if i % 2 == 1 {                                   // inside backticks
            spans.push(chunk.to_string().yellow().on_black());
        } else {
            for (j, b) in chunk.split("**").enumerate() {
                if j % 2 == 1 { spans.push(b.to_string().bold()); }
                else if !b.is_empty() { spans.push(Span::raw(b.to_string())); }
            }
        }
    }
    spans
}
fn inline_md(s: &str) -> Span<'static> { Span::raw(s.to_string()) }   // or spans_md per-piece
```

The app shell:

```rust
use crossterm::event::{self, KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::{Block, Paragraph, Wrap};

fn main() -> std::io::Result<()> {
    let mut editor = Editor::default();
    let mut show_preview = true;
    ratatui::run(|terminal| {
        loop {
            terminal.draw(|frame| {
                let areas: Vec<_> = if show_preview {
                    Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(frame.area()).to_vec()
                } else {
                    vec![frame.area()]
                };
                editor.render(frame, areas[0], true);
                if show_preview {
                    let preview = Paragraph::new(markdown_to_text(&editor.text()))
                        .wrap(Wrap { trim: false })
                        .block(Block::bordered().title("Preview"));
                    frame.render_widget(preview, areas[1]);
                }
            })?;

            if let Some(key) = event::read()?.as_key_press_event() {
                match (key.code, key.modifiers) {
                    (KeyCode::Char('q'), KeyModifiers::CONTROL) => return Ok(()),
                    (KeyCode::Char('p'), KeyModifiers::CONTROL) => show_preview = !show_preview,
                    _ => editor.on_key(key, 20),
                }
            }
        }
    })
}
```

For production markdown rendering use [`tui-markdown`](https://crates.io/crates/tui-markdown) (pulldown-cmark → ratatui `Text`) or parse with `pulldown-cmark` yourself and map events to spans — the toy parser above doesn't handle nesting, links, or tables. Add syntax highlighting in code fences with `syntect` mapped to spans.

## 5. LLM-agent chat input

The requirements that make a chat input different from an editor:

1. **Enter sends; some modifier inserts a newline.** Plain terminals *cannot* see Shift+Enter (it arrives as Enter). Strategy: feature-detect the kitty keyboard protocol for true Shift+Enter, and always support **Alt+Enter** as the universal fallback (see 05-events-and-input.md).
2. **Soft-wrap + auto-grow**: the input wraps at the box width and grows from 1 up to ~8 rows, shrinking the transcript above.
3. **Bracketed paste**: multi-line pastes must *not* trigger sends (without it, each pasted `\n` is an Enter key!). One `Event::Paste(String)` → insert verbatim.
4. **History recall** (Up/Down on an empty/first-line input) and a **stick-to-bottom transcript** that stops sticking while the user scrolls back.

### Wrap + cursor layout (the core trick)

One function computes the wrapped visual lines *and* the cursor's visual position from the same walk, so they can never disagree:

```rust
use unicode_width::UnicodeWidthChar;

/// Char-accurate soft wrap at `width` display columns.
/// Returns (visual lines, (cursor_x, cursor_y) in visual coords).
fn wrap_with_cursor(
    lines: &[String],
    cursor: (usize, usize),          // (logical row, char col)
    width: usize,
) -> (Vec<String>, (usize, usize)) {
    let width = width.max(1);
    let mut visual: Vec<String> = Vec::new();
    let mut cur_xy = (0, 0);
    for (row, line) in lines.iter().enumerate() {
        let mut current = String::new();
        let mut col = 0usize;                    // display col within current visual line
        let mut char_i = 0usize;                 // char index within logical line
        if row == cursor.0 && cursor.1 == 0 {
            cur_xy = (0, visual.len());
        }
        for c in line.chars() {
            let w = c.width().unwrap_or(0).max(1);
            if col + w > width {                 // wrap before this char
                visual.push(std::mem::take(&mut current));
                col = 0;
            }
            current.push(c);
            col += w;
            char_i += 1;
            if row == cursor.0 && char_i == cursor.1 {
                // cursor sits AFTER this char
                if col >= width {
                    cur_xy = (0, visual.len() + 1);
                } else {
                    cur_xy = (col, visual.len());
                }
            }
        }
        visual.push(current);
    }
    if visual.is_empty() { visual.push(String::new()); }
    if cur_xy.1 >= visual.len() { visual.push(String::new()); }   // cursor just past a full last row
    (visual, cur_xy)
}
```

(Word-aware wrapping: same structure but break at the last space before overflow — or hand the *display* problem to `textwrap` and keep this char-walk only for the cursor.)

### Complete runnable chat app

Std-only (background thread fakes a streaming LLM; swap in your API client — async version in 07). ~Everything else is real: kitty detection, Alt+Enter fallback, paste, auto-grow, history, stick-to-bottom scrolling.

```rust
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyModifiers,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::supports_keyboard_enhancement;
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::Stylize;
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

const MAX_INPUT_ROWS: u16 = 8;

struct Msg { role: &'static str, content: String }

#[derive(Default)]
struct ChatInput {
    lines: Vec<String>,            // logical lines (explicit newlines only)
    cursor: (usize, usize),        // (row, char col)
    history: Vec<String>,
    history_pos: Option<usize>,
}

impl ChatInput {
    fn new() -> Self { Self { lines: vec![String::new()], ..Default::default() } }
    fn text(&self) -> String { self.lines.join("\n") }
    fn is_empty(&self) -> bool { self.lines.len() == 1 && self.lines[0].is_empty() }
    fn clear(&mut self) { self.lines = vec![String::new()]; self.cursor = (0, 0); self.history_pos = None; }

    fn insert_char(&mut self, c: char) {
        let (r, col) = self.cursor;
        let at = byte_idx(&self.lines[r], col);
        self.lines[r].insert(at, c);
        self.cursor.1 += 1;
    }
    fn insert_str(&mut self, s: &str) {
        for (i, part) in s.split('\n').enumerate() {
            if i > 0 { self.newline(); }
            let (r, col) = self.cursor;
            let at = byte_idx(&self.lines[r], col);
            self.lines[r].insert_str(at, part);
            self.cursor.1 += part.chars().count();
        }
    }
    fn newline(&mut self) {
        let (r, col) = self.cursor;
        let at = byte_idx(&self.lines[r], col);
        let rest = self.lines[r].split_off(at);
        self.lines.insert(r + 1, rest);
        self.cursor = (r + 1, 0);
    }
    fn backspace(&mut self) {
        let (r, col) = self.cursor;
        if col > 0 {
            let at = byte_idx(&self.lines[r], col - 1);
            self.lines[r].remove(at);
            self.cursor.1 -= 1;
        } else if r > 0 {
            let removed = self.lines.remove(r);
            let prev = self.lines[r - 1].chars().count();
            self.lines[r - 1].push_str(&removed);
            self.cursor = (r - 1, prev);
        }
    }
    fn left(&mut self) {
        if self.cursor.1 > 0 { self.cursor.1 -= 1; }
        else if self.cursor.0 > 0 {
            self.cursor.0 -= 1;
            self.cursor.1 = self.lines[self.cursor.0].chars().count();
        }
    }
    fn right(&mut self) {
        if self.cursor.1 < self.lines[self.cursor.0].chars().count() { self.cursor.1 += 1; }
        else if self.cursor.0 + 1 < self.lines.len() { self.cursor = (self.cursor.0 + 1, 0); }
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() { return; }
        let pos = match self.history_pos { None => self.history.len() - 1, Some(0) => 0, Some(p) => p - 1 };
        self.history_pos = Some(pos);
        let entry = self.history[pos].clone();
        self.lines = entry.split('\n').map(str::to_string).collect();
        self.cursor = (self.lines.len() - 1, self.lines.last().unwrap().chars().count());
    }
    fn history_next(&mut self) {
        match self.history_pos {
            Some(p) if p + 1 < self.history.len() => {
                self.history_pos = Some(p + 1);
                let entry = self.history[p + 1].clone();
                self.lines = entry.split('\n').map(str::to_string).collect();
                self.cursor = (self.lines.len() - 1, self.lines.last().unwrap().chars().count());
            }
            Some(_) => self.clear(),
            None => {}
        }
    }
}

struct App {
    messages: Vec<Msg>,
    input: ChatInput,
    scroll: usize,            // transcript scroll (visual rows from top)
    stick_to_bottom: bool,
    streaming: Option<mpsc::Receiver<String>>,   // tokens from the worker
    quit: bool,
}

impl App {
    fn send(&mut self) {
        let prompt = self.input.text();
        if prompt.trim().is_empty() { return; }
        self.input.history.push(prompt.clone());
        self.input.clear();
        self.messages.push(Msg { role: "user", content: prompt.clone() });
        self.messages.push(Msg { role: "assistant", content: String::new() });
        self.stick_to_bottom = true;

        // Fake streaming worker — replace with your LLM API call.
        let (tx, rx) = mpsc::channel();
        self.streaming = Some(rx);
        std::thread::spawn(move || {
            for word in format!("You said: {prompt}. Here is a long streamed reply…").split_inclusive(' ') {
                if tx.send(word.to_string()).is_err() { return; }
                std::thread::sleep(Duration::from_millis(40));
            }
        });
    }

    /// Drain streamed tokens into the last message. Returns true if anything changed.
    fn pump_stream(&mut self) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.streaming {
            loop {
                match rx.try_recv() {
                    Ok(tok) => {
                        self.messages.last_mut().unwrap().content.push_str(&tok);
                        changed = true;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => { self.streaming = None; break; }
                }
            }
        }
        changed
    }

    fn handle_event(&mut self, event: &Event, enhanced_keys: bool) {
        match event {
            Event::Paste(s) => self.input.insert_str(s),
            Event::Key(key) if key.is_press() => {
                let m = key.modifiers;
                match key.code {
                    KeyCode::Char('c') if m == KeyModifiers::CONTROL => self.quit = true,
                    // newline: Alt+Enter always; Shift/Ctrl+Enter when kitty protocol active
                    KeyCode::Enter if m.contains(KeyModifiers::ALT)
                        || (enhanced_keys && (m.contains(KeyModifiers::SHIFT) || m.contains(KeyModifiers::CONTROL)))
                        => self.input.newline(),
                    KeyCode::Enter => self.send(),
                    KeyCode::Char(c) if m == KeyModifiers::NONE || m == KeyModifiers::SHIFT
                        => self.input.insert_char(c),
                    KeyCode::Backspace => self.input.backspace(),
                    KeyCode::Left => self.input.left(),
                    KeyCode::Right => self.input.right(),
                    // Up/Down: history when input is empty or already browsing history
                    KeyCode::Up if self.input.is_empty() || self.input.history_pos.is_some()
                        => self.input.history_prev(),
                    KeyCode::Down if self.input.is_empty() || self.input.history_pos.is_some()
                        => self.input.history_next(),
                    KeyCode::PageUp => { self.stick_to_bottom = false; self.scroll = self.scroll.saturating_sub(10); }
                    KeyCode::PageDown => self.scroll += 10,   // clamped in render; re-sticks at bottom
                    KeyCode::Esc => self.quit = true,
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        // --- input geometry first (auto-grow): wrap at inner width, clamp rows
        let total_w = frame.area().width;
        let input_inner_w = total_w.saturating_sub(2) as usize;
        let (vis_lines, cur_xy) =
            wrap_with_cursor(&self.input.lines, self.input.cursor, input_inner_w);
        let input_rows = (vis_lines.len() as u16).clamp(1, MAX_INPUT_ROWS);
        let [chat_area, input_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(input_rows + 2)])
                .areas(frame.area());

        // --- transcript: wrap every message at the chat width, stick to bottom
        let chat_w = chat_area.width.saturating_sub(2) as usize;
        let mut rows: Vec<Line> = Vec::new();
        for msg in &self.messages {
            let (tag, color_user) = (msg.role, msg.role == "user");
            let logical: Vec<String> = msg.content.split('\n').map(str::to_string).collect();
            let (wrapped, _) = wrap_with_cursor(&logical, (0, 0), chat_w.saturating_sub(2));
            rows.push(Line::from(if color_user { format!("● {tag}").bold().green() }
                                 else { format!("● {tag}").bold().magenta() }));
            for l in wrapped { rows.push(Line::from(format!("  {l}"))); }
            rows.push(Line::from(""));
        }
        let viewport = chat_area.height.saturating_sub(2) as usize;
        let max_scroll = rows.len().saturating_sub(viewport);
        if self.scroll >= max_scroll { self.stick_to_bottom = true; }
        if self.stick_to_bottom { self.scroll = max_scroll; }
        self.scroll = self.scroll.min(max_scroll);

        let transcript = Paragraph::new(Text::from(rows))
            .scroll((self.scroll as u16, 0))
            .block(Block::bordered().title("Chat").title_bottom(
                Line::from(if self.streaming.is_some() { " streaming… " } else { "" }).right_aligned(),
            ));
        frame.render_widget(transcript, chat_area);

        // --- input box (scroll vertically if content exceeds the clamp)
        let top = (cur_xy.1 as u16).saturating_sub(input_rows - 1);
        let shown: Vec<Line> = vis_lines.iter().skip(top as usize).take(input_rows as usize)
            .map(|l| Line::from(l.as_str().to_string())).collect();
        let hint = if self.input.is_empty() { " Enter: send · Alt+Enter: newline " } else { "" };
        let input_box = Paragraph::new(shown)
            .block(Block::bordered().title("Message").title_bottom(Line::from(hint).right_aligned().dim()));
        frame.render_widget(input_box, input_area);
        frame.set_cursor_position(Position::new(
            input_area.x + 1 + cur_xy.0 as u16,
            input_area.y + 1 + cur_xy.1 as u16 - top,
        ));
    }
}

fn main() -> std::io::Result<()> {
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    ratatui::run(|terminal| {
        execute!(std::io::stdout(), EnableBracketedPaste)?;
        if enhanced {
            execute!(std::io::stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
        }

        let mut app = App {
            messages: vec![],
            input: ChatInput::new(),
            scroll: 0,
            stick_to_bottom: true,
            streaming: None,
            quit: false,
        };

        let result = (|| {
            while !app.quit {
                app.pump_stream();
                terminal.draw(|frame| app.render(frame))?;
                // Short poll while streaming (keeps tokens flowing); block otherwise.
                let timeout = if app.streaming.is_some() { Duration::from_millis(33) }
                              else { Duration::from_secs(3600) };
                if event::poll(timeout)? {
                    let ev = event::read()?;
                    app.handle_event(&ev, enhanced);
                }
            }
            Ok(())
        })();

        if enhanced { execute!(std::io::stdout(), PopKeyboardEnhancementFlags)?; }
        execute!(std::io::stdout(), DisableBracketedPaste)?;
        result
    })
}
```

What to study in this example:

- **Auto-grow** falls out of computing the wrap *before* the layout: input height = wrapped rows (clamped), and `Layout::vertical([Fill(1), Length(h+2)])` does the rest.
- **Stick-to-bottom** is one boolean: stick while at the bottom; any upward scroll unsticks; scrolling back to (or past) the bottom re-sticks. Streaming tokens keep `scroll = max_scroll` only while stuck.
- **`key.is_press()`** (crossterm 0.29 helper on `KeyEvent`) keeps Windows from double-typing.
- The poll timeout is the *only* tuning between "instant token rendering" and "zero idle CPU".
- For a real LLM: replace the thread body with your HTTP/SSE client sending chunks over the channel — the UI code doesn't change. With tokio, replace the loop with `tokio::select!` over an `EventStream`, an interval, and the token channel (see 07).

## 6. Ecosystem widgets

- **`edtui`** (`0.11`, ratatui 0.30 ✓) — full Vim-emulation editor widget (modes, motions, visual select, search, mouse, soft-wrap, syntax highlighting). Best choice when users expect Vim:

```rust
use edtui::{EditorEventHandler, EditorState, EditorTheme, EditorView};

struct App { state: EditorState, events: EditorEventHandler }
// events: EditorEventHandler::default() = vim mode; ::emacs_mode() for modeless

// handle: self.events.on_key_event(key_event, &mut self.state);
//         (or on_event for mouse too)
// render:
EditorView::new(&mut app.state)
    .theme(EditorTheme::default())
    .wrap(true)
    .render(area, buf);
// text out: String::from(app.state.lines.clone())
```

- **`tui-textarea`** (`0.7`) — the historically most-used textarea (multi-line, search, undo/redo, validation). **Still pinned to ratatui `^0.29` as of June 2026** — check `cargo info tui-textarea` for a 0.30-compatible release before choosing it; using it forces your whole app onto ratatui 0.29 (where its API is: `TextArea::default()`, `textarea.input(Input::from(event))`, `textarea.lines()`, render as `&TextArea` widget).
- **`tui-input`** — single-line only; see §2.
- **`tui-prompts`** — prompt-style flows (text/password/confirm), check version compatibility.

### Choosing

| Need | Use |
|---|---|
| One-line prompt/search/command bar | `tui-input` or §1 (60 lines, no dep) |
| Chat input (Enter-to-send, auto-grow) | §5 from scratch — crates fight you on Enter semantics |
| Notes/markdown editing, plain keybindings | §3 from-scratch editor |
| Vim-style editing | `edtui` |
| Big textarea + undo/search out of the box, on ratatui 0.29 | `tui-textarea` |
