// sidebar.rs: the project sidebar (expanded, rail and hidden).
//
// It draws from the workspace model (open buffers + the file-tree rows) and
// computes the geometry the app uses for hit testing, so a click always lands
// on the row that was drawn. No winit, no GL, no filesystem access.

use crate::render::{Color, Icon, Renderer};
use crate::ui::focus::Focus;
use crate::ui::layout::Layout;
use crate::ui::theme;
use crate::workspace::Workspace;

/// Height of the app-mark header at the top of the sidebar.
pub const HEADER_H: f32 = 36.0;
/// Height of a "OPEN FILES" / "FILES" section label.
pub const LABEL_H: f32 = 22.0;
/// Height of the Settings row above the project card.
pub const FOOTER_H: f32 = 28.0;
/// Height of the project card at the bottom.
pub const CARD_H: f32 = 48.0;
/// Height of one icon slot in rail mode.
const RAIL_SLOT: f32 = 40.0;

/// Mouse/keyboard state the sidebar keeps between frames.
#[derive(Clone, Default)]
pub struct SidebarState {
    /// Selected row in the file tree, for keyboard navigation.
    pub selected: usize,
    /// Row under the mouse in the file tree.
    pub hover_tree: Option<usize>,
    /// Row under the mouse in the OPEN FILES list.
    pub hover_open: Option<usize>,
    /// Settings row / rail icon under the mouse.
    pub hover_icon: Option<usize>,
    /// File-tree scroll offset in points (always a whole number of rows).
    pub scroll: f32,
}

/// Vertical geometry of the sidebar, recomputed each frame.
pub struct Geometry {
    pub sidebar_w: f32,
    pub top: f32,
    pub height: f32,
    pub open_y: f32,
    pub open_visible: usize,
    pub tree_y: f32,
    pub tree_h: f32,
    pub tree_rows: usize,
    pub footer_y: f32,
    pub card_y: f32,
    pub rail: bool,
}

impl Geometry {
    /// Lay the sidebar out between the title bar and the bottom of the window.
    pub fn new(sidebar_w: f32, top: f32, height: f32, open_count: usize) -> Self {
        let rail = sidebar_w > 0.0 && sidebar_w < theme::SIDEBAR_W;
        if rail {
            return Self {
                sidebar_w,
                top,
                height,
                open_y: top,
                open_visible: 0,
                tree_y: top,
                tree_h: 0.0,
                tree_rows: 0,
                footer_y: top,
                card_y: top,
                rail,
            };
        }

        let pad = theme::PAD * 0.5;
        let open_y = top + HEADER_H + LABEL_H;
        // Open files never take more than a third of the sidebar, so the tree
        // keeps its space even with many buffers open.
        let max_open = ((height * 0.33 / theme::ROW_H).floor() as usize).max(1);
        let open_visible = open_count.min(max_open);

        let divider = open_y + open_visible as f32 * theme::ROW_H + pad;
        let tree_y = divider + 1.0 + pad + LABEL_H;
        let footer_y = (top + height - CARD_H - FOOTER_H - pad).max(tree_y);
        let tree_h = (footer_y - pad - tree_y).max(theme::ROW_H);

        Self {
            sidebar_w,
            top,
            height,
            open_y,
            open_visible,
            tree_y,
            tree_h,
            tree_rows: (tree_h / theme::ROW_H).floor().max(1.0) as usize,
            footer_y,
            card_y: footer_y + FOOTER_H,
            rail,
        }
    }

    pub fn from_layout(l: &Layout, open_count: usize) -> Self {
        Self::new(l.sidebar_w, l.title_h, l.height - l.title_h, open_count)
    }

    /// Y of tree row `index` for the current scroll.
    fn tree_row_y(&self, index: usize, scroll: f32) -> f32 {
        self.tree_y + index as f32 * theme::ROW_H - scroll
    }

