// command.rs: every edit or movement is one of these values.
// Input never touches the buffer directly; it only produces Commands.

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Insert(char),
    Newline,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// Same movements, but extending the selection instead of clearing it.
    SelectLeft,
    SelectRight,
    SelectUp,
    SelectDown,
    SelectHome,
    SelectEnd,
    SelectAll,
    Copy,
    Cut,
    /// Paste text from the system clipboard (the app reads it, not the editor).
    Paste(String),
    PageUp,
    PageDown,
    /// Scroll by a number of lines (positive = down).
    Scroll(f32),
    Undo,
    Redo,
    Save,
    Quit,
}
