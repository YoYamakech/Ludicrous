// view.rs: turns editor state into Renderer calls.
// Drawing is stateless: everything is recomputed each frame from `Editor`
// plus the geometry in `Layout`. Only visible lines are touched.

use crate::editor::Editor;
use crate::render::{Icon, Renderer};
use crate::ui::focus::Focus;
use crate::ui::layout::Layout;
use crate::ui::sidebar::{self, SidebarState};
use crate::ui::theme::Palette;
use crate::window::{Hit, WindowButton};
use crate::workspace::Workspace;

const GUTTER_PAD: f32 = 8.0;
const CURSOR_W: f32 = 2.0;

/// Everything the chrome needs that is not editor state.
pub struct Chrome<'a> {
    /// `Ludicrous: <file name>`, or just `Ludicrous` for an unnamed buffer.
    pub title: &'a str,
    pub modified: bool,
    pub maximized: bool,
    /// The item under the mouse, for hover feedback.
    pub hover: Option<Hit>,
    pub focus: Focus,
    pub sidebar: SidebarState,
    /// Colours for the active theme.
    pub palette: Palette,
    /// The settings page replaces the text area; only the chrome is drawn.
    pub settings: bool,
    /// Window corner radius, from the config.
    pub radius: f32,
    /// Window buttons on the left instead of the right.
    pub buttons_left: bool,
    /// False during the dark half of a blinking cursor.
    pub caret: bool,
}

pub fn draw(r: &mut Renderer, ws: &Workspace, l: &Layout, chrome: &Chrome) {
    let p = &chrome.palette;
    // The sidebar needs the active buffer for the text area; without one there
    // is nothing to draw at all.
    let Some(ed) = ws.active_editor() else {
        return;
    };
    // The window itself: one rounded rectangle, with a 1 px border. The
    // renderer's window mask clips everything to the same shape.
    let radius = if chrome.maximized { 0.0 } else { chrome.radius };
    r.set_window_shape(l.width, l.height, radius);
    r.rounded_rect(0.0, 0.0, l.width, l.height, radius, p.bg);

    draw_title_bar(r, l, chrome);
    if l.sidebar_w > 0.0 {
        let geo = sidebar::Geometry::from_layout(l, ws.file_labels().len());
        sidebar::draw(r, ws, l, &geo, &chrome.sidebar, chrome.focus, p);
    }
    if !chrome.settings {
        draw_editor(r, ed, l, p, chrome.caret);
    }
    draw_status_bar(r, ed, l, p);

    // Border last, so it sits on top of the panels' edges.
    if !chrome.maximized {
        r.rounded_outline(0.0, 0.0, l.width, l.height, radius, 1.0, p.border);
    }
}