    /// What is under `(x, y)`? Mirrors the drawing order exactly.
    pub fn hit(&self, x: f32, y: f32, open_count: usize, row_count: usize, scroll: f32) -> Hit {
        if self.sidebar_w <= 0.0 || x >= self.sidebar_w || y < self.top {
            return Hit::Empty;
        }
        if self.rail {
            let first = self.top + theme::PAD;
            if y >= first && y < first + RAIL_SLOT {
                return Hit::RailFiles;
            }
            if y >= first + RAIL_SLOT && y < first + RAIL_SLOT * 2.0 {
                return Hit::RailSettings;
            }
            return Hit::Empty;
        }
        if y < self.top + HEADER_H {
            return Hit::Header;
        }
        if y >= self.open_y && y < self.open_y + self.open_visible as f32 * theme::ROW_H {
            let i = ((y - self.open_y) / theme::ROW_H).floor() as usize;
            return if i < open_count.min(self.open_visible) {
                Hit::OpenFile(i)
            } else {
                Hit::Empty
            };
        }
        if y >= self.tree_y && y < self.tree_y + self.tree_h {
            let i = ((y - self.tree_y + scroll) / theme::ROW_H).floor() as usize;
            return if i < row_count {
                Hit::TreeRow(i)
            } else {
                Hit::Empty
            };
        }
        if y >= self.footer_y && y < self.footer_y + FOOTER_H {
            return Hit::Settings;
        }
        Hit::Empty
    }
}

/// A clickable region of the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Header,
    OpenFile(usize),
    TreeRow(usize),
    Settings,
    RailFiles,
    RailSettings,
    Empty,
}

/// Largest scroll offset that still shows the last row.
pub fn max_scroll(tree_h: f32, row_count: usize) -> f32 {
    (row_count as f32 * theme::ROW_H - tree_h).max(0.0)
}

