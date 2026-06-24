//! The starter template for `marathon new`.
//!
//! Kept in the library (not `main.rs`) so it can be unit-tested — in particular,
//! round-tripped through [`crate::book::Runbook::new`] to prove the scaffold we
//! emit is always a valid runbook.

/// Derive a human-ish title from a file stem: `deploy_prod-api` → `Deploy prod api`.
/// Falls back to `Runbook` for an empty/odd stem.
pub fn title_from_stem(stem: &str) -> String {
    let cleaned = stem.replace(['-', '_'], " ");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return "Runbook".to_string();
    }
    let mut chars = cleaned.chars();
    let first: String = chars.next().unwrap().to_uppercase().collect();
    format!("{first}{}", chars.as_str())
}

/// The minimal starter runbook: frontmatter (title + one env var), a heading, a
/// line of prose, and one runnable shell cell. Valid standalone markdown that other
/// tools render fine and `marathon` treats as a runbook.
pub fn runbook_template(title: &str) -> String {
    // Quote + escape the title so an odd stem can't produce invalid YAML.
    let title = title.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"---
title: "{title}"
description: A new runbook.
env:
  GREETING: "Hello"
---

# {title}

This is an ordinary markdown file — open it in any editor or renderer and it reads
fine. Open it in `marathon` and the shell blocks below become runnable cells.

```sh
echo "$GREETING from your new runbook"
```

## Next steps

Add more shell cells, mark illustrative ones with `skip=true`, and collect input
with a `json mrthn=input` block. See the samples for more.
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::book::Runbook;

    #[test]
    fn title_casing_and_separators() {
        assert_eq!(title_from_stem("deploy_prod-api"), "Deploy prod api");
        assert_eq!(title_from_stem("hello"), "Hello");
        assert_eq!(title_from_stem(""), "Runbook");
        assert_eq!(title_from_stem("  "), "Runbook");
    }

    #[test]
    fn scaffold_parses_as_a_valid_runbook() {
        let doc = runbook_template(&title_from_stem("my-runbook"));
        let rb = Runbook::new(None::<&str>, &doc).expect("scaffold should be a valid runbook");
        assert_eq!(rb.frontmatter.title.as_deref(), Some("My runbook"));
        // Exactly one runnable shell cell.
        let runnable = rb
            .blocks
            .iter()
            .filter(|b| matches!(b, crate::book::BookBlock::Code(c) if c.is_runnable()))
            .count();
        assert_eq!(runnable, 1);
    }

    #[test]
    fn odd_title_stays_valid_yaml() {
        // A stem yielding quotes/colons must not break the frontmatter.
        let doc = runbook_template("we\"ird: title");
        let rb = Runbook::new(None::<&str>, &doc).expect("escaped title should parse");
        assert_eq!(rb.frontmatter.title.as_deref(), Some("we\"ird: title"));
    }
}
