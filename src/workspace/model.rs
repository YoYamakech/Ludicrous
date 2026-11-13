// model.rs: the set of open buffers plus the project folder.
//
// Still no winit, no GL, no font: this is plain data plus the rules for
// switching, closing and opening files. Each `Editor` keeps its own cursor and
// undo history, which is why a `Vec<Editor>` is the whole model.

use std::path::{Path, PathBuf};

use crate::editor::{Buffer, Control, Editor};
use crate::workspace::filetree::FileTree;

pub struct Workspace {
    /// Project folder, if one was opened. `None` with a single buffer means we
    /// were started with `ludicrous file.cpp` or no argument at all.
    pub root: Option<PathBuf>,
    pub buffers: Vec<Editor>,
    pub active: usize,
    pub tree: FileTree,
    /// One flag per buffer: the user has already said "discard" for it. Kept
    /// parallel to `buffers` so quitting with several dirty files asks about
    /// each one in turn instead of losing them silently.
    quit_ok: Vec<bool>,
    /// Spaces per indent level, applied to every buffer including ones opened
    /// after the setting changed.
    indent: usize,
}

/// What happened when a file was opened. The app uses this to decide whether
/// the window needs a redraw and whether to report an error.
#[derive(Debug, PartialEq)]
pub enum OpenResult {
    /// A new buffer was created at index `usize`.
    Opened(usize),
    /// The file was already open; we just switched to `usize`.
    Switched(usize),
}

impl Workspace {
    /// Start empty: one unnamed buffer, no project folder.
    pub fn new() -> Self {
        Self {
            root: None,
            buffers: vec![Editor::new(Buffer::empty())],
            active: 0,
            tree: FileTree::new(),
            quit_ok: vec![false],
            indent: crate::editor::state::INDENT_WIDTH,
        }
    }

