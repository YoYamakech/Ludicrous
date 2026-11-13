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
}

/// Geometry of one drawn row, kept so hit testing matches the drawing.
/// `x`/`y` inside the row do not matter here: every control row is the same
/// height, so the click handler already knows which row it is in.
pub struct RowGeom {
    /// Left edge of the control area (stepper minus button, choice segments).
    pub control_x: f32,
}

impl RowGeom {
    /// Which part of the control is at `x`? `None` when left of the control.
    /// For a choice row the result is the segment index; for a stepper it is
    /// 0 (minus), 1 (value), 2 (plus); for a toggle 0.
    pub fn part(&self, x: f32, widget: &Widget) -> Option<usize> {
        if x < self.control_x {
            return None;
        }
        let dx = x - self.control_x;
        match widget {
            Widget::Toggle(_) => Some(0),
            Widget::Stepper(_) => {
                // The stepper box is right-aligned in the control column.
                let dx = x - (self.control_x + CONTROL_W - STEP_W);
                if dx < 0.0 {
                    None
                } else if dx < STEP_BTN {
                    Some(0)
                } else if dx >= STEP_W - STEP_BTN {
                    Some(2)
                } else {
                    Some(1)
                }
            }
            Widget::Choice { options, .. } => {
                let seg = (CONTROL_W / options.len().max(1) as f32).max(1.0);
                Some(((dx / seg).floor() as usize).min(options.len().saturating_sub(1)))
            }
        }
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

/// The pill radius used behind focused rows (matches `theme::PILL_RADIUS`).
fn theme_pill() -> f32 {
    crate::ui::theme::PILL_RADIUS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepper_parts_are_minus_value_plus() {
        let g = RowGeom { control_x: 100.0 };
        let w = Widget::Stepper("15");
        let x0 = 100.0 + CONTROL_W - STEP_W; // stepper box left edge
        assert_eq!(g.part(x0 + 5.0, &w), Some(0), "minus");
        assert_eq!(g.part(x0 + STEP_W * 0.5, &w), Some(1), "value");
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
}
