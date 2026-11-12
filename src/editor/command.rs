// command.rs: every edit or movement is one of these values.
// Input never touches the buffer directly; it only produces Commands.

#[derive(Clone, Copy, Debug, PartialEq)]
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
    PageUp,
    PageDown,
    /// Scroll by a number of lines (positive = down).
    Scroll(f32),
    Undo,
    Redo,
    Save,
    Quit,
}
