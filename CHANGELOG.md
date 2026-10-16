# Changelog

## [0.5.0] - 2026-10-16

### Changed

- Replaced the Markdown parser with Comrak, preserving runbook execution rules,
  fence metadata, source copying, and code-body line endings. Local profiling
  showed faster startup and lower memory use for large runbooks.
- Raised the minimum supported Rust version to 1.89.

### Fixed

- Addressed Markdown parser crashes discovered through fuzzing, with regression
  coverage for unclosed frontmatter and setext-heading edge cases.
- Corrected Unicode header alignment for wide characters, combining accents,
  and emoji.
- Clear macOS download quarantine before Homebrew runs the installed executable
  to generate shell completions.

### Verification

- Added bounded parsing/output fuzz campaigns and output property tests.
- Expanded packaged TUI smoke coverage for inputs, recovery, prerequisites,
  concurrent execution, cancellation, cleanup, paging, search, and resizing.
- Verified archives on all five release targets and Homebrew installation/removal
  on both macOS architectures. Fixed Linux smoke-test process reaping.

### Platform support

- Shell execution is supported on macOS and Linux. Windows archives receive
  packaging/startup checks; native Windows execution is not supported.
- Interactive command stdin/PTYs and managed background services remain unsupported.

Earlier releases are documented on the [GitHub releases page](https://github.com/a-poor/marathon/releases).

[0.5.0]: https://github.com/a-poor/marathon/compare/v0.4.0...v0.5.0
