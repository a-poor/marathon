//! Literal, case-insensitive search over the bounded rendered document.

use std::ops::Range;

use ratatui::text::Line;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Hit {
    pub block: usize,
    pub row: usize,
    pub columns: Range<usize>,
}

#[derive(Clone, Copy)]
enum Jump {
    First,
    Next,
    Previous,
}

#[derive(Clone, Default)]
pub(super) struct Search {
    pub query: String,
    pub hits: Vec<Hit>,
    pub current: Option<usize>,
    pub dirty: bool,
    jump: Option<Jump>,
}

impl Search {
    pub fn set_query(&mut self, query: String) {
        if self.query != query {
            self.query = query;
            self.dirty = true;
            self.jump = Some(Jump::First);
        }
    }

    pub fn step(&mut self, backwards: bool) {
        self.jump = Some(if backwards {
            Jump::Previous
        } else {
            Jump::Next
        });
    }

    /// Reindex only after the query or rendered content changes. Keep the current
    /// hit anchored to its block so changes in earlier cells do not move it.
    pub fn update(
        &mut self,
        lines: &[Line<'_>],
        ranges: &[Range<usize>],
        selected: usize,
    ) -> Option<Hit> {
        if self.dirty {
            let previous = self.current.and_then(|i| self.hits.get(i)).cloned();
            self.hits.clear();
            let query = self
                .query
                .chars()
                .flat_map(char::to_lowercase)
                .collect::<String>();
            if !query.is_empty() {
                for (block, range) in ranges.iter().enumerate() {
                    for (row, line) in lines[range.clone()].iter().enumerate() {
                        let text = line.to_string();
                        for columns in matching_columns(&text, &query) {
                            self.hits.push(Hit {
                                block,
                                row,
                                columns,
                            });
                        }
                    }
                }
            }
            self.current = previous
                .as_ref()
                .and_then(|old| self.hits.iter().position(|hit| hit == old));
            self.dirty = false;
        }
        let jump = self.jump.take()?;
        if self.hits.is_empty() {
            self.current = None;
            return None;
        }
        let first = || {
            self.hits
                .iter()
                .position(|hit| hit.block >= selected)
                .unwrap_or(0)
        };
        let index = match (jump, self.current) {
            (Jump::First, _) | (Jump::Next, None) => first(),
            (Jump::Previous, None) => self
                .hits
                .iter()
                .rposition(|hit| hit.block <= selected)
                .unwrap_or(self.hits.len() - 1),
            (Jump::Next, Some(i)) => (i + 1) % self.hits.len(),
            (Jump::Previous, Some(i)) => (i + self.hits.len() - 1) % self.hits.len(),
        };
        self.current = Some(index);
        Some(self.hits[index].clone())
    }

    pub fn count_label(&self) -> String {
        if self.hits.is_empty() {
            "no matches".into()
        } else {
            format!("{}/{}", self.current.map_or(0, |i| i + 1), self.hits.len())
        }
    }
}

/// Lowercasing may expand a character (e.g. İ). Map folded bytes back to source
/// character boundaries before converting to terminal display columns.
fn matching_columns(text: &str, query: &str) -> Vec<Range<usize>> {
    let mut folded = String::new();
    let mut source = Vec::new();
    for (start, ch) in text.char_indices() {
        let previous_len = folded.len();
        folded.extend(ch.to_lowercase());
        source.extend(std::iter::repeat_n(
            start..start + ch.len_utf8(),
            folded.len() - previous_len,
        ));
    }
    let mut columns = Vec::new();
    for (start, matched) in folded.match_indices(query) {
        let from = source[start].start;
        let to = source[start + matched.len() - 1].end;
        let column = Line::raw(&text[..from]).width();
        let range = column..column + Line::raw(&text[from..to]).width();
        if columns.last() != Some(&range) {
            columns.push(range);
        }
    }
    columns
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_matches_cross_spans_and_keep_unicode_display_columns() {
        assert_eq!(matching_columns("界 café CAFÉ", "café"), [3..7, 8..12]);
        let expanded = matching_columns("界 İ!", "i");
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0], 3..4);
        assert_eq!(matching_columns("a.*b A.*B", ".*"), [1..3, 6..8]);
        let mut search = Search::default();
        search.set_query("hello".into());
        let lines = [Line::from(vec!["he".into(), "llo".into()])];
        search.update(&lines, std::slice::from_ref(&(0..1)), 0);
        assert_eq!(search.hits.len(), 1);
        search.set_query("ΟΣ".into());
        search.update(&[Line::raw("ΟΣ")], std::slice::from_ref(&(0..1)), 0);
        assert_eq!(
            search.hits.len(),
            1,
            "query and text use identical lowercase rules"
        );
    }

    #[test]
    fn search_wraps_in_both_directions_and_handles_no_matches() {
        let lines = [Line::raw("hello"), Line::raw("hello hello")];
        let ranges = [0..1, 1..2];
        let mut search = Search::default();
        search.set_query("HELLO".into());
        assert_eq!(search.update(&lines, &ranges, 1).unwrap().block, 1);
        assert_eq!(search.count_label(), "2/3");
        search.step(true);
        search.update(&lines, &ranges, 1);
        assert_eq!(search.current, Some(0));
        search.step(true);
        search.update(&lines, &ranges, 0);
        assert_eq!(search.current, Some(2));
        search.step(false);
        search.update(&lines, &ranges, 1);
        assert_eq!(search.current, Some(0));
        search.set_query("missing".into());
        assert!(search.update(&lines, &ranges, 0).is_none());
        assert_eq!(search.count_label(), "no matches");
        search.set_query(String::new());
        assert!(search.update(&lines, &ranges, 0).is_none());
        assert!(search.hits.is_empty());
    }

    #[test]
    fn revisions_keep_hit_in_its_block_without_requesting_a_jump() {
        let mut search = Search::default();
        search.set_query("target".into());
        search.update(
            &[Line::raw("before"), Line::raw("target")],
            &[0..1, 1..2],
            0,
        );
        search.dirty = true;
        let lines = [Line::raw("before"), Line::raw("extra"), Line::raw("target")];
        assert!(search.update(&lines, &[0..2, 2..3], 1).is_none());
        assert_eq!(search.current, Some(0));
        assert_eq!(search.hits[0].block, 1);
    }
}
