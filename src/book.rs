use serde::{Deserialize, Serialize};

/// The frontmatter from a runbook
///
/// Expected to be deserialized from yaml.
#[derive(Debug, Serialize, Deserialize)]
pub struct BookFrontmatter {}

/// The key/value data stored in a md code block
///
/// Expected to be deserialized from `serde-kv`
#[derive(Debug, Serialize, Deserialize)]
pub struct CodeBlockMeta {
    /// Is this code block runnable
    pub skip: Option<bool>,
}
