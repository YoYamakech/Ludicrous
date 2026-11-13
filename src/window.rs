// window.rs: the shell's own window decorations.
//
// winit gives us an undecorated window; this module decides what a point in
// that window means (drag the title bar, resize an edge, press a button, or
// just edit text). It is the only place that knows about window chrome, and
// like input.rs it never touches the editor or the renderer.

use winit::dpi::PhysicalPosition;

use crate::ui::hitmap::{HitMap, Rect, Target};

/// Height of the title bar in points.
pub const TITLE_H: f32 = 30.0;
/// How far inside the window edge a resize drag starts, in points.
pub const RESIZE_EDGE: f32 = 5.0;
/// Width of one window button (minimize / maximize / close).
pub const BUTTON_W: f32 = 46.0;
/// Square size of the sidebar toggle at the left of the title bar.
pub const TOGGLE_W: f32 = 30.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowButton {
    Minimize,
    Maximize,
    Close,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResizeDirection {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

/// What the hit test needs to know about the window's current state.
#[derive(Clone, Copy)]
pub struct HitState {
    pub maximized: bool,
    pub fullscreen: bool,
    /// Window buttons on the left (traffic lights) instead of the right.
    pub buttons_left: bool,
}

/// Push the title bar and the resize edges into the hit map, back to front.
///
/// Order matters: the edges are pushed last because they are the front-most few
/// pixels, then the title bar's buttons, then the drag region. `size` is in
/// points.
pub fn chrome_regions(out: &mut HitMap, size: (f32, f32), state: HitState) {
    let (w, _h) = size;

    // Title bar: exactly one region per drawn thing, in the same order as
    // `ui/view.rs` draws them.
    out.push(Rect::new(0.0, 0.0, w, TITLE_H), Target::TitleDrag);
    out.push(
        Rect::new(0.0, 0.0, TOGGLE_W, TITLE_H),
        Target::SidebarToggle,
    );
    let buttons_w = BUTTON_W * 3.0;
    let start = if state.buttons_left {
        TOGGLE_W
    } else {
        w - buttons_w
    };
    let order = if state.buttons_left {
        [
            WindowButton::Close,
            WindowButton::Minimize,
            WindowButton::Maximize,
        ]
    } else {
        [
            WindowButton::Minimize,
            WindowButton::Maximize,
            WindowButton::Close,
        ]
    };
    for (i, button) in order.into_iter().enumerate() {
        // Skip a button that would hang off the window on a very narrow width.
        if start + i as f32 * BUTTON_W >= w {
            break;
        }
        out.push(
            Rect::new(start + i as f32 * BUTTON_W, 0.0, BUTTON_W, TITLE_H),
            Target::Button(button),
        );
    }

    // Resize edges last: they win over everything, and they do not exist on a
    // maximized or fullscreen window.
    if !state.maximized && !state.fullscreen {
        let e = RESIZE_EDGE;
        let dirs = [
            (Rect::new(0.0, 0.0, e, e), ResizeDirection::NorthWest),
            (Rect::new(w - e, 0.0, e, e), ResizeDirection::NorthEast),
            (Rect::new(0.0, size.1 - e, e, e), ResizeDirection::SouthWest),
            (
                Rect::new(w - e, size.1 - e, e, e),
                ResizeDirection::SouthEast,
            ),
            (
                Rect::new(0.0, e, e, size.1 - 2.0 * e),
                ResizeDirection::West,
            ),
            (
                Rect::new(w - e, e, e, size.1 - 2.0 * e),
                ResizeDirection::East,
            ),
            (Rect::new(e, 0.0, w - 2.0 * e, e), ResizeDirection::North),
            (
                Rect::new(e, size.1 - e, w - 2.0 * e, e),
                ResizeDirection::South,
            ),
        ];
        for (rect, dir) in dirs {
            out.push(rect, Target::Resize(dir));
        }
    }
}

/// Single-point convenience used by the tests (and by nothing else: the app
/// reads the map it already built).
#[cfg(test)]
pub fn hit_test(pos: (f32, f32), size: (f32, f32), state: HitState) -> Option<Target> {
    let mut m = HitMap::new();
    chrome_regions(&mut m, size, state);
    m.target_at(pos)
}

/// Convert a physical cursor position to points, which is what the layout and
/// the hit test use.
pub fn to_points(pos: PhysicalPosition<f64>, scale: f32) -> (f32, f32) {
    (pos.x as f32 / scale, pos.y as f32 / scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (f32, f32) = (1000.0, 660.0);

    fn state() -> HitState {
        HitState {
            maximized: false,
            fullscreen: false,
            buttons_left: false,
        }
    }

    #[test]
    fn the_middle_of_the_window_has_no_chrome() {
        // The text area itself is registered by the app, not by the chrome.
        assert_eq!(hit_test((500.0, 300.0), SIZE, state()), None);
    }

    #[test]
    fn a_click_on_the_content_band_is_inert_chrome() {
        // Nothing here is a resize edge or a title bar, so the earlier map (the
        // text area, the sidebar) decides. Chrome itself adds no region.
        let mut m = HitMap::new();
        chrome_regions(&mut m, SIZE, state());
        assert_eq!(m.target_at((500.0, 300.0)), None);
    }

    #[test]
    fn empty_title_bar_drags() {
        assert_eq!(
            hit_test((400.0, 15.0), SIZE, state()),
            Some(Target::TitleDrag)
        );
    }

    #[test]
    fn buttons_are_on_the_right_by_default() {
        // Close is the last of the three buttons.
        assert_eq!(
            hit_test((1000.0 - 20.0, 15.0), SIZE, state()),
            Some(Target::Button(WindowButton::Close))
        );
        assert_eq!(
            hit_test((1000.0 - 46.0 * 2.5, 15.0), SIZE, state()),
            Some(Target::Button(WindowButton::Minimize))
        );
    }

    #[test]
    fn buttons_can_be_on_the_left() {
        let s = HitState {
            buttons_left: true,
            ..state()
        };
        assert_eq!(
            hit_test((TOGGLE_W + 10.0, 15.0), SIZE, s),
            Some(Target::Button(WindowButton::Close))
        );
    }

    #[test]
    fn sidebar_toggle_is_at_the_top_left() {
        assert_eq!(
            hit_test((10.0, 15.0), SIZE, state()),
            Some(Target::SidebarToggle)
        );
    }

    #[test]
    fn all_eight_edges_and_corners_resize() {
        let cases = [
            ((2.0, 300.0), ResizeDirection::West),
            ((998.0, 300.0), ResizeDirection::East),
            ((500.0, 2.0), ResizeDirection::North),
            ((500.0, 658.0), ResizeDirection::South),
            ((2.0, 2.0), ResizeDirection::NorthWest),
            ((998.0, 2.0), ResizeDirection::NorthEast),
            ((2.0, 658.0), ResizeDirection::SouthWest),
            ((998.0, 658.0), ResizeDirection::SouthEast),
        ];
        for (pos, want) in cases {
            assert_eq!(
                hit_test(pos, SIZE, state()),
                Some(Target::Resize(want)),
                "{pos:?}"
            );
        }
    }

    #[test]
    fn maximized_windows_do_not_resize() {
        let s = HitState {
            maximized: true,
            ..state()
        };
        assert_eq!(hit_test((200.0, 2.0), SIZE, s), Some(Target::TitleDrag));
        // With no edges left, the app's own regions decide; chrome adds nothing.
        assert_eq!(hit_test((2.0, 300.0), SIZE, s), None);
    }
}
