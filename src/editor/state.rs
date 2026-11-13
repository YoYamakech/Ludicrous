// state.rs: the editor as a whole: text + cursor + scroll position.
// `apply` is the only place that mutates any of it, so every change (typing,
// saving, scrolling) follows exactly one path.

use std::time::{Duration, Instant};

use crate::editor::buffer::Buffer;
use crate::editor::command::Command;
use crate::editor::cursor::Cursor;
use crate::editor::history::{Edit, EditKind, History};

/// How long a quit warning stays armed. A second Ctrl+Q inside this window
/// discards the unsaved changes; after it, the warning has to be asked for
/// again.
const QUIT_ARM_WINDOW: Duration = Duration::from_secs(3);

/// What the app should do after a command has been applied.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Control {
    Continue,
    Quit,
}

pub struct Editor {
    pub buffer: Buffer,
    pub cursor: Cursor,
    /// First visible line (0 = top of file).
    pub scroll: usize,
    /// First visible column (0 = left edge of the text area). Long lines are
    /// scrolled horizontally instead of being clipped at the right edge.
    pub scroll_x: usize,
    /// How many lines fit in the text area; set by the UI before drawing.
    pub rows: usize,
    /// How many columns fit in the text area; set by the UI before drawing.
    pub cols: usize,
    /// Short message for the status bar (e.g. a failed save). Cleared as soon
    /// as the next command runs, so it never sticks around forever.
    pub status: Option<String>,
    /// When the user first asked to quit with unsaved changes. `None` means no
    /// warning is showing. Kept here (not in app.rs) so the window close button
    /// and Ctrl+Q share exactly one guard.
    quit_armed: Option<Instant>,
    /// Undo/redo stacks. Also the source of truth for "is this file dirty?".
    pub history: History,
    /// Where a shift-selection started. `None` means nothing is selected; the
    /// selection is always the range between this and the cursor.
    pub anchor: Option<Cursor>,
}

