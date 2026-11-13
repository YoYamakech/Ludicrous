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
            self.buffers[0] = Editor::new(buffer);
            self.active = 0;
            return Ok(OpenResult::Opened(0));
        }
        self.buffers.push(Editor::new(buffer));
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
        if self.buffers.is_empty() {
            // Never leave the window without an editor: keep one empty buffer.
            self.buffers.push(Editor::new(Buffer::empty()));
            self.active = 0;
        } else if self.active >= self.buffers.len() {
            self.active = self.buffers.len() - 1;
        }
        Control::Continue
    }

    /// Quit guard for the whole workspace: every dirty buffer must be
    /// acknowledged. Returns Quit only when nothing is left to save.
    pub fn request_quit(&mut self) -> Control {
        // Ask the emptiest question first: if nothing is dirty, go.
        if !self.any_dirty() {
            return Control::Quit;
        }
        // Warn on the active buffer; the status bar shows its message.
        let ed = &mut self.buffers[self.active];
        ed.request_quit()
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
