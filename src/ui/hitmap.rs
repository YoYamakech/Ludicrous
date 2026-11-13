// hitmap.rs: one list of regions, front-most first, built once per layout.
//
// Clicks, hover pills and the mouse cursor all read this map, so a row can never
// draw a hover pill while showing a text cursor. Regions are pushed in drawing
// order (back to front); the list is searched back to front, so the last thing
// drawn wins -- which is exactly what the user sees.
//
// The map is rebuilt only when the layout changes (resize, sidebar mode,
// settings open/closed, tree expanded, scroll), never on mouse movement alone.
// `App` keeps the `Vec` and reuses its capacity via `clear()`.

use crate::window::{ResizeDirection, WindowButton};

/// An axis-aligned rectangle in points: x, y, width, height.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, (px, py): (f32, f32)) -> bool {
        self.w > 0.0
            && self.h > 0.0
            && px >= self.x
            && px < self.x + self.w
            && py >= self.y
            && py < self.y + self.h
    }
}

/// What a click does, and what the hover highlight belongs to.
///
/// `None` means "drawn, but inert": it still swallows the click (so it does not
/// fall through to the layer behind) and always shows the arrow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    None,
    Resize(ResizeDirection),
    SidebarToggle,
    Button(WindowButton),
    TitleDrag,
    OpenFile(usize),
    TreeRow(usize),
    SidebarSettings,
    RailFiles,
    RailSettings,
    ProjectCard,
    /// Settings page row; `part` is the control piece (`None` = the label).
    SettingsRow {
        row: usize,
        part: Option<usize>,
    },
    TextGutter,
    TextArea,
    StatusBar,
}

/// One hit region. `clip` is the visible part: a row scrolled out of view is
/// outside its clip and must not be hit even if the maths overlaps.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct HitRegion {
    pub rect: Rect,
    pub clip: Rect,
    pub target: Target,
}

#[derive(Default)]
pub struct HitMap {
    regions: Vec<HitRegion>,
}

impl HitMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop every region but keep the allocation: the map is rebuilt on every
    /// layout change, which must not allocate after the first few frames.
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn push(&mut self, rect: Rect, target: Target) {
        self.push_clipped(rect, rect, target);
    }

    pub fn push_clipped(&mut self, rect: Rect, clip: Rect, target: Target) {
        self.regions.push(HitRegion { rect, clip, target });
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    #[cfg(test)] // the app only needs the counts the overlay prints
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Front-most region under the point.
    pub fn region_at(&self, pos: (f32, f32)) -> Option<&HitRegion> {
        self.regions
            .iter()
            .rev()
            .find(|r| r.clip.contains(pos) && r.rect.contains(pos))
    }

    pub fn target_at(&self, pos: (f32, f32)) -> Option<Target> {
        self.region_at(pos).map(|r| r.target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::cursor::CursorKind;

    #[test]
    fn last_pushed_region_wins() {
        let mut m = HitMap::new();
        m.push(Rect::new(0.0, 0.0, 100.0, 100.0), Target::TitleDrag);
        m.push(
            Rect::new(0.0, 0.0, 50.0, 50.0),
            Target::Button(WindowButton::Close),
        );
        assert_eq!(
            m.target_at((10.0, 10.0)),
            Some(Target::Button(WindowButton::Close))
        );
        assert_eq!(m.target_at((75.0, 75.0)), Some(Target::TitleDrag));
    }

    #[test]
    fn a_clipped_row_is_not_hit() {
        let mut m = HitMap::new();
        // Row that is scrolled above its viewport: the rect exists, the clip
        // does not include it.
        m.push_clipped(
            Rect::new(0.0, -10.0, 200.0, 20.0),
            Rect::new(0.0, 100.0, 200.0, 300.0),
            Target::TreeRow(3),
        );
        assert_eq!(m.target_at((10.0, 0.0)), None);
    }

    #[test]
    fn clear_keeps_capacity() {
        let mut m = HitMap::new();
        for i in 0..10 {
            m.push(Rect::new(0.0, i as f32, 10.0, 1.0), Target::None);
        }
        let cap = m.regions.capacity();
        m.clear();
        assert!(m.is_empty());
        assert!(m.regions.capacity() >= cap);
    }

    #[test]
    fn zero_sized_rects_are_never_hit() {
        let mut m = HitMap::new();
        m.push(Rect::new(0.0, 0.0, 0.0, 100.0), Target::TextArea);
        assert_eq!(m.target_at((0.0, 50.0)), None);
    }

    #[test]
    fn the_cursor_and_the_click_target_always_agree() {
        // One map, one answer: `resolve` and `cursor_at` must not diverge.
        let mut m = HitMap::new();
        m.push(Rect::new(0.0, 0.0, 100.0, 100.0), Target::TextArea);
        m.push(Rect::new(0.0, 0.0, 20.0, 20.0), Target::OpenFile(0));
        let at = |p| crate::ui::cursor::resolve(p, &m);
        assert_eq!(at((10.0, 10.0)), CursorKind::Pointer);
        assert_eq!(m.target_at((10.0, 10.0)), Some(Target::OpenFile(0)));
        assert_eq!(at((50.0, 50.0)), CursorKind::Text);
        assert_eq!(m.target_at((50.0, 50.0)), Some(Target::TextArea));
        assert_eq!(at((500.0, 500.0)), CursorKind::Default);
        assert_eq!(m.target_at((500.0, 500.0)), None);
    }

    #[test]
    fn edges_are_half_open() {
        let r = Rect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.contains((0.0, 0.0)));
        assert!(r.contains((9.99, 9.99)));
        assert!(!r.contains((10.0, 5.0)));
    }
}
