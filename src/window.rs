// window.rs: the shell's own window decorations.
//
// winit gives us an undecorated window; this module decides what a point in
// that window means (drag the title bar, resize an edge, press a button, or
// just edit text). It is the only place that knows about window chrome, and
// like input.rs it never touches the editor or the renderer.

use winit::dpi::PhysicalPosition;

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    /// The text area: clicks go to the editor.
    Content,
    /// Empty title bar: drag the window, double-click to maximize.
    TitleDrag,
    /// The sidebar toggle button.
    SidebarToggle,
    Button(WindowButton),
    Resize(ResizeDirection),
}

/// What the hit test needs to know about the window's current state.
#[derive(Clone, Copy)]
pub struct HitState {
    pub maximized: bool,
    pub fullscreen: bool,
    /// Window buttons on the left (traffic lights) instead of the right.
    pub buttons_left: bool,
}

/// Decide what is under `pos` (in points, top-left origin).
///
/// Order matters: resize edges win over everything, because they are the
/// outermost few pixels; then the title bar's buttons; then the drag region.
pub fn hit_test(pos: (f32, f32), size: (f32, f32), state: HitState) -> Hit {
    let (x, y) = pos;
    let (w, h) = size;

    // Maximized and fullscreen windows have no draggable edges.
    if !state.maximized && !state.fullscreen {
        if let Some(dir) = resize_direction(x, y, w, h) {
            return Hit::Resize(dir);
        }
    }

    if y < TITLE_H {
        // Sidebar toggle sits at the far left, before the title text.
        if x < TOGGLE_W {
            return Hit::SidebarToggle;
        }
        // Three buttons, laid out from the chosen side.
        let buttons_w = BUTTON_W * 3.0;
        let start = if state.buttons_left {
            TOGGLE_W
        } else {
            w - buttons_w
        };
        if x >= start && x < start + buttons_w {
            let index = ((x - start) / BUTTON_W).floor() as usize;
            let button = if state.buttons_left {
                // Traffic-light order: close, minimize, maximize.
                [
                    WindowButton::Close,
                    WindowButton::Minimize,
                    WindowButton::Maximize,
                ][index.min(2)]
            } else {
                [
                    WindowButton::Minimize,
                    WindowButton::Maximize,
                    WindowButton::Close,
                ][index.min(2)]
            };
            return Hit::Button(button);
        }
        return Hit::TitleDrag;
    }

    Hit::Content
}

/// Which edge or corner `(x, y)` is on, if any.
fn resize_direction(x: f32, y: f32, w: f32, h: f32) -> Option<ResizeDirection> {
    let left = x < RESIZE_EDGE;
    let right = x > w - RESIZE_EDGE;
    let top = y < RESIZE_EDGE;
    let bottom = y > h - RESIZE_EDGE;
    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeDirection::NorthWest),
        (_, true, true, _) => Some(ResizeDirection::NorthEast),
        (true, _, _, true) => Some(ResizeDirection::SouthWest),
        (_, true, _, true) => Some(ResizeDirection::SouthEast),
        (true, _, _, _) => Some(ResizeDirection::West),
        (_, true, _, _) => Some(ResizeDirection::East),
        (_, _, true, _) => Some(ResizeDirection::North),
        (_, _, _, true) => Some(ResizeDirection::South),
        _ => None,
    }
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
    fn middle_of_the_window_is_content() {
        assert_eq!(hit_test((500.0, 300.0), SIZE, state()), Hit::Content);
    }

    #[test]
    fn empty_title_bar_drags() {
        assert_eq!(hit_test((400.0, 15.0), SIZE, state()), Hit::TitleDrag);
    }

    #[test]
    fn buttons_are_on_the_right_by_default() {
        // Close is the last of the three buttons.
        assert_eq!(
            hit_test((1000.0 - 20.0, 15.0), SIZE, state()),
            Hit::Button(WindowButton::Close)
        );
        assert_eq!(
            hit_test((1000.0 - 46.0 * 2.5, 15.0), SIZE, state()),
            Hit::Button(WindowButton::Minimize)
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
            Hit::Button(WindowButton::Close)
        );
    }

    #[test]
    fn sidebar_toggle_is_at_the_top_left() {
        assert_eq!(hit_test((10.0, 15.0), SIZE, state()), Hit::SidebarToggle);
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
            assert_eq!(hit_test(pos, SIZE, state()), Hit::Resize(want), "{pos:?}");
        }
    }

    #[test]
    fn maximized_windows_do_not_resize() {
        let s = HitState {
            maximized: true,
            ..state()
        };
        assert_eq!(hit_test((200.0, 2.0), SIZE, s), Hit::TitleDrag);
        assert_eq!(hit_test((2.0, 300.0), SIZE, s), Hit::Content);
    }
}
