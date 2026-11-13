// layout.rs: where things go, in points. No drawing here.
//
// The window is one rounded rectangle: a title bar across the top, an optional
// sidebar on the left, then the editor (gutter + text area) and a status bar.

use crate::render::Renderer;
use crate::ui::theme;

pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    /// Title bar height; the editor starts below it.
    pub title_h: f32,
    /// Sidebar width in points (0 when hidden).
    pub sidebar_w: f32,
    pub gutter_w: f32,
    /// False when line numbers are turned off: the gutter disappears and the
    /// text area gets its width back.
    pub show_numbers: bool,
    pub text_x: f32,
    pub text_y: f32,
    /// Width of the text area in points (right edge of the window minus the
    /// gutter and padding). Used for clipping and horizontal scrolling.
    pub text_w: f32,
    pub status_h: f32,
    /// Number of full text lines that fit between the top and the status bar.
    pub rows: usize,
    /// Number of character cells that fit across the text area.
    pub cols: usize,
}

const PAD_X: f32 = 8.0; // padding inside the gutter and the text area
const PAD_Y: f32 = 4.0; // padding above the first line
const STATUS_LINES: f32 = 1.6; // status bar height in character rows

impl Layout {
    pub fn new(r: &Renderer, line_count: usize, width: f32, height: f32, sidebar_w: f32) -> Self {
        Self::with_numbers(r, line_count, width, height, sidebar_w, true)
    }

    /// Same, with the line-number gutter optionally turned off.
    pub fn with_numbers(
        r: &Renderer,
        line_count: usize,
        width: f32,
        height: f32,
        sidebar_w: f32,
        show_numbers: bool,
    ) -> Self {
        let (cell_w, cell_h) = r.cell_size();
        let digits = line_count.max(1).to_string().len() as f32;
        let gutter_w = if show_numbers {
            (digits * cell_w + PAD_X * 2.0).ceil()
        } else {
            0.0
        };
        let status_h = (cell_h * STATUS_LINES).ceil();
        let title_h = theme::TITLE_H;
        let text_y = title_h + PAD_Y;
        let usable = (height - status_h - text_y).max(cell_h);
        let rows = (usable / cell_h).floor().max(1.0) as usize;
        let text_x = sidebar_w + gutter_w + PAD_X;
        let text_w = (width - text_x - PAD_X).max(cell_w);
        let cols = (text_w / cell_w).floor().max(1.0) as usize;

        Self {
            width,
            height,
            cell_w,
            cell_h,
            title_h,
            sidebar_w,
            gutter_w,
            show_numbers,
            text_x,
            text_y,
            text_w,
            status_h,
            rows,
            cols,
        }
    }

    /// Map a point in the window to a buffer position, or `None` when the
    /// point is outside the text area (gutter, sidebar, title or status bar).
    ///
    /// `scroll`/`scroll_x` are the editor's current scroll offsets, so the
    /// result is in buffer coordinates, not screen ones.
    pub fn text_position(
        &self,
        x: f32,
        y: f32,
        scroll: usize,
        scroll_x: usize,
    ) -> Option<(usize, usize)> {
        if x < self.text_x || x >= self.text_x + self.text_w {
            return None;
        }
        if y < self.text_y || y >= self.height - self.status_h {
            return None;
        }
        let row = ((y - self.text_y) / self.cell_h).floor().max(0.0) as usize;
        let col = ((x - self.text_x) / self.cell_w).floor().max(0.0) as usize;
        Some((scroll + row, scroll_x + col))
    }
}
