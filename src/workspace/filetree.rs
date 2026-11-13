// filetree.rs: a lazy directory model for the sidebar.
//
// Nothing here touches GL or winit. The tree reads a directory only the first
// time it is expanded (the target machine has a spinning disk, so scanning a
// whole project at startup is not acceptable). Expansion state lives in a set
// of paths; `rows()` flattens whatever is currently open into a list the view
// can draw.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Names never shown in the tree. One list, one place to change it.
pub const IGNORED: &[&str] = &["target", "build", "node_modules", "__pycache__"];

/// A directory with more entries than this is cut off; the last row says how
/// many were left out instead of freezing the UI.
pub const MAX_ENTRIES: usize = 2000;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// One drawable line of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub depth: usize,
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub expanded: bool,
    /// The directory could not be read: show a muted "(unreadable)" line.
    pub unreadable: bool,
    /// Non-zero for the synthetic "… N more" row.
    pub more: usize,
}

#[allow(dead_code)] // the view (Phase D) consumes rows()/toggle()/refresh()
pub struct FileTree {
    pub root: Option<PathBuf>,
    /// Directory contents, filled on first expansion.
    cache: HashMap<PathBuf, Vec<Entry>>,
    expanded: HashSet<PathBuf>,
    /// Directories that failed to read, so we do not retry every frame.
    failed: HashSet<PathBuf>,
    rows: Vec<Row>,
}

#[allow(dead_code)]
impl FileTree {
    pub fn new() -> Self {
        Self {
            root: None,
            cache: HashMap::new(),
            expanded: HashSet::new(),
            failed: HashSet::new(),
            rows: Vec::new(),
        }
    }

    /// Point the tree at a project folder. The root itself is always expanded.
    pub fn set_root(&mut self, root: Option<PathBuf>) {
        self.root = root.clone();
        self.cache.clear();
        self.expanded.clear();
        self.failed.clear();
        if let Some(r) = root {
            self.expanded.insert(r);
        }
        self.rebuild();
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// Expand or collapse a directory, loading it on first expansion.
    pub fn toggle(&mut self, path: &Path) {
        if !path.is_dir() {
            return;
        }
        if self.expanded.contains(path) {
            self.expanded.remove(path);
        } else {
            self.expanded.insert(path.to_path_buf());
            self.load(path);
        }
        self.rebuild();
    }

    /// Drop cached contents so the next draw re-reads the disk.
    pub fn refresh(&mut self) {
        self.cache.clear();
        self.failed.clear();
        self.rebuild();
    }

    /// Read a directory once, sorted and filtered.
    fn load(&mut self, dir: &Path) {
        if self.cache.contains_key(dir) || self.failed.contains(dir) {
            return;
        }
        match std::fs::read_dir(dir) {
            Ok(iter) => {
                let mut entries: Vec<Entry> = iter
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        // Hidden files and build junk stay out of the way.
                        if name.starts_with('.') || IGNORED.contains(&name.as_str()) {
                            return None;
                        }
                        let path = e.path();
                        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        Some(Entry { name, path, is_dir })
                    })
                    .collect();
                // Directories first, then case-insensitive alphabetical.
                entries.sort_by(|a, b| {
                    b.is_dir
                        .cmp(&a.is_dir)
                        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                });
                self.cache.insert(dir.to_path_buf(), entries);
            }
            Err(_) => {
                self.failed.insert(dir.to_path_buf());
            }
        }
    }

    /// Flatten the expanded part of the tree into drawable rows.
    fn rebuild(&mut self) {
        self.rows.clear();
        let Some(root) = self.root.clone() else {
            return;
        };
        // The root's own children are the top level; the folder name is in the
        // sidebar header, so we do not repeat it as a row.
        self.push_children(&root, 0);
    }

    fn push_children(&mut self, dir: &Path, depth: usize) {
        self.load(dir);
        if self.failed.contains(dir) {
            self.rows.push(Row {
                depth,
                name: "(unreadable)".to_string(),
                path: dir.to_path_buf(),
                is_dir: false,
                expanded: false,
                unreadable: true,
                more: 0,
            });
            return;
        }
        let Some(entries) = self.cache.get(dir).cloned() else {
            return;
        };
        let shown = entries.len().min(MAX_ENTRIES);
        for entry in entries.iter().take(shown) {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            self.rows.push(Row {
                depth,
                name: entry.name.clone(),
                path: entry.path.clone(),
                is_dir: entry.is_dir,
                expanded,
                unreadable: false,
                more: 0,
            });
            if expanded {
                self.push_children(&entry.path, depth + 1);
            }
        }
        if entries.len() > shown {
            self.rows.push(Row {
                depth,
                name: format!("… {} more", entries.len() - shown),
                path: dir.to_path_buf(),
                is_dir: false,
                expanded: false,
                unreadable: false,
                more: entries.len() - shown,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway directory tree for the tests.
    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ludicrous-tree-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/nested")).unwrap();
        std::fs::create_dir_all(dir.join("target")).unwrap();
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        std::fs::write(dir.join("A.txt"), "a").unwrap();
        std::fs::write(dir.join(".hidden"), "h").unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("target/junk"), "j").unwrap();
        dir
    }

    #[test]
    fn root_is_expanded_and_ignored_names_are_hidden() {
        let dir = fixture("root");
        let mut t = FileTree::new();
        t.set_root(Some(dir.clone()));
        let names: Vec<_> = t.rows().iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"src"));
        assert!(names.contains(&"A.txt"));
        assert!(!names.contains(&".hidden"));
        assert!(!names.contains(&"target"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn directories_come_first_then_case_insensitive_names() {
        let dir = fixture("sort");
        let mut t = FileTree::new();
        t.set_root(Some(dir.clone()));
        let names: Vec<_> = t.rows().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["src", "A.txt", "b.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn children_appear_only_after_expanding() {
        let dir = fixture("lazy");
        let mut t = FileTree::new();
        t.set_root(Some(dir.clone()));
        assert_eq!(t.rows().len(), 3);
        t.toggle(&dir.join("src"));
        let names: Vec<_> = t.rows().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["src", "nested", "main.rs", "A.txt", "b.txt"]);
        // Nested children are one level deeper.
        assert_eq!(t.rows()[1].depth, 1);
        // Collapsing removes them again.
        t.toggle(&dir.join("src"));
        assert_eq!(t.rows().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_directory_shows_a_row() {
        let dir = fixture("unreadable");
        let mut t = FileTree::new();
        t.set_root(Some(dir.clone()));
        // A path that does not exist behaves like an unreadable directory.
        t.toggle(&dir.join("src"));
        t.cache.clear();
        let missing = dir.join("does-not-exist");
        std::fs::create_dir(&missing).ok();
        // Make it disappear after being expanded once.
        t.toggle(&missing);
        std::fs::remove_dir(&missing).ok();
        t.refresh();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_root_has_no_rows() {
        let t = FileTree::new();
        assert!(t.rows().is_empty());
    }
}
