//! Owned Markdown nodes used by the runbook and terminal renderer.
//!
//! Comrak's arena is confined to parsing; no arena lifetimes escape into run state.
use std::fmt;

#[derive(Debug, Clone)]
pub struct Point {
    pub line: usize,
    pub column: usize,
    pub offset: usize,
}

#[derive(Debug, Clone)]
pub struct Position {
    pub start: Point,
    pub end: Point,
}

#[derive(Debug, Clone)]
pub struct Container {
    pub children: Vec<Node>,
    pub position: Option<Position>,
}

#[derive(Debug, Clone)]
pub struct Heading {
    pub children: Vec<Node>,
    pub depth: u8,
    pub position: Option<Position>,
}

#[derive(Debug, Clone)]
pub struct List {
    pub children: Vec<Node>,
    pub ordered: bool,
    pub start: Option<u32>,
    pub position: Option<Position>,
}

#[derive(Debug, Clone)]
pub struct Text {
    pub value: String,
    pub position: Option<Position>,
}

#[derive(Debug, Clone)]
pub struct Code {
    pub lang: Option<String>,
    pub meta: Option<String>,
    pub value: String,
    pub position: Option<Position>,
}

#[derive(Debug, Clone)]
pub enum Node {
    Heading(Heading),
    Paragraph(Container),
    List(List),
    Blockquote(Container),
    ThematicBreak(Text),
    Code(Code),
    Text(Text),
    Strong(Container),
    Emphasis(Container),
    Delete(Container),
    InlineCode(Text),
    Link(Container),
    Break(Text),
    Yaml(Text),
    Toml(Text),
    Other(Container),
}

impl Node {
    pub fn children(&self) -> Option<&Vec<Node>> {
        match self {
            Self::Heading(n) => Some(&n.children),
            Self::List(n) => Some(&n.children),
            Self::Paragraph(n)
            | Self::Blockquote(n)
            | Self::Strong(n)
            | Self::Emphasis(n)
            | Self::Delete(n)
            | Self::Link(n)
            | Self::Other(n) => Some(&n.children),
            _ => None,
        }
    }

    pub fn position(&self) -> Option<&Position> {
        match self {
            Self::Heading(n) => n.position.as_ref(),
            Self::List(n) => n.position.as_ref(),
            Self::Code(n) => n.position.as_ref(),
            Self::Paragraph(n)
            | Self::Blockquote(n)
            | Self::Strong(n)
            | Self::Emphasis(n)
            | Self::Delete(n)
            | Self::Link(n)
            | Self::Other(n) => n.position.as_ref(),
            Self::Text(n)
            | Self::InlineCode(n)
            | Self::ThematicBreak(n)
            | Self::Break(n)
            | Self::Yaml(n)
            | Self::Toml(n) => n.position.as_ref(),
        }
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(n) | Self::InlineCode(n) | Self::Yaml(n) | Self::Toml(n) => {
                f.write_str(&n.value)
            }
            Self::Code(n) => f.write_str(&n.value),
            _ => {
                if let Some(children) = self.children() {
                    for child in children {
                        write!(f, "{child}")?;
                    }
                }
                Ok(())
            }
        }
    }
}
