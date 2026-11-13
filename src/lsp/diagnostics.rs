// diagnostics.rs: what the server said is wrong, per file. A plain model: no
// GL, no window, no threads. The UI reads it, the client writes it.
//
// Ranges are stored as *character* indexes, not LSP positions, so the whole
// rest of the editor (view, gutter, hover hit test) can use them without
// knowing about encodings. `lsp::position` does the conversion once, when a
// publish arrives.
//
// Republished text is not tracked: while the user types ahead of the server,
// ranges would otherwise point at the wrong characters. Instead, every edit
// *shifts* the stored ranges (see `Diagnostics::shift`), which is exact for
// inserting or deleting text before or after a range, and only fuzzy for an
// edit inside one. The spec allows either; shifting is the cheaper option and
// keeps the squiggle on the same word instead of dropping it.

use crate::editor::buffer::Buffer;

/// Severity, ordered so the worst one wins in the gutter and the counts.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    /// LSP's numbers: 1 error, 2 warning, 3 information, 4 hint.
    pub fn from_lsp(n: i64) -> Severity {
        match n {
            1 => Severity::Error,
            2 => Severity::Warning,
            3 => Severity::Information,
            _ => Severity::Hint,
        }
    }
}

/// One diagnostic, already converted into the buffer's character space.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Diagnostic {
    /// Character range, `end` exclusive. May be empty (zero-width: the server
    /// points at a position, not a span).
    pub range: crate::lsp::position::CharRange,
    pub severity: Severity,
    pub message: String,
    /// Server name, e.g. `rust-analyzer`. Shown in the tooltip.
    pub source: Option<String>,
}

impl Diagnostic {
    /// Line the diagnostic starts on, and the line it ends on. Uses the buffer
    /// because a character index alone cannot know its line.
    pub fn lines(&self, buffer: &Buffer) -> (usize, usize) {
        let (a, _) = buffer.line_col_of(self.range.start);
        let (b, _) = buffer.line_col_of(self.range.end);
        (a, b)
    }
}

