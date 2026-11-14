// view.rs: turns editor state into Renderer calls.
// Drawing is stateless: everything is recomputed each frame from `Editor`
// plus the geometry in `Layout`. Only visible lines are touched.

use crate::editor::highlight::{Span, TokenKind};
use crate::editor::Editor;
use crate::render::{Icon, Renderer};
use crate::ui::focus::Focus;
use crate::ui::hitmap::Target;
use crate::ui::layout::Layout;
use crate::ui::sidebar::{self, SidebarState};
use crate::ui::theme::Palette;
use crate::window::WindowButton;
use crate::workspace::Workspace;

const GUTTER_PAD: f32 = 8.0;
const CURSOR_W: f32 = 2.0;
/// How far left of the caret's own column the active indent guide is drawn.
/// The caret sits exactly on that column, and a 1 px guide under a 2 px caret
/// is invisible: pressing Tab used to bury the guide you just moved to. Six
/// pixels read as "behind the cursor" without looking detached (issue #7).
const GUIDE_CARET_GAP: f32 = 6.0;

/// Everything the chrome needs that is not editor state.
pub struct Chrome<'a> {
    /// `Ludicrous: <file name>`, or just `Ludicrous` for an unnamed buffer.
    pub title: &'a str,
    pub modified: bool,
    pub maximized: bool,
    /// The item under the mouse, for hover feedback. Same `Target` the hit map
    /// and the cursor use, so the pill and the pointer can never disagree.
    pub hover: Option<Target>,
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
    /// Highlight spans for the visible lines, one vec per line starting at the
    /// first drawn line. Empty means "plain text": no highlighting, off, or a
    /// large file.
    pub spans: &'a [Vec<Span>],
    /// Line index the first `spans` entry belongs to.
    pub span_first: usize,
    /// Vertical scroll in (fractional) lines. Whole numbers when animations are
    /// off; in between while a scroll is easing. Glyphs are always placed on
    /// whole pixels, so the text never blurs.
    pub scroll_lines: f32,
    /// True while files are being dragged over the window: draw the drop
    /// highlight (v1.2 section 5.2).
    pub drop_hover: bool,
    /// The updater's one-line message, or `None`. Shown at the right of the
    /// status bar with a small download badge (v1.2 section 9.2).
    pub update_note: Option<&'a str>,
    /// Whether to draw the badge next to the note. False for a progress or a
    /// failure message, true for "available" and "ready".
    pub update_badge: bool,
    /// Diagnostics for the active file (v1.2 section 7.3). Squiggles, gutter
    /// markers and the status bar counts all read this one list.
    pub diagnostics: &'a crate::lsp::diagnostics::Diagnostics,
    /// The tooltip to draw now, if the pointer has rested on a diagnostic.
    pub tooltip: Option<&'a DiagTooltip>,
    /// Spaces per indent level, for the indent guides (v1.2 section 6).
    pub tab_width: usize,
    /// Draw the indent guides at all. Off while the settings page is open
    /// (there is no text) or when they would only add noise.
    pub indent_guides: bool,
}

/// A diagnostic hover message that has earned its 300 ms and is on screen.
pub struct DiagTooltip {
    /// Window points, already inside the window (the view clamps it).
    pub x: f32,
    pub y: f32,
    /// The message's first line only, already clipped by the app.
    pub text: String,
    /// Errors get the red border, warnings the amber one.
    pub severity: crate::lsp::diagnostics::Severity,
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
        draw_editor(r, ed, l, p, chrome);
    }
    draw_status_bar(r, ed, l, p, chrome);

    // Drop highlight: a 2 px accent ring around the whole window, on top of
    // everything, plus one short line of text. Both disappear with the drop.
    if chrome.drop_hover {
        r.rounded_outline(
            1.0,
            1.0,
            l.width - 2.0,
            l.height - 2.0,
            radius,
            2.0,
            p.accent,
        );
        let label = "Drop to open";
        let w = r.ui_text_width(label);
        let (cx, cy) = (l.width * 0.5, l.height * 0.5);
        r.rounded_rect(
            cx - w * 0.5 - 10.0,
            cy - 11.0,
            w + 20.0,
            22.0,
            5.0,
            p.sidebar_bg,
        );
        r.text_ui(cx - w * 0.5, cy - 6.5, label, p.text);
    }

    // The diagnostic tooltip sits above everything except the frame border.
    if let Some(tooltip) = chrome.tooltip {
        draw_tooltip(r, l, p, tooltip);
    }

    // Border last, so it sits on top of the panels' edges.
    if !chrome.maximized {
        r.rounded_outline(0.0, 0.0, l.width, l.height, radius, 1.0, p.border);
    }
}

