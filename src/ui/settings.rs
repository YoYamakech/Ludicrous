// settings.rs: the settings page.
//
// It replaces the text area inside the window (no separate window, no overlap),
// opens with Ctrl+, or the sidebar footer, and closes with Esc or Ctrl+, again.
// Every change is applied live and written straight back to the config file.
//
// The lower half of the page (the updater rows and the privacy note) only
// exists with the `updater` feature on. Keeping the list fixed per build means
// drawing, hit testing and keyboard navigation stay in step: `Row::ALL` is
// compiled, not filtered at run time.
//
// The page is a title, a short hint, and a fixed list of rows. Each row is one
// of the widget types from `widgets.rs`, including the v1.2 action row (a
// button that does something instead of holding a value). Keyboard: Up/Down
// select, Left/Right change, Enter activates a toggle or an action. Mouse:
// click any part of a control.

use crate::config::{Buttons, Config, SidebarMode, Theme};
use crate::render::Renderer;
use crate::ui::hitmap::{HitMap, Rect, Target};
use crate::ui::layout::Layout;
use crate::ui::theme::Palette;
use crate::ui::widgets::{self, Widget, CONTROL_W};

/// The settings rows, in display order. Keeping this in one place means the
/// drawing, hit testing and keyboard code cannot drift apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Row {
    FontSize = 0,
    TabWidth,
    LineNumbers,
    CursorBlink,
    WindowButtons,
    CornerRadius,
    Sidebar,
    Theme,
    Animations,
    #[cfg(feature = "updater")]
    CheckUpdates,
    #[cfg(feature = "updater")]
    UpdateAction,
}

impl Row {
    #[cfg(feature = "updater")]
    pub const COUNT: usize = 11;
    #[cfg(not(feature = "updater"))]
    pub const COUNT: usize = 9;
    #[cfg(feature = "updater")]
    pub const ALL: [Row; Row::COUNT] = [
        Row::FontSize,
        Row::TabWidth,
        Row::LineNumbers,
        Row::CursorBlink,
        Row::WindowButtons,
        Row::CornerRadius,
        Row::Sidebar,
        Row::Theme,
        Row::Animations,
        Row::CheckUpdates,
        Row::UpdateAction,
    ];
    #[cfg(not(feature = "updater"))]
    pub const ALL: [Row; Row::COUNT] = [
        Row::FontSize,
        Row::TabWidth,
        Row::LineNumbers,
        Row::CursorBlink,
        Row::WindowButtons,
        Row::CornerRadius,
        Row::Sidebar,
        Row::Theme,
        Row::Animations,
    ];

    pub fn from_index(i: usize) -> Row {
        Row::ALL[i.min(Row::COUNT - 1)]
    }

    pub fn label(self) -> &'static str {
        match self {
            Row::FontSize => "Font size",
            Row::TabWidth => "Tab width",
            Row::LineNumbers => "Show line numbers",
            Row::CursorBlink => "Cursor blink",
            Row::WindowButtons => "Window buttons",
            Row::CornerRadius => "Corner radius",
            Row::Sidebar => "Sidebar",
            Row::Theme => "Theme",
            Row::Animations => "Animations",
            #[cfg(feature = "updater")]
            Row::CheckUpdates => "Check for updates",
            #[cfg(feature = "updater")]
            Row::UpdateAction => "Update",
        }
    }
}

const TAB_WIDTHS: [usize; 3] = [2, 4, 8];
const TAB_LABELS: [&str; 3] = ["2", "4", "8"];
const RADII: [u32; 4] = [0, 6, 10, 14];
const RADIUS_LABELS: [&str; 4] = ["0", "6", "10", "14"];
const BUTTON_LABELS: [&str; 2] = ["left", "right"];
const SIDEBAR_LABELS: [&str; 4] = ["auto", "expanded", "rail", "hidden"];
const THEME_LABELS: [&str; 2] = ["dark", "light"];

pub const PAGE_H: f32 = 46.0; // title + hint above the first row
const PANEL_MAX_W: f32 = 620.0;

