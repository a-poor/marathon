use anyhow::{Result, anyhow};
use markdown::mdast::{Node, Root};

/// Parse a markdown document as an mdast node
/// with the expected options and with the
/// correct error type.
pub(crate) fn parse_markdown(doc: &str) -> Result<Root> {
    // GFM + frontmatter
    let mut opt = markdown::ParseOptions::gfm();
    opt.constructs.frontmatter = true;

    // Parse the ast
    let ast = markdown::to_mdast(doc, &opt)
        .map_err(|msg| anyhow!("unable to parse markdown: {:?}", msg))?;

    // Confirm that node is root?
    match ast {
        Node::Root(n) => Ok(n),
        _ => Err(anyhow!("Expected root node got: {:?}", ast)),
    }
}

/// Optional YAML configuration; ordinary Markdown needs no frontmatter.
pub(crate) fn get_frontmatter_node(root: &Root) -> Option<String> {
    root.children.iter().find_map(|n| match n {
        Node::Yaml(yaml) => Some(yaml.value.clone()),
        _ => None,
    })
}
