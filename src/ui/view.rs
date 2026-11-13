// view.rs: turns editor state into Renderer calls.
// Drawing is stateless: everything is recomputed each frame from `Editor`
// plus the geometry in `Layout`. Only visible lines are touched.

use crate::editor::Editor;
use crate::render::Renderer;
use crate::ui::layout::Layout;

// Hard-coded dark theme for v1. "Ugly but readable" is the brief.
pub mod theme {
    use crate::render::Color;

    pub const BACKGROUND: Color = [0.09, 0.10, 0.12, 1.0];
    pub const GUTTER_BG: Color = [0.12, 0.13, 0.15, 1.0];
    pub const GUTTER_TEXT: Color = [0.38, 0.41, 0.47, 1.0];
    pub const TEXT: Color = [0.86, 0.88, 0.90, 1.0];
    pub const CURSOR: Color = [0.95, 0.70, 0.30, 1.0];
    pub const STATUS_BG: Color = [0.16, 0.18, 0.21, 1.0];
    pub const STATUS_TEXT: Color = [0.72, 0.76, 0.82, 1.0];
    pub const MODIFIED: Color = [0.95, 0.70, 0.30, 1.0];
    pub const ERROR: Color = [0.95, 0.45, 0.40, 1.0];
    pub const SELECTION: Color = [0.20, 0.32, 0.48, 1.0];
}

const GUTTER_PAD: f32 = 8.0;
const CURSOR_W: f32 = 2.0;

pub fn draw(r: &mut Renderer, ed: &Editor, l: &Layout) {
    // Background for the whole window, then the gutter band.
    r.rect(0.0, 0.0, l.width, l.height, theme::BACKGROUND);
    let body_h = l.height - l.status_h;
    r.rect(0.0, 0.0, l.gutter_w, body_h, theme::GUTTER_BG);
    r.rect(l.gutter_w, 0.0, 1.0, body_h, theme::STATUS_BG);

    let first = ed.scroll;
    let last = (first + l.rows).min(ed.buffer.line_count());
    let mut digits_buf = [0u8; 20];

    // Clip to the text area so a long line can never paint over the gutter or
    // the status bar. The clip is in points; the renderer scales it.
    r.push_clip(
        l.text_x,
        l.text_y,
        l.text_w,
        l.height - l.status_h - l.text_y,
    );

    for idx in first..last {
        let y = l.text_y + (idx - first) as f32 * l.cell_h;

        // Right-align the line number inside the gutter.
        let num = format_usize(idx + 1, &mut digits_buf);
        let nx = (l.gutter_w - GUTTER_PAD - num.len() as f32 * l.cell_w).max(0.0);
        r.text(nx, y, num, theme::GUTTER_TEXT);

        let line = ed.buffer.line(idx);
        // Selection highlight goes behind the text of this line.
        if let Some((start, end)) = ed.selection() {
            let line_start = ed.buffer.char_index(idx, 0);
            let line_end = line_start + line.chars().count();
            let a = start.max(line_start);
            let b = end.min(line_end);
            if a < b {
                let (from, to) = (a - line_start, b - line_start);
                let x0 = r.text_pen_x(l.text_x, &line, from);
                let x1 = r.text_pen_x(l.text_x, &line, to);
                let sx = x0 - (r.text_pen_x(l.text_x, &line, ed.scroll_x) - l.text_x);
                r.rect(sx, y, x1 - x0, l.cell_h, theme::SELECTION);
            }
        }
        // Only the visible columns are laid out: a 100,000-character line
        // costs the same as a short one.
        let visible = visible_slice(&line, ed.scroll_x, l.cols);
        r.text(l.text_x, y, visible, theme::TEXT);
    }

    // Cursor: a thin vertical bar at the character cell under the cursor.
    // The x comes from the renderer's pen position, not `col * cell_w`, so it
    // lines up with the glyphs (see issue #1: fractional advance drift).
    if ed.cursor.line >= first && ed.cursor.line < last {
        let y = l.text_y + (ed.cursor.line - first) as f32 * l.cell_h;
        let line = ed.buffer.line(ed.cursor.line);
        // Both positions are measured on the full line, so tab stops stay
        // correct; the difference is where the caret sits in the scrolled view.
        let x = r.text_pen_x(l.text_x, &line, ed.cursor.col)
            - (r.text_pen_x(l.text_x, &line, ed.scroll_x) - l.text_x);
        r.rect(x, y, CURSOR_W, l.cell_h, theme::CURSOR);
    }

    r.pop_clip();

    // Status bar: filename on the left, position on the right.
    let sy = l.height - l.status_h;
    r.rect(0.0, sy, l.width, l.status_h, theme::STATUS_BG);
    let text_y = sy + ((l.status_h - l.cell_h) / 2.0).max(0.0) + 1.0;

    let name = match &ed.buffer.path {
        Some(p) => p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("[No Name]"),
        None => "[No Name]",
    };
    r.text(GUTTER_PAD, text_y, name, theme::STATUS_TEXT);
    if let Some(msg) = &ed.status {
        // A failed save is more urgent than the modified marker, so it takes
        // that spot in the bar.
        let mx = GUTTER_PAD + (name.chars().count() + 1) as f32 * l.cell_w;
        r.text(mx, text_y, msg, theme::ERROR);
    } else if ed.buffer.modified {
        let mx = GUTTER_PAD + (name.chars().count() + 1) as f32 * l.cell_w;
        r.text(mx, text_y, "[modified]", theme::MODIFIED);
    }

    let pos = format!("Ln {}, Col {}", ed.cursor.line + 1, ed.cursor.col + 1);
    let px = (l.width - GUTTER_PAD - pos.len() as f32 * l.cell_w).max(0.0);
    r.text(px, text_y, &pos, theme::STATUS_TEXT);
}

/// The part of `line` that is visible when scrolled `scroll_x` columns right.
/// Returns a slice of the original string, so no allocation per line.
fn visible_slice(line: &str, scroll_x: usize, cols: usize) -> &str {
    if scroll_x == 0 {
        // Fast path: take at most `cols` characters.
        return match line.char_indices().nth(cols) {
            Some((i, _)) => &line[..i],
            None => line,
        };
    }
    let mut it = line.char_indices().skip(scroll_x);
    let Some((start, _)) = it.next() else {
        return "";
    };
    match it.nth(cols.saturating_sub(1)) {
        Some((end, _)) => &line[start..end],
        None => &line[start..],
    }
}

/// Small stack-formatted line number, avoiding an allocation per visible line.
fn format_usize(mut n: usize, buf: &mut [u8; 20]) -> &str {
    if n == 0 {
        return "0";
    }
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    // Every byte written is an ASCII digit.
    std::str::from_utf8(&buf[i..]).unwrap_or("?")
}
