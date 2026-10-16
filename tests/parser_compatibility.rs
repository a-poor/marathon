use marathon::book::{BookBlock, Runbook};

fn book(doc: &str) -> Runbook {
    Runbook::new(None::<&str>, doc).unwrap()
}

#[test]
fn only_explicit_top_level_shell_fences_are_runnable() {
    let doc = "> ```sh\n> echo nested\n> ```\n\n- item\n\n  ```bash\n  echo nested\n  ```\n\n    echo indented\n\n```\necho unlabeled\n```\n\n```python\nprint('display')\n```\n\n```sh skip=true\necho skipped\n```\n\n~~~zsh\necho runnable\n~~~\n";
    let book = book(doc);
    let runnable: Vec<_> = book
        .blocks
        .iter()
        .filter_map(|b| match b {
            BookBlock::Code(c) if c.is_runnable() => Some(c.content.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(runnable, ["echo runnable"]);
}

#[test]
fn decoding_fence_info_cannot_create_a_language_separator() {
    for info in [
        "sh&#32;skip=true",
        "sh&#9;skip=true",
        "sh\u{a0}skip=true",
        "sh\\ skip=true",
    ] {
        let book = book(&format!("```{info}\necho display\n```"));
        let BookBlock::Code(code) = &book.blocks[0] else {
            panic!("expected code")
        };
        assert!(!code.is_runnable(), "{info}");
    }
    let book = book("```s&#104; id=first\necho first\n```\n\n```sh needs=first\necho second\n```");
    let BookBlock::Code(code) = &book.blocks[0] else {
        panic!("expected code")
    };
    assert!(code.is_runnable());
    assert_eq!(code.lang, "sh");
}

#[test]
fn source_copy_preserves_unicode_bom_and_line_endings() {
    for eol in ["\n", "\r\n", "\r"] {
        for prefix in ["", "\u{feff}", "---\ntitle: Example\n---\n"] {
            let heading = "# Café 界 🦀";
            let paragraph = format!("first **bold**{eol}second é");
            let doc = format!("{prefix}{heading}{eol}{eol}{paragraph}{eol}");
            let book = book(&doc);
            assert_eq!(book.copy_text(0).as_deref(), Some(heading), "{doc:?}");
            assert_eq!(
                book.copy_text(1).as_deref(),
                Some(paragraph.as_str()),
                "{doc:?}"
            );
        }
    }
}

#[test]
fn code_bodies_keep_original_interior_line_endings() {
    for eol in ["\n", "\r\n", "\r"] {
        let body = format!("printf 'café'{eol}printf '界'");
        for ending in [format!("{eol}```"), String::new()] {
            let book = book(&format!("```sh{eol}{body}{ending}"));
            assert_eq!(book.copy_text(0).as_deref(), Some(body.as_str()));
        }
    }
}
