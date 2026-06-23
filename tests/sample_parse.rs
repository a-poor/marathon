use marathon::book::{BookBlock, Runbook};

#[test]
fn interactive_sample_parses_with_input_cells() {
    let doc = std::fs::read_to_string("samples/interactive.md").unwrap();
    let rb = Runbook::new(Some("samples/interactive.md"), &doc).unwrap();
    let inputs = rb
        .blocks
        .iter()
        .filter(|b| matches!(b, BookBlock::Input(_)))
        .count();
    assert!(
        inputs >= 1,
        "expected at least one input cell, got {inputs}"
    );
}