/// The hover tooltip for a diagnostic (v1.2 section 7.3): a small panel with a
/// severity-coloured edge and one clipped line of message.
///
/// It is clamped into the window, so a diagnostic on the last visible line
/// still shows its message above the status bar and a long one cannot run past
/// the right edge.
fn draw_tooltip(r: &mut Renderer, l: &Layout, p: &Palette, t: &DiagTooltip) {
    let pad = 8.0;
    let text_w = r.ui_text_width(&t.text);
    let w = text_w + pad * 2.0 + 4.0;
    let h = l.cell_h + pad * 2.0 - 4.0;
    // Anchor near the pointer, then push it inside the window.
    let x = t.x.min(l.width - w - 4.0).max(4.0);
    let y =
        t.y.min(l.height - l.status_h - h - 4.0)
            .max(l.title_h + 2.0);
    r.rounded_rect(x, y, w, h, 4.0, p.tooltip_bg);
    r.rounded_outline(x, y, w, h, 4.0, 1.0, p.tooltip_border);
    // A 2 px severity stripe on the left edge: the same colour as the squiggle.
    r.rect(x + 1.0, y + 3.0, 2.0, h - 6.0, diag_color(p, t.severity));
    r.text_ui(x + pad + 2.0, y + pad - 2.0, &t.text, p.text);
}

