// history.rs: undo/redo as inverse operations.
//
// We never snapshot the whole buffer. Each edit records just what changed and
// where, so undoing a keystroke in a 10 MB file costs the same as in a 10 byte
// one. Consecutive typing is merged into one entry so Ctrl+Z removes a word,
// not a single letter.

use std::time::{Duration, Instant};

use crate::editor::cursor::Cursor;

/// A pause longer than this starts a new undo group.
const GROUP_PAUSE: Duration = Duration::from_secs(1);
/// Oldest entries are dropped beyond this, so history cannot grow forever.
const MAX_ENTRIES: usize = 1000;

/// What kind of edit produced an entry. Only same-kind edits can be grouped.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EditKind {
    Insert,
    Delete,
    /// A newline always stands alone, so Ctrl+Z undoes the line break first.
    Newline,
}

/// One reversible change, in character (not byte) indices.
#[derive(Clone)]
pub enum Edit {
    /// `text` was inserted at `pos`.
    Insert { pos: usize, text: String },
    /// `text` was removed at `pos`.
    Delete { pos: usize, text: String },
}

pub struct HistoryEntry {
    pub edits: Vec<Edit>,
    pub cursor_before: Cursor,
    pub cursor_after: Cursor,
}

pub struct History {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    /// `undo.len()` at the last save. `None` means the saved state has been
    /// dropped from the bottom of the stack, so it can never be reached again.
    saved: Option<usize>,
    last_at: Option<Instant>,
    last_kind: Option<EditKind>,
}

impl History {
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            saved: Some(0),
            last_at: None,
            last_kind: None,
        }
    }

    /// True when the text differs from what is on disk.
    pub fn is_dirty(&self) -> bool {
        self.saved != Some(self.undo.len())
    }

    pub fn mark_saved(&mut self) {
        self.saved = Some(self.undo.len());
    }

    /// Stop merging the next edit into the current group. Called on cursor
    /// movement, save, undo and redo.
    pub fn break_group(&mut self) {
        self.last_at = None;
        self.last_kind = None;
    }

    /// Record an edit that has already been applied to the buffer.
    pub fn record(
        &mut self,
        edit: Edit,
        kind: EditKind,
        before: Cursor,
        after: Cursor,
        now: Instant,
    ) {
        // Any new edit makes the redo branch unreachable.
        self.redo.clear();

        let recent = self
            .last_at
            .is_some_and(|t| now.duration_since(t) < GROUP_PAUSE);
        let can_group = kind != EditKind::Newline
            && self.last_kind == Some(kind)
            && recent
            && self
                .undo
                .last()
                .is_some_and(|entry| contiguous(entry, &edit));

        if can_group {
            let entry = self.undo.last_mut().expect("checked above");
            entry.edits.push(edit);
            entry.cursor_after = after;
        } else {
            self.undo.push(HistoryEntry {
                edits: vec![edit],
                cursor_before: before,
                cursor_after: after,
            });
            if self.undo.len() > MAX_ENTRIES {
                self.undo.remove(0);
                // The saved state may have just fallen off the bottom.
                self.saved = match self.saved {
                    Some(0) => None,
                    Some(n) => Some(n - 1),
                    None => None,
                };
            }
        }

        self.last_at = Some(now);
        self.last_kind = Some(kind);
    }

    /// Move the newest entry to the redo stack and hand it back so the caller
    /// can apply its inverse.
    pub fn undo(&mut self) -> Option<HistoryEntry> {
        let entry = self.undo.pop()?;
        self.redo.push(HistoryEntry {
            edits: entry.edits.clone(),
            cursor_before: entry.cursor_before,
            cursor_after: entry.cursor_after,
        });
        self.break_group();
        Some(entry)
    }

    /// Move the newest redo entry back to the undo stack and hand it back so
    /// the caller can re-apply it.
    pub fn redo(&mut self) -> Option<HistoryEntry> {
        let entry = self.redo.pop()?;
        self.undo.push(HistoryEntry {
            edits: entry.edits.clone(),
            cursor_before: entry.cursor_before,
            cursor_after: entry.cursor_after,
        });
        self.break_group();
        Some(entry)
    }
}