    /// Start with a project folder, or with one file, from the command line.
    pub fn open_argument(path: &Path) -> Result<Self, String> {
        let mut ws = Self::new();
        if path.is_dir() {
            ws.root = Some(path.to_path_buf());
            ws.tree.set_root(Some(path.to_path_buf()));
            // A project folder opens with its single empty buffer kept: the
            // file tree is useful on its own, and the editor area must never
            // vanish (the first file opened will reuse this buffer).
        } else {
            // A file: use its parent as the project root so the tree is useful.
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                ws.root = Some(parent.to_path_buf());
                ws.tree.set_root(Some(parent.to_path_buf()));
            }
            ws.open_file(path)?;
        }
        Ok(ws)
    }

    pub fn active_editor(&self) -> Option<&Editor> {
        self.buffers.get(self.active)
    }

    pub fn active_editor_mut(&mut self) -> Option<&mut Editor> {
        self.buffers.get_mut(self.active)
    }

    /// Set the indent width for every buffer, now and for ones opened later.
    pub fn set_indent(&mut self, width: usize) {
        self.indent = width.max(1);
        for b in self.buffers.iter_mut() {
            b.set_indent(self.indent);
        }
    }

    /// A new editor that already follows `self.indent`, so a file opened after
    /// the tab-width setting changed gets the same indentation.
    fn new_editor(&self, buffer: Buffer) -> Editor {
        let mut ed = Editor::new(buffer);
        ed.set_indent(self.indent);
        ed
    }

    /// True when any buffer has unsaved changes; feeds the quit guard.
    pub fn any_dirty(&self) -> bool {
        self.buffers.iter().any(|b| b.is_dirty())
    }

    /// Names for the OPEN FILES list, with the modified flag.
    #[allow(dead_code)] // the sidebar list consumes this in Phase D
    pub fn file_labels(&self) -> Vec<(String, bool)> {
        self.buffers
            .iter()
            .map(|b| (buffer_name(b), b.is_dirty()))
            .collect()
    }

    /// Open a file, or switch to it when it is already open.
    pub fn open_file(&mut self, path: &Path) -> Result<OpenResult, String> {
        if let Some(i) = self.index_of(path) {
            self.active = i;
            return Ok(OpenResult::Switched(i));
        }
        let buffer = Buffer::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // Reuse the single empty unnamed buffer instead of piling up tabs.
        if self.buffers.len() == 1
            && self.buffers[0].is_pristine()
            && self.buffers[0].path().is_none()
        {
            self.buffers[0] = self.new_editor(buffer);
            self.quit_ok[0] = false;
            self.active = 0;
            return Ok(OpenResult::Opened(0));
        }
        let ed = self.new_editor(buffer);
        self.buffers.push(ed);
        self.quit_ok.push(false);
        self.active = self.buffers.len() - 1;
        Ok(OpenResult::Opened(self.active))
    }

    fn index_of(&self, path: &Path) -> Option<usize> {
        let want = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.buffers.iter().position(|b| {
            b.buffer
                .path
                .as_ref()
                .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()) == want)
                .unwrap_or(false)
        })
    }

    /// Switch to the next (or previous) buffer. Wraps around.
    pub fn cycle(&mut self, forward: bool) {
        if self.buffers.len() < 2 {
            return;
        }
        let n = self.buffers.len();
        self.active = if forward {
            (self.active + 1) % n
        } else {
            (self.active + n - 1) % n
        };
    }

    /// Close the active buffer. A dirty buffer arms a warning first; a second
    /// Ctrl+W within the window discards it, matching Ctrl+Q.
    pub fn close_active(&mut self) -> Control {
        if self.buffers.is_empty() {
            return Control::Continue;
        }
        let ed = &mut self.buffers[self.active];
        if ed.is_dirty() && ed.request_quit() == Control::Continue {
            // The shared guard talks about Ctrl+Q; closing has its own key.
            ed.status = Some("Unsaved changes: press Ctrl+W again to discard".to_string());
            return Control::Continue;
        }
        self.buffers.remove(self.active);
        if self.active < self.quit_ok.len() {
            self.quit_ok.remove(self.active);
        }
        if self.buffers.is_empty() {
            // Never leave the window without an editor: keep one empty buffer.
            let ed = self.new_editor(Buffer::empty());
            self.buffers.push(ed);
            self.quit_ok.push(false);
            self.active = 0;
        } else if self.active >= self.buffers.len() {
            self.active = self.buffers.len() - 1;
        }
        Control::Continue
    }

    /// Quit guard for the whole workspace: every dirty buffer must be
    /// acknowledged. Returns Quit only when nothing is left to save.
    ///
    /// With several dirty buffers the guard walks them one at a time: the
    /// window switches to the next unacknowledged one and warns, so pressing
    /// Ctrl+Q from a *clean* buffer can never silently drop another buffer's
    /// changes.
    pub fn request_quit(&mut self) -> Control {
        // Ask the emptiest question first: if nothing is dirty, go.
        if !self.any_dirty() {
            return Control::Quit;
        }
        if self.quit_ok.len() != self.buffers.len() {
            self.quit_ok.resize(self.buffers.len(), false);
        }

        // Everything dirty has been acknowledged: really quit.
        let pending: Vec<usize> = (0..self.buffers.len())
            .filter(|&i| self.buffers[i].is_dirty() && !self.quit_ok[i])
            .collect();
        if pending.is_empty() {
            return Control::Quit;
        }

        // Switch to a pending buffer so its warning is the one on screen.
        let next = if pending.contains(&self.active) {
            self.active
        } else {
            pending[0]
        };
        self.active = next;

        // The first request only warns; a second one within the window is the
        // "discard" acknowledgement for this buffer.
        let ed = &mut self.buffers[next];
        if ed.request_quit() == Control::Quit {
            self.quit_ok[next] = true;
            // Ask about the rest in the same gesture.
            return self.request_quit();
        }
        let remaining = pending.len();
        ed.status = Some(if remaining > 1 {
            format!(
                "{} has unsaved changes ({} files left): press Ctrl+Q again to discard",
                buffer_name(ed),
                remaining
            )
        } else {
            format!(
                "{} has unsaved changes: press Ctrl+Q again to discard",
                buffer_name(ed)
            )
        });
        Control::Continue
    }
}

/// File name for the title bar and the open-files list.
pub fn buffer_name(ed: &Editor) -> String {
    ed.buffer
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "[No Name]".to_string())
}