/// Every diagnostic for one buffer.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    /// Replace the whole list: `publishDiagnostics` is a full snapshot, never a
    /// delta.
    pub fn replace(&mut self, items: Vec<Diagnostic>) {
        self.items = items;
        // Worst first, so a hover on a shared range shows the error. Stable
        // sort, so the server's order survives within a severity.
        self.items.sort_by_key(|d| d.severity);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[Diagnostic] {
        &self.items
    }

    /// Counts for the status bar: `(errors, warnings)`.
    pub fn counts(&self) -> (usize, usize) {
        let errors = self
            .items
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let warnings = self
            .items
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();
        (errors, warnings)
    }

    /// Worst severity on one line, for the gutter marker.
    pub fn worst_on_line(&self, buffer: &Buffer, line: usize) -> Option<Severity> {
        self.items
            .iter()
            .filter(|d| {
                let (start, end) = d.lines(buffer);
                // A zero-width diagnostic on line N belongs to line N.
                line >= start && line <= end
            })
            .map(|d| d.severity)
            .min()
    }

    /// The diagnostic under a character index, worst first. Hover uses this.
    pub fn at(&self, buffer: &Buffer, pos: usize) -> Option<&Diagnostic> {
        self.items.iter().find(|d| {
            if d.range.len() == 0 {
                // A point diagnostic: hovering anywhere on its line finds it.
                buffer.line_col_of(d.range.start).0 == buffer.line_col_of(pos).0
            } else {
                pos >= d.range.start && pos <= d.range.end
            }
        })
    }

    /// Move every range for an edit at `at` that removed `removed` characters
    /// and added `added`.
    ///
    /// The rules, in one place:
    /// * an edit entirely before the range shifts it by the delta;
    /// * an edit entirely after the range leaves it alone;
    /// * an edit that overlaps a range shrinks or grows it: the start is pulled
    ///   to `at` if it was past the edit, the end is nudged by the delta.
    ///
    /// A range made empty this way is kept (a point diagnostic), never dropped,
    /// so a squiggle does not flicker away while the user is fixing the line.
    pub fn shift(&mut self, at: usize, removed: usize, added: usize) {
        let end_of_edit = at + removed;
        // Signed delta, applied to the end of anything that straddles the edit.
        let delta = added as isize - removed as isize;
        for d in &mut self.items {
            let start = d.range.start;
            let end = d.range.end;
            let new_start = if start >= end_of_edit {
                (start as isize + delta).max(0) as usize
            } else if start > at {
                at
            } else {
                start
            };
            let new_end = if end >= end_of_edit {
                (end as isize + delta).max(0) as usize
            } else if end > at {
                at
            } else {
                end
            };
            d.range.start = new_start;
            d.range.end = new_end.max(new_start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::position::CharRange;

    fn diag(start: usize, end: usize, severity: Severity) -> Diagnostic {
        Diagnostic {
            range: CharRange { start, end },
            severity,
            message: "m".into(),
            source: None,
        }
    }

    fn store(items: Vec<Diagnostic>) -> Diagnostics {
        let mut d = Diagnostics::default();
        d.replace(items);
        d
    }

    #[test]
    fn counts_split_errors_from_warnings() {
        let d = store(vec![
            diag(0, 1, Severity::Error),
            diag(2, 3, Severity::Warning),
            diag(4, 5, Severity::Error),
            diag(6, 7, Severity::Hint),
        ]);
        assert_eq!(d.counts(), (2, 1));
    }

    #[test]
    fn the_worst_severity_wins_on_a_line() {
        let b = Buffer::from_text("one\ntwo\nthree\n");
        let d = store(vec![
            diag(4, 7, Severity::Warning),
            diag(5, 6, Severity::Error),
        ]);
        assert_eq!(d.worst_on_line(&b, 1), Some(Severity::Error));
        assert_eq!(d.worst_on_line(&b, 0), None);
    }

    #[test]
    fn replace_is_a_snapshot_not_a_merge() {
        let mut d = store(vec![diag(0, 1, Severity::Error)]);
        d.replace(vec![]);
        assert!(d.is_empty());
    }

    #[test]
    fn an_edit_before_a_range_moves_it() {
        let mut d = store(vec![diag(10, 12, Severity::Error)]);
        d.shift(2, 0, 3); // three characters inserted at 2
        assert_eq!(d.items()[0].range, CharRange { start: 13, end: 15 });
        d.shift(2, 3, 0); // and removed again
        assert_eq!(d.items()[0].range, CharRange { start: 10, end: 12 });
    }

    #[test]
    fn an_edit_after_a_range_does_not_move_it() {
        let mut d = store(vec![diag(2, 4, Severity::Error)]);
        d.shift(8, 0, 5);
        assert_eq!(d.items()[0].range, CharRange { start: 2, end: 4 });
    }

    #[test]
    fn an_edit_inside_a_range_stretches_or_shrinks_it() {
        let mut d = store(vec![diag(4, 10, Severity::Error)]);
        d.shift(6, 0, 2); // insert two characters in the middle
        assert_eq!(d.items()[0].range, CharRange { start: 4, end: 12 });
        let mut d = store(vec![diag(4, 10, Severity::Error)]);
        d.shift(6, 4, 0); // delete the rest of the word
        assert_eq!(d.items()[0].range, CharRange { start: 4, end: 6 });
    }

    #[test]
    fn an_edit_covering_a_range_leaves_a_point_not_a_wreck() {
        let mut d = store(vec![diag(4, 5, Severity::Error)]);
        d.shift(0, 20, 0); // everything deleted
        assert_eq!(d.items()[0].range, CharRange { start: 0, end: 0 });
    }

    #[test]
    fn hover_finds_a_point_diagnostic_on_its_line() {
        let b = Buffer::from_text("abc\ndef\n");
        // Character 4 is the start of line 1.
        let d = store(vec![diag(4, 4, Severity::Error)]);
        assert!(d.at(&b, 0).is_none(), "line 0 has nothing");
        assert!(d.at(&b, 5).is_some(), "line 1 finds the point diagnostic");
    }

    #[test]
    fn a_hover_on_a_shared_range_prefers_the_error() {
        let b = Buffer::from_text("value\n");
        let d = store(vec![
            diag(0, 5, Severity::Warning),
            diag(0, 5, Severity::Error),
        ]);
        assert_eq!(d.at(&b, 2).unwrap().severity, Severity::Error);
    }
}