/// True when `edit` continues the last edit of `entry` without a gap, which is
/// what makes merging safe.
fn contiguous(entry: &HistoryEntry, edit: &Edit) -> bool {
    let Some(last) = entry.edits.last() else {
        return false;
    };
    match (last, edit) {
        (
            Edit::Insert {
                pos: last_pos,
                text: last_text,
            },
            Edit::Insert { pos, .. },
        ) => last_pos + last_text.chars().count() == *pos,
        (Edit::Delete { pos: last_pos, .. }, Edit::Delete { pos, text }) => {
            pos + text.chars().count() == *last_pos
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(line: usize, col: usize) -> Cursor {
        Cursor {
            line,
            col,
            desired_col: col,
        }
    }

    fn insert(pos: usize, text: &str) -> Edit {
        Edit::Insert {
            pos,
            text: text.to_string(),
        }
    }

    #[test]
    fn typing_within_a_second_is_one_entry() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(
            insert(0, "a"),
            EditKind::Insert,
            cursor(0, 0),
            cursor(0, 1),
            t0,
        );
        h.record(
            insert(1, "b"),
            EditKind::Insert,
            cursor(0, 1),
            cursor(0, 2),
            t0 + Duration::from_millis(100),
        );
        assert_eq!(h.undo.len(), 1);
        assert_eq!(h.undo[0].edits.len(), 2);
    }

    #[test]
    fn a_pause_starts_a_new_entry() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(
            insert(0, "a"),
            EditKind::Insert,
            cursor(0, 0),
            cursor(0, 1),
            t0,
        );
        h.record(
            insert(1, "b"),
            EditKind::Insert,
            cursor(0, 1),
            cursor(0, 2),
            t0 + Duration::from_secs(2),
        );
        assert_eq!(h.undo.len(), 2);
    }

    #[test]
    fn newline_never_groups() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(
            insert(0, "a"),
            EditKind::Insert,
            cursor(0, 0),
            cursor(0, 1),
            t0,
        );
        h.record(
            insert(1, "\n"),
            EditKind::Newline,
            cursor(0, 1),
            cursor(1, 0),
            t0 + Duration::from_millis(10),
        );
        assert_eq!(h.undo.len(), 2);
    }

    #[test]
    fn insert_and_delete_do_not_mix() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(
            insert(0, "a"),
            EditKind::Insert,
            cursor(0, 0),
            cursor(0, 1),
            t0,
        );
        h.record(
            Edit::Delete {
                pos: 0,
                text: "a".to_string(),
            },
            EditKind::Delete,
            cursor(0, 1),
            cursor(0, 0),
            t0 + Duration::from_millis(10),
        );
        assert_eq!(h.undo.len(), 2);
    }

    #[test]
    fn undo_then_redo_round_trips() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(
            insert(0, "a"),
            EditKind::Insert,
            cursor(0, 0),
            cursor(0, 1),
            t0,
        );
        let entry = h.undo().unwrap();
        assert_eq!(entry.cursor_before.col, 0);
        assert!(h.undo().is_none());
        let entry = h.redo().unwrap();
        assert_eq!(entry.cursor_after.col, 1);
        assert!(h.redo().is_none());
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut h = History::new();
        let t0 = Instant::now();
        h.record(
            insert(0, "a"),
            EditKind::Insert,
            cursor(0, 0),
            cursor(0, 1),
            t0,
        );
        h.undo();
        h.record(
            insert(0, "b"),
            EditKind::Insert,
            cursor(0, 0),
            cursor(0, 1),
            t0,
        );
        assert!(h.redo().is_none());
    }

    #[test]
    fn dirty_tracks_the_saved_position() {
        let mut h = History::new();
        let t0 = Instant::now();
        assert!(!h.is_dirty());
        h.record(
            insert(0, "a"),
            EditKind::Insert,
            cursor(0, 0),
            cursor(0, 1),
            t0,
        );
        assert!(h.is_dirty());
        h.mark_saved();
        assert!(!h.is_dirty());
        h.undo();
        assert!(h.is_dirty());
        h.redo();
        assert!(!h.is_dirty());
    }

    #[test]
    fn history_is_capped() {
        let mut h = History::new();
        let t0 = Instant::now();
        for i in 0..MAX_ENTRIES + 5 {
            h.record(
                insert(i, "x"),
                EditKind::Insert,
                cursor(0, i),
                cursor(0, i + 1),
                t0 + Duration::from_secs(i as u64 * 2),
            );
        }
        assert_eq!(h.undo.len(), MAX_ENTRIES);
    }
}
