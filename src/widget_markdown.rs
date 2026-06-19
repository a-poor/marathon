use anyhow::Result;
use markdown::mdast;

pub fn render_md_node(n: &mdast::Node) -> Result<()> {
    match n {
        mdast::Node::Root(n) => {}
        mdast::Node::Blockquote(n) => {}
        mdast::Node::FootnoteDefinition(n) => {}
        mdast::Node::MdxJsxFlowElement(n) => {}
        mdast::Node::List(n) => {}
        mdast::Node::MdxjsEsm(n) => {}
        mdast::Node::Toml(n) => {}
        mdast::Node::Yaml(n) => {}
        mdast::Node::Break(n) => {}
        mdast::Node::InlineCode(n) => {}
        mdast::Node::InlineMath(n) => {}
        mdast::Node::Delete(n) => {}
        mdast::Node::Emphasis(n) => {}
        mdast::Node::MdxTextExpression(n) => {}
        mdast::Node::FootnoteReference(n) => {}
        mdast::Node::Html(n) => {}
        mdast::Node::Image(n) => {}
        mdast::Node::ImageReference(n) => {}
        mdast::Node::MdxJsxTextElement(n) => {}
        mdast::Node::Link(n) => {}
        mdast::Node::LinkReference(n) => {}
        mdast::Node::Strong(n) => {}
        mdast::Node::Text(n) => {}
        mdast::Node::Code(n) => {}
        mdast::Node::Math(n) => {}
        mdast::Node::MdxFlowExpression(n) => {}
        mdast::Node::Heading(n) => {}
        mdast::Node::Table(n) => {}
        mdast::Node::ThematicBreak(n) => {}
        mdast::Node::TableRow(n) => {}
        mdast::Node::TableCell(n) => {}
        mdast::Node::ListItem(n) => {}
        mdast::Node::Definition(n) => {}
        mdast::Node::Paragraph(n) => {}
    }
    todo!();
}