/// Panel geometry; `row_y` and `hit` must stay in step with `draw`.
pub struct Geometry {
    pub x: f32,
    pub y: f32,
    pub w: f32,
}

impl Geometry {
    pub fn new(l: &Layout) -> Self {
        // Centre the panel in the editing area, with a sane minimum.
        let avail = (l.width - l.sidebar_w).max(0.0);
        let w = (avail - 80.0).clamp(240.0, PANEL_MAX_W);
        let x = l.sidebar_w + (avail - w) * 0.5;
        Self {
            x,
            y: l.title_h + 24.0,
            w,
        }
    }

    pub fn row_y(&self, row: usize) -> f32 {
        self.y + PAGE_H + row as f32 * widgets::ROW_H
    }

    /// Which row is under `(x, y)`? The *part* of the control comes from the
    /// hit map instead, so clicks and the cursor agree.
    pub fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        if x < self.x || x > self.x + self.w {
            return None;
        }
        let first = self.row_y(0);
        if y < first {
            return None;
        }
        let row = ((y - first) / widgets::ROW_H).floor() as usize;
        (row < Row::COUNT).then_some(row)
    }
}

/// What the pointer is doing, as far as the page cares. Grouped so `draw`
/// keeps a readable argument list.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pointer {
    /// Row under the pointer, if any.
    pub row: Option<usize>,
    /// Option of that row's choice control under the pointer, if any. Comes
    /// from the hit map, so it is the piece a click would hit.
    pub segment: Option<usize>,
}

impl Pointer {
    /// Is this row hovered but not keyboard-selected? The selected row shows a
    /// stronger focus pill, so it never draws the hover wash too.
    fn hovers(self, row: usize, selected: usize) -> bool {
        self.row == Some(row) && row != selected
    }
}

/// Draw the page. Returns the row geometry for hit testing (same maths).
#[allow(clippy::too_many_arguments)] // renderer, layout, theme, config, state
pub fn draw(
    r: &mut Renderer,
    l: &Layout,
    p: &Palette,
    cfg: &Config,
    update: &UpdateInfo,
    selected: usize,
    pointer: Pointer,
) -> Geometry {
    let geo = Geometry::new(l);

    // Panel title and hint.
    r.text_ui(geo.x, geo.y, "Settings", p.text);
    r.text_ui(
        geo.x,
        geo.y + 20.0,
        "Up/Down select  Left/Right change  Esc closes",
        p.muted,
    );

    for (i, row) in Row::ALL.iter().enumerate() {
        let y = geo.row_y(i);
        let focused = i == selected;
        // Which segment is under the pointer, for the per-segment hover wash.
        let hovered = pointer.hovers(i, selected);
        let seg = if hovered { pointer.segment } else { None };
        let widget = widget_for(*row, cfg, update, seg);
        let label = match *row {
            // The updater row explains itself: what is available, or what
            // `Check now` will do. The action button carries the verb.
            #[cfg(feature = "updater")]
            Row::UpdateAction => update.label.as_str(),
            other => other.label(),
        };

        // A full-width hairline between rows keeps the page readable without
        // boxes.
        if i > 0 {
            r.rect(geo.x, y - 1.0, geo.w, 1.0, p.border);
        }
        // The hover wash is drawn by `draw_row`, behind the control, so it can
        // never cover a label (issue #6.3).
        widgets::draw_row(r, p, y, geo.x, geo.w, label, &widget, focused, hovered);
    }

    // Privacy note (v1.2 section 9): say exactly what leaves the machine,
    // directly under the rows, because that is where the toggle is.
    #[cfg(feature = "updater")]
    {
        let y = geo.row_y(Row::COUNT) + 14.0;
        r.text_ui(
            geo.x,
            y,
            "Updates check github.com once a day. No other data is sent.",
            p.muted,
        );
        r.text_ui(
            geo.x,
            y + 16.0,
            "With \"Check for updates\" off, no request is ever made.",
            p.muted,
        );
    }
    geo
}

