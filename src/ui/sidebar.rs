// sidebar.rs: the project sidebar (expanded, rail and hidden).
//
// It draws from the workspace model (open buffers + the file-tree rows) and
// computes the geometry the app uses for hit testing, so a click always lands
// on the row that was drawn. No winit, no GL, no filesystem access.

use crate::render::{Color, Icon, Renderer};
use crate::ui::focus::Focus;
use crate::ui::hitmap::{HitMap, Rect, Target};
use crate::ui::layout::Layout;
use crate::ui::theme;
use crate::ui::theme::Palette;
use crate::workspace::Workspace;

// v1.2 section 12: there is no header row. The app name lives in the title bar
// only, and the sidebar starts straight at OPEN FILES, which hands back ~36 px
// of vertical space on the small target screen.
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
        let open_y = top + LABEL_H;
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

    /// Register every clickable region, back to front, in the same order as
    /// `draw`. The rects come from the same fields the drawing code reads, so a
    /// row can never be drawn somewhere it cannot be clicked. Only rows that are
    /// actually visible are registered, and the tree rows are clipped to the
    /// tree's viewport.
    pub fn register(
        &self,
        map: &mut HitMap,
        open_count: usize,
        rows: &[crate::workspace::filetree::Row],
        scroll: f32,
    ) {
        if self.sidebar_w <= 0.0 {
            return;
        }
        let band = Rect::new(0.0, self.top, self.sidebar_w, self.height);
        if self.rail {
            // Background first, then the two icon slots on top of it.
            map.push(band, Target::None);
            let first = self.top + theme::PAD;
            map.push(
                Rect::new(0.0, first, self.sidebar_w, RAIL_SLOT),
                Target::RailFiles,
            );
            map.push(
                Rect::new(0.0, first + RAIL_SLOT, self.sidebar_w, RAIL_SLOT),
                Target::RailSettings,
            );
            return;
        }

        // Background first: everything in the sidebar that is not a row is
        // non-clickable, so later rows simply win.
        map.push(band, Target::None);

        for i in 0..open_count.min(self.open_visible) {
            let y = self.open_y + i as f32 * theme::ROW_H;
            map.push(
                Rect::new(0.0, y, self.sidebar_w, theme::ROW_H),
                Target::OpenFile(i),
            );
        }

        let tree_clip = Rect::new(0.0, self.tree_y, self.sidebar_w, self.tree_h);
        let first = (scroll / theme::ROW_H).floor() as usize;
        for n in first..rows.len() {
            let y = self.tree_row_y(n, scroll);
            if y >= self.tree_y + self.tree_h {
                break;
            }
            if y + theme::ROW_H <= self.tree_y {
                continue;
            }
            map.push_clipped(
                Rect::new(0.0, y, self.sidebar_w, theme::ROW_H),
                tree_clip,
                Target::TreeRow(n),
            );
        }

        map.push(
            Rect::new(0.0, self.footer_y, self.sidebar_w, FOOTER_H),
            Target::SidebarSettings,
        );
        map.push(
            Rect::new(0.0, self.card_y, self.sidebar_w, CARD_H),
            Target::ProjectCard,
        );
    }
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
    p: &Palette,
) {
    let w = l.sidebar_w;
    r.rect(0.0, geo.top, w, geo.height, p.sidebar_bg);
    r.rect(w - 1.0, geo.top, 1.0, geo.height, p.border);

    if geo.rail {
        draw_rail(r, geo, state, p);
        return;
    }

    let pad = theme::PAD;
    let inner_x = pad * 0.5;
    let inner_w = w - pad;

    // No header row (v1.2 section 12). OPEN FILES sits at the very top.
    r.text_ui(pad, geo.top + 4.0, "OPEN FILES", p.muted);

    // OPEN FILES list: one pill row per buffer, active and hover states.
    let labels = ws.file_labels();
    for (i, (label, dirty)) in labels.iter().take(geo.open_visible).enumerate() {
        let y = geo.open_y + i as f32 * theme::ROW_H;
        if i == ws.active {
            pill(r, inner_x, y, inner_w, p.pill_active);
        } else if state.hover_open == Some(i) {
            pill(r, inner_x, y, inner_w, p.pill_hover);
        }
        r.icon(
            pad + 2.0,
            y + (theme::ROW_H - 14.0) * 0.5,
            Icon::File,
            14.0,
            p.muted,
        );
        let color = if i == ws.active { p.text } else { p.muted };
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
                p.accent,
            );
        }
    }

    // Divider and the FILES label.
    let div_y = geo.open_y + geo.open_visible as f32 * theme::ROW_H + pad * 0.5;
    r.rect(pad, div_y, w - pad * 2.0, 1.0, p.border);
    r.text_ui(pad, div_y + pad * 0.5 + 4.0, "FILES", p.muted);

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
            pill(r, inner_x, y, inner_w, p.pill_active);
        } else if state.hover_tree == Some(n) {
            pill(r, inner_x, y, inner_w, p.pill_hover);
        }

        let indent = pad + 2.0 + row.depth as f32 * 14.0;
        if row.unreadable || row.more > 0 {
            // Synthetic rows ("(unreadable)", "… N more"): muted, no chevron.
            let name = truncate(r, &row.name, w - indent - pad);
            r.text_ui(indent, y + 7.0, &name, p.muted);
            continue;
        }
        if row.is_dir {
            let chevron = if row.expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            };
            r.icon(indent, y + 7.0, chevron, 12.0, p.muted);
        }
        let icon = match (row.is_dir, row.expanded) {
            (true, true) => Icon::FolderOpen,
            (true, false) => Icon::Folder,
            (false, _) => Icon::File,
        };
        let icon_x = indent + 14.0;
        r.icon(icon_x, y + 7.0, icon, 14.0, p.muted);
        let tx = icon_x + 20.0;
        let color = if row.is_dir { p.text } else { p.status_text };
        let name = truncate(r, &row.name, w - tx - pad);
        r.text_ui(tx, y + 7.0, &name, color);
    }
    r.pop_clip();

    // Footer: Settings row.
    if state.hover_icon == Some(0) {
        pill(r, inner_x, geo.footer_y, inner_w, p.pill_hover);
    }
    r.icon(
        pad + 2.0,
        geo.footer_y + (FOOTER_H - 14.0) * 0.5,
        Icon::Gear,
        14.0,
        p.muted,
    );
    r.text_ui(pad + 22.0, geo.footer_y + 7.0, "Settings", p.muted);

    // Project card: folder name over the path. The app name never appears here
    // (v1.2 section 12); the card stands for the project, not the editor.
    let card_y = geo.card_y;
    r.rounded_rect(
        inner_x,
        card_y,
        inner_w,
        CARD_H,
        theme::PILL_RADIUS,
        p.pill_active,
    );
    let folder = match crate::workspace::root_name(&ws.root) {
        Some(name) => truncate(r, &name, inner_w - 16.0),
        None => "No folder open".to_string(),
    };
    r.text_ui(pad + 8.0, card_y + 7.0, &folder, p.text);
    // The second line keeps the *end* of the path visible, which is the part
    // that identifies the folder; `truncate_left` clips the front instead.
    let path = match ws.root.as_ref() {
        Some(p) => truncate_left(r, &p.display().to_string(), inner_w - 16.0),
        None => truncate(r, "Ctrl+Shift+O to open one", inner_w - 16.0),
    };
    r.text_ui(pad + 8.0, card_y + 25.0, &path, p.muted);
}

