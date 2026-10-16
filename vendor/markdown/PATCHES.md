# Local markdown-rs patch

This directory contains the source and MIT license from the crates.io `markdown`
1.0.0 release. `Cargo.toml` retains its runtime dependencies/features and omits
upstream development tools and workspace members. Marathon uses a direct path
dependency so both the application and the separate fuzz workspace use this copy.

The only source change is in `src/construct/heading_setext.rs`: when a setext
underline is reinterpreted as the start of a paragraph, mark the old paragraph
enter event as data before scheduling its removal. The edit map is deferred until
the resolver ends; otherwise a subsequent underline finds the stale enter and
creates an unbalanced heading event stream, panicking during AST construction.

Reproducer: `":\n -\n--\nt\n-"`. Expected: two depth-2 headings, with text `:` and
`--\nt`. Regression coverage lives in `src/util.rs`, `tests/cli_validate.rs`, and
`fuzz/seeds/runbook/setext-heading.txt`.

The upstream file at <https://github.com/wooorm/markdown-rs/blob/main/src/construct/heading_setext.rs>
was identical to the released file when checked on 2026-10-16. Remove this local
copy when a released version passes these regressions and the fuzz campaigns.