/// Register the settings page's regions. The whole panel is inert (the arrow),
/// every control piece is clickable (the hand), and the labels stay inert: the
/// cursor follows what a click would actually do.
///
/// The pieces come from `RowGeom::spans`, the same source `RowGeom::part` uses,
/// so the map and a point lookup can never disagree.
pub fn register(map: &mut HitMap, l: &Layout, cfg: &Config, update: &UpdateInfo) {
    let geo = Geometry::new(l);
    // Panel background: drawn over the text area, so it swallows clicks there.
    // The height includes the privacy note when the updater is on, otherwise a
    // click on that text would fall through to the editor.
    #[cfg(feature = "updater")]
    let note_h = 40.0;
    #[cfg(not(feature = "updater"))]
    let note_h = 0.0;
    map.push(
        Rect::new(
            geo.x,
            geo.y,
            geo.w,
            PAGE_H + Row::COUNT as f32 * widgets::ROW_H + note_h,
        ),
        Target::None,
    );
    let control_x = geo.x + geo.w - CONTROL_W;
    let geom = widgets::RowGeom { control_x };
    for i in 0..Row::COUNT {
        let row = Row::from_index(i);
        let widget = widget_for(row, cfg, update, None);
        let y = geo.row_y(i);
        for (x, w, part) in geom.spans(&widget) {
            map.push(
                Rect::new(x, y, w, widgets::ROW_H),
                Target::SettingsRow {
                    row: i,
                    part: Some(part),
                },
            );
        }
    }
}

/// The control a row shows, built from the config so drawing and changing
/// cannot disagree.
/// What the two updater rows say. Built by the app from its `UpdateState`, so
/// this module never needs the updater feature or the network types.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateInfo {
    /// Left-hand label of the action row.
    pub label: String,
    /// Text on the action button.
    pub action: String,
    /// False while a check or download is already running.
    pub enabled: bool,
}

impl Default for UpdateInfo {
    fn default() -> Self {
        Self {
            label: "Check now".to_string(),
            action: "Check now".to_string(),
            enabled: true,
        }
    }
}

pub fn widget_for<'a>(
    row: Row,
    cfg: &Config,
    update: &'a UpdateInfo,
    hover: Option<usize>,
) -> Widget<'a> {
    // Without the updater there is no row to build from it, but the signature
    // stays the same so the callers do not need a feature check.
    #[cfg(not(feature = "updater"))]
    let _ = update;
    match row {
        Row::FontSize => Widget::Stepper(FONT_SIZES_TEXT[font_index(cfg)]),
        Row::TabWidth => Widget::Choice {
            options: &TAB_LABELS,
            index: TAB_WIDTHS
                .iter()
                .position(|w| *w == cfg.tab_width)
                .unwrap_or(1),
            hover,
        },
        Row::LineNumbers => Widget::Toggle(cfg.show_line_numbers),
        Row::CursorBlink => Widget::Toggle(cfg.cursor_blink),
        Row::WindowButtons => Widget::Choice {
            options: &BUTTON_LABELS,
            index: match cfg.window_buttons {
                Buttons::Left => 0,
                Buttons::Right => 1,
            },
            hover,
        },
        Row::CornerRadius => Widget::Choice {
            options: &RADIUS_LABELS,
            index: RADII
                .iter()
                .position(|w| *w == cfg.corner_radius)
                .unwrap_or(2),
            hover,
        },
        Row::Sidebar => Widget::Choice {
            options: &SIDEBAR_LABELS,
            index: match cfg.sidebar {
                SidebarMode::Auto => 0,
                SidebarMode::Expanded => 1,
                SidebarMode::Rail => 2,
                SidebarMode::Hidden => 3,
            },
            hover,
        },
        Row::Theme => Widget::Choice {
            options: &THEME_LABELS,
            index: match cfg.theme {
                Theme::Dark => 0,
                Theme::Light => 1,
            },
            hover,
        },
        Row::Animations => Widget::Toggle(cfg.animations),
        #[cfg(feature = "updater")]
        Row::CheckUpdates => Widget::Toggle(cfg.check_updates),
        // The button's text is the verb the state allows right now.
        #[cfg(feature = "updater")]
        Row::UpdateAction => Widget::Action {
            label: update.action.as_str(),
            enabled: update.enabled,
        },
    }
}