/// Draw the sidebar. `geo` must come from `Geometry::from_layout`.
pub fn draw(
    r: &mut Renderer,
    ws: &Workspace,
    l: &Layout,
    geo: &Geometry,
    state: &SidebarState,
    focus: Focus,
) {
    let w = l.sidebar_w;
    r.rect(0.0, geo.top, w, geo.height, theme::SIDEBAR_BG);
    r.rect(w - 1.0, geo.top, 1.0, geo.height, theme::BORDER);

    if geo.rail {
        draw_rail(r, geo, state);
        return;
    }

    let pad = theme::PAD;
    let inner_x = pad * 0.5;
    let inner_w = w - pad;

    // Header: accent mark + project folder name.
    let mark = 14.0;
    r.rounded_rect(
        pad,
        geo.top + (HEADER_H - mark) * 0.5,
        mark,
        mark,
        3.0,
        theme::ACCENT,
    );
    let name = crate::workspace::root_name(&ws.root).unwrap_or_else(|| "Ludicrous".to_string());
    let tx = pad + mark + 8.0;
    let name = truncate(r, &name, w - tx - pad);
    r.text_ui(tx, geo.top + (HEADER_H - 13.0) * 0.5, &name, theme::TEXT);
    r.rect(0.0, geo.top + HEADER_H, w, 1.0, theme::BORDER);

    r.text_ui(pad, geo.top + HEADER_H + 4.0, "OPEN FILES", theme::MUTED);

    // OPEN FILES list: one pill row per buffer, active and hover states.
    let labels = ws.file_labels();
    for (i, (label, dirty)) in labels.iter().take(geo.open_visible).enumerate() {
        let y = geo.open_y + i as f32 * theme::ROW_H;
        if i == ws.active {
            pill(r, inner_x, y, inner_w, theme::PILL_ACTIVE);
        } else if state.hover_open == Some(i) {
            pill(r, inner_x, y, inner_w, theme::PILL_HOVER);
        }
        r.icon(
            pad + 2.0,
            y + (theme::ROW_H - 14.0) * 0.5,
            Icon::File,
            14.0,
            theme::MUTED,
        );
        let color = if i == ws.active {
            theme::TEXT
        } else {
            theme::MUTED
        };
        let label = truncate(r, label, w - pad - 44.0);
        r.text_ui(pad + 22.0, y + 7.0, &label, color);
        if *dirty {
            let dot = 6.0;
            r.rounded_rect(
                w - pad - dot - 2.0,
                y + (theme::ROW_H - dot) * 0.5,
                dot,
                dot,
                dot * 0.5,
                theme::ACCENT,
            );
        }
    }

    // Divider and the FILES label.
    let div_y = geo.open_y + geo.open_visible as f32 * theme::ROW_H + pad * 0.5;
    r.rect(pad, div_y, w - pad * 2.0, 1.0, theme::BORDER);
    r.text_ui(pad, div_y + pad * 0.5 + 4.0, "FILES", theme::MUTED);

    // File tree: clipped to its viewport, scrolled by whole rows, and only the
    // visible rows are drawn (same rule as the text area).
    let rows = ws.tree.rows();
    let scroll = state.scroll.clamp(0.0, max_scroll(geo.tree_h, rows.len()));
    r.push_clip(0.0, geo.tree_y, w - 1.0, geo.tree_h);
    let first = (scroll / theme::ROW_H).floor() as usize;
    for (n, row) in rows.iter().enumerate().skip(first).take(geo.tree_rows + 1) {
        let y = geo.tree_row_y(n, scroll);
        if y + theme::ROW_H <= geo.tree_y {
            continue;
        }
        if n == state.selected && focus == Focus::Sidebar {
            pill(r, inner_x, y, inner_w, theme::PILL_ACTIVE);
        } else if state.hover_tree == Some(n) {
            pill(r, inner_x, y, inner_w, theme::PILL_HOVER);
        }

        let indent = pad + 2.0 + row.depth as f32 * 14.0;
        if row.unreadable || row.more > 0 {
            // Synthetic rows ("(unreadable)", "… N more"): muted, no chevron.
            let name = truncate(r, &row.name, w - indent - pad);
            r.text_ui(indent, y + 7.0, &name, theme::MUTED);
            continue;
        }
        if row.is_dir {
            let chevron = if row.expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            };
            r.icon(indent, y + 7.0, chevron, 12.0, theme::MUTED);
        }
        let icon = match (row.is_dir, row.expanded) {
            (true, true) => Icon::FolderOpen,
            (true, false) => Icon::Folder,
            (false, _) => Icon::File,
        };
        let icon_x = indent + 14.0;
        r.icon(icon_x, y + 7.0, icon, 14.0, theme::MUTED);
        let tx = icon_x + 20.0;
        let color = if row.is_dir {
            theme::TEXT
        } else {
            theme::STATUS_TEXT
        };
        let name = truncate(r, &row.name, w - tx - pad);
        r.text_ui(tx, y + 7.0, &name, color);
    }
    r.pop_clip();

    // Footer: Settings row.
    if state.hover_icon == Some(0) {
        pill(r, inner_x, geo.footer_y, inner_w, theme::PILL_HOVER);
    }
    r.icon(
        pad + 2.0,
        geo.footer_y + (FOOTER_H - 14.0) * 0.5,
        Icon::Gear,
        14.0,
        theme::MUTED,
    );
    r.text_ui(pad + 22.0, geo.footer_y + 7.0, "Settings", theme::MUTED);

    // Project card: folder name over the (clipped) path.
    let card_y = geo.card_y;
    r.rounded_rect(
        inner_x,
        card_y,
        inner_w,
        CARD_H,
        theme::PILL_RADIUS,
        theme::PILL_ACTIVE,
    );
    let short = truncate(r, &name, inner_w - 16.0);
    r.text_ui(pad + 8.0, card_y + 7.0, &short, theme::TEXT);
    let path = ws
        .root
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "no folder open".to_string());
    let path = truncate(r, &path, inner_w - 16.0);
    r.text_ui(pad + 8.0, card_y + 25.0, &path, theme::MUTED);
}

