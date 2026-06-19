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

/// Pulls the yaml frontmatter string from an mdast document.
///
/// If there is no frontmatter or if it isn't yaml, will error.
///
/// TODO: This should probably return a custom error that can be
/// checked against. Especially for the `check` subcommand.
pub(crate) fn get_frontmatter_node(root: &Root) -> Result<String> {
    for n in root.children.iter() {
        if let Node::Yaml(yn) = n {
            return Ok(yn.value.clone());
        }
    }
    Err(anyhow!("no frontmatter yaml found"))
}
