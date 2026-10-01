//! References, selection, and prerequisite rules shared by the CLI and TUI.

use std::collections::{BTreeSet, HashSet};

use anyhow::{Context, Result, bail};
use clap::Args;

use crate::book::{BookBlock, Cancel, CodeBlockMeta, CodeBlockState, Runbook};

#[derive(Debug, Default, Args)]
pub struct Selection {
    /// Execute only these cell IDs or 1-based ordinals (repeatable).
    #[arg(long, value_name = "REF", conflicts_with_all = ["from", "to"])]
    pub cell: Vec<String>,
    /// Start at this cell ID or ordinal, inclusive. Skipped commands are not replayed.
    #[arg(long, value_name = "REF")]
    pub from: Option<String>,
    /// Stop after this cell ID or ordinal, inclusive.
    #[arg(long, value_name = "REF")]
    pub to: Option<String>,
}

impl Selection {
    /// Validate every reference before creating scratch space or executing anything.
    /// Preceding inputs are always included, even when their producers are omitted.
    pub fn resolve(&self, book: &Runbook) -> Result<BTreeSet<usize>> {
        let mut selected = BTreeSet::new();
        if !self.cell.is_empty() {
            if self.from.is_some() || self.to.is_some() {
                bail!("--cell cannot be combined with --from or --to");
            }
            for reference in &self.cell {
                selected.insert(book.resolve_cell(reference)?);
            }
        } else {
            let start = self
                .from
                .as_deref()
                .map(|r| book.resolve_cell(r))
                .transpose()?
                .unwrap_or(0);
            let end = self
                .to
                .as_deref()
                .map(|r| book.resolve_cell(r))
                .transpose()?
                .unwrap_or(book.blocks.len());
            if start > end {
                bail!("--from must precede or equal --to");
            }
            selected.extend(book.cells().filter(|&idx| idx >= start && idx <= end));
        }
        if let Some(&last) = selected.last() {
            selected
                .extend((0..last).filter(|&idx| matches!(book.blocks[idx], BookBlock::Input(_))));
        }
        Ok(selected)
    }
}

