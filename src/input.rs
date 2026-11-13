// input.rs: the only place that translates winit events into Commands.
// It never touches the buffer or the renderer; app.rs applies what it returns.

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::editor::Command;

/// Map a key press to an editor command, or None if the key is not ours.
pub fn key(event: &KeyEvent, mods: ModifiersState) -> Option<Command> {
    let ctrl = mods.control_key();

    if ctrl {
        return match event.logical_key {
            Key::Character(ref c) => match c.as_str().to_ascii_lowercase().as_str() {
                "s" => Some(Command::Save),
                "q" => Some(Command::Quit),
                "z" if mods.shift_key() => Some(Command::Redo),
                "z" => Some(Command::Undo),
                "y" => Some(Command::Redo),
                "a" => Some(Command::SelectAll),
                "c" => Some(Command::Copy),
                "x" => Some(Command::Cut),
                // Paste needs the clipboard, which only the app can read, so
                // it is handled there; returning None keeps this pure.
                _ => None,
            },
            _ => None,
        };
    }

    // Shift+movement extends the selection; plain movement clears it.
    let shift = mods.shift_key();
    match &event.logical_key {
        Key::Named(NamedKey::Enter) => Some(Command::Newline),
        Key::Named(NamedKey::Backspace) => Some(Command::Backspace),
        Key::Named(NamedKey::Delete) => Some(Command::Delete),
        Key::Named(NamedKey::ArrowLeft) if shift => Some(Command::SelectLeft),
        Key::Named(NamedKey::ArrowRight) if shift => Some(Command::SelectRight),
        Key::Named(NamedKey::ArrowUp) if shift => Some(Command::SelectUp),
        Key::Named(NamedKey::ArrowDown) if shift => Some(Command::SelectDown),
        Key::Named(NamedKey::Home) if shift => Some(Command::SelectHome),
        Key::Named(NamedKey::End) if shift => Some(Command::SelectEnd),
        Key::Named(NamedKey::ArrowLeft) => Some(Command::Left),
        Key::Named(NamedKey::ArrowRight) => Some(Command::Right),
        Key::Named(NamedKey::ArrowUp) => Some(Command::Up),
        Key::Named(NamedKey::ArrowDown) => Some(Command::Down),
        Key::Named(NamedKey::Home) => Some(Command::Home),
        Key::Named(NamedKey::End) => Some(Command::End),
        Key::Named(NamedKey::PageUp) => Some(Command::PageUp),
        Key::Named(NamedKey::PageDown) => Some(Command::PageDown),
        Key::Named(NamedKey::Tab) => Some(Command::Insert('\t')),
        Key::Named(NamedKey::Space) => Some(Command::Insert(' ')),
        // Text input (layout-aware, handles shift and dead keys for us).
        Key::Character(s) => {
            // Ignore AltGr-style combinations and control characters.
            if mods.alt_key() || mods.super_key() {
                return None;
            }
            s.chars().find(|c| !c.is_control()).map(Command::Insert)
        }
        _ => None,
    }
}

/// Ctrl+V is handled by the app because it needs the system clipboard.
pub fn is_paste(event: &KeyEvent, mods: ModifiersState) -> bool {
    if !mods.control_key() {
        return false;
    }
    matches!(&event.logical_key, Key::Character(c) if c.as_str().eq_ignore_ascii_case("v"))
}

/// Mouse wheel: one notch is three lines. Pixel deltas are converted by the
/// caller because only it knows the line height.
pub fn wheel_lines(line_delta: f32) -> Command {
    Command::Scroll(line_delta * 3.0)
}