fn draw_rail(r: &mut Renderer, geo: &Geometry, state: &SidebarState) {
    let w = geo.sidebar_w;
    let first = geo.top + theme::PAD;
    for (i, icon) in [Icon::Files, Icon::Gear].into_iter().enumerate() {
        let y = first + i as f32 * RAIL_SLOT;
        if state.hover_icon == Some(i) {
            r.rounded_rect(
                theme::PAD * 0.5,
                y + 4.0,
                w - theme::PAD,
                RAIL_SLOT - 8.0,
                theme::PILL_RADIUS,
                theme::PILL_HOVER,
            );
        }
        let size = 18.0;
        r.icon(
            (w - size) * 0.5,
            y + (RAIL_SLOT - size) * 0.5,
            icon,
            size,
            theme::MUTED,
        );
    }
}

fn pill(r: &mut Renderer, x: f32, y: f32, w: f32, color: Color) {
    r.rounded_rect(x, y + 1.0, w, theme::ROW_H - 2.0, theme::PILL_RADIUS, color);
}

/// Clip `s` to `width` points, ending in `…` when it does not fit.
fn truncate(r: &Renderer, s: &str, width: f32) -> String {
    if width <= 0.0 {
        return String::new();
    }
    let cell = r.ui_cell_size().0;
    let fits = (width / cell).floor() as usize;
    if s.chars().count() <= fits {
        return s.to_string();
    }
    if fits <= 1 {
        return "..".to_string();
    }
    // ASCII ellipsis: the atlas holds ASCII only, so "…" would draw as "?".
    let mut out: String = s.chars().take(fits.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_files_sit_below_the_header() {
        let geo = Geometry::new(theme::SIDEBAR_W, 30.0, 670.0, 3);
        assert_eq!(geo.open_visible, 3);
        assert_eq!(
            geo.hit(100.0, geo.open_y + 2.0, 3, 10, 0.0),
            Hit::OpenFile(0)
        );
        assert_eq!(
            geo.hit(100.0, geo.open_y + theme::ROW_H + 2.0, 3, 10, 0.0),
            Hit::OpenFile(1)
        );
    }

    #[test]
    fn the_header_and_footer_are_distinct() {
        let geo = Geometry::new(theme::SIDEBAR_W, 30.0, 670.0, 2);
        assert_eq!(geo.hit(100.0, 34.0, 2, 10, 0.0), Hit::Header);
        assert_eq!(
            geo.hit(100.0, geo.footer_y + 4.0, 2, 10, 0.0),
            Hit::Settings
        );
    }

    #[test]
    fn tree_rows_follow_the_scroll_offset() {
        let geo = Geometry::new(theme::SIDEBAR_W, 30.0, 670.0, 1);
        assert_eq!(
            geo.hit(100.0, geo.tree_y + 2.0, 1, 40, 0.0),
            Hit::TreeRow(0)
        );
        // Scrolled down one row, the same point is row 1.
        assert_eq!(
            geo.hit(100.0, geo.tree_y + 2.0, 1, 40, theme::ROW_H),
            Hit::TreeRow(1)
        );
        // Past the last row nothing is hit.
        assert_eq!(
            geo.hit(100.0, geo.tree_y + 2.0, 1, 40, theme::ROW_H * 40.0),
            Hit::Empty
        );
    }

    #[test]
    fn the_rail_has_files_and_settings_slots() {
        let geo = Geometry::new(theme::SIDEBAR_RAIL_W, 30.0, 670.0, 2);
        assert!(geo.rail);
        assert_eq!(geo.hit(20.0, 30.0 + 20.0, 2, 0, 0.0), Hit::RailFiles);
        assert_eq!(
            geo.hit(20.0, 30.0 + 20.0 + RAIL_SLOT, 2, 0, 0.0),
            Hit::RailSettings
        );
    }

    #[test]
    fn the_tree_never_scrolls_past_its_last_row() {
        assert_eq!(max_scroll(300.0, 5), 0.0);
        assert_eq!(max_scroll(100.0, 10), 10.0 * theme::ROW_H - 100.0);
    }
}