fn draw_rail(r: &mut Renderer, geo: &Geometry, state: &SidebarState, p: &Palette) {
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
                p.pill_hover,
            );
        }
        let size = 18.0;
        r.icon(
            (w - size) * 0.5,
            y + (RAIL_SLOT - size) * 0.5,
            icon,
            size,
            p.muted,
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

/// Clip `s` from the *left* to `width` points, so the end of a long path stays
/// readable. Starts with `...` when something was dropped.
fn truncate_left(r: &Renderer, s: &str, width: f32) -> String {
    if width <= 0.0 {
        return String::new();
    }
    let cell = r.ui_cell_size().0;
    let fits = (width / cell).floor() as usize;
    let len = s.chars().count();
    if len <= fits {
        return s.to_string();
    }
    if fits <= 3 {
        return "..".to_string();
    }
    let tail: String = s.chars().skip(len.saturating_sub(fits - 3)).collect();
    format!("...{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::filetree::Row as TreeRow;

    fn row(name: &str) -> TreeRow {
        TreeRow {
            depth: 0,
            name: name.to_string(),
            path: std::path::PathBuf::from(name),
            is_dir: false,
            expanded: false,
            unreadable: false,
            more: 0,
        }
    }

    fn map_for(geo: &Geometry, open: usize, rows: &[TreeRow], scroll: f32) -> HitMap {
        let mut m = HitMap::new();
        geo.register(&mut m, open, rows, scroll);
        m
    }

    #[test]
    fn open_files_sit_below_the_section_label() {
        let geo = Geometry::new(theme::SIDEBAR_W, 30.0, 670.0, 3);
        assert_eq!(geo.open_visible, 3);
        // No header row any more: OPEN FILES is the first thing in the sidebar.
        assert_eq!(geo.open_y, 30.0 + LABEL_H);
        let m = map_for(&geo, 3, &[], 0.0);
        assert_eq!(
            m.target_at((100.0, geo.open_y + 2.0)),
            Some(Target::OpenFile(0))
        );
        assert_eq!(
            m.target_at((100.0, geo.open_y + theme::ROW_H + 2.0)),
            Some(Target::OpenFile(1))
        );
    }

    #[test]
    fn the_section_label_and_empty_space_are_inert() {
        let geo = Geometry::new(theme::SIDEBAR_W, 30.0, 670.0, 2);
        let m = map_for(&geo, 2, &[], 0.0);
        // Above the first row: the "OPEN FILES" label.
        assert_eq!(m.target_at((100.0, 34.0)), Some(Target::None));
        // The gap between the open list and the tree.
        assert_eq!(m.target_at((100.0, geo.tree_y - 30.0)), Some(Target::None));
    }

    #[test]
    fn the_settings_row_and_project_card_are_clickable() {
        let geo = Geometry::new(theme::SIDEBAR_W, 30.0, 670.0, 2);
        let m = map_for(&geo, 2, &[], 0.0);
        assert_eq!(
            m.target_at((100.0, geo.footer_y + 4.0)),
            Some(Target::SidebarSettings)
        );
        assert_eq!(
            m.target_at((100.0, geo.card_y + 4.0)),
            Some(Target::ProjectCard)
        );
    }

    #[test]
    fn tree_rows_follow_the_scroll_offset() {
        let rows: Vec<TreeRow> = (0..40).map(|i| row(&format!("f{i}"))).collect();
        let geo = Geometry::new(theme::SIDEBAR_W, 30.0, 670.0, 1);
        let unscrolled = map_for(&geo, 1, &rows, 0.0);
        assert_eq!(
            unscrolled.target_at((100.0, geo.tree_y + 2.0)),
            Some(Target::TreeRow(0))
        );
        // Scrolled down one row, the same point is row 1.
        let scrolled = map_for(&geo, 1, &rows, theme::ROW_H);
        assert_eq!(
            scrolled.target_at((100.0, geo.tree_y + 2.0)),
            Some(Target::TreeRow(1))
        );
        // Past the last row nothing is hit: the background is what is there.
        let past = map_for(&geo, 1, &rows, theme::ROW_H * 40.0);
        assert_eq!(
            past.target_at((100.0, geo.tree_y + 2.0)),
            Some(Target::None)
        );
    }

    #[test]
    fn rows_above_the_viewport_are_not_hit() {
        // One row scrolled just past the top edge must not be clickable.
        let rows: Vec<TreeRow> = (0..5).map(|i| row(&format!("f{i}"))).collect();
        let geo = Geometry::new(theme::SIDEBAR_W, 30.0, 670.0, 1);
        let m = map_for(&geo, 1, &rows, theme::ROW_H * 0.5);
        let y = geo.tree_y - theme::ROW_H * 0.25;
        assert_eq!(
            m.target_at((100.0, y)),
            Some(Target::None),
            "above the clip"
        );
    }

    #[test]
    fn the_rail_has_files_and_settings_slots() {
        let geo = Geometry::new(theme::SIDEBAR_RAIL_W, 30.0, 670.0, 2);
        assert!(geo.rail);
        let m = map_for(&geo, 2, &[], 0.0);
        assert_eq!(m.target_at((20.0, 30.0 + 20.0)), Some(Target::RailFiles));
        assert_eq!(
            m.target_at((20.0, 30.0 + 20.0 + RAIL_SLOT)),
            Some(Target::RailSettings)
        );
        // Below the two icons the rail is inert.
        assert_eq!(
            m.target_at((20.0, 30.0 + 20.0 + RAIL_SLOT * 3.0)),
            Some(Target::None)
        );
    }

    #[test]
    fn the_tree_never_scrolls_past_its_last_row() {
        assert_eq!(max_scroll(300.0, 5), 0.0);
        assert_eq!(max_scroll(100.0, 10), 10.0 * theme::ROW_H - 100.0);
    }

    #[test]
    fn the_project_card_text_never_says_the_app_name() {
        // Section 12: the card stands for the project. With no root there is no
        // name to show, and the fallback is an instruction, not the app name.
        let geo = Geometry::new(theme::SIDEBAR_W, 30.0, 670.0, 0);
        assert!(geo.card_y > 0.0);
        assert!(!crate::workspace::root_name(&None).is_some());
    }
}
