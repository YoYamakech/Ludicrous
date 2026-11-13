// widgets.rs: the three control types the settings page is allowed to use.
//
// All of them are drawn with the normal renderer and return the geometry their
// hit region needs, so clicking and pressing keys always agree with what is on
// screen. Nothing here knows about config keys; the settings page maps rows to
// changes.

use crate::render::Renderer;
use crate::ui::theme::Palette;

/// Height of one settings row.
pub const ROW_H: f32 = 40.0;
/// Width of the control column, right-aligned in the panel.
pub const CONTROL_W: f32 = 240.0;
/// Stepper box width, and the width of its two buttons.
pub const STEP_W: f32 = 124.0;
/// Width of the minus/plus buttons inside the stepper.
pub const STEP_BTN: f32 = 24.0;

/// A row's control, in its "what is drawn" form.
pub enum Widget<'a> {
    /// On/off pill switch.
    Toggle(bool),
    /// `- value +` stepper.
    Stepper(&'a str),
    /// Segmented row: one labelled choice per option, `index` selected.
    Choice {
        options: &'a [&'a str],
        index: usize,
    },
    /// A button that does something instead of holding a value (v1.2 section
    /// 10: "add one more, an action row for buttons like `Check now`").
    /// `enabled` false draws it dimmed and makes the click do nothing.
    ///
    /// Only the updater's rows use it, so a `--no-default-features` build
    /// compiles it without constructing one.
    #[cfg_attr(not(feature = "updater"), allow(dead_code))]
    Action { label: &'a str, enabled: bool },
}

/// Geometry of one drawn row, kept so hit testing matches the drawing.
/// `x`/`y` inside the row do not matter here: every control row is the same
/// height, so the click handler already knows which row it is in.
pub struct RowGeom {
    /// Left edge of the control area (stepper minus button, choice segments).
    pub control_x: f32,
}

impl RowGeom {
    /// The clickable pieces of a control, as `(x, width, part)` in row
    /// coordinates. This is the single source of hit geometry: `part()` answers
    /// a point, `settings::register` builds the hit map, and both read this.
    ///
    /// The value box in the middle of a stepper is deliberately absent: it does
    /// nothing, so it must not show a hand cursor.
    pub fn spans(&self, widget: &Widget) -> Vec<(f32, f32, usize)> {
        match widget {
            Widget::Toggle(_) => vec![(self.control_x, CONTROL_W, 0)],
            Widget::Stepper(_) => {
                // The stepper box is right-aligned in the control column.
                let x = self.control_x + CONTROL_W - STEP_W;
                vec![(x, STEP_BTN, 0), (x + STEP_W - STEP_BTN, STEP_BTN, 2)]
            }
            Widget::Action { .. } => vec![(self.control_x, CONTROL_W, 0)],
            Widget::Choice { options, .. } => {
                let n = options.len().max(1);
                let seg = CONTROL_W / n as f32;
                (0..n)
                    .map(|k| (self.control_x + k as f32 * seg, seg, k))
                    .collect()
            }
        }
    }

    /// Which part of the control is at `x`? `None` when left of the control or
    /// Used by the tests to prove `spans` covers exactly what a point can hit.
    /// between the pieces of a stepper.
    /// For a choice row the result is the segment index; for a stepper it is
    /// 0 (minus) or 2 (plus); for a toggle 0.
    #[cfg(test)] // the app reads `spans`; this is the same maths, for tests
    pub fn part(&self, x: f32, widget: &Widget) -> Option<usize> {
        self.spans(widget)
            .into_iter()
            .find(|(sx, sw, _)| x >= *sx && x < *sx + *sw)
            .map(|(_, _, part)| part)
    }
}

/// Draw the label on the left and the control on the right of the row.
/// `x`/`w` span the row; `y` is its top edge.
#[allow(clippy::too_many_arguments)] // all seven are distinct drawing inputs
pub fn draw_row(
    r: &mut Renderer,
    p: &Palette,
    y: f32,
    x: f32,
    w: f32,
    label: &str,
    widget: &Widget,
    focused: bool,
) -> RowGeom {
    let h = ROW_H;
    // Focus ring: a subtle pill behind the whole row.
    if focused {
        r.rounded_rect(
            x - 6.0,
            y + 2.0,
            w + 12.0,
            h - 4.0,
            theme_pill(),
            p.pill_active,
        );
    }
    r.text_ui(x, y + (h - 13.0) * 0.5, label, p.text);

    let control_x = x + w - CONTROL_W;
    let geom = RowGeom { control_x };
    match widget {
        Widget::Toggle(on) => draw_toggle(r, p, control_x + CONTROL_W - 40.0, y, *on),
        Widget::Stepper(value) => draw_stepper(r, p, control_x + CONTROL_W - STEP_W, y, value),
        Widget::Choice { options, index } => {
            draw_choice(r, p, control_x, y, options, *index);
        }
        Widget::Action { label, enabled } => {
            draw_action(r, p, control_x, y, label, *enabled);
        }
    }
    geom
}

fn draw_toggle(r: &mut Renderer, p: &Palette, x: f32, y: f32, on: bool) {
    // A 40x20 pill with a sliding knob: the clearest on/off affordance.
    let (w, h) = (40.0, 20.0);
    let top = y + (ROW_H - h) * 0.5;
    let track = if on { p.accent } else { p.border };
    r.rounded_rect(x, top, w, h, h * 0.5, track);
    let knob = 14.0;
    let kx = if on { x + w - knob - 3.0 } else { x + 3.0 };
    r.rounded_rect(kx, top + 3.0, knob, knob, knob * 0.5, p.bg);
}

