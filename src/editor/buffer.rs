// buffer.rs: the text itself, plus the file it came from.
// ropey stores the text as a tree of chunks, so inserting a character in a
// large file does not copy the whole string.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use ropey::Rope;

/// How the file used newlines when we opened it. The rope always stores plain
/// `\n`, so the editor only ever deals with one line ending; this is applied
/// again on save so we do not rewrite a Windows file as a Unix one.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LineEnding {
    Lf,
    CrLf,
}

#[derive(Debug)]
pub struct Buffer {
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub modified: bool,
    pub line_ending: LineEnding,
    /// Bumped on every text change. The highlighter caches spans per version,
    /// so a redraw that changes nothing never re-parses (v1.2 section 6).
    pub version: u64,
    /// The most recent change, in character indexes: `(start, removed, added)`.
    /// `Editor::apply` turns this into a tree-sitter `InputEdit` without having
    /// to know which command produced it. `None` until the first edit.
    pub last_edit: Option<(usize, usize, usize)>,
}

impl Buffer {
    pub fn empty() -> Self {
        Self {
            rope: Rope::new(),
            path: None,
            modified: false,
            line_ending: LineEnding::Lf,
            version: 1,
            last_edit: None,
        }
    }

    pub fn open(path: &Path) -> io::Result<Self> {
        let bytes = std::fs::read(path)?;
        // Refuse anything that is not UTF-8 instead of mangling it: a lossy
        // conversion would silently corrupt the file on the next save.
        let text = String::from_utf8(bytes).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "file is not valid UTF-8 (refusing to open it)",
            )
        })?;
        // Detect the dominant line ending, then normalise to `\n` in the rope.
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count();
        let line_ending = if crlf > 0 && crlf * 2 >= lf {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        let text = if line_ending == LineEnding::CrLf {
            text.replace("\r\n", "\n")
        } else {
            text
        };
        Ok(Self {
            rope: Rope::from_str(&text),
            path: Some(path.to_path_buf()),
            modified: false,
            line_ending,
            version: 1,
            last_edit: None,
        })
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines()
    }

    /// Text of one line, without its trailing newline.
    pub fn line(&self, line: usize) -> String {
        let mut s = self.rope.line(line).to_string();
        while s.ends_with('\n') || s.ends_with('\r') {
            s.pop();
        }
        s
    }

    /// Number of characters in a line, excluding the newline.
    pub fn line_len(&self, line: usize) -> usize {
        let mut n = self.rope.line(line).len_chars();
        if n > 0 && self.rope.line(line).char(n - 1) == '\n' {
            n -= 1;
        }
        if n > 0 && self.rope.line(line).char(n - 1) == '\r' {
            n -= 1;
        }
        n
    }

    fn index(&self, line: usize, col: usize) -> usize {
        self.rope.line_to_char(line) + col
    }

    /// Character index (not bytes) of a line/column pair. History stores
    /// positions this way so an edit stays valid across the whole file.
    pub fn char_index(&self, line: usize, col: usize) -> usize {
        self.index(line, col)
    }

    /// The character at a character index, if it exists.
    pub fn char_at(&self, i: usize) -> Option<char> {
        if i < self.rope.len_chars() {
            Some(self.rope.char(i))
        } else {
            None
        }
    }

    /// True when nothing has been typed into this buffer yet: used to decide
    /// whether an empty unnamed buffer can be replaced when a file is opened.
    pub fn is_pristine(&self) -> bool {
        !self.modified && self.rope.len_chars() == 0
    }

    /// Line and column (both 0-based, in characters) of a character index.
    pub fn line_col_of(&self, i: usize) -> (usize, usize) {
        let i = i.min(self.rope.len_chars());
        let line = self.rope.char_to_line(i);
        (line, i - self.rope.line_to_char(line))
    }

    /// Record that a change happened, for the highlighter. Every mutation goes
    /// through one of the methods below, so this is the only place that has to
    /// remember to bump the version.
    fn changed(&mut self, start: usize, removed: usize, added: usize) {
        self.modified = true;
        self.version = self.version.wrapping_add(1);
        self.last_edit = Some((start, removed, added));
    }

    /// Insert a whole string at a character index (used by undo/redo).
    pub fn insert_str_at(&mut self, i: usize, text: &str) {
        let added = text.chars().count();
        self.rope.insert(i, text);
        self.changed(i, 0, added);
    }

    /// Remove `len` characters starting at a character index.
    pub fn remove_at(&mut self, i: usize, len: usize) {
        self.rope.remove(i..i + len);
        self.changed(i, len, 0);
    }

    pub fn insert_char(&mut self, line: usize, col: usize, ch: char) {
        let i = self.index(line, col);
        self.rope.insert_char(i, ch);
        self.changed(i, 0, 1);
    }

    /// Split the line at the cursor. Returns the new cursor position.
    /// (The editor now inserts the newline plus auto-indent in one step, so
    /// this is only used by the buffer tests.)
    #[allow(dead_code)]
    pub fn insert_newline(&mut self, line: usize, col: usize) -> (usize, usize) {
        let i = self.index(line, col);
        self.rope.insert_char(i, '\n');
        self.changed(i, 0, 1);
        (line + 1, 0)
    }

    /// Delete the character before the cursor, joining lines at column 0.
    pub fn backspace(&mut self, line: usize, col: usize) -> (usize, usize) {
        if col > 0 {
            let i = self.index(line, col);
            self.rope.remove(i - 1..i);
            self.changed(i - 1, 1, 0);
            (line, col - 1)
        } else if line > 0 {
            let prev_len = self.line_len(line - 1);
            let i = self.index(line, 0);
            self.rope.remove(i - 1..i);
            self.changed(i - 1, 1, 0);
            (line - 1, prev_len)
        } else {
            (line, col)
        }
    }

    /// Delete the character under the cursor, joining lines at end of line.
    pub fn delete(&mut self, line: usize, col: usize) -> (usize, usize) {
        // Either the character under the cursor, or the newline joining the
        // next line, lives at this rope index.
        if col < self.line_len(line) || line + 1 < self.line_count() {
            let i = self.index(line, col);
            self.rope.remove(i..i + 1);
            self.changed(i, 1, 0);
        }
        (line, col)
    }

    /// Write the text back to disk without ever truncating the original file.
    ///
    /// We write a sibling temp file, flush it to the disk, then rename it over
    /// the original. A crash or a full disk therefore leaves either the old
    /// file or the new one, never a half-written file. `modified` is only
    /// cleared once the rename succeeded.
    pub fn save(&mut self) -> io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "No file name"));
        };

        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("buffer");
        let tmp = dir.join(format!(".{name}.ludicrous-tmp"));

        let result = (|| -> io::Result<()> {
            let mut file = std::fs::File::create(&tmp)?;
            // The rope is always LF; put the original style back on disk.
            let text = self.rope.to_string();
            let out = match self.line_ending {
                LineEnding::Lf => text,
                LineEnding::CrLf => text.replace('\n', "\r\n"),
            };
            file.write_all(out.as_bytes())?;
            // sync_all pushes the bytes to the device before the rename, so a
            // power loss cannot leave the new name pointing at empty data.
            file.sync_all()?;
            drop(file);
            // Keep the original permissions (File::create would use 0644).
            if let Ok(meta) = std::fs::metadata(&path) {
                let _ = std::fs::set_permissions(&tmp, meta.permissions());
            }
            std::fs::rename(&tmp, &path)
        })();

        if result.is_err() {
            // Best effort: do not leave temp files behind on failure.
            let _ = std::fs::remove_file(&tmp);
            return result;
        }
        self.modified = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        Buffer {
            rope: Rope::from_str(text),
            path: None,
            modified: false,
            line_ending: LineEnding::Lf,
            version: 1,
            last_edit: None,
        }
    }

    #[test]
    fn line_len_excludes_newline() {
        let b = buf("abc\nde\r\nf");
        assert_eq!(b.line_len(0), 3);
        assert_eq!(b.line_len(1), 2);
        assert_eq!(b.line_len(2), 1);
    }

    #[test]
    fn empty_buffer_has_one_line() {
        let b = Buffer::empty();
        assert_eq!(b.line_count(), 1);
        assert_eq!(b.line_len(0), 0);
    }

    #[test]
    fn insert_char_marks_modified() {
        let mut b = buf("ac");
        b.insert_char(0, 1, 'b');
        assert_eq!(b.line(0), "abc");
        assert!(b.modified);
    }

    #[test]
    fn newline_splits_line() {
        let mut b = buf("abcd");
        assert_eq!(b.insert_newline(0, 2), (1, 0));
        assert_eq!(b.line(0), "ab");
        assert_eq!(b.line(1), "cd");
    }

    #[test]
    fn backspace_at_col_zero_joins_lines() {
        let mut b = buf("ab\ncd");
        assert_eq!(b.backspace(1, 0), (0, 2));
        assert_eq!(b.line(0), "abcd");
        assert_eq!(b.line_count(), 1);
    }

    #[test]
    fn backspace_at_start_is_a_noop() {
        let mut b = buf("ab");
        assert_eq!(b.backspace(0, 0), (0, 0));
        assert_eq!(b.line(0), "ab");
        assert!(!b.modified);
    }

    #[test]
    fn delete_at_end_of_line_joins_next_line() {
        let mut b = buf("ab\ncd");
        assert_eq!(b.delete(0, 2), (0, 2));
        assert_eq!(b.line(0), "abcd");
    }

    #[test]
    fn delete_at_end_of_file_is_a_noop() {
        let mut b = buf("ab");
        b.delete(0, 2);
        assert_eq!(b.line(0), "ab");
        assert!(!b.modified);
    }

    #[test]
    fn multibyte_chars_are_indexed_by_char() {
        let mut b = buf("aé日");
        assert_eq!(b.line_len(0), 3);
        // Insert between the multi-byte characters: a byte index would split
        // them and panic inside ropey.
        b.insert_char(0, 2, 'x');
        assert_eq!(b.line(0), "aéx日");
        b.insert_char(0, 1, 'z');
        assert_eq!(b.line(0), "azéx日");
    }

    #[test]
    fn deleting_multibyte_chars_keeps_the_rest_intact() {
        let mut b = buf("é日🙂");
        assert_eq!(b.line_len(0), 3);
        b.delete(0, 1);
        assert_eq!(b.line(0), "é🙂");
        b.delete(0, 1);
        assert_eq!(b.line(0), "é");
        assert_eq!(b.backspace(0, 1), (0, 0));
        assert_eq!(b.line(0), "");
    }

    #[test]
    fn crlf_files_round_trip() {
        let dir = std::env::temp_dir().join("ludicrous-test-crlf");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("win.txt");
        std::fs::write(&path, "one\r\ntwo\r\n").unwrap();
        let mut b = Buffer::open(&path).unwrap();
        assert_eq!(b.line_ending, LineEnding::CrLf);
        // The rope is normalised, so the editor never sees a stray `\r`.
        assert_eq!(b.line(0), "one");
        assert_eq!(b.line_len(0), 3);
        b.insert_char(0, 3, '!');
        b.save().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "one!\r\ntwo\r\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn invalid_utf8_is_refused() {
        let dir = std::env::temp_dir().join("ludicrous-test-badutf8");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("latin1.txt");
        std::fs::write(&path, [0x63, 0x61, 0x66, 0xe9]).unwrap(); // "café" in Latin-1
        let err = Buffer::open(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        // The file on disk is untouched.
        assert_eq!(std::fs::read(&path).unwrap(), vec![0x63, 0x61, 0x66, 0xe9]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_clears_modified() {
        let dir = std::env::temp_dir().join("ludicrous-test-save");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.txt");
        std::fs::write(&path, "hi").unwrap();
        let mut b = Buffer::open(&path).unwrap();
        b.insert_char(0, 0, '>');
        assert!(b.modified);
        b.save().unwrap();
        assert!(!b.modified);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), ">hi");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_without_path_is_an_error() {
        let mut b = buf("text");
        b.modified = true;
        let err = b.save().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        // Nothing was written, so the buffer is still dirty.
        assert!(b.modified);
    }

    #[test]
    fn failed_save_leaves_original_and_no_temp_file() {
        let dir = std::env::temp_dir().join("ludicrous-test-save-fail");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A directory in place of the target file makes the final rename fail.
        let path = dir.join("target");
        std::fs::create_dir(&path).unwrap();
        let mut b = Buffer {
            rope: Rope::from_str("new"),
            path: Some(path.clone()),
            modified: true,
            line_ending: LineEnding::Lf,
            version: 1,
            last_edit: None,
        };
        assert!(b.save().is_err());
        assert!(b.modified);
        assert!(path.is_dir(), "original entry must be untouched");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("target")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn save_keeps_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join("ludicrous-test-save-perms");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret.txt");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut b = Buffer::open(&path).unwrap();
        b.insert_char(0, 0, 'x');
        b.save().unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_file(&path);
    }
}
