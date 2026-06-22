//! Width-aware wrapping of styled text.
//!
//! ratatui's `List`/`Paragraph` can wrap, but neither exposes wrapping as a
//! reusable function over `Line`/`Span` that preserves per-span styling across a
//! break — which is exactly what the scrollview needs, since it lays out the
//! whole document into a flat `Vec<Line>` itself. These helpers fill that gap:
//!
//! - [`wrap_with_prefix`] greedily word-wraps styled content, keeping each span's
//!   style across line breaks and supporting a hanging indent (a first-line marker
//!   plus an aligned continuation indent).
//! - [`hard_break`] char-wraps a single run with no word boundaries — used for
//!   code, where word-wrapping is wrong.
//!
//! Display width is measured unicode-aware via `textwrap::core::display_width`, so
//! CJK/emoji count as their true terminal width.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Display width (terminal cells) of a string, unicode-aware.
pub fn display_width(s: &str) -> usize {
    textwrap::core::display_width(s)
}

/// Internal short alias.
fn width(s: &str) -> usize {
    display_width(s)
}

/// A single word: one or more styled fragments with no internal whitespace. Kept
/// together when wrapping (only split if wider than the available line).
type Word = Vec<Span<'static>>;

fn word_width(word: &[Span<'static>]) -> usize {
    word.iter().map(|s| width(&s.content)).sum()
}

/// Yield `(text, is_whitespace)` runs of a string, splitting at every
/// whitespace/non-whitespace boundary.
fn runs(s: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<bool> = None;
    for (i, ch) in s.char_indices() {
        let ws = ch.is_whitespace();
        match prev {
            Some(p) if p != ws => {
                out.push((&s[start..i], p));
                start = i;
            }
            _ => {}
        }
        prev = Some(ws);
    }
    if let Some(p) = prev {
        out.push((&s[start..], p));
    }
    out
}

/// Split styled content into words. Whitespace is dropped (it becomes the
/// single-space separators reinserted during packing). Style is preserved per
/// fragment, so a word whose styling changes mid-way (e.g. `foo` + `**bar**`
/// with no space between) stays one word made of two fragments.
fn tokenize(spans: &[Span<'static>]) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut cur: Word = Vec::new();
    for span in spans {
        for (text, is_ws) in runs(&span.content) {
            if is_ws {
                if !cur.is_empty() {
                    words.push(std::mem::take(&mut cur));
                }
            } else {
                cur.push(Span::styled(text.to_string(), span.style));
            }
        }
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Char-wrap a single word into chunks each no wider than `avail`, preserving
/// each fragment's style across the breaks. Used for over-long words and (via a
/// one-fragment word that keeps its spaces) for code lines.
pub fn hard_break(word: Word, avail: usize) -> Vec<Word> {
    let avail = avail.max(1);
    let mut out: Vec<Word> = Vec::new();
    let mut cur: Word = Vec::new();
    let mut cur_w = 0usize;
    let mut buf = String::new();
    let mut buf_style = Style::default();

    let flush_buf = |buf: &mut String, buf_style: Style, cur: &mut Word| {
        if !buf.is_empty() {
            cur.push(Span::styled(std::mem::take(buf), buf_style));
        }
    };

    for frag in &word {
        for ch in frag.content.chars() {
            let cw = width(&ch.to_string()).max(1);
            if cur_w + cw > avail && cur_w > 0 {
                flush_buf(&mut buf, buf_style, &mut cur);
                out.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            if buf.is_empty() {
                buf_style = frag.style;
            } else if buf_style != frag.style {
                cur.push(Span::styled(std::mem::take(&mut buf), buf_style));
                buf_style = frag.style;
            }
            buf.push(ch);
            cur_w += cw;
        }
    }
    flush_buf(&mut buf, buf_style, &mut cur);
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(Vec::new());
    }
    out
}

/// Greedily word-wrap `content` to `width` columns, prefixing the first produced
/// line with `first` and every continuation line with `rest`. `first` and `rest`
/// must have the same display width (the marker vs. the aligned indent under it);
/// the content area is `width - first.width()`.
///
/// Always returns at least one line (empty content yields a single line carrying
/// just the prefix), so a block never silently disappears.
pub fn wrap_with_prefix(
    content: &[Span<'static>],
    width_cols: usize,
    first: Span<'static>,
    rest: Span<'static>,
) -> Vec<Line<'static>> {
    // first/rest are assumed equal-width; reserve that many columns on the left.
    let indent = width(&first.content);
    let avail = width_cols.saturating_sub(indent).max(1);

    let words = tokenize(content);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur: Word = Vec::new();
    let mut cur_w = 0usize;

    let push_line = |lines: &mut Vec<Line<'static>>, cur: &mut Word| {
        let prefix = if lines.is_empty() {
            first.clone()
        } else {
            rest.clone()
        };
        let mut spans: Vec<Span<'static>> = Vec::with_capacity(cur.len() + 1);
        if width(&prefix.content) > 0 {
            spans.push(prefix);
        }
        spans.append(cur);
        lines.push(Line::from(spans));
    };

    for word in words {
        let wlen = word_width(&word);
        // If the word won't fit after a separating space, break the line first.
        if !cur.is_empty() && cur_w + 1 + wlen > avail {
            push_line(&mut lines, &mut cur);
            cur_w = 0;
        }
        if cur.is_empty() && wlen > avail {
            // Word alone is too wide: char-break it.
            let mut chunks = hard_break(word, avail);
            let last = chunks.pop().unwrap_or_default();
            for chunk in chunks {
                cur = chunk;
                push_line(&mut lines, &mut cur);
            }
            cur = last;
            cur_w = word_width(&cur);
        } else {
            if !cur.is_empty() {
                cur.push(Span::raw(" "));
                cur_w += 1;
            }
            cur.extend(word);
            cur_w += wlen;
        }
    }
    push_line(&mut lines, &mut cur);
    lines
}

/// Word-wrap with no prefix or indent.
pub fn wrap(content: &[Span<'static>], width_cols: usize) -> Vec<Line<'static>> {
    wrap_with_prefix(content, width_cols, Span::raw(""), Span::raw(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flatten a line back to its plain text for assertions.
    fn text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn texts(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(text).collect()
    }

    #[test]
    fn empty_content_yields_one_line() {
        let out = wrap(&[], 10);
        assert_eq!(texts(&out), vec![""]);
    }

    #[test]
    fn simple_word_wrap() {
        let out = wrap(&[Span::raw("the quick brown fox")], 9);
        assert_eq!(texts(&out), vec!["the quick", "brown fox"]);
    }

    #[test]
    fn no_wrap_when_it_fits() {
        let out = wrap(&[Span::raw("hello world")], 80);
        assert_eq!(texts(&out), vec!["hello world"]);
    }

    #[test]
    fn over_long_word_is_hard_broken() {
        let out = wrap(&[Span::raw("supercalifragilistic")], 5);
        assert_eq!(texts(&out), vec!["super", "calif", "ragil", "istic"]);
    }

    #[test]
    fn style_is_preserved_across_a_break() {
        let bold = Style::new().add_modifier(ratatui::style::Modifier::BOLD);
        let out = wrap(&[Span::styled("alpha beta gamma", bold)], 10);
        assert_eq!(texts(&out), vec!["alpha beta", "gamma"]);
        for line in &out {
            for span in &line.spans {
                if !span.content.trim().is_empty() {
                    assert_eq!(span.style, bold, "word span lost its style");
                }
            }
        }
    }

    #[test]
    fn mid_word_style_change_stays_one_word() {
        // "foo" + "bar" adjacent with no space form a single 6-wide word.
        let bold = Style::new().add_modifier(ratatui::style::Modifier::BOLD);
        let spans = vec![Span::raw("foo"), Span::styled("bar", bold)];
        let out = wrap(&spans, 6);
        assert_eq!(texts(&out), vec!["foobar"]);
        // ...and it splits into two styled fragments, not one.
        assert_eq!(out[0].spans.len(), 2);
    }

    #[test]
    fn hanging_indent_aligns_continuation() {
        let out = wrap_with_prefix(
            &[Span::raw("one two three four")],
            8,
            Span::raw("- "),
            Span::raw("  "),
        );
        assert_eq!(texts(&out), vec!["- one", "  two", "  three", "  four"]);
    }

    #[test]
    fn hard_break_keeps_spaces_for_code() {
        // A code line passed as a single space-bearing word keeps its spaces.
        let word = vec![Span::raw("a b c d e")];
        let chunks = hard_break(word, 4);
        let lines: Vec<String> = chunks
            .iter()
            .map(|w| w.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(lines, vec!["a b ", "c d ", "e"]);
    }
}