fn draw_stepper(r: &mut Renderer, p: &Palette, x: f32, y: f32, value: &str) {
    let h = 24.0;
    let top = y + (ROW_H - h) * 0.5;
    // Minus box, value, plus box. These offsets are mirrored by `part`.
    r.rounded_rect(x, top, STEP_BTN, h, 4.0, p.pill_active);
    r.rect(x + 7.0, top + h * 0.5 - 1.0, 10.0, 2.0, p.muted);
    let plus = x + STEP_W - STEP_BTN;
    r.text_ui(x + STEP_BTN + 4.0, y + (ROW_H - 13.0) * 0.5, value, p.text);
    r.rounded_rect(plus, top, STEP_BTN, h, 4.0, p.pill_active);
    r.rect(plus + 7.0, top + h * 0.5 - 1.0, 10.0, 2.0, p.muted);
    r.rect(plus + 11.0, top + h * 0.5 - 5.0, 2.0, 10.0, p.muted);
}

fn draw_choice(r: &mut Renderer, p: &Palette, x: f32, y: f32, options: &[&str], index: usize) {
    let h = 24.0;
    let top = y + (ROW_H - h) * 0.5;
    let seg = CONTROL_W / options.len().max(1) as f32;
    r.rounded_rect(x, top, CONTROL_W, h, 4.0, p.pill_active);
    for (i, option) in options.iter().enumerate() {
        let sx = x + i as f32 * seg;
        if i == index {
            r.rounded_rect(sx + 2.0, top + 2.0, seg - 4.0, h - 4.0, 3.0, p.border);
        }
        // Centre the label in its segment.
        let tw = r.ui_text_width(option);
        let tx = sx + (seg - tw) * 0.5;
        let color = if i == index { p.text } else { p.muted };
        r.text_ui(tx, y + (ROW_H - 13.0) * 0.5, option, color);
    }
}

fn draw_action(r: &mut Renderer, p: &Palette, x: f32, y: f32, label: &str, enabled: bool) {
    let h = 26.0;
    let top = y + (ROW_H - h) * 0.5;
    // Sized to the label plus padding, but never wider than the control column
    // and never narrower than a comfortable click target.
    let tw = r.ui_text_width(label);
    let w = (tw + 28.0).clamp(88.0, CONTROL_W);
    let bx = x + CONTROL_W - w;
    let (fill, text) = if enabled {
        (p.pill_active, p.text)
    } else {
        (p.pill_hover, p.muted)
    };
    r.rounded_rect(bx, top, w, h, 5.0, fill);
    r.text_ui(bx + (w - tw) * 0.5, y + (ROW_H - 13.0) * 0.5, label, text);
}

/// The pill radius used behind focused rows (matches `theme::PILL_RADIUS`).
fn theme_pill() -> f32 {
    crate::ui::theme::PILL_RADIUS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepper_parts_are_minus_and_plus() {
        let g = RowGeom { control_x: 100.0 };
        let w = Widget::Stepper("15");
        let x0 = 100.0 + CONTROL_W - STEP_W; // stepper box left edge
        assert_eq!(g.part(x0 + 5.0, &w), Some(0), "minus");
        // The value box does nothing, so it is not a clickable part and shows
        // the arrow rather than the hand (v1.2 section 3b: inert is Default).
        assert_eq!(g.part(x0 + STEP_W * 0.5, &w), None, "value box");
        assert_eq!(g.part(x0 + STEP_W - 5.0, &w), Some(2), "plus");
        assert_eq!(g.part(50.0, &w), None, "left of the control is the label");
    }

    #[test]
    fn choice_segments_map_to_option_indexes() {
        let g = RowGeom { control_x: 0.0 };
        let w = Widget::Choice {
            options: &["2", "4", "8"],
            index: 1,
        };
        assert_eq!(g.part(10.0, &w), Some(0));
        assert_eq!(g.part(CONTROL_W * 0.5, &w), Some(1));
        assert_eq!(g.part(CONTROL_W - 1.0, &w), Some(2));
    }

    #[test]
    fn spans_and_part_agree_for_every_control_type() {
        let g = RowGeom { control_x: 0.0 };
        let widgets = [
            Widget::Toggle(true),
            Widget::Stepper("15"),
            Widget::Choice {
                options: &["2", "4", "8"],
                index: 1,
            },
        ];
        for w in widgets {
            let spans = g.spans(&w);
            // Every part reachable by `part()` is inside exactly one span.
            let mut x = 0.0;
            while x < CONTROL_W {
                if let Some(part) = g.part(x, &w) {
                    let hits: Vec<_> = spans
                        .iter()
                        .filter(|(sx, sw, _)| x >= *sx && x < *sx + *sw)
                        .collect();
                    assert_eq!(hits.len(), 1, "x={x} part={part} spans={spans:?}");
                    assert_eq!(hits[0].2, part);
                }
                x += 0.5;
            }
        }
    }

    #[test]
    fn the_stepper_value_box_is_not_clickable() {
        let g = RowGeom { control_x: 0.0 };
        let w = Widget::Stepper("15");
        let box_x = CONTROL_W - STEP_W + STEP_BTN + 1.0;
        assert_eq!(g.part(box_x, &w), None);
    }
}
