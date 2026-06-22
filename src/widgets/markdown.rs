//! Render a markdown (`mdast`) block node into styled, width-wrapped lines.
//!
//! Scope is "lean core" (see DESIGN.md / the rendering plan): headings,
//! paragraphs, ordered/unordered lists, blockquotes, thematic breaks, and the
//! inline set bold / italic / strikethrough / inline-code / links. Anything else
//! degrades to its plain text rather than being dropped.
//!
//! The output is a flat `Vec<Line>` already wrapped to `width`, so the scrollview
//! can concatenate blocks into one scrollable document. Fenced code blocks are
//! *not* handled here — they are separate runnable cells.

use markdown::mdast::Node;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};

use crate::widgets::wrap::{display_width, hard_break, wrap, wrap_with_prefix};

/// Style applied to inline `` `code` `` runs.
fn code_style() -> Style {
    Style::new().fg(Color::Cyan)
}

/// Style for a link's text.
fn link_style(base: Style) -> Style {
    base.add_modifier(Modifier::UNDERLINED).fg(Color::Blue)
}

/// Content style for a heading of the given depth (1-6).
fn heading_style(depth: u8) -> Style {
    let s = Style::new().add_modifier(Modifier::BOLD);
    match depth {
        1 => s.fg(Color::LightBlue),
        2 => s.fg(Color::LightCyan),
        _ => s,
    }
}

/// Render a single top-level markdown block to wrapped lines.
pub fn render_md(node: &Node, width: usize) -> Vec<Line<'static>> {
    render_block(node, width, 0)
}

fn render_block(node: &Node, width: usize, indent: usize) -> Vec<Line<'static>> {
    match node {
        Node::Heading(h) => {
            let hashes = format!("{} ", "#".repeat(h.depth as usize));
            let first = Span::styled(hashes.clone(), Style::new().dim());
            let rest = Span::raw(" ".repeat(display_width(&hashes)));
            let spans = inline(&h.children, heading_style(h.depth));
            wrap_with_prefix(&spans, width, first, rest)
        }
        Node::Paragraph(p) => {
            let spans = inline(&p.children, Style::default());
            indent_lines(wrap(&spans, width.saturating_sub(indent)), indent)
        }
        Node::List(l) => render_list(l.ordered, l.start.unwrap_or(1), &l.children, width, indent),
        Node::Blockquote(b) => render_blockquote(&b.children, width, indent),
        Node::ThematicBreak(_) => {
            vec![Line::from("─".repeat(width.max(1))).dim()]
        }
        Node::Code(c) => render_code_plain(&c.value, width, indent),
        // Unknown / unsupported block: fall back to its inline text so nothing
        // silently disappears.
        other => {
            let spans = inline(std::slice::from_ref(other), Style::default());
            if spans.is_empty() {
                Vec::new()
            } else {
                indent_lines(wrap(&spans, width.saturating_sub(indent)), indent)
            }
        }
    }
}

/// Walk inline nodes into styled spans, accumulating style top-down.
fn inline(nodes: &[Node], base: Style) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    inline_into(nodes, base, &mut out);
    out
}

fn inline_into(nodes: &[Node], base: Style, out: &mut Vec<Span<'static>>) {
    for n in nodes {
        match n {
            Node::Text(t) => out.push(Span::styled(t.value.clone(), base)),
            Node::Strong(s) => inline_into(&s.children, base.add_modifier(Modifier::BOLD), out),
            Node::Emphasis(e) => inline_into(&e.children, base.add_modifier(Modifier::ITALIC), out),
            Node::Delete(d) => {
                inline_into(&d.children, base.add_modifier(Modifier::CROSSED_OUT), out)
            }
            Node::InlineCode(c) => {
                out.push(Span::styled(c.value.clone(), base.patch(code_style())))
            }
            Node::Link(l) => inline_into(&l.children, link_style(base), out),
            // A hard break inside a wrapped paragraph collapses to a space.
            Node::Break(_) => out.push(Span::styled(" ".to_string(), base)),
            // Anything else: recurse children if present, else ignore.
            other => {
                if let Some(kids) = other.children() {
                    inline_into(kids, base, out);
                }
            }
        }
    }
}

fn render_list(
    ordered: bool,
    start: u32,
    items: &[Node],
    width: usize,
    indent: usize,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let marker = if ordered {
            format!("{}. ", start as usize + i)
        } else {
            "• ".to_string()
        };
        let children = item.children().map(Vec::as_slice).unwrap_or(&[]);
        render_item(children, &marker, width, indent, &mut out);
    }
    out
}

