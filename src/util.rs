use anyhow::{Result, anyhow};
use markdown::mdast::{Node, Root};

/// Parse a markdown document as an mdast node
/// with the expected options and with the
/// correct error type.
pub(crate) fn parse_markdown(doc: &str) -> Result<Root> {
    // GFM + frontmatter
    let mut opt = markdown::ParseOptions::gfm();
    // markdown 1.0.0 can panic when an unclosed frontmatter candidate precedes
    // a container (minimized fuzz input: "---\n\n>"). Only enable the extension
    // when its required closing fence exists; otherwise this is ordinary GFM.
    // Match the extension's exact markers, trailing spaces/tabs, BOM, and EOLs.
    let mut lines = doc
        .strip_prefix('\u{feff}')
        .unwrap_or(doc)
        .split(['\r', '\n']);
    let marker = lines.next().unwrap_or("").trim_end_matches([' ', '\t']);
    opt.constructs.frontmatter = matches!(marker, "---" | "+++")
        && lines.any(|line| line.trim_end_matches([' ', '\t']) == marker);

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
