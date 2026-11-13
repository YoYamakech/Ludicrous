// focus.rs: which part of the shell receives keyboard input.
//
// Keyboard events go to exactly one place. Clicking a part focuses it, and
// Ctrl+Shift+E jumps to the file tree, Esc returns to the editor.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Focus {
    #[default]
    Editor,
    Sidebar,
    /// Reserved for the settings page (Phase E); nothing uses it yet.
    #[allow(dead_code)]
    Settings,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_editor_starts_focused() {
        assert_eq!(Focus::default(), Focus::Editor);
    }
}