fn draw_title_bar(r: &mut Renderer, l: &Layout, chrome: &Chrome) {
    let p = &chrome.palette;
    let h = l.title_h;
    r.rect(0.0, 0.0, l.width, h, p.sidebar_bg);
    r.rect(0.0, h - 1.0, l.width, 1.0, p.border);

    // One shared layout decides where the toggle, the title and the buttons
    // go, so left-aligned window buttons can never sit on the title
    // (issue #6.2).
    let t = crate::window::TitleLayout::new(l.width, chrome.buttons_left);

    // Sidebar toggle: the panel icon, on a subtle hover pill.
    let toggle_hover = matches!(chrome.hover, Some(Target::SidebarToggle));
    if toggle_hover {
        r.rounded_rect(t.toggle_x + 4.0, 4.0, 22.0, h - 8.0, 4.0, p.pill_hover);
    }
    r.icon(
        t.toggle_x + 6.0,
        (h - 16.0) * 0.5,
        Icon::Sidebar,
        16.0,
        p.muted,
    );

    // Title text, centred vertically in the bar.
    let text_y = (h - l.cell_h) * 0.5 + 1.0;
    let title_x = t.title_x;
    r.text_ui(title_x, text_y, chrome.title, p.text);
    if chrome.modified {
        let dot_x = title_x + r.ui_text_width(chrome.title) + 6.0;
        r.rounded_rect(dot_x, h * 0.5 - 3.0, 6.0, 6.0, 3.0, p.accent);
    }

    // Window buttons: right as minimize/maximize/close, or left as
    // close/minimize/maximize next to the sidebar toggle. The order matches
    // `window::chrome_regions`' order so clicks land on what is drawn.
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
    let start = t.buttons_x;
    for (i, button) in order.into_iter().enumerate() {
        let x = start + i as f32 * bw;
        let hovered = chrome.hover == Some(Target::Button(button));
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

fn draw_editor(r: &mut Renderer, ed: &Editor, l: &Layout, p: &Palette, chrome: &Chrome) {
    let caret = chrome.caret;
    let scroll_lines = chrome.scroll_lines;
    let body_top = l.title_h;
    let body_h = l.height - l.status_h - body_top;

    // Gutter band and its divider (nothing to separate when it is off).
    if l.show_numbers {
        r.rect(l.sidebar_w, body_top, l.gutter_w, body_h, p.gutter_bg);
        r.rect(l.sidebar_w + l.gutter_w, body_top, 1.0, body_h, p.border);
    }

    // The animated position may sit between two lines. `first` is the top line
    // that is (partly) visible and `frac` how far it has moved up, in lines.
    let anim = scroll_lines.max(0.0);
    let first = anim.floor() as usize;
    let frac = anim - first as f32;
    // One extra line at the bottom because the top one is partly scrolled out.
    let last = (first + l.rows + 1).min(ed.buffer.line_count());
    let mut digits_buf = [0u8; 20];

    // Line numbers live in the gutter, outside the text-area clip, so they are
    // drawn first. `show_line_numbers` can turn them off entirely.
    if l.show_numbers {
        for idx in first..last {
            let y = l.text_y + (idx - first) as f32 * l.cell_h - frac * l.cell_h;
            let num = format_usize(idx + 1, &mut digits_buf);
            let nx = (l.sidebar_w + l.gutter_w - GUTTER_PAD - num.len() as f32 * l.cell_w).max(0.0);
            r.text(nx, y, num, p.gutter_text);
        }
    }
    // Gutters markers, one per line with a diagnostic (v1.2 section 7.3): the
    // worst severity on the line, as a 3 px bar at the left edge of the band.
    // Drawn before the text so it can never cover a line number.
    if l.show_numbers && !chrome.diagnostics.is_empty() {
        for idx in first..last {
            let Some(sev) = chrome.diagnostics.worst_on_line(&ed.buffer, idx) else {
                continue;
            };
            let y = (l.text_y + (idx - first) as f32 * l.cell_h - frac * l.cell_h).round();
            r.rect(
                l.sidebar_w + 2.0,
                y + l.cell_h * 0.5 - 5.0,
                3.0,
                10.0,
                diag_color(p, sev),
            );
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
        // Whole pixels only: a glyph drawn at a fractional y is blurry.
        let y = (l.text_y + (idx - first) as f32 * l.cell_h - frac * l.cell_h).round();

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
        // Indent guides (section 6): 1 px lines at each tab stop inside the
        // leading whitespace. Queued before the text so they sit behind the
        // glyphs, and as hairlines they stay in the background batch, so a
        // screen full of them adds no draw call.
        if chrome.indent_guides {
            let guides = crate::ui::indent::guides(&line, chrome.tab_width);
            if !guides.is_empty() {
                // How far the line has scrolled left, in points. `text_pen_x`
                // already knows about tab widths, so the guides land on the
                // text's own boundaries rather than on a guessed cell count.
                let scroll_px = r.text_pen_x(l.text_x, &line, ed.scroll_x) - l.text_x;
                let here = (idx == ed.cursor.line).then(|| {
                    crate::ui::indent::active_level(&guides, &line, ed.cursor.col, chrome.tab_width)
                });
                for g in guides {
                    if g.col < ed.scroll_x {
                        continue;
                    }
                    // Absolute window x, minus the scroll: this is a *point*
                    // coordinate like every other `r.rect` argument.
                    let x = (r.text_pen_x(l.text_x, &line, g.col) - scroll_px).round();
                    let active = here.flatten() == Some(g.level);
                    let color = if active {
                        p.indent_guide_active
                    } else {
                        p.indent_guide
                    };
                    // The caret is `CURSOR_W` wide and sits on the same x as
                    // the guide of the level it is in. Nudge the caret's own
                    // guide back so both stay visible, but never out of the
                    // text area.
                    let gx = if active {
                        (x - GUIDE_CARET_GAP).max(l.text_x)
                    } else {
                        x
                    };
                    r.rect(gx, y + 1.0, 1.0, l.cell_h - 2.0, color);
                }
            }
        }
        // Only the visible columns are laid out: a 100,000-character line
        // costs the same as a short one.
        let spans = chrome
            .spans
            .get(idx.wrapping_sub(chrome.span_first))
            .filter(|s| !s.is_empty());
        match spans {
            // Highlighted: draw one run per colour. All runs share the glyph
            // batch, so this is more bytes, not more draw calls.
            Some(spans) => draw_runs(r, &line, spans, ed.scroll_x, l, y, p),
            // Plain text: the same single call as before, which is why a file
            // with no language (or highlighting off) costs nothing extra.
            None => {
                let visible = visible_slice(&line, ed.scroll_x, l.cols);
                r.text(l.text_x, y, visible, p.text);
            }
        }
    }

    // Squiggles: one per diagnostic range, on the visible lines only. They are
    // queued after the text so they paint over it, and the renderer keeps them
    // in the over-text batch, so they add no draw call (v1.2 section 7.3).
    if !chrome.diagnostics.is_empty() {
        let gutter = r.text_pen_x(l.text_x, "", ed.scroll_x) - l.text_x;
        for idx in first..last {
            for d in chrome.diagnostics.items() {
                let (d_first, d_last) = d.lines(&ed.buffer);
                if idx < d_first || idx > d_last {
                    continue;
                }
                let line = ed.buffer.line(idx);
                let line_start = ed.buffer.char_index(idx, 0);
                let line_len = line.chars().count();
                // Clamp the range to this line: a multi-line diagnostic gets a
                // squiggle per line, which is what editors show.
                let a = d.range.start.max(line_start) - line_start;
                let b = (d.range.end.min(line_start + line_len)) - line_start;
                let (a, b) = (a.min(line_len), b.min(line_len));
                // A zero-width diagnostic still gets a visible mark.
                let (a, b) = if b <= a {
                    (a, (a + 1).min(line_len.max(a + 1)))
                } else {
                    (a, b)
                };
                let x0 = r.text_pen_x(l.text_x, &line, a) - gutter;
                let x1 = r.text_pen_x(l.text_x, &line, b) - gutter;
                if x1 <= l.text_x || x0 >= l.text_x + l.text_w {
                    continue; // scrolled out of view
                }
                let y = (l.text_y + (idx - first) as f32 * l.cell_h - frac * l.cell_h).round();
                r.squiggle(
                    x0.max(l.text_x),
                    y + l.cell_h - 3.0,
                    (x1 - x0.max(l.text_x)).max(2.0),
                    3.0,
                    diag_color(p, d.severity),
                );
            }
        }
    }

    // Cursor: a thin vertical bar at the character cell under the cursor.
    // The x comes from the renderer's pen position, not `col * cell_w`, so it
    // lines up with the glyphs (see issue #1: fractional advance drift).
    if caret && ed.cursor.line >= first && ed.cursor.line < last + 1 {
        let y = (l.text_y + (ed.cursor.line - first) as f32 * l.cell_h - frac * l.cell_h).round();
        let line = ed.buffer.line(ed.cursor.line);
        // Both positions are measured on the full line, so tab stops stay
        // correct; the difference is where the caret sits in the scrolled view.
        let x = r.text_pen_x(l.text_x, &line, ed.cursor.col)
            - (r.text_pen_x(l.text_x, &line, ed.scroll_x) - l.text_x);
        r.rect(x, y, CURSOR_W, l.cell_h, p.accent);
    }

    r.pop_clip();
}

fn draw_status_bar(r: &mut Renderer, ed: &Editor, l: &Layout, p: &Palette, chrome: &Chrome) {
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
    } else if let Some(reason) = ed.highlighting_off_reason() {
        // v1.2 section 6: say why there are no colours instead of leaving the
        // user guessing. Muted, because it is information, not a problem.
        r.text_ui(mx, text_y, reason, p.muted);
    }

    let pos = format!("Ln {}, Col {}", ed.cursor.line + 1, ed.cursor.col + 1);
    let px = (l.width - GUTTER_PAD - r.ui_text_width(&pos)).max(sx);
    r.text_ui(px, text_y, &pos, p.status_text);

    // Diagnostic counts, left of the position (v1.2 section 7.3): an icon and
    // a number per severity, with the errors first. Nothing is drawn for a
    // clean file, so a healthy status bar is unchanged.
    let (errors, warnings) = chrome.diagnostics.counts();
    if errors + warnings > 0 {
        let mut cx = px - 10.0;
        let icon = 11.0;
        let iy = text_y + (l.cell_h - icon) * 0.5;
        if warnings > 0 {
            let label = warnings.to_string();
            let tw = r.ui_text_width(&label);
            cx -= tw;
            r.text_ui(cx, text_y, &label, p.diag_warning);
            cx -= icon + 3.0;
            r.icon(cx, iy, Icon::TriangleAlert, icon, p.diag_warning);
            cx -= 10.0;
        }
        if errors > 0 {
            let label = errors.to_string();
            let tw = r.ui_text_width(&label);
            cx -= tw;
            r.text_ui(cx, text_y, &label, p.diag_error);
            cx -= icon + 3.0;
            r.icon(cx, iy, Icon::CircleX, icon, p.diag_error);
        }
        // The counts must never be pushed under the position text.
        let _ = cx;
    }

    // The updater's line sits left of the position, right-aligned so it grows
    // toward the middle of the bar and never pushes the cursor position away.
    if let Some(note) = chrome.update_note {
        let nw = r.ui_text_width(note);
        let mut nx = px - 16.0 - nw;
        if chrome.update_badge {
            // A tiny download mark: the tray, the stem and the arrow head, all
            // rects so it looks the same at any font size.
            let icon_x = nx - 13.0;
            let iy = text_y + 1.0;
            r.rect(icon_x + 2.0, iy, 2.0, 7.0, p.accent);
            r.rect(icon_x + 1.0, iy + 5.0, 4.0, 2.0, p.accent);
            r.rect(icon_x + 0.0, iy + 9.0, 6.0, 1.5, p.accent);
            nx = icon_x - 5.0 - nw;
        }
        if nx > sx + 4.0 {
            r.text_ui(nx, text_y, note, p.muted);
        }
    }
}

/// The part of `line` that is visible when scrolled `scroll_x` columns right.
/// Returns a slice of the original string, so no allocation per line.
/// Draw one line as coloured runs.
///
/// Spans can overlap (a function name inside a call expression) and later spans
/// win, so the line is walked once from the left, taking whichever span is
/// active at each character. Bytes are not tracked separately: the slice is
/// made by character index, which is what the spans carry.
fn draw_runs(
    r: &mut Renderer,
    line: &str,
    spans: &[Span],
    scroll_x: usize,
    l: &Layout,
    y: f32,
    p: &Palette,
) {
    let chars: Vec<char> = line.chars().skip(scroll_x).take(l.cols).collect();
    if chars.is_empty() {
        return;
    }
    let base = scroll_x as u32;
    // Start positions are sorted; a span covers `[start, end)` in line columns.
    let mut active: Option<(u32, TokenKind)> = None;
    let mut run_start = 0usize;
    let mut run_kind = TokenKind::Unknown;

    // `kind_at(col)` = the last span that covers `col`, or Unknown.
    let mut si = 0usize;
    let kind_at = |col: u32, si: &mut usize, active: &mut Option<(u32, TokenKind)>| {
        // Advance past spans that ended before this column.
        while let Some((end, _)) = *active {
            if end <= col {
                *active = None;
            } else {
                break;
            }
        }
        while let Some(sp) = spans.get(*si) {
            if sp.start_col > col {
                break;
            }
            // A later span starting here replaces the current one.
            if sp.end_col > col {
                *active = Some((sp.end_col, sp.kind));
            }
            *si += 1;
        }
        active.map(|(_, k)| k).unwrap_or(TokenKind::Unknown)
    };

    for (i, _c) in chars.iter().enumerate() {
        let col = base + i as u32;
        let kind = kind_at(col, &mut si, &mut active);
        if i == 0 {
            run_kind = kind;
            run_start = 0;
            continue;
        }
        if kind != run_kind {
            emit_run(r, line, base, run_start, i, l, y, p.syntax(run_kind));
            run_start = i;
            run_kind = kind;
        }
    }
    emit_run(
        r,
        line,
        base,
        run_start,
        chars.len(),
        l,
        y,
        p.syntax(run_kind),
    );
}

/// Draw one same-colour run of a line, at the x where its first character
/// belongs.
///
/// Runs used to be queued at `l.text_x` regardless of where they started,
/// which put every run back at the left margin: with highlighting on, indented
/// code was drawn as overlapping text from column 0 (issue #7). The start x is
/// measured on the *full* line so tab stops and the horizontal scroll agree
/// with what the caret and the guides compute.
#[allow(clippy::too_many_arguments)] // geometry, line, span and palette
fn emit_run(
    r: &mut Renderer,
    line: &str,
    base: u32,
    from: usize,
    to: usize,
    l: &Layout,
    y: f32,
    color: crate::render::Color,
) {
    if to <= from {
        return;
    }
    let (text, col) = run_slice(line, base as usize, from, to);
    let scroll_px = r.text_pen_x(l.text_x, line, base as usize) - l.text_x;
    let x = r.text_pen_x(l.text_x, line, col) - scroll_px;
    r.text(x, y, &text, color);
}

/// The text of one run and the column of the line it starts at.
///
/// Separate from `emit_run` so the column arithmetic can be tested without a
/// GL context: a run starting at index 8 must report column 8, not 0. That is
/// the bug in issue #7, where every run was drawn at the left margin.
fn run_slice(line: &str, base: usize, from: usize, to: usize) -> (String, usize) {
    let text: String = line
        .chars()
        .skip(base + from)
        .take(to.saturating_sub(from))
        .collect();
    (text, base + from)
}

/// Squiggle and gutter colour for a severity. Info and hints share the calm
/// blue: they are information, not a problem.
fn diag_color(p: &Palette, sev: crate::lsp::diagnostics::Severity) -> crate::render::Color {
    use crate::lsp::diagnostics::Severity;
    match sev {
        Severity::Error => p.diag_error,
        Severity::Warning => p.diag_warning,
        Severity::Information | Severity::Hint => p.diag_info,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_reports_the_column_it_starts_at() {
        let line = "        let x = 1;";
        // Indented code with a keyword span over `let`: the run must start at
        // column 8, not at 0 (issue #7).
        let (text, col) = run_slice(line, 0, 8, 11);
        assert_eq!(text, "let");
        assert_eq!(col, 8);
    }

    #[test]
    fn a_run_after_a_horizontal_scroll_keeps_its_place() {
        let line = "1234567890abcdef";
        // Scrolled 10 columns, run covering the visible span 2..6.
        let (text, col) = run_slice(line, 10, 2, 6);
        assert_eq!(text, "cdef");
        assert_eq!(col, 12);
    }

    #[test]
    fn a_run_past_the_end_of_the_line_is_empty() {
        let (text, col) = run_slice("abc", 0, 5, 9);
        assert!(text.is_empty());
        assert_eq!(col, 5);
    }
}