impl Runbook {
    /// Actionable cells only: prose, skipped cells, and other languages never count.
    pub fn cells(&self) -> impl Iterator<Item = usize> + '_ {
        self.blocks
            .iter()
            .enumerate()
            .filter_map(|(idx, _)| self.cell_meta(idx).map(|_| idx))
    }

    pub fn cell_meta(&self, idx: usize) -> Option<&CodeBlockMeta> {
        match self.blocks.get(idx)? {
            BookBlock::Code(c) if c.is_runnable() => Some(&c.meta),
            BookBlock::Input(c) => Some(&c.meta),
            _ => None,
        }
    }

    pub fn resolve_cell(&self, reference: &str) -> Result<usize> {
        if !reference.is_empty() && reference.bytes().all(|b| b.is_ascii_digit()) {
            return reference
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|n| self.cells().nth(n))
                .with_context(|| format!("unknown cell ordinal '{reference}' (use exec --list)"));
        }
        self.cells()
            .find(|&idx| self.cell_meta(idx).and_then(|m| m.id.as_deref()) == Some(reference))
            .with_context(|| format!("unknown cell ID '{reference}' (use exec --list)"))
    }

    pub fn cell_label(&self, idx: usize) -> String {
        let ordinal = self.cells().position(|i| i == idx).map_or(0, |n| n + 1);
        match self.cell_meta(idx).and_then(|m| m.id.as_deref()) {
            Some(id) => format!("#{ordinal} ({id})"),
            None => format!("#{ordinal}"),
        }
    }

    pub fn validate_execution(&self) -> Result<()> {
        let mut ids = HashSet::new();
        for idx in self.cells() {
            let meta = self.cell_meta(idx).expect("actionable cell");
            if let Some(id) = &meta.id {
                if !id.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                {
                    bail!(
                        "cell {}: id must start with an ASCII letter and contain only letters, digits, '_' or '-'",
                        self.cell_label(idx)
                    );
                }
                if !ids.insert(id) {
                    bail!("duplicate cell ID '{id}'");
                }
            }
        }
        for idx in self.cells() {
            if let Some(needs) = &self.cell_meta(idx).expect("actionable cell").needs {
                for reference in needs.split(',') {
                    let dependency = self
                        .resolve_cell(reference)
                        .with_context(|| format!("cell {}: needs", self.cell_label(idx)))?;
                    if dependency >= idx {
                        bail!(
                            "cell {}: prerequisite '{reference}' must be an earlier cell",
                            self.cell_label(idx)
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Every earlier input contributes environment; code dependencies are explicit.
    pub fn prerequisites(&self, idx: usize) -> BTreeSet<usize> {
        let mut result: BTreeSet<_> = (0..idx)
            .filter(|&i| matches!(self.blocks[i], BookBlock::Input(_)))
            .collect();
        if let Some(needs) = self.cell_meta(idx).and_then(|m| m.needs.as_deref()) {
            result.extend(needs.split(',').filter_map(|r| self.resolve_cell(r).ok()));
        }
        result
    }

    pub fn cell_complete(&self, idx: usize) -> bool {
        match &self.blocks[idx] {
            BookBlock::Code(c) => c.state == CodeBlockState::Success && c.cancel == Cancel::None,
            BookBlock::Input(c) => c.resolved().is_some(),
            _ => false,
        }
    }

    pub fn blocked_by(&self, idx: usize) -> Option<usize> {
        self.ancestors(idx)
            .into_iter()
            .find(|&i| !self.cell_complete(i))
    }

    /// Transitive ancestors, used to prevent changing prerequisites of active runs.
    pub fn depends_on(&self, idx: usize, ancestor: usize) -> bool {
        self.ancestors(idx).contains(&ancestor)
    }

    fn ancestors(&self, idx: usize) -> BTreeSet<usize> {
        let mut pending = self.prerequisites(idx);
        let mut seen = BTreeSet::new();
        while let Some(i) = pending.pop_last() {
            if seen.insert(i) {
                pending.extend(self.prerequisites(i));
            }
        }
        seen
    }

    pub fn next_remaining(&self) -> Option<usize> {
        self.cells().find(|&idx| !self.cell_complete(idx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_survive_prose_and_actionable_cell_insertions() {
        let source = "```sh id=build\nprintf build\n```";
        let original = Runbook::new(None::<&str>, source).unwrap();
        let edited = Runbook::new(
            None::<&str>,
            &format!(
                "# Intro\n\n```sh skip=true\nexample\n```\n\n```sh\nprintf new\n```\n\n{source}"
            ),
        )
        .unwrap();
        assert_eq!(
            original.cell_label(original.resolve_cell("build").unwrap()),
            "#1 (build)"
        );
        assert_eq!(
            edited.cell_label(edited.resolve_cell("build").unwrap()),
            "#2 (build)"
        );
        assert_eq!(
            edited.resolve_cell("2").unwrap(),
            edited.resolve_cell("build").unwrap()
        );
    }

    #[test]
    fn invalid_ids_and_prerequisites_are_rejected_during_parse() {
        for source in [
            "```sh id=1\ntrue\n```",
            "```sh id=bad.name\ntrue\n```",
            "```sh id=same\ntrue\n```\n```sh id=same\ntrue\n```",
            "```sh id=a needs=a\ntrue\n```",
            "```sh id=a needs=b\ntrue\n```\n```sh id=b\ntrue\n```",
            "```sh needs=missing\ntrue\n```",
            "```sh id=example skip=true\ntrue\n```\n```sh needs=example\ntrue\n```",
            "```sh id=a\ntrue\n```\n```sh needs=a,\ntrue\n```",
        ] {
            assert!(Runbook::new(None::<&str>, source).is_err(), "{source}");
        }
    }

    #[test]
    fn prerequisite_checks_include_ancestors_and_prior_inputs() {
        let mut book = Runbook::new(None::<&str>, "```sh id=a\ntrue\n```\n```sh id=b needs=a\ntrue\n```\n```json mrthn=input id=c needs=b\n{\"type\":\"input\",\"prompt\":\"?\",\"target\":\"ANSWER\"}\n```\n```sh id=d\ntrue\n```").unwrap();
        assert_eq!(book.blocked_by(3), Some(0));
        assert!(book.depends_on(3, 0));
        for idx in [0, 1] {
            if let BookBlock::Code(c) = &mut book.blocks[idx] {
                c.finish(true, Some(0));
            }
        }
        assert_eq!(book.blocked_by(3), Some(2));
        book.input_at_mut(2).unwrap().answer("yes".into()).unwrap();
        assert_eq!(book.blocked_by(3), None);
        if let BookBlock::Code(c) = &mut book.blocks[0] {
            c.begin_run();
        }
        assert_eq!(book.blocked_by(3), Some(0));
    }
}
