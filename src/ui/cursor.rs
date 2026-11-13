// cursor.rs: the mouse cursor is a *result* of where the pointer is.
//
// v1.1 had one function that mapped a hover target to an icon and fell back to
// `Text`, so the I-beam leaked over the whole sidebar. The rule now: nothing
// sets the cursor ad hoc. `ui/hitmap.rs` describes every region, and this file
// turns the region under the pointer into a `CursorKind`. `ui/` must not import
// winit, so `app.rs` maps `CursorKind` to winit's `CursorIcon`.
//
// The cursor is also remembered by `App`, which only calls `set_cursor` when the
// resolved kind actually changes.

use crate::ui::hitmap::{HitMap, Target};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CursorKind {
    /// Arrow: non-clickable area, labels, dividers, empty space.
    #[default]
    Default,
    /// Hand: anything clickable.
    Pointer,
    /// I-beam: editor text only.
    Text,
    ResizeN,
    ResizeS,
    ResizeE,
    ResizeW,
    ResizeNE,
    ResizeNW,
    ResizeSE,
    ResizeSW,
}

impl CursorKind {
    /// Three letters for the F3 overlay, so the live cursor can be read without
    /// guessing from a screenshot.
    pub fn short(self) -> &'static str {
        match self {
            CursorKind::Default => "arr",
            CursorKind::Pointer => "hand",
            CursorKind::Text => "ibeam",
            CursorKind::ResizeN => "rn",
            CursorKind::ResizeS => "rs",
            CursorKind::ResizeE => "re",
            CursorKind::ResizeW => "rw",
            CursorKind::ResizeNE => "rne",
            CursorKind::ResizeNW => "rnw",
            CursorKind::ResizeSE => "rse",
            CursorKind::ResizeSW => "rsw",
        }
    }
}

/// The cursor for a point, from the hit map built for the current layout.
///
/// A pure function of (position, map): the same call answers "what does a click
/// here do?" and "what should the pointer look like?".
pub fn resolve(pos: (f32, f32), hits: &HitMap) -> CursorKind {
    match hits.target_at(pos) {
        Some(target) => target.cursor(),
        None => CursorKind::Default,
    }
}

impl Target {
    /// Cursor for a region. Kept next to `Target` so a new target cannot be
    /// added without deciding its cursor.
    pub fn cursor(self) -> CursorKind {
        match self {
            Target::None | Target::TitleDrag | Target::TextGutter | Target::StatusBar => {
                CursorKind::Default
            }
            Target::SidebarToggle
            | Target::Button(_)
            | Target::OpenFile(_)
            | Target::TreeRow(_)
            | Target::SidebarSettings
            | Target::RailFiles
            | Target::RailSettings
            | Target::ProjectCard
            | Target::SettingsRow { .. } => CursorKind::Pointer,
            Target::TextArea => CursorKind::Text,
            Target::Resize(dir) => match dir {
                crate::window::ResizeDirection::North => CursorKind::ResizeN,
                crate::window::ResizeDirection::South => CursorKind::ResizeS,
                crate::window::ResizeDirection::East => CursorKind::ResizeE,
                crate::window::ResizeDirection::West => CursorKind::ResizeW,
                crate::window::ResizeDirection::NorthEast => CursorKind::ResizeNE,
                crate::window::ResizeDirection::NorthWest => CursorKind::ResizeNW,
                crate::window::ResizeDirection::SouthEast => CursorKind::ResizeSE,
                crate::window::ResizeDirection::SouthWest => CursorKind::ResizeSW,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::hitmap::{HitMap, Rect};

    fn map() -> HitMap {
        let mut m = HitMap::new();
        // Back to front, as the builder pushes them.
        m.push(Rect::new(0.0, 30.0, 1200.0, 630.0), Target::TextArea);
        m.push(Rect::new(0.0, 30.0, 200.0, 630.0), Target::OpenFile(0));
        m
    }

    #[test]
    fn later_regions_win() {
        assert_eq!(resolve((50.0, 100.0), &map()), CursorKind::Pointer);
        assert_eq!(resolve((500.0, 100.0), &map()), CursorKind::Text);
    }

    #[test]
    fn nothing_under_the_pointer_is_the_arrow() {
        assert_eq!(resolve((500.0, 5.0), &map()), CursorKind::Default);
    }

    #[test]
    fn every_resize_direction_has_its_own_cursor() {
        use crate::window::ResizeDirection as R;
        let kinds = [
            (R::North, CursorKind::ResizeN),
            (R::South, CursorKind::ResizeS),
            (R::East, CursorKind::ResizeE),
            (R::West, CursorKind::ResizeW),
            (R::NorthEast, CursorKind::ResizeNE),
            (R::NorthWest, CursorKind::ResizeNW),
            (R::SouthEast, CursorKind::ResizeSE),
            (R::SouthWest, CursorKind::ResizeSW),
        ];
        for (dir, kind) in kinds {
            assert_eq!(Target::Resize(dir).cursor(), kind);
        }
    }

    #[test]
    fn labels_and_dividers_are_the_arrow() {
        assert_eq!(Target::None.cursor(), CursorKind::Default);
        assert_eq!(Target::TitleDrag.cursor(), CursorKind::Default);
        assert_eq!(Target::StatusBar.cursor(), CursorKind::Default);
        assert_eq!(Target::TextGutter.cursor(), CursorKind::Default);
    }
}
