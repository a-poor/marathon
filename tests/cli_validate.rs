//! Validation must diagnose static input errors without reading generated files.
use std::process::{Command, Output};

fn validate(doc: &str) -> Output {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("book.md");
    std::fs::write(&path, doc).unwrap();
    Command::new(env!("CARGO_BIN_EXE_marathon"))
        .arg("validate")
        .arg(path)
        .output()
        .unwrap()
}

#[test]
fn invalid_input_targets_and_defaults_report_source_and_cell() {
    let cases = [
        (
            r#"{"type":"input","prompt":"P","target":""}"#,
            "invalid environment target",
        ),
        (
            r#"{"type":"input","prompt":"P","target":"BAD-NAME"}"#,
            "invalid environment target",
        ),
        (
            r#"{"type":"confirm","prompt":"P","target":"1NAME"}"#,
            "invalid environment target",
        ),
        (
            r#"{"type":"input","prompt":"P","target":"X=Y"}"#,
            "invalid environment target",
        ),
        (
            r#"{"type":"input","prompt":"P","target":"X\u0000"}"#,
            "invalid environment target",
        ),
        (
            r#"{"type":"input","prompt":"P","target":"NAME","default":false}"#,
            "input JSON",
        ),
        (
            r#"{"type":"confirm","prompt":"P","target":"OK","default":"yes"}"#,
            "input JSON",
        ),
        (
            r#"{"type":"select","prompt":"P","target":"CHOICE","options":["a"],"default":"b"}"#,
            "invalid default",
        ),
        (
            r#"{"type":"select","prompt":"P","target":"CHOICE","options":[]}"#,
            "no options available",
        ),
        (
            r#"{"type":"select","prompt":"P","target":"CHOICE"}"#,
            "no options available",
        ),
        (
            r#"{"type":"input","prompt":"P","target":"NAME","default":"a\u0000b"}"#,
            "NUL",
        ),
        (
            r#"{"type":"select","prompt":"P","target":"CHOICE","options":["a\u0000b"]}"#,
            "NUL",
        ),
        (
            r#"{"type":"select","prompt":"P","target":"CHOICE","option_file":""}"#,
            "nonempty path",
        ),
        (
            r#"{"type":"select","prompt":"P","target":"CHOICE","options":["a"],"default":1}"#,
            "input JSON",
        ),
        (
            r#"{"type":"input","prompt":"P","target":"NAME","default": }"#,
            "input JSON",
        ),
    ];
    for (config, expected) in cases {
        let doc =
            format!("# Heading\n\n```sh\necho never\n```\n\n```json mrthn=input\n{config}\n```\n");
        let output = validate(&doc);
        assert!(!output.status.success(), "accepted {config}");
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("book.md:7:1: cell 2"), "{error}");
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn frontmatter_environment_checks_preserve_unrelated_metadata() {
    let output = validate(
        "---\ntags: [deploy, local]\nlayout: {name: post}\ncustom: {nested: true}\nenv:\n  VALID_NAME: value\ntmp_dir:\n  var_name: _SCRATCH2\n---\n# Valid\n",
    );
    assert!(output.status.success(), "{output:?}");
    for field in [
        "env:\n  BAD-NAME: value",
        "tmp_dir:\n  var_name: 1BAD",
        "env:\n  NAME: \"a\\0b\"",
    ] {
        let output = validate(&format!("---\n{field}\n---\n"));
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("book.md:1:1: frontmatter"), "{error}");
        assert!(
            error.contains("invalid environment target") || error.contains("NUL"),
            "{error}"
        );
    }
}

#[test]
fn generated_files_and_their_defaults_are_deferred_even_for_literal_paths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("choices");
    for contents in [None, Some(""), Some("different\n")] {
        if let Some(contents) = contents {
            std::fs::write(&path, contents).unwrap();
        }
        let config = serde_json::json!({
            "type": "select", "prompt": "Pick", "target": "CHOICE",
            "option_file": path, "default": "generated-later",
        });
        let output = validate(&format!("```json mrthn=input\n{config}\n```"));
        assert!(output.status.success(), "{output:?}");
    }
}

#[test]
fn fence_metadata_and_frontmatter_parse_errors_have_source_locations() {
    let output = validate("# Heading\n\n```sh skip=invalid\necho never\n```\n");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(error.contains("book.md:3:1: cell 1"), "{error}");
    let output = validate("---\ntitle: [\n---\n");
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("book.md:"), "{error}");
    assert!(error.contains("frontmatter"), "{error}");
}
