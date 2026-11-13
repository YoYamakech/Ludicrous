// overlay.rs: the F3 debug panel.
//
// Performance work must be measured, not guessed, so the editor can show the
// numbers it is actually achieving. The panel is drawn with the normal
// renderer in a corner of the editing area and lists:
//
//   * frame time — GPU time when `ARB_timer_query` works, CPU time otherwise
//   * draw calls, instances and batch flushes for the last frame
//   * the instance-upload path in use (ring + base_instance, or orphaning)
//   * which optional GL paths are active
//   * the driver's GL version and renderer strings
//
// Nothing here is called unless the panel is open, so a closed overlay costs
// exactly zero: no counters are read, no strings are formatted, no quads are
// queued.

use crate::render::{Color, Renderer};
use crate::ui::theme::Palette;

/// Everything the panel shows. Built by the app once per frame while open.
pub struct Info<'a> {
    /// Wall-clock time of the previous frame (input to redraw), in ms.
    pub cpu_ms: f32,
    /// Exact GPU time of the last completed frame, when timer queries work.
    pub gpu_ms: Option<f32>,
    pub draw_calls: u32,
    pub instances: u32,
    pub flushes: u32,
    /// Human-readable instance-upload path.
    pub upload: &'a str,
    /// One letter per optional path: B storage, I base instance, T texture
    /// storage, S sRGB, D DSA, V invalidate, F flush control, Q timer query,
    /// and `SAFE` when LUDICROUS_GL_SAFE=1 forced the fallbacks.
    pub flags: &'a str,
    pub gl_version: &'a str,
    pub gl_renderer: &'a str,
}

const LINE_H: f32 = 15.0;
const PAD: f32 = 8.0;
const WIDTH: f32 = 330.0;

/// Draw the panel in the top-right of the text area. `x`/`y` are the panel's
/// top-left corner in points; the size is fixed so the text never clips.
pub fn draw(r: &mut Renderer, p: &Palette, x: f32, y: f32, info: &Info) {
    let lines = 7;
    let h = PAD * 2.0 + lines as f32 * LINE_H;
    // A slightly darker plate so the numbers stay readable over text.
    r.rounded_rect(x, y, WIDTH, h, 6.0, p.sidebar_bg);
    r.rounded_outline(x, y, WIDTH, h, 6.0, 1.0, p.border);

    let mut ty = y + PAD;
    let mut line = |r: &mut Renderer, text: &str, color: Color| {
        r.text_ui(x + PAD, ty, text, color);
        ty += LINE_H;
    };

    let frame = match info.gpu_ms {
        Some(ms) => format!("frame {:.2} ms gpu ({:.0} fps)", ms, 1000.0 / ms.max(0.01)),
        None => format!(
            "frame {:.2} ms cpu ({:.0} fps)",
            info.cpu_ms,
            1000.0 / info.cpu_ms.max(0.01)
        ),
    };
    line(r, &frame, p.text);
    line(
        r,
        &format!("draws {}  instances {}", info.draw_calls, info.instances),
        p.muted,
    );
    line(r, &format!("batch flushes {}", info.flushes), p.muted);
    line(r, &format!("upload {}", info.upload), p.muted);
    line(r, &format!("paths {}", info.flags), p.muted);
    line(r, info.gl_version, p.muted);
    line(r, info.gl_renderer, p.muted);
}