/// Project folder name for the sidebar header.
#[allow(dead_code)]
pub fn root_name(root: &Option<PathBuf>) -> Option<String> {
    root.as_ref()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .or_else(|| root.as_ref().map(|p| p.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn tmp(name: &str, contents: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ludicrous-ws-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.txt");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn opening_the_same_file_twice_switches_instead() {
        let path = tmp("same", "hello");
        let mut ws = Workspace::new();
        assert_eq!(ws.open_file(&path), Ok(OpenResult::Opened(0)));
        assert_eq!(ws.open_file(&path), Ok(OpenResult::Switched(0)));
        assert_eq!(ws.buffers.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn opening_a_second_file_adds_a_buffer() {
        let a = tmp("second-a", "a");
        let b = tmp("second-b", "b");
        let mut ws = Workspace::new();
        ws.open_file(&a).unwrap();
        ws.open_file(&b).unwrap();
        assert_eq!(ws.buffers.len(), 2);
        assert_eq!(ws.active, 1);
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn cycling_wraps_around() {
        let a = tmp("cycle-a", "a");
        let b = tmp("cycle-b", "b");
        let mut ws = Workspace::new();
        ws.open_file(&a).unwrap();
        ws.open_file(&b).unwrap();
        ws.cycle(true);
        assert_eq!(ws.active, 0);
        ws.cycle(false);
        assert_eq!(ws.active, 1);
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn closing_a_clean_buffer_removes_it() {
        let a = tmp("close-a", "a");
        let b = tmp("close-b", "b");
        let mut ws = Workspace::new();
        ws.open_file(&a).unwrap();
        ws.open_file(&b).unwrap();
        assert_eq!(ws.close_active(), Control::Continue);
        assert_eq!(ws.buffers.len(), 1);
        assert_eq!(ws.active, 0);
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn closing_a_dirty_buffer_asks_twice() {
        let a = tmp("dirty-a", "a");
        let mut ws = Workspace::new();
        ws.open_file(&a).unwrap();
        ws.active_editor_mut()
            .unwrap()
            .apply(crate::editor::Command::Insert('x'));
        ws.close_active();
        assert_eq!(ws.buffers.len(), 1, "first Ctrl+W only warns");
        ws.close_active();
        assert_eq!(ws.buffers.len(), 1, "an empty buffer is always kept");
        assert!(ws.buffers[0].buffer.path.is_none());
        let _ = std::fs::remove_file(&a);
    }

    #[test]
    fn any_dirty_tracks_every_buffer() {
        let a = tmp("any-a", "a");
        let b = tmp("any-b", "b");
        let mut ws = Workspace::new();
        ws.open_file(&a).unwrap();
        ws.open_file(&b).unwrap();
        assert!(!ws.any_dirty());
        ws.buffers[0].apply(crate::editor::Command::Insert('z'));
        assert!(ws.any_dirty());
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn request_quit_only_quits_when_nothing_is_dirty() {
        let mut ws = Workspace::new();
        assert_eq!(ws.request_quit(), Control::Quit);
        ws.active_editor_mut()
            .unwrap()
            .apply(crate::editor::Command::Insert('x'));
        assert_eq!(ws.request_quit(), Control::Continue);
        assert_eq!(ws.request_quit(), Control::Quit);
    }

    #[test]
    fn quitting_from_a_clean_buffer_still_asks_about_a_dirty_one() {
        let a = tmp("guard-a", "a");
        let b = tmp("guard-b", "b");
        let mut ws = Workspace::new();
        ws.open_file(&a).unwrap();
        ws.open_file(&b).unwrap();
        // Dirty buffer 0 while buffer 1 (active) is clean.
        ws.buffers[0].apply(crate::editor::Command::Insert('z'));
        assert_eq!(ws.active, 1);
        assert_eq!(ws.request_quit(), Control::Continue, "must warn, not quit");
        assert_eq!(ws.active, 0, "the guard switches to the dirty buffer");
        assert!(ws.buffers[0]
            .status
            .as_deref()
            .unwrap_or_default()
            .contains("press Ctrl+Q again"));
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn the_quit_guard_walks_every_dirty_buffer() {
        let a = tmp("walk-a", "a");
        let b = tmp("walk-b", "b");
        let mut ws = Workspace::new();
        ws.open_file(&a).unwrap();
        ws.open_file(&b).unwrap();
        ws.buffers[0].apply(crate::editor::Command::Insert('x'));
        ws.buffers[1].apply(crate::editor::Command::Insert('y'));
        // Each buffer needs two requests: warn, then discard.
        assert_eq!(ws.request_quit(), Control::Continue);
        assert_eq!(ws.request_quit(), Control::Continue, "next dirty buffer");
        assert_eq!(ws.request_quit(), Control::Quit);
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn closing_never_desyncs_the_quit_flags() {
        let a = tmp("sync-a", "a");
        let b = tmp("sync-b", "b");
        let mut ws = Workspace::new();
        ws.open_file(&a).unwrap();
        ws.open_file(&b).unwrap();
        ws.close_active();
        assert_eq!(ws.buffers.len(), 1);
        assert_eq!(ws.quit_ok.len(), 1);
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn unreadable_file_reports_an_error() {
        let mut ws = Workspace::new();
        let err = ws
            .open_file(Path::new("/definitely/not/here.txt"))
            .unwrap_err();
        assert!(err.contains("not/here.txt"));
    }

    #[test]
    fn buffer_name_falls_back_for_unnamed_buffers() {
        let ed = Editor::new(Buffer {
            rope: Rope::new(),
            path: None,
            modified: false,
            line_ending: crate::editor::buffer::LineEnding::Lf,
        });
        assert_eq!(buffer_name(&ed), "[No Name]");
    }
}