fn render_item(
    children: &[Node],
    marker: &str,
    width: usize,
    indent: usize,
    out: &mut Vec<Line<'static>>,
) {
    let lead = " ".repeat(indent);
    let mw = display_width(marker);
    let first_prefix = Span::styled(format!("{lead}{marker}"), Style::new().dim());
    let cont = format!("{lead}{}", " ".repeat(mw));
    let mut emitted = false;

    for child in children {
        match child {
            // Nested list: recurse one level deeper.
            Node::List(l) => {
                out.extend(render_list(
                    l.ordered,
                    l.start.unwrap_or(1),
                    &l.children,
                    width,
                    indent + mw,
                ));
            }
            // Paragraph (the common case) or any other block: render its inline
            // content; the first such block carries the marker.
            other => {
                let spans = inline(std::slice::from_ref(other), Style::default());
                let first = if emitted {
                    Span::raw(cont.clone())
                } else {
                    first_prefix.clone()
                };
                emitted = true;
                out.extend(wrap_with_prefix(
                    &spans,
                    width,
                    first,
                    Span::raw(cont.clone()),
                ));
            }
        }
    }

    // An empty item still shows its marker.
    if !emitted {
        out.push(Line::from(first_prefix));
    }
}

fn render_blockquote(children: &[Node], width: usize, indent: usize) -> Vec<Line<'static>> {
    let lead = " ".repeat(indent);
    let bar = Span::styled(format!("{lead}│ "), Style::new().fg(Color::DarkGray));
    let mut out = Vec::new();
    for (i, child) in children.iter().enumerate() {
        if i > 0 {
            // Blank quoted line between block children.
            out.push(Line::from(bar.clone()));
        }
        let spans = inline(std::slice::from_ref(child), Style::new().italic());
        out.extend(wrap_with_prefix(&spans, width, bar.clone(), bar.clone()));
    }
    out
}

/// Render a (rare, nested) code block as dimmed, char-wrapped lines.
fn render_code_plain(value: &str, width: usize, indent: usize) -> Vec<Line<'static>> {
    let lead = " ".repeat(indent);
    let prefix = Span::raw(lead);
    let mut out = Vec::new();
    for line in value.lines() {
        for chunk in hard_break(
            vec![Span::raw(line.to_string())],
            width.saturating_sub(indent).max(1),
        ) {
            let mut spans = vec![prefix.clone()];
            spans.extend(chunk);
            out.push(Line::from(spans).fg(Color::Green));
        }
    }
    out
}

/// Left-pad already-wrapped lines by `indent` columns (used when a paragraph
/// sits inside an indented context).
fn indent_lines(lines: Vec<Line<'static>>, indent: usize) -> Vec<Line<'static>> {
    if indent == 0 {
        return lines;
    }
    let pad = " ".repeat(indent);
    lines
        .into_iter()
        .map(|mut l| {
            l.spans.insert(0, Span::raw(pad.clone()));
            l
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_block(src: &str) -> Node {
        let opts = markdown::ParseOptions::gfm();
        let root = markdown::to_mdast(src, &opts).unwrap();
        root.children().unwrap().first().unwrap().clone()
    }

    fn text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn heading_gets_hash_prefix_and_bold() {
        let node = first_block("## Hello world");
        let lines = render_md(&node, 80);
        assert_eq!(text(&lines[0]), "## Hello world");
        // The content span carries bold.
        let content = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("Hello"))
            .unwrap();
        assert!(content.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn paragraph_inline_styles() {
        let node = first_block("plain **bold** and *italic* and `code`");
        let lines = render_md(&node, 80);
        assert_eq!(text(&lines[0]), "plain bold and italic and code");
        let find = |needle: &str| {
            lines[0]
                .spans
                .iter()
                .find(|s| s.content.as_ref() == needle)
                .unwrap()
                .style
        };
        assert!(find("bold").add_modifier.contains(Modifier::BOLD));
        assert!(find("italic").add_modifier.contains(Modifier::ITALIC));
        assert_eq!(find("code").fg, Some(Color::Cyan));
    }

    #[test]
    fn unordered_list_marker() {
        let node = first_block("- one\n- two");
        let lines = render_md(&node, 80);
        assert_eq!(text(&lines[0]), "• one");
        assert_eq!(text(&lines[1]), "• two");
    }

    #[test]
    fn ordered_list_numbers() {
        let node = first_block("1. first\n2. second");
        let lines = render_md(&node, 80);
        assert_eq!(text(&lines[0]), "1. first");
        assert_eq!(text(&lines[1]), "2. second");
    }

    #[test]
    fn list_item_wraps_with_hanging_indent() {
        let node = first_block("- alpha beta gamma delta");
        let lines = render_md(&node, 9);
        assert_eq!(text(&lines[0]), "• alpha");
        // Continuation lines align under the text, not the marker.
        assert!(text(&lines[1]).starts_with("  "));
    }

    #[test]
    fn blockquote_bar_prefix() {
        let node = first_block("> quoted text");
        let lines = render_md(&node, 80);
        assert_eq!(text(&lines[0]), "│ quoted text");
    }

    #[test]
    fn link_is_underlined() {
        let node = first_block("see [the docs](https://example.com)");
        let lines = render_md(&node, 80);
        let link = lines[0]
            .spans
            .iter()
            .find(|s| s.content.contains("docs"))
            .unwrap();
        assert!(link.style.add_modifier.contains(Modifier::UNDERLINED));
    }

    #[test]
    fn thematic_break_is_a_rule() {
        let node = first_block("---\n");
        let lines = render_md(&node, 10);
        assert_eq!(text(&lines[0]), "──────────");
    }
}