/// Font sizes 10..=28, as static strings so the stepper can borrow one.
const FONT_SIZES_TEXT: [&str; 19] = [
    "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24", "25",
    "26", "27", "28",
];

fn font_index(cfg: &Config) -> usize {
    (cfg.font_size.clamp(10, 28) - 10) as usize
}

/// Change a row's value by `delta` (-1 or +1 is what the UI sends). Toggles
/// flip regardless of the delta. Returns true when the config changed.
pub fn step(cfg: &mut Config, row: Row, delta: i32) -> bool {
    let before = cfg.clone();
    match row {
        Row::FontSize => {
            let f = cfg.font_size.clamp(10, 28) as i32 + delta;
            cfg.font_size = f.clamp(10, 28) as u32;
        }
        Row::TabWidth => {
            let i = TAB_WIDTHS
                .iter()
                .position(|w| *w == cfg.tab_width)
                .unwrap_or(1) as i32;
            let i = (i + delta).clamp(0, TAB_WIDTHS.len() as i32 - 1) as usize;
            cfg.tab_width = TAB_WIDTHS[i];
        }
        Row::LineNumbers => cfg.show_line_numbers = !cfg.show_line_numbers,
        Row::CursorBlink => cfg.cursor_blink = !cfg.cursor_blink,
        Row::WindowButtons => {
            cfg.window_buttons = match cfg.window_buttons {
                Buttons::Left => Buttons::Right,
                Buttons::Right => Buttons::Left,
            }
        }
        Row::CornerRadius => {
            let i = RADII
                .iter()
                .position(|w| *w == cfg.corner_radius)
                .unwrap_or(2) as i32;
            let i = (i + delta).clamp(0, RADII.len() as i32 - 1) as usize;
            cfg.corner_radius = RADII[i];
        }
        Row::Sidebar => {
            let i = match cfg.sidebar {
                SidebarMode::Auto => 0,
                SidebarMode::Expanded => 1,
                SidebarMode::Rail => 2,
                SidebarMode::Hidden => 3,
            };
            let i = (i + delta).clamp(0, 3) as usize;
            cfg.sidebar = match i {
                0 => SidebarMode::Auto,
                1 => SidebarMode::Expanded,
                2 => SidebarMode::Rail,
                _ => SidebarMode::Hidden,
            };
        }
        Row::Theme => {
            cfg.theme = match cfg.theme {
                Theme::Dark => Theme::Light,
                Theme::Light => Theme::Dark,
            }
        }
        Row::Animations => cfg.animations = !cfg.animations,
        #[cfg(feature = "updater")]
        Row::CheckUpdates => cfg.check_updates = !cfg.check_updates,
        // An action row changes no value; the app runs it.
        #[cfg(feature = "updater")]
        Row::UpdateAction => {}
    }
    *cfg != before
}

/// Set a row directly from a clicked segment index. Returns true on change.
pub fn choose(cfg: &mut Config, row: Row, index: usize) -> bool {
    let before = cfg.clone();
    match row {
        Row::TabWidth => {
            if let Some(w) = TAB_WIDTHS.get(index) {
                cfg.tab_width = *w;
            }
        }
        Row::WindowButtons => match index {
            0 => cfg.window_buttons = Buttons::Left,
            _ => cfg.window_buttons = Buttons::Right,
        },
        Row::CornerRadius => {
            if let Some(r) = RADII.get(index) {
                cfg.corner_radius = *r;
            }
        }
        Row::Sidebar => {
            cfg.sidebar = match index {
                0 => SidebarMode::Auto,
                1 => SidebarMode::Expanded,
                2 => SidebarMode::Rail,
                _ => SidebarMode::Hidden,
            }
        }
        Row::Theme => {
            cfg.theme = if index == 0 {
                Theme::Dark
            } else {
                Theme::Light
            };
        }
        // The action row has no segments to choose.
        #[cfg(feature = "updater")]
        Row::UpdateAction => return false,
        #[cfg(feature = "updater")]
        Row::CheckUpdates => return step(cfg, row, 1),
        // Toggles and steppers are not segment choices.
        Row::LineNumbers | Row::CursorBlink | Row::Animations | Row::FontSize => {
            return step(cfg, row, if index == 0 { -1 } else { 1 })
        }
    }
    *cfg != before
}

