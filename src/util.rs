use crate::markdown::{Code, Container, Heading, List, Node, Point, Position, Text};
use anyhow::Result;
use comrak::{
    Arena, Options,
    nodes::{AstNode, ListType, NodeValue},
    parse_document,
};

type Root = Container;

/// Source lines including their original EOL, with byte offsets for clipboard copies.
fn lines(doc: &str) -> Vec<(usize, &str)> {
    let bytes = doc.as_bytes();
    let mut result = Vec::new();
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        if matches!(bytes[at], b'\r' | b'\n') {
            at += if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            result.push((start, &doc[start..at]));
            start = at;
        } else {
            at += 1;
        }
    }
    result.push((start, &doc[start..]));
    result
}

fn line_text(line: &str) -> &str {
    line.trim_end_matches(['\r', '\n'])
}

pub(crate) fn parse_markdown(doc: &str) -> Result<Root> {
    let source_lines = lines(doc);
    let mut frontmatter = None;
    let first = line_text(source_lines[0].1)
        .strip_prefix('\u{feff}')
        .unwrap_or(line_text(source_lines[0].1));
    let marker = first.trim_end_matches([' ', '\t']);
    let mut masked = doc.to_owned();
    // Match Marathon's existing delimiter rules, including BOM, trailing tabs,
    // and lone CRs. Blank configuration in-place so all source offsets survive.
    if matches!(marker, "---" | "+++")
        && let Some(end) = source_lines
            .iter()
            .skip(1)
            .position(|(_, line)| line_text(line).trim_end_matches([' ', '\t']) == marker)
            .map(|i| i + 1)
    {
        let start = source_lines.get(1).map(|l| l.0).unwrap_or(doc.len());
        let value = doc[start..source_lines[end].0]
            .strip_suffix("\r\n")
            .or_else(|| doc[start..source_lines[end].0].strip_suffix(['\r', '\n']))
            .unwrap_or(&doc[start..source_lines[end].0])
            .to_owned();
        let text = Text {
            value,
            position: None,
        };
        frontmatter = Some(if marker == "---" {
            Node::Yaml(text)
        } else {
            Node::Toml(text)
        });
        let end = source_lines[end].0 + source_lines[end].1.len();
        let blank: String = doc[..end]
            .bytes()
            .map(|b| {
                if matches!(b, b'\r' | b'\n') {
                    b as char
                } else {
                    ' '
                }
            })
            .collect();
        masked.replace_range(..end, &blank);
    }
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.parse.leave_footnote_definitions = true;
    let arena = Arena::new();
    let root = parse_document(&arena, &masked, &options);
    let mut children: Vec<Node> = frontmatter.into_iter().collect();
    children.extend(
        root.children()
            .map(|n| convert(n, &source_lines, doc.len())),
    );
    Ok(Container {
        children,
        position: None,
    })
}

fn convert<'a>(node: &'a AstNode<'a>, lines: &[(usize, &str)], len: usize) -> Node {
    let data = node.data();
    let point = |line: usize, column: usize, end: bool| {
        let offset = lines
            .get(line.saturating_sub(1))
            .map(|(offset, text)| {
                offset
                    + column
                        .saturating_sub(usize::from(!end))
                        .min(line_text(text).len())
            })
            .unwrap_or(len);
        Point {
            line,
            column: column + usize::from(end),
            offset,
        }
    };
    let position = Some(Position {
        start: point(
            data.sourcepos.start.line,
            data.sourcepos.start.column,
            false,
        ),
        end: point(data.sourcepos.end.line, data.sourcepos.end.column, true),
    });
    let text = |value: String| Text {
        value,
        position: position.clone(),
    };
    let children = node.children().map(|n| convert(n, lines, len)).collect();
    let container = Container {
        children,
        position: position.clone(),
    };
    match &data.value {
        NodeValue::Heading(h) => Node::Heading(Heading {
            depth: h.level,
            children: container.children,
            position,
        }),
        NodeValue::Paragraph => Node::Paragraph(container),
        NodeValue::BlockQuote => Node::Blockquote(container),
        NodeValue::List(l) => Node::List(List {
            ordered: l.list_type == ListType::Ordered,
            start: Some(l.start as u32),
            children: container.children,
            position,
        }),
        NodeValue::ThematicBreak => Node::ThematicBreak(text(String::new())),
        NodeValue::Text(t) => Node::Text(text(t.to_string())),
        NodeValue::Code(c) => Node::InlineCode(text(c.literal.clone())),
        NodeValue::Strong => Node::Strong(container),
        NodeValue::Emph => Node::Emphasis(container),
        NodeValue::Strikethrough => Node::Delete(container),
        NodeValue::Link(_) => Node::Link(container),
        NodeValue::LineBreak => Node::Break(text(String::new())),
        NodeValue::SoftBreak => {
            let eol = lines
                .get(data.sourcepos.start.line.saturating_sub(1))
                .map(|(_, s)| &s[line_text(s).len()..])
                .unwrap_or("\n");
            Node::Text(text(eol.to_owned()))
        }
        NodeValue::CodeBlock(c) => {
            // Split the raw info string before decoding escapes/entities. An
            // encoded space in a language must not turn display-only code into
            // a runnable shell cell (for example `sh&#32;skip=true`).
            let opening = lines
                .get(data.sourcepos.start.line.saturating_sub(1))
                .map(|(_, s)| line_text(s))
                .unwrap_or("");
            let fence = (c.fence_char as char).to_string().repeat(c.fence_length);
            let info = if c.fenced {
                opening
                    .find(&fence)
                    .map(|at| &opening[at + fence.len()..])
                    .unwrap_or("")
                    .trim_matches([' ', '\t'])
            } else {
                ""
            };
            let (lang, meta) = info.split_once([' ', '\t']).unwrap_or((info, ""));
            let lang = decode_info(lang);
            let meta = decode_info(meta.trim_matches([' ', '\t']));
            // Remove only the structural final EOL, retaining interior bytes.
            let value = c
                .literal
                .strip_suffix("\r\n")
                .or_else(|| c.literal.strip_suffix(['\r', '\n']))
                .unwrap_or(&c.literal)
                .to_owned();
            Node::Code(Code {
                lang: (!lang.is_empty()).then_some(lang),
                meta: (!meta.is_empty()).then_some(meta),
                value,
                position,
            })
        }
        // Images and raw HTML were not rendered by the existing lean renderer.
        NodeValue::Image(_) | NodeValue::HtmlInline(_) | NodeValue::HtmlBlock(_) => {
            Node::Other(Container {
                children: vec![],
                position,
            })
        }
        _ => Node::Other(container),
    }
}

