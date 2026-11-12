// buffer.rs: the text itself, plus the file it came from.
// ropey stores the text as a tree of chunks, so inserting a character in a
// large file does not copy the whole string.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use ropey::Rope;

pub struct Buffer {
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub modified: bool,
}

impl Buffer {
    pub fn empty() -> Self {
        Self {
            rope: Rope::new(),
            path: None,
            modified: false,
        }
    }

    pub fn open(path: &Path) -> io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(Self {
            rope: Rope::from_str(&text),
            path: Some(path.to_path_buf()),
            modified: false,
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

    pub fn insert_char(&mut self, line: usize, col: usize, ch: char) {
        let i = self.index(line, col);
        self.rope.insert_char(i, ch);
        self.modified = true;
    }

    /// Split the line at the cursor. Returns the new cursor position.
    pub fn insert_newline(&mut self, line: usize, col: usize) -> (usize, usize) {
        let i = self.index(line, col);
        self.rope.insert_char(i, '\n');
        self.modified = true;
        (line + 1, 0)
    }

    /// Delete the character before the cursor, joining lines at column 0.
    pub fn backspace(&mut self, line: usize, col: usize) -> (usize, usize) {
        if col > 0 {
            let i = self.index(line, col);
            self.rope.remove(i - 1..i);
            self.modified = true;
            (line, col - 1)
        } else if line > 0 {
            let prev_len = self.line_len(line - 1);
            let i = self.index(line, 0);
            self.rope.remove(i - 1..i);
            self.modified = true;
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
            self.modified = true;
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
            file.write_all(self.rope.to_string().as_bytes())?;
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