/// Change the config for one control piece, the same way a click does.
/// `part` is what `RowGeom::part` returned; `None` means the label, which only
/// selects the row.
pub fn apply_part(cfg: &mut Config, row: Row, part: Option<usize>) -> bool {
    let Some(part) = part else {
        return false;
    };
    match widget_for(row, cfg, &UpdateInfo::default(), None) {
        Widget::Toggle(_) => step(cfg, row, 1),
        // An action is not a value: the app decides what clicking it means.
        Widget::Action { .. } => false,
        Widget::Stepper(_) => match part {
            0 => step(cfg, row, -1),
            2 => step(cfg, row, 1),
            _ => false, // the value box in the middle does nothing
        },
        Widget::Choice { .. } => choose(cfg, row, part),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_never_starts_above_the_title_bar() {
        // Issue #6.7: the page must begin below the title bar, so nothing can
        // bleed over its text or buttons.
        let l = Layout {
            width: 1100.0,
            height: 700.0,
            cell_w: 9.0,
            cell_h: 18.0,
            title_h: 30.0,
            sidebar_w: 220.0,
            gutter_w: 44.0,
            show_numbers: true,
            text_x: 272.0,
            text_y: 30.0,
            text_w: 820.0,
            status_h: 24.0,
            rows: 36,
            cols: 91,
        };
        let geo = Geometry::new(&l);
        assert!(
            geo.y > l.title_h,
            "panel top {} vs title {}",
            geo.y,
            l.title_h
        );
        // And the whole page fits inside the editing area horizontally.
        assert!(geo.x >= l.sidebar_w - 0.01);
        assert!(geo.x + geo.w <= l.width + 0.01);
    }

    #[test]
    fn the_spec_defaults_are_reachable_from_the_rows() {
        let cfg = Config::default();
        assert!(matches!(
            widget_for(Row::FontSize, &cfg, &UpdateInfo::default(), None),
            Widget::Stepper("15")
        ));
        assert!(matches!(
            widget_for(Row::LineNumbers, &cfg, &UpdateInfo::default(), None),
            Widget::Toggle(true)
        ));
        assert!(matches!(
            widget_for(Row::CursorBlink, &cfg, &UpdateInfo::default(), None),
            Widget::Toggle(false)
        ));
    }

    #[test]
    fn font_size_stays_inside_10_to_28() {
        let mut cfg = Config::default();
        for _ in 0..50 {
            step(&mut cfg, Row::FontSize, 1);
        }
        assert_eq!(cfg.font_size, 28);
        for _ in 0..50 {
            step(&mut cfg, Row::FontSize, -1);
        }
        assert_eq!(cfg.font_size, 10);
    }

    #[test]
    fn steppers_refuse_to_leave_their_option_list() {
        let mut cfg = Config::default();
        for _ in 0..10 {
            step(&mut cfg, Row::TabWidth, 1);
        }
        assert_eq!(cfg.tab_width, 8);
        for _ in 0..10 {
            step(&mut cfg, Row::TabWidth, -1);
        }
        assert_eq!(cfg.tab_width, 2);
        for _ in 0..10 {
            step(&mut cfg, Row::CornerRadius, -1);
        }
        assert_eq!(cfg.corner_radius, 0);
    }

    #[test]
    fn toggles_flip_both_ways() {
        let mut cfg = Config::default();
        assert!(step(&mut cfg, Row::LineNumbers, 1));
        assert!(!cfg.show_line_numbers);
        assert!(step(&mut cfg, Row::LineNumbers, 1));
        assert!(cfg.show_line_numbers);
    }

    #[test]
    fn stepping_at_a_boundary_reports_no_change() {
        let mut cfg = Config {
            font_size: 28,
            ..Config::default()
        };
        // Still inside the range after the clamp: no change means no save.
        assert!(!step(&mut cfg, Row::FontSize, 1));
        assert_eq!(cfg.font_size, 28);
    }

    #[test]
    fn choosing_a_segment_sets_that_value() {
        let mut cfg = Config::default();
        assert!(choose(&mut cfg, Row::TabWidth, 2));
        assert_eq!(cfg.tab_width, 8);
        assert!(choose(&mut cfg, Row::Theme, 1));
        assert_eq!(cfg.theme, Theme::Light);
        assert!(choose(&mut cfg, Row::Sidebar, 3));
        assert_eq!(cfg.sidebar, SidebarMode::Hidden);
        assert!(choose(&mut cfg, Row::CornerRadius, 0));
        assert_eq!(cfg.corner_radius, 0);
    }

    #[cfg(feature = "updater")]
    #[test]
    fn the_updater_rows_are_the_last_two_and_toggle_the_setting() {
        assert_eq!(Row::from_index(Row::COUNT - 2), Row::CheckUpdates);
        assert_eq!(Row::from_index(Row::COUNT - 1), Row::UpdateAction);
        // The toggle is a real setting: stepping it flips the config key.
        let mut cfg = Config::default();
        assert!(cfg.check_updates);
        assert!(step(&mut cfg, Row::CheckUpdates, 1));
        assert!(!cfg.check_updates);
        assert!(apply_part(&mut cfg, Row::CheckUpdates, Some(0)));
        assert!(cfg.check_updates);
        // The action row never changes the config; the app runs it instead.
        assert!(!step(&mut cfg, Row::UpdateAction, 1));
        assert!(!apply_part(&mut cfg, Row::UpdateAction, Some(0)));
        assert!(!choose(&mut cfg, Row::UpdateAction, 0));
    }

    #[cfg(feature = "updater")]
    #[test]
    fn the_action_button_says_what_the_state_allows() {
        let cfg = Config::default();
        let cases = [
            ("Check now", true),
            ("Download and install", true),
            ("Restart", true),
            ("Retry", true),
        ];
        for (action, enabled) in cases {
            let info = UpdateInfo {
                label: "Update available: v1.2.1".to_string(),
                action: action.to_string(),
                enabled,
            };
            match widget_for(Row::UpdateAction, &cfg, &info, None) {
                Widget::Action { label, enabled: e } => {
                    assert_eq!(label, action);
                    assert_eq!(e, enabled);
                }
                _ => panic!("expected an action row"),
            }
        }
        // A disabled action is not clickable at all: no piece to hit.
        let info = UpdateInfo {
            label: "Checking for updates…".to_string(),
            action: "Checking…".to_string(),
            enabled: false,
        };
        let geom = widgets::RowGeom { control_x: 0.0 };
        let spans = geom.spans(&widget_for(Row::UpdateAction, &cfg, &info, None));
        assert_eq!(spans.len(), 1, "one piece, so the cursor is a hand");
    }

    #[test]
    fn rows_are_in_spec_order_and_have_labels() {
        assert_eq!(Row::from_index(0), Row::FontSize);
        assert_eq!(Row::from_index(7), Row::Theme);
        assert_eq!(Row::ALL.len(), Row::COUNT);
        for row in Row::ALL {
            assert!(!row.label().is_empty());
        }
    }

    #[test]
    fn every_row_changes_something() {
        for row in Row::ALL {
            let mut cfg = Config::default();
            // Toggles and steppers change with a step; choices with a choice.
            let changed = match widget_for(row, &cfg, &UpdateInfo::default(), None) {
                Widget::Choice { .. } => choose(&mut cfg, row, 0) || choose(&mut cfg, row, 1),
                Widget::Action { .. } => continue, // not a value row
                _ => step(&mut cfg, row, 1) || step(&mut cfg, row, -1),
            };
            assert!(changed, "{row:?} did not change with any interaction");
        }
    }
}
