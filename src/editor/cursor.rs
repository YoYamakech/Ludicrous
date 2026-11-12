// cursor.rs: a position in the buffer, in characters (not bytes).
// `desired_col` remembers the column the user aimed for, so moving up through
// a short line and back down returns to the original column.

pub struct Cursor {
    pub line: usize,
    pub col: usize,
    pub desired_col: usize,
}

impl Cursor {
    pub fn new() -> Self {
        Self {
            line: 0,
            col: 0,
            desired_col: 0,
        }
    }

    /// Keep the cursor inside the buffer after an edit.
    ///
    /// `desired_col` is deliberately left alone: it is what makes moving up
    /// through a short line and back down return to the original column.
    /// Commands that really move the cursor horizontally reset it themselves.
    pub fn clamp(&mut self, line_count: usize, line_len: impl Fn(usize) -> usize) {
        self.line = self.line.min(line_count.saturating_sub(1));
        self.col = self.col.min(line_len(self.line));
    }
}