/// Ask the parser to decode Markdown fence escapes, without interpreting inline
/// markup. The common unescaped case needs no additional parse.
fn decode_info(raw: &str) -> String {
    if !raw.contains(['&', '\\', '\0']) {
        return raw.to_owned();
    }
    let arena = Arena::new();
    let root = parse_document(&arena, &format!("~~~ {raw}\n~~~"), &Options::default());
    root.children()
        .find_map(|n| match &n.data().value {
            NodeValue::CodeBlock(c) => Some(c.info.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

pub(crate) fn get_frontmatter_node(root: &Root) -> Option<String> {
    root.children.iter().find_map(|n| match n {
        Node::Yaml(yaml) => Some(yaml.value.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_setext_headings_after_a_reinterpreted_underline() {
        for eol in ["\n", "\r\n", "\r"] {
            for marker in ["-", "="] {
                let doc = [":", " -", "--", "t", marker].join(eol);
                let root = parse_markdown(&doc).unwrap();
                assert_eq!(root.children.len(), 2);
                for (index, (node, expected)) in root
                    .children
                    .iter()
                    .zip([":".to_owned(), format!("--{eol}t")])
                    .enumerate()
                {
                    let Node::Heading(heading) = node else {
                        panic!("expected a heading: {node:?}");
                    };
                    assert_eq!(
                        heading.depth,
                        if index == 1 && marker == "=" { 1 } else { 2 }
                    );
                    assert_eq!(node.to_string(), expected);
                    let position = node.position().unwrap();
                    assert_eq!(position.start.line, if index == 0 { 1 } else { 3 });
                    assert_eq!(position.end.line, if index == 0 { 2 } else { 5 });
                }
            }
        }
    }

    #[test]
    fn unclosed_frontmatter_candidates_remain_ordinary_markdown() {
        for doc in ["---\n\n>", "+++\n\n>", "---\r\r>", "\u{feff}---\r\n\r\n>"] {
            let root = parse_markdown(doc).unwrap();
            assert!(get_frontmatter_node(&root).is_none());
            assert!(
                root.children
                    .iter()
                    .any(|node| matches!(node, Node::Blockquote(_)))
            );
        }
    }

    #[test]
    fn frontmatter_delimiters_preserve_line_endings_and_source_positions() {
        for eol in ["\n", "\r\n", "\r"] {
            for bom in ["", "\u{feff}"] {
                let doc = format!("{bom}--- \t{eol}title: example{eol}---\t{eol}# Body");
                let root = parse_markdown(&doc).unwrap();
                assert_eq!(
                    get_frontmatter_node(&root).as_deref(),
                    Some("title: example")
                );
                let heading = root.children.last().unwrap();
                assert!(matches!(heading, Node::Heading(_)));
                assert_eq!(heading.position().unwrap().start.line, 4);
            }
        }
        let root = parse_markdown("+++\nignored = true\n+++\n# Body").unwrap();
        assert!(matches!(root.children[0], Node::Toml(_)));
        for closing in [" ---", "----", "---x", "+++"] {
            let root = parse_markdown(&format!("---\ntitle: example\n{closing}")).unwrap();
            assert!(get_frontmatter_node(&root).is_none());
        }
    }
}