impl Editor {
    pub fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            cursor: Cursor::new(),
            scroll: 0,
            scroll_x: 0,
            rows: 1,
            cols: 80,
            status: None,
            quit_armed: None,
            history: History::new(),
            anchor: None,
        }
    }

    /// The selected range as `(start, end)` character indices, or `None` when
    /// nothing is selected. `start` is always before `end`.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let a = self.buffer.char_index(anchor.line, anchor.col);
        let b = self.buffer.char_index(self.cursor.line, self.cursor.col);
        if a == b {
            None
        } else if a < b {
            Some((a, b))
        } else {
            Some((b, a))
        }
    }

    /// The selected text, for the clipboard.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection()?;
        Some(self.buffer.rope.slice(start..end).to_string())
    }

    /// Delete the selection as one undoable step and return the removed text.
    fn delete_selection(&mut self) -> Option<String> {
        let (start, end) = self.selection()?;
        let text = self.buffer.rope.slice(start..end).to_string();
        let before = self.cursor;
        self.buffer.remove_at(start, end - start);
        // Put the cursor where the selection began.
        let (line, col) = self.buffer.line_col_of(start);
        self.cursor.line = line;
        self.cursor.col = col;
        self.cursor.desired_col = col;
        self.anchor = None;
        self.history.record_entry(
            vec![Edit::Delete {
                pos: start,
                text: text.clone(),
            }],
            before,
            self.cursor,
        );
        Some(text)
    }

    /// Replace the selection (or the cursor position) with `text` as one
    /// undoable step. Used by typing, paste and cut.
    pub fn insert_text(&mut self, text: &str) {
        let before = self.cursor;
        let mut edits = Vec::new();
        if let Some((start, end)) = self.selection() {
            let removed = self.buffer.rope.slice(start..end).to_string();
            self.buffer.remove_at(start, end - start);
            let (line, col) = self.buffer.line_col_of(start);
            self.cursor.line = line;
            self.cursor.col = col;
            edits.push(Edit::Delete {
                pos: start,
                text: removed,
            });
        }
        self.anchor = None;
        let pos = self.buffer.char_index(self.cursor.line, self.cursor.col);
        self.buffer.insert_str_at(pos, text);
        edits.push(Edit::Insert {
            pos,
            text: text.to_string(),
        });
        // Move the cursor to the end of what was inserted.
        let end = pos + text.chars().count();
        let (line, col) = self.buffer.line_col_of(end);
        self.cursor.line = line;
        self.cursor.col = col;
        self.cursor.desired_col = col;
        self.history.record_entry(edits, before, self.cursor);
        self.sync_modified();
    }

    /// True when the text differs from the last saved version.
    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    /// Keep `Buffer::modified` in step with the history. The buffer flag is
    /// what the UI and the save path already read, so we mirror it rather than
    /// duplicating the dirty logic in two places.
    fn sync_modified(&mut self) {
        self.buffer.modified = self.history.is_dirty();
    }

    /// Decide whether a quit request may exit. Dirty text arms a warning
    /// instead; a second request within `QUIT_ARM_WINDOW` goes through.
    /// Used by both Ctrl+Q and the window close button.
    pub fn request_quit(&mut self) -> Control {
        if !self.is_dirty() {
            return Control::Quit;
        }
        if let Some(at) = self.quit_armed {
            if at.elapsed() < QUIT_ARM_WINDOW {
                return Control::Quit;
            }
        }
        self.quit_armed = Some(Instant::now());
        self.status = Some("Unsaved changes: press Ctrl+Q again to discard".to_string());
        Control::Continue
    }

    /// Start a cursor movement. `extend` keeps (or starts) a selection.
    fn begin_move(&mut self, extend: bool) {
        self.history.break_group();
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
    }

    fn move_left(&mut self) {
        if self.cursor.col > 0 {
            self.cursor.col -= 1;
        } else if self.cursor.line > 0 {
            self.cursor.line -= 1;
            self.cursor.col = self.buffer.line_len(self.cursor.line);
        }
        self.cursor.desired_col = self.cursor.col;
    }

    fn move_right(&mut self) {
        if self.cursor.col < self.buffer.line_len(self.cursor.line) {
            self.cursor.col += 1;
        } else if self.cursor.line + 1 < self.buffer.line_count() {
            self.cursor.line += 1;
            self.cursor.col = 0;
        }
        self.cursor.desired_col = self.cursor.col;
    }

    fn move_up(&mut self) {
        if self.cursor.line > 0 {
            self.cursor.line -= 1;
            // Aim for the remembered column, not the clamped one.
            self.cursor.col = self
                .cursor
                .desired_col
                .min(self.buffer.line_len(self.cursor.line));
        }
    }

    fn move_down(&mut self) {
        if self.cursor.line + 1 < self.buffer.line_count() {
            self.cursor.line += 1;
            self.cursor.col = self
                .cursor
                .desired_col
                .min(self.buffer.line_len(self.cursor.line));
        }
    }

    pub fn apply(&mut self, cmd: Command) -> Control {
        // A status message only describes the command that produced it, so any
        // new command clears it first. Any key other than Ctrl+Q also cancels a
        // pending quit warning.
        self.status = None;
        if cmd != Command::Quit {
            self.quit_armed = None;
        }
        match cmd {
            Command::Insert(ch) => {
                if ch == '\n' || ch == '\r' {
                    return self.apply(Command::Newline);
                }
                // Typing over a selection replaces it in one undo step.
                if self.selection().is_some() {
                    self.insert_text(&ch.to_string());
                    return Control::Continue;
                }
                let before = self.cursor;
                let (l, c) = (self.cursor.line, self.cursor.col);
                let pos = self.buffer.char_index(l, c);
                self.buffer.insert_char(l, c, ch);
                self.cursor.col += 1;
                self.cursor.desired_col = self.cursor.col;
                self.history.record(
                    Edit::Insert {
                        pos,
                        text: ch.to_string(),
                    },
                    EditKind::Insert,
                    before,
                    self.cursor,
                    Instant::now(),
                );
            }
            Command::Newline => {
                if self.selection().is_some() {
                    self.insert_text("\n");
                    return Control::Continue;
                }
                let before = self.cursor;
                let pos = self.buffer.char_index(self.cursor.line, self.cursor.col);
                let (l, c) = self
                    .buffer
                    .insert_newline(self.cursor.line, self.cursor.col);
                self.cursor.line = l;
                self.cursor.col = c;
                self.cursor.desired_col = c;
                self.history.record(
                    Edit::Insert {
                        pos,
                        text: "\n".to_string(),
                    },
                    EditKind::Newline,
                    before,
                    self.cursor,
                    Instant::now(),
                );
            }
            Command::Backspace => {
                if self.selection().is_some() {
                    self.delete_selection();
                    return Control::Continue;
                }
                let before = self.cursor;
                // Work out what is about to disappear before deleting it, so
                // the history entry can put exactly that text back.
                let (l, c) = (self.cursor.line, self.cursor.col);
                let removed = if c > 0 {
                    self.buffer.char_at(self.buffer.char_index(l, c - 1))
                } else if l > 0 {
                    Some('\n')
                } else {
                    None
                };
                let (l, c) = self.buffer.backspace(self.cursor.line, self.cursor.col);
                self.cursor.line = l;
                self.cursor.col = c;
                self.cursor.desired_col = c;
                if let Some(ch) = removed {
                    let pos = self.buffer.char_index(l, c);
                    self.history.record(
                        Edit::Delete {
                            pos,
                            text: ch.to_string(),
                        },
                        EditKind::Delete,
                        before,
                        self.cursor,
                        Instant::now(),
                    );
                }
            }
            Command::Delete => {
                if self.selection().is_some() {
                    self.delete_selection();
                    return Control::Continue;
                }
                let before = self.cursor;
                let (l, c) = (self.cursor.line, self.cursor.col);
                let removed = self.buffer.char_at(self.buffer.char_index(l, c));
                self.buffer.delete(l, c);
                self.cursor.desired_col = self.cursor.col;
                if let Some(ch) = removed {
                    let pos = self.buffer.char_index(l, c);
                    self.history.record(
                        Edit::Delete {
                            pos,
                            text: ch.to_string(),
                        },
                        EditKind::Delete,
                        before,
                        self.cursor,
                        Instant::now(),
                    );
                }
            }
            Command::Undo => {
                if let Some(entry) = self.history.undo() {
                    // Walk the edits backwards: the last change is undone first.
                    for edit in entry.edits.iter().rev() {
                        match edit {
                            Edit::Insert { pos, text } => {
                                self.buffer.remove_at(*pos, text.chars().count());
                            }
                            Edit::Delete { pos, text } => {
                                self.buffer.insert_str_at(*pos, text);
                            }
                        }
                    }
                    self.cursor = entry.cursor_before;
                }
            }
            Command::Redo => {
                if let Some(entry) = self.history.redo() {
                    for edit in entry.edits.iter() {
                        match edit {
                            Edit::Insert { pos, text } => {
                                self.buffer.insert_str_at(*pos, text);
                            }
                            Edit::Delete { pos, text } => {
                                self.buffer.remove_at(*pos, text.chars().count());
                            }
                        }
                    }
                    self.cursor = entry.cursor_after;
                }
            }
            Command::Left | Command::SelectLeft => {
                self.begin_move(matches!(cmd, Command::SelectLeft));
                self.move_left();
            }
            Command::Right | Command::SelectRight => {
                self.begin_move(matches!(cmd, Command::SelectRight));
                self.move_right();
            }
            Command::Up | Command::SelectUp => {
                self.begin_move(matches!(cmd, Command::SelectUp));
                self.move_up();
            }
            Command::Down | Command::SelectDown => {
                self.begin_move(matches!(cmd, Command::SelectDown));
                self.move_down();
            }
            Command::Home | Command::SelectHome => {
                self.begin_move(matches!(cmd, Command::SelectHome));
                self.cursor.col = 0;
                self.cursor.desired_col = 0;
            }
            Command::End | Command::SelectEnd => {
                self.begin_move(matches!(cmd, Command::SelectEnd));
                self.cursor.col = self.buffer.line_len(self.cursor.line);
                self.cursor.desired_col = self.cursor.col;
            }
            Command::PageUp => {
                self.begin_move(false);
                self.cursor.line = self.cursor.line.saturating_sub(self.rows.max(1));
                self.cursor.col = self
                    .cursor
                    .desired_col
                    .min(self.buffer.line_len(self.cursor.line));
            }
            Command::PageDown => {
                self.begin_move(false);
                let last = self.buffer.line_count().saturating_sub(1);
                self.cursor.line = (self.cursor.line + self.rows.max(1)).min(last);
                self.cursor.col = self
                    .cursor
                    .desired_col
                    .min(self.buffer.line_len(self.cursor.line));
            }
            Command::SelectAll => {
                self.history.break_group();
                self.anchor = Some(Cursor::new());
                let last = self.buffer.line_count().saturating_sub(1);
                self.cursor.line = last;
                self.cursor.col = self.buffer.line_len(last);
                self.cursor.desired_col = self.cursor.col;
            }
            Command::Copy => {
                // The app reads `selected_text()` and talks to the clipboard;
                // the editor only reports what is selected.
                if self.selection().is_none() {
                    self.status = Some("Nothing selected".to_string());
                }
            }
            Command::Cut => {
                if self.selection().is_none() {
                    self.status = Some("Nothing selected".to_string());
                }
                // The app copies `selected_text()` first, then this deletes it.
                self.delete_selection();
            }
            Command::Paste(text) => {
                if text.is_empty() {
                    return Control::Continue;
                }
                self.insert_text(&text);
            }
            Command::Scroll(lines) => {
                // Fractional values accumulate in `scroll` as whole lines only;
                // the remainder is dropped, which is fine for wheel steps.
                let delta = lines.round() as isize;
                let max = self.buffer.line_count().saturating_sub(self.rows.max(1)) as isize;
                let next = (self.scroll as isize + delta).clamp(0, max.max(0));
                self.scroll = next as usize;
            }
            Command::Save => {
                self.history.break_group();
                if let Err(e) = self.buffer.save() {
                    eprintln!("ludicrous: save failed: {e}");
                    // Keep the buffer dirty: the text on disk is still the old
                    // version, so the user must not think the save worked.
                    self.status = Some(format!("Save failed: {e}"));
                } else {
                    self.history.mark_saved();
                }
            }
            Command::Quit => return self.request_quit(),
        }

        self.cursor
            .clamp(self.buffer.line_count(), |l| self.buffer.line_len(l));
        self.sync_modified();
        Control::Continue
    }

    /// Scroll as little as possible so the cursor stays on screen.
    /// Called by the app once it knows how many lines and columns fit.
    pub fn ensure_visible(&mut self, rows: usize, cols: usize) {
        self.rows = rows.max(1);
        self.cols = cols.max(1);
        if self.cursor.line < self.scroll {
            self.scroll = self.cursor.line;
        } else if self.cursor.line >= self.scroll + self.rows {
            self.scroll = self.cursor.line + 1 - self.rows;
        }
        // Horizontal: keep one column of context on the right so the caret is
        // never glued to the edge.
        let visible = self.cols.saturating_sub(1).max(1);
        if self.cursor.col < self.scroll_x {
            self.scroll_x = self.cursor.col;
        } else if self.cursor.col >= self.scroll_x + visible {
            self.scroll_x = self.cursor.col + 1 - visible;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn editor(text: &str) -> Editor {
        Editor::new(Buffer {
            rope: Rope::from_str(text),
            path: None,
            modified: false,
            line_ending: crate::editor::buffer::LineEnding::Lf,
        })
    }

    #[test]
    fn typing_moves_cursor_right() {
        let mut ed = editor("");
        ed.apply(Command::Insert('h'));
        ed.apply(Command::Insert('i'));
        assert_eq!(ed.buffer.line(0), "hi");
        assert_eq!((ed.cursor.line, ed.cursor.col), (0, 2));
    }

    #[test]
    fn newline_then_typing_lands_on_second_line() {
        let mut ed = editor("");
        ed.apply(Command::Insert('a'));
        ed.apply(Command::Newline);
        ed.apply(Command::Insert('b'));
        assert_eq!(ed.buffer.line(0), "a");
        assert_eq!(ed.buffer.line(1), "b");
        assert_eq!((ed.cursor.line, ed.cursor.col), (1, 1));
    }

    #[test]
    fn up_down_keeps_desired_column() {
        let mut ed = editor("long line\nx\nlong line");
        ed.apply(Command::End);
        assert_eq!(ed.cursor.col, 9);
        ed.apply(Command::Down); // short line clamps to column 1
        assert_eq!(ed.cursor.col, 1);
        ed.apply(Command::Down); // back to a long line restores column 9
        assert_eq!(ed.cursor.col, 9);
    }

    #[test]
    fn home_and_end() {
        let mut ed = editor("hello");
        ed.apply(Command::End);
        assert_eq!(ed.cursor.col, 5);
        ed.apply(Command::Home);
        assert_eq!(ed.cursor.col, 0);
    }

    #[test]
    fn left_wraps_to_previous_line() {
        let mut ed = editor("ab\ncd");
        ed.apply(Command::Down);
        assert_eq!((ed.cursor.line, ed.cursor.col), (1, 0));
        ed.apply(Command::Left);
        assert_eq!((ed.cursor.line, ed.cursor.col), (0, 2));
    }

    #[test]
    fn backspace_at_start_of_second_line_joins() {
        let mut ed = editor("ab\ncd");
        ed.apply(Command::Down);
        ed.apply(Command::Backspace);
        assert_eq!(ed.buffer.line(0), "abcd");
        assert_eq!((ed.cursor.line, ed.cursor.col), (0, 2));
    }

    #[test]
    fn scroll_is_clamped_to_content() {
        let mut ed = editor("1\n2\n3\n4\n5");
        ed.rows = 2;
        ed.apply(Command::Scroll(100.0));
        assert_eq!(ed.scroll, 3);
        ed.apply(Command::Scroll(-100.0));
        assert_eq!(ed.scroll, 0);
    }

    #[test]
    fn ensure_visible_follows_cursor() {
        let mut ed = editor(&"x\n".repeat(20));
        ed.rows = 5;
        for _ in 0..10 {
            ed.apply(Command::Down);
        }
        ed.ensure_visible(5, 80);
        assert!(ed.cursor.line >= ed.scroll);
        assert!(ed.cursor.line < ed.scroll + 5);
    }

    #[test]
    fn quit_returns_control_quit() {
        let mut ed = editor("");
        assert_eq!(ed.apply(Command::Quit), Control::Quit);
    }

    #[test]
    fn dirty_quit_warns_then_quits() {
        let mut ed = editor("text");
        ed.apply(Command::Insert('!'));
        assert_eq!(ed.apply(Command::Quit), Control::Continue);
        assert_eq!(
            ed.status.as_deref(),
            Some("Unsaved changes: press Ctrl+Q again to discard")
        );
        // Second Ctrl+Q inside the window discards.
        assert_eq!(ed.apply(Command::Quit), Control::Quit);
    }

    #[test]
    fn other_key_cancels_quit_warning() {
        let mut ed = editor("text");
        ed.apply(Command::Insert('!'));
        ed.apply(Command::Quit);
        ed.apply(Command::Left);
        assert!(ed.status.is_none());
        // The warning is disarmed, so the next Ctrl+Q warns again.
        assert_eq!(ed.apply(Command::Quit), Control::Continue);
    }

    #[test]
    fn saving_cancels_quit_warning() {
        let mut ed = editor("text");
        ed.apply(Command::Insert('!'));
        ed.apply(Command::Quit);
        ed.apply(Command::Save); // fails (no path) but still cancels the warning
        assert_eq!(ed.apply(Command::Quit), Control::Continue);
    }

    #[test]
    fn failed_save_sets_status_and_keeps_dirty() {
        let mut ed = editor("text");
        ed.apply(Command::Insert('!'));
        ed.apply(Command::Save); // no path, so this must fail
        let msg = ed.status.clone().expect("status message");
        assert!(msg.starts_with("Save failed:"), "{msg}");
        assert!(ed.buffer.modified);
    }

    #[test]
    fn next_command_clears_status() {
        let mut ed = editor("text");
        ed.apply(Command::Save);
        assert!(ed.status.is_some());
        ed.apply(Command::Left);
        assert!(ed.status.is_none());
    }

    #[test]
    fn undo_removes_typed_text_and_redo_puts_it_back() {
        let mut ed = editor("");
        ed.apply(Command::Insert('a'));
        ed.apply(Command::Insert('b'));
        assert!(ed.is_dirty());
        ed.apply(Command::Undo);
        assert_eq!(ed.buffer.line(0), "");
        assert_eq!((ed.cursor.line, ed.cursor.col), (0, 0));
        ed.apply(Command::Redo);
        assert_eq!(ed.buffer.line(0), "ab");
        assert_eq!((ed.cursor.line, ed.cursor.col), (0, 2));
    }

    #[test]
    fn undo_restores_a_deleted_newline() {
        let mut ed = editor("ab\ncd");
        ed.apply(Command::Down);
        ed.apply(Command::Backspace); // joins the lines
        assert_eq!(ed.buffer.line(0), "abcd");
        ed.apply(Command::Undo);
        assert_eq!(ed.buffer.line(0), "ab");
        assert_eq!(ed.buffer.line(1), "cd");
        assert_eq!((ed.cursor.line, ed.cursor.col), (1, 0));
    }

    #[test]
    fn undo_restores_a_deleted_character() {
        let mut ed = editor("abc");
        ed.apply(Command::Delete);
        assert_eq!(ed.buffer.line(0), "bc");
        ed.apply(Command::Undo);
        assert_eq!(ed.buffer.line(0), "abc");
    }

    #[test]
    fn undo_after_save_makes_the_file_dirty_again() {
        let dir = std::env::temp_dir().join("ludicrous-test-undo-save");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("undo.txt");
        std::fs::write(&path, "hi").unwrap();
        let mut ed = Editor::new(Buffer::open(&path).unwrap());
        ed.apply(Command::Insert('!'));
        ed.apply(Command::Save);
        assert!(!ed.is_dirty());
        ed.apply(Command::Undo);
        assert!(ed.is_dirty());
        ed.apply(Command::Redo);
        assert!(!ed.is_dirty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn undo_with_empty_history_is_a_noop() {
        let mut ed = editor("abc");
        ed.apply(Command::Undo);
        assert_eq!(ed.buffer.line(0), "abc");
        assert!(!ed.is_dirty());
    }

    #[test]
    fn horizontal_scroll_follows_the_cursor() {
        let mut ed = editor(&"x".repeat(200));
        ed.ensure_visible(10, 20);
        assert_eq!(ed.scroll_x, 0);
        ed.apply(Command::End);
        ed.ensure_visible(10, 20);
        // The caret must be inside the visible window, with one column spare.
        assert!(ed.cursor.col >= ed.scroll_x);
        assert!(ed.cursor.col < ed.scroll_x + 20);
        ed.apply(Command::Home);
        ed.ensure_visible(10, 20);
        assert_eq!(ed.scroll_x, 0);
    }

    #[test]
    fn shift_movement_selects_and_plain_movement_clears() {
        let mut ed = editor("hello");
        ed.apply(Command::SelectRight);
        ed.apply(Command::SelectRight);
        assert_eq!(ed.selection(), Some((0, 2)));
        assert_eq!(ed.selected_text().as_deref(), Some("he"));
        ed.apply(Command::Right);
        assert_eq!(ed.selection(), None);
    }

    #[test]
    fn select_all_covers_the_whole_buffer() {
        let mut ed = editor("ab\ncd");
        ed.apply(Command::SelectAll);
        assert_eq!(ed.selected_text().as_deref(), Some("ab\ncd"));
    }

    #[test]
    fn typing_over_a_selection_replaces_it_in_one_undo_step() {
        let mut ed = editor("hello world");
        ed.apply(Command::SelectAll);
        ed.apply(Command::Insert('x'));
        assert_eq!(ed.buffer.line(0), "x");
        ed.apply(Command::Undo);
        assert_eq!(ed.buffer.line(0), "hello world");
    }

    #[test]
    fn paste_is_one_undo_step_and_handles_multibyte_text() {
        let mut ed = editor("ab");
        ed.apply(Command::End);
        ed.apply(Command::Paste("é日🙂\nnext".to_string()));
        assert_eq!(ed.buffer.line(0), "abé日🙂");
        assert_eq!(ed.buffer.line(1), "next");
        assert_eq!((ed.cursor.line, ed.cursor.col), (1, 4));
        ed.apply(Command::Undo);
        assert_eq!(ed.buffer.line(0), "ab");
        assert_eq!(ed.buffer.line_count(), 1);
    }

    #[test]
    fn cut_removes_the_selection_and_undo_restores_it() {
        let mut ed = editor("one two");
        ed.apply(Command::SelectAll);
        ed.apply(Command::Cut);
        assert_eq!(ed.buffer.line(0), "");
        ed.apply(Command::Undo);
        assert_eq!(ed.buffer.line(0), "one two");
    }

    #[test]
    fn backspace_with_a_selection_deletes_only_the_selection() {
        let mut ed = editor("abcdef");
        ed.apply(Command::SelectRight);
        ed.apply(Command::SelectRight);
        ed.apply(Command::Backspace);
        assert_eq!(ed.buffer.line(0), "cdef");
        assert_eq!((ed.cursor.line, ed.cursor.col), (0, 0));
    }

    #[test]
    fn moving_the_cursor_starts_a_new_undo_group() {
        let mut ed = editor("");
        ed.apply(Command::Insert('a'));
        ed.apply(Command::Left);
        ed.apply(Command::Insert('b'));
        ed.apply(Command::Undo);
        // Only the 'b' goes away; the 'a' was in an earlier group.
        assert_eq!(ed.buffer.line(0), "a");
    }
}
