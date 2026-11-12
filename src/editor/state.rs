// state.rs: the editor as a whole: text + cursor + scroll position.
// `apply` is the only place that mutates any of it, so every change (typing,
// saving, scrolling) follows exactly one path.

use std::time::{Duration, Instant};

use crate::editor::buffer::Buffer;
use crate::editor::command::Command;
use crate::editor::cursor::Cursor;

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
    /// How many lines fit in the text area; set by the UI before drawing.
    pub rows: usize,
    /// Short message for the status bar (e.g. a failed save). Cleared as soon
    /// as the next command runs, so it never sticks around forever.
    pub status: Option<String>,
    /// When the user first asked to quit with unsaved changes. `None` means no
    /// warning is showing. Kept here (not in app.rs) so the window close button
    /// and Ctrl+Q share exactly one guard.
    quit_armed: Option<Instant>,
}

impl Editor {
    pub fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            cursor: Cursor::new(),
            scroll: 0,
            rows: 1,
            status: None,
            quit_armed: None,
        }
    }

    /// Decide whether a quit request may exit. Dirty text arms a warning
    /// instead; a second request within `QUIT_ARM_WINDOW` goes through.
    /// Used by both Ctrl+Q and the window close button.
    pub fn request_quit(&mut self) -> Control {
        if !self.buffer.modified {
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
                let (l, c) = (self.cursor.line, self.cursor.col);
                self.buffer.insert_char(l, c, ch);
                self.cursor.col += 1;
                self.cursor.desired_col = self.cursor.col;
            }
            Command::Newline => {
                let (l, c) = self
                    .buffer
                    .insert_newline(self.cursor.line, self.cursor.col);
                self.cursor.line = l;
                self.cursor.col = c;
                self.cursor.desired_col = c;
            }
            Command::Backspace => {
                let (l, c) = self.buffer.backspace(self.cursor.line, self.cursor.col);
                self.cursor.line = l;
                self.cursor.col = c;
                self.cursor.desired_col = c;
            }
            Command::Delete => {
                self.buffer.delete(self.cursor.line, self.cursor.col);
                self.cursor.desired_col = self.cursor.col;
            }
            Command::Left => {
                if self.cursor.col > 0 {
                    self.cursor.col -= 1;
                } else if self.cursor.line > 0 {
                    self.cursor.line -= 1;
                    self.cursor.col = self.buffer.line_len(self.cursor.line);
                }
                self.cursor.desired_col = self.cursor.col;
            }
            Command::Right => {
                if self.cursor.col < self.buffer.line_len(self.cursor.line) {
                    self.cursor.col += 1;
                } else if self.cursor.line + 1 < self.buffer.line_count() {
                    self.cursor.line += 1;
                    self.cursor.col = 0;
                }
                self.cursor.desired_col = self.cursor.col;
            }
            Command::Up => {
                if self.cursor.line > 0 {
                    self.cursor.line -= 1;
                    // Aim for the remembered column, not the clamped one.
                    self.cursor.col = self
                        .cursor
                        .desired_col
                        .min(self.buffer.line_len(self.cursor.line));
                }
            }
            Command::Down => {
                if self.cursor.line + 1 < self.buffer.line_count() {
                    self.cursor.line += 1;
                    self.cursor.col = self
                        .cursor
                        .desired_col
                        .min(self.buffer.line_len(self.cursor.line));
                }
            }
            Command::Home => {
                self.cursor.col = 0;
                self.cursor.desired_col = 0;
            }
            Command::End => {
                self.cursor.col = self.buffer.line_len(self.cursor.line);
                self.cursor.desired_col = self.cursor.col;
            }
            Command::PageUp => {
                self.cursor.line = self.cursor.line.saturating_sub(self.rows.max(1));
                self.cursor.col = self
                    .cursor
                    .desired_col
                    .min(self.buffer.line_len(self.cursor.line));
            }
            Command::PageDown => {
                let last = self.buffer.line_count().saturating_sub(1);
                self.cursor.line = (self.cursor.line + self.rows.max(1)).min(last);
                self.cursor.col = self
                    .cursor
                    .desired_col
                    .min(self.buffer.line_len(self.cursor.line));
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
                if let Err(e) = self.buffer.save() {
                    eprintln!("ludicrous: save failed: {e}");
                    // Keep `modified` set: the text on disk is still the old
                    // version, so the user must not think the save worked.
                    self.status = Some(format!("Save failed: {e}"));
                }
            }
            Command::Quit => return self.request_quit(),
        }

        self.cursor
            .clamp(self.buffer.line_count(), |l| self.buffer.line_len(l));
        Control::Continue
    }

    /// Scroll as little as possible so the cursor stays on screen.
    /// Called by the app once it knows how many lines fit.
    pub fn ensure_visible(&mut self, rows: usize) {
        self.rows = rows.max(1);
        if self.cursor.line < self.scroll {
            self.scroll = self.cursor.line;
        } else if self.cursor.line >= self.scroll + self.rows {
            self.scroll = self.cursor.line + 1 - self.rows;
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
        ed.ensure_visible(5);
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
}