fn draw_title_bar(r: &mut Renderer, l: &Layout, chrome: &Chrome) {
    let p = &chrome.palette;
    let h = l.title_h;
    r.rect(0.0, 0.0, l.width, h, p.sidebar_bg);
    r.rect(0.0, h - 1.0, l.width, 1.0, p.border);

    // Sidebar toggle: the panel icon, on a subtle hover pill.
    let toggle_hover = matches!(chrome.hover, Some(Hit::SidebarToggle));
    if toggle_hover {
        r.rounded_rect(4.0, 4.0, 22.0, h - 8.0, 4.0, p.pill_hover);
    }
    r.icon(6.0, (h - 16.0) * 0.5, Icon::Sidebar, 16.0, p.muted);

    // Title text, centred vertically in the bar.
    let text_y = (h - l.cell_h) * 0.5 + 1.0;
    let title_x = 30.0;
    r.text_ui(title_x, text_y, chrome.title, p.text);
    if chrome.modified {
        let dot_x = title_x + r.ui_text_width(chrome.title) + 6.0;
        r.rounded_rect(dot_x, h * 0.5 - 3.0, 6.0, 6.0, 3.0, p.accent);
    }

    // Window buttons: right as minimize/maximize/close, or left as
    // close/minimize/maximize next to the sidebar toggle. The order matches
    // `window::hit_test` so clicks land on what is drawn.
    let bw = crate::window::BUTTON_W;
    let order: [WindowButton; 3] = if chrome.buttons_left {
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
    let start = if chrome.buttons_left {
        crate::window::TOGGLE_W
    } else {
        l.width - bw * 3.0
    };
    for (i, button) in order.into_iter().enumerate() {
        let x = start + i as f32 * bw;
        let hovered = chrome.hover == Some(Hit::Button(button));
        if hovered {
            let color = if button == WindowButton::Close {
                p.close_hover
            } else {
                p.pill_hover
            };
            r.rect(x, 0.0, bw, h, color);
        }
        let cx = x + bw * 0.5;
        let cy = h * 0.5;
        let fg = if hovered && button == WindowButton::Close {
            p.text
        } else {
            p.muted
        };
        let icon = match button {
            WindowButton::Minimize => Icon::Minimize,
            WindowButton::Maximize if chrome.maximized => Icon::Restore,
            WindowButton::Maximize => Icon::Maximize,
            WindowButton::Close => Icon::Close,
        };
        let size = 14.0;
        r.icon(cx - size * 0.5, cy - size * 0.5, icon, size, fg);
    }
}

fn draw_editor(r: &mut Renderer, ed: &Editor, l: &Layout, p: &Palette, caret: bool) {
    let body_top = l.title_h;
    let body_h = l.height - l.status_h - body_top;

    // Gutter band and its divider (nothing to separate when it is off).
    if l.show_numbers {
        r.rect(l.sidebar_w, body_top, l.gutter_w, body_h, p.gutter_bg);
        r.rect(l.sidebar_w + l.gutter_w, body_top, 1.0, body_h, p.border);
    }

    let first = ed.scroll;
    let last = (first + l.rows).min(ed.buffer.line_count());
    let mut digits_buf = [0u8; 20];

    // Line numbers live in the gutter, outside the text-area clip, so they are
    // drawn first. `show_line_numbers` can turn them off entirely.
    if l.show_numbers {
        for idx in first..last {
            let y = l.text_y + (idx - first) as f32 * l.cell_h;
            let num = format_usize(idx + 1, &mut digits_buf);
            let nx = (l.sidebar_w + l.gutter_w - GUTTER_PAD - num.len() as f32 * l.cell_w).max(0.0);
            r.text(nx, y, num, p.gutter_text);
        }
    }

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
                r.rect(sx, y, x1 - x0, l.cell_h, p.selection);
            }
        }
        // Only the visible columns are laid out: a 100,000-character line
        // costs the same as a short one.
        let visible = visible_slice(&line, ed.scroll_x, l.cols);
        r.text(l.text_x, y, visible, p.text);
    }

    // Cursor: a thin vertical bar at the character cell under the cursor.
    // The x comes from the renderer's pen position, not `col * cell_w`, so it
    // lines up with the glyphs (see issue #1: fractional advance drift).
    if caret && ed.cursor.line >= first && ed.cursor.line < last {
        let y = l.text_y + (ed.cursor.line - first) as f32 * l.cell_h;
        let line = ed.buffer.line(ed.cursor.line);
        // Both positions are measured on the full line, so tab stops stay
        // correct; the difference is where the caret sits in the scrolled view.
        let x = r.text_pen_x(l.text_x, &line, ed.cursor.col)
            - (r.text_pen_x(l.text_x, &line, ed.scroll_x) - l.text_x);
        r.rect(x, y, CURSOR_W, l.cell_h, p.accent);
    }

    r.pop_clip();
}

fn draw_status_bar(r: &mut Renderer, ed: &Editor, l: &Layout, p: &Palette) {
    // The bar belongs to the editing area, so it starts right of the sidebar:
    // the sidebar's project card runs all the way to the bottom edge.
    let sx = l.sidebar_w;
    let sw = l.width - sx;
    let sy = l.height - l.status_h;
    r.rect(sx, sy, sw, l.status_h, p.status_bg);
    r.rect(sx, sy, sw, 1.0, p.border);
    let text_y = sy + ((l.status_h - l.cell_h) / 2.0).max(0.0) + 1.0;

    let name = match &ed.buffer.path {
        Some(p) => p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("[No Name]"),
        None => "[No Name]",
    };
    // Text is offset by the bar's own left edge: with a sidebar open the bar
    // starts at `sx`, and drawing at x = 8 would overlap the project card.
    let tx = sx + GUTTER_PAD;
    r.text_ui(tx, text_y, name, p.status_text);
    let mx = tx + r.ui_text_width(name) + 8.0;
    if let Some(msg) = &ed.status {
        // A failed save is more urgent than the modified marker, so it takes
        // that spot in the bar.
        r.text_ui(mx, text_y, msg, p.close_hover);
    } else if ed.is_dirty() {
        r.text_ui(mx, text_y, "[modified]", p.accent);
    }

    let pos = format!("Ln {}, Col {}", ed.cursor.line + 1, ed.cursor.col + 1);
    let px = (l.width - GUTTER_PAD - r.ui_text_width(&pos)).max(sx);
    r.text_ui(px, text_y, &pos, p.status_text);
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
