// layout.rs: where things go, in points. No drawing here.
// The UI is deliberately simple: a left gutter, a text area, a status bar.

use crate::render::Renderer;

pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub cell_w: f32,
    pub cell_h: f32,
    pub gutter_w: f32,
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
    pub fn new(r: &Renderer, line_count: usize, width: f32, height: f32) -> Self {
        let (cell_w, cell_h) = r.cell_size();
        let digits = line_count.max(1).to_string().len() as f32;
        let gutter_w = (digits * cell_w + PAD_X * 2.0).ceil();
        let status_h = (cell_h * STATUS_LINES).ceil();
        let text_y = PAD_Y;
        let usable = (height - status_h - text_y).max(cell_h);
        let rows = (usable / cell_h).floor().max(1.0) as usize;
        let text_x = gutter_w + PAD_X;
        let text_w = (width - text_x - PAD_X).max(cell_w);
        let cols = (text_w / cell_w).floor().max(1.0) as usize;

        Self {
            width,
            height,
            cell_w,
            cell_h,
            gutter_w,
            text_x,
            text_y,
            text_w,
            status_h,
            rows,
            cols,
        }
    }
}
