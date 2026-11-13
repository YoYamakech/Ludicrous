// render/mod.rs: the only drawing API the UI is allowed to use.
// It knows nothing about the editor or the filesystem.
//
// Everything is batched: shape and text calls only append instance data, and
// `flush` issues one instanced draw call for the shapes and one per atlas.
// Ordering is preserved by flushing glyphs first whenever a shape is queued
// after text (a shape must paint over earlier text, e.g. a panel).

use glow::HasContext;

use crate::render::caps::GlCaps;
use crate::render::font::Atlas;
use crate::render::gl::{Batch, Upload};
pub use crate::render::icons::Icon;
use crate::render::icons::IconAtlas;

pub mod caps;
pub mod font;
pub mod gl;
pub mod icons;

pub type Color = [f32; 4]; // RGBA 0.0..=1.0, straight (not premultiplied)

const RECT_VERT: &str = include_str!("shaders/rect.vert");
const RECT_FRAG: &str = include_str!("shaders/rect.frag");
const TEXT_VERT: &str = include_str!("shaders/text.vert");
const TEXT_FRAG: &str = include_str!("shaders/text.frag");

// Generous upper bounds; the buffers are allocated once and never resized.
const MAX_RECTS: usize = 8192;
const MAX_GLYPHS: usize = 32768;

/// Default tab stop width. The `tab_width` setting can change it at runtime
/// with `Renderer::set_tab_width`; the editor itself inserts spaces, so this
/// only affects tabs already in a file.
pub const TAB_COLUMNS: usize = 4;

/// Paint order. Switching forces a flush, so `Background` always lands under
/// `Content`, and `Overlay` (tooltips, menus) lands on top of everything.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[allow(dead_code)] // Overlay is used once menus/tooltips land.
pub enum Layer {
    Background = 0,
    Content = 1,
    Overlay = 2,
}

/// Per-frame counters for the F3 debug overlay.
#[derive(Clone, Copy, Default, Debug)]
pub struct Stats {
    /// `glDraw*` calls issued this frame.
    pub draw_calls: u32,
    /// Instances (rects + glyphs + icons) submitted this frame.
    pub instances: u32,
    /// How many times the batches had to be flushed (batching quality).
    pub flushes: u32,
}

pub struct Renderer {
    gl: glow::Context,
    pub caps: GlCaps,
    /// Driver strings, kept for the F3 overlay.
    gl_version: String,
    gl_renderer: String,
    rect_program: glow::Program,
    text_program: glow::Program,
    /// Shapes queued before any text: the usual background and panel fills.
    rects: Batch,
    /// Shapes queued *over* text (selection bands, carets, focused pills).
    /// They are drawn after the glyphs of the same flush, so covering a label
    /// no longer costs an extra pair of draw calls (issue #4).
    rects_over: Batch,
    glyphs: Batch,
    ui_glyphs: Batch,
    icon_glyphs: Batch,
    atlas: Atlas,
    ui_atlas: Atlas,
    icon_atlas: IconAtlas,
    // Logical size of the framebuffer, in points (not physical pixels).
    width: f32,
    height: f32,
    // HiDPI: physical pixels per point. Set by the app on resize/scale change.
    scale: f32,
    // Kept so the atlases can be re-rasterized when the scale factor changes.
    font_bytes: Vec<u8>,
    icon_bytes: Vec<u8>,
    /// Nerd Font Symbols subset, for file-type icons. Rasterized into the icon
    /// atlas, so file icons and chrome icons are both `Renderer::icon` quads.
    symbols_bytes: Vec<u8>,
    font_px: f32,
    ui_px: f32,
    /// Columns per tab stop (the `tab_width` setting).
    tab_columns: usize,
    // Clip stack in physical pixels; each entry is [x, y, w, h].
    clip: Vec<[f32; 4]>,
    /// Boxes of the shapes queued over text since the last flush, in physical
    /// pixels. Text queued afterwards must paint over them, so it flushes when
    /// it lands on one. Per shape, not one merged box: a merged box would cover
    /// the whole tree and make every later label look like a collision.
    over_boxes: Vec<[f32; 4]>,
    layer: Layer,
    // Window shape mask uniforms, in physical pixels.
    window_w: f32,
    window_h: f32,
    window_radius: f32,
    // Frame statistics for the debug overlay. Counting is a few increments per
    // flush, which is free next to a draw call; the overlay itself is only
    // drawn when it is open, so it really does cost nothing when closed.
    stats: Stats,
    /// GPU timer query in flight, if `ARB_timer_query` is available.
    timer: Option<glow::Query>,
    /// Timer state: 0 idle, 1 running, 2 ended and awaiting its result.
    timer_state: u8,
    /// Exact GPU frame time of the last completed frame, in milliseconds.
    gpu_ms: Option<f32>,
}

impl Renderer {
    /// `font_px` is the editor font size in points; the atlas is rasterized at
    /// `font_px * scale` so text stays sharp on HiDPI screens.
    pub fn new(
        gl: glow::Context,
        font_bytes: &[u8],
        icon_bytes: &[u8],
        symbols_bytes: &[u8],
        font_px: f32,
    ) -> Self {
        let caps = GlCaps::detect(&gl);
        let (gl_version, gl_renderer) = unsafe {
            (
                gl.get_parameter_string(glow::VERSION),
                gl.get_parameter_string(glow::RENDERER),
            )
        };
        let rect_program = gl::program(&gl, RECT_VERT, RECT_FRAG);
        let text_program = gl::program(&gl, TEXT_VERT, TEXT_FRAG);

        // Instance layouts must match the shaders:
        // shape: rect(4) color(4) params(4: radius, border, -, -) clip(4)
        let attrs: [(u32, i32); 4] = [(1, 4), (2, 4), (3, 4), (4, 4)];
        let rects = Batch::new(&gl, &caps, &attrs, MAX_RECTS);
        let rects_over = Batch::new(&gl, &caps, &attrs, MAX_RECTS);
        // glyph: rect(4) uv(4) color(4) clip(4)
        let glyphs = Batch::new(&gl, &caps, &attrs, MAX_GLYPHS);
        let ui_glyphs = Batch::new(&gl, &caps, &attrs, MAX_GLYPHS);
        let icon_glyphs = Batch::new(&gl, &caps, &attrs, 256);

        let scale = 1.0;
        let ui_px = 13.0;
        let atlas = font::build(&gl, font_bytes, font_px * scale);
        let ui_atlas = font::build(&gl, font_bytes, ui_px * scale);
        let icon_atlas = icons::build(&gl, icon_bytes, symbols_bytes, icons::ICON_PX * scale);

        unsafe {
            gl.enable(glow::BLEND);
            // Premultiplied alpha: our shaders output vec4(rgb * a, a), and
            // this blend function composites that correctly over transparency
            // (and over the desktop when the window is translucent).
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        }

        let timer = if caps.timer_query {
            unsafe { gl.create_query().ok() }
        } else {
            None
        };

        Self {
            gl,
            caps,
            gl_version,
            gl_renderer,
            rect_program,
            text_program,
            rects,
            rects_over,
            glyphs,
            ui_glyphs,
            icon_glyphs,
            atlas,
            ui_atlas,
            icon_atlas,
            width: 1.0,
            height: 1.0,
            scale,
            font_bytes: font_bytes.to_vec(),
            icon_bytes: icon_bytes.to_vec(),
            symbols_bytes: symbols_bytes.to_vec(),
            font_px,
            ui_px,
            tab_columns: TAB_COLUMNS,
            clip: Vec::new(),
            over_boxes: Vec::new(),
            layer: Layer::Background,
            window_w: 1.0,
            window_h: 1.0,
            window_radius: 0.0,
            stats: Stats::default(),
            timer,
            timer_state: 0,
            gpu_ms: None,
        }
    }

    /// Driver description for the debug overlay: version and renderer string.
    pub fn gl_strings(&self) -> (&str, &str) {
        (&self.gl_version, &self.gl_renderer)
    }

    /// Which instance-upload path the batches actually use.
    pub fn upload_path(&self) -> Upload {
        self.rects.upload()
    }

    /// Statistics for the last completed frame.
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Exact GPU time of the last completed frame, when timer queries work.
    pub fn gpu_ms(&self) -> Option<f32> {
        self.gpu_ms
    }

    /// Set the tab stop width in columns (2, 4 or 8 from the settings page).
    pub fn set_tab_width(&mut self, columns: usize) {
        self.tab_columns = columns.clamp(1, 32);
    }

    /// Size of one editor character cell in points: (width, height).
    pub fn cell_size(&self) -> (f32, f32) {
        (
            self.atlas.cell_w / self.scale,
            self.atlas.cell_h / self.scale,
        )
    }

    /// Size of one UI character cell in points: (width, height).
    #[allow(dead_code)] // shell chrome (Phase B+)
    pub fn ui_cell_size(&self) -> (f32, f32) {
        (
            self.ui_atlas.cell_w / self.scale,
            self.ui_atlas.cell_h / self.scale,
        )
    }

    /// Change the editor/UI font size live (the settings stepper). Rebuilds the
    /// text atlas at the new size; the icon atlas does not depend on it.
    pub fn set_font_size(&mut self, px: f32) {
        if (px - self.font_px).abs() < 0.01 {
            return;
        }
        self.font_px = px;
        // The UI atlas follows the editor size at a fixed offset, the way the
        // spec's "smaller UI size" reads.
        self.ui_px = (px - 2.0).max(9.0);
        unsafe {
            self.gl.delete_texture(self.atlas.texture);
            self.gl.delete_texture(self.ui_atlas.texture);
        }
        let scale = self.scale;
        self.atlas = font::build(&self.gl, &self.font_bytes, self.font_px * scale);
        self.ui_atlas = font::build(&self.gl, &self.font_bytes, self.ui_px * scale);
    }

    /// Update the HiDPI factor. Re-rasterizes the atlases at the new density so
    /// glyphs stay sharp when the window moves to a different monitor.
    pub fn set_scale(&mut self, scale: f32) {
        if (scale - self.scale).abs() < 0.01 {
            return;
        }
        self.scale = scale;
        unsafe {
            self.gl.delete_texture(self.atlas.texture);
            self.gl.delete_texture(self.ui_atlas.texture);
            self.gl.delete_texture(self.icon_atlas.texture);
        }
        self.atlas = font::build(&self.gl, &self.font_bytes, self.font_px * scale);
        self.ui_atlas = font::build(&self.gl, &self.font_bytes, self.ui_px * scale);
        self.icon_atlas = icons::build(
            &self.gl,
            &self.icon_bytes,
            &self.symbols_bytes,
            icons::ICON_PX * scale,
        );
    }

    /// Start a frame: set the viewport, clear, and reset the batches.
    /// `w`/`h` are physical pixels.
    pub fn begin_frame(&mut self, w: u32, h: u32) {
        self.width = w as f32;
        self.height = h as f32;
        self.clip.clear();
        self.layer = Layer::Background;
        if self.window_w <= 1.0 || self.window_h <= 1.0 {
            self.window_w = self.width;
            self.window_h = self.height;
        }
        self.stats = Stats::default();
        unsafe {
            // One query at a time: collect the previous result when it has
            // arrived, then start timing this frame.
            if let Some(q) = self.timer {
                if self.timer_state == 2
                    && self
                        .gl
                        .get_query_parameter_u32(q, glow::QUERY_RESULT_AVAILABLE)
                        != 0
                {
                    let ns = self.gl.get_query_parameter_u64(q, glow::QUERY_RESULT);
                    // A query that was never really timed can report garbage
                    // (older drivers do this for the first frame). Anything
                    // over a second is not a frame, so keep the previous value.
                    if ns <= 1_000_000_000 {
                        self.gpu_ms = Some(ns as f32 / 1_000_000.0);
                    }
                    self.timer_state = 0;
                }
                if self.timer_state == 0 {
                    self.gl.begin_query(glow::TIME_ELAPSED, q);
                    self.timer_state = 1;
                }
            }
            self.gl.viewport(0, 0, w as i32, h as i32);
            // Transparent clear: the shell draws its own rounded background,
            // and the compositor shows the desktop outside that shape.
            self.gl.clear_color(0.0, 0.0, 0.0, 0.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        self.rects.data.clear();
        self.rects_over.data.clear();
        self.glyphs.data.clear();
        self.ui_glyphs.data.clear();
        self.icon_glyphs.data.clear();
        self.over_boxes.clear();
    }

    /// Set the rounded window mask, in points. Every fragment outside this
    /// shape is discarded, which is what makes the corners transparent.
    #[allow(dead_code)] // shell chrome (Phase B+)
    pub fn set_window_shape(&mut self, w: f32, h: f32, radius: f32) {
        let s = self.scale;
        self.window_w = w * s;
        self.window_h = h * s;
        self.window_radius = radius * s;
    }

    /// Set the drawing layer; flushes when it changes so paint order is exact.
    #[allow(dead_code)] // shell chrome (Phase B+)
    pub fn layer(&mut self, layer: Layer) {
        if layer != self.layer {
            self.flush_batches();
            self.layer = layer;
        }
    }

    /// Clip all following draws to `x, y, w, h` (points), intersected with the
    /// current clip. Per-instance clipping nests naturally.
    #[allow(dead_code)] // shell chrome (Phase B+)
    pub fn push_clip(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let s = self.scale;
        let r = [x * s, y * s, w * s, h * s];
        let next = intersect(self.current_clip(), r);
        self.clip.push(next);
    }

    #[allow(dead_code)] // shell chrome (Phase B+)
    pub fn pop_clip(&mut self) {
        self.clip.pop();
    }

    fn current_clip(&self) -> [f32; 4] {
        self.clip
            .last()
            .copied()
            .unwrap_or([0.0, 0.0, self.width, self.height])
    }

    /// Queue a filled rectangle. Coordinates are in points, top-left origin.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        self.shape(x, y, w, h, 0.0, 0.0, color);
    }

    /// A wavy underline across `w` points at `y`, `h` points tall (v1.2
    /// section 7.3). Same batch, same program, one instance: the shader's
    /// `params.z` switches to the wave, so a squiggle costs exactly what a rect
    /// costs and never adds a draw call.
    pub fn squiggle(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let s = self.scale;
        if w * s < 1.0 || h * s < 2.0 || color[3] <= 0.0 {
            return;
        }
        // Drawn over text, so it belongs in the over-text batch when it lands
        // on pending glyphs (and only then: an empty line's squiggle does not
        // need a flush).
        let over = self.covers_pending_text(x, y, w, h);
        let clip = self.current_clip();
        if over {
            self.over_boxes.push([x * s, y * s, w * s, h * s]);
        }
        let batch = if over {
            &mut self.rects_over
        } else {
            &mut self.rects
        };
        batch.push(&[
            x * s,
            y * s,
            w * s,
            h * s,
            color[0],
            color[1],
            color[2],
            color[3],
            0.0,
            0.0,
            1.0, // style: squiggle
            0.0,
            clip[0],
            clip[1],
            clip[2],
            clip[3],
        ]);
    }

    /// Filled rectangle with rounded corners (radius in points).
    #[allow(dead_code)] // shell chrome (Phase B+)
    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color) {
        self.shape(x, y, w, h, radius, 0.0, color);
    }

    /// Rounded border of `width` points, drawn inside the shape.
    #[allow(dead_code)] // shell chrome (Phase B+)
    #[allow(clippy::too_many_arguments)] // geometry + style, all explicit
    pub fn rounded_outline(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        width: f32,
        color: Color,
    ) {
        self.shape(x, y, w, h, radius, width, color);
    }

    /// Axis-aligned line. The shell only needs separators and rules, so this
    /// is a thin rectangle rather than a rotated quad.
    #[allow(dead_code)] // shell chrome (Phase B+)
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, color: Color) {
        if (x1 - x0).abs() >= (y1 - y0).abs() {
            let x = x0.min(x1);
            self.rect(x, y0 - width * 0.5, (x1 - x0).abs(), width, color);
        } else {
            let y = y0.min(y1);
            self.rect(x0 - width * 0.5, y, width, (y1 - y0).abs(), color);
        }
    }

    /// Queue an icon centered in a `size` square whose top-left is `x, y`.
    /// Icons are tinted by `color`, so one atlas serves every state.
    #[allow(dead_code)] // shell chrome (Phase D)
    pub fn icon(&mut self, x: f32, y: f32, icon: Icon, size: f32, color: Color) {
        self.icon_char(x, y, icon.ch(), size, color);
    }

    /// A file-type symbol from the Nerd Font subset, drawn exactly like a chrome
    /// icon because both live in the icon atlas.
    pub fn file_icon(&mut self, x: f32, y: f32, glyph: char, size: f32, color: Color) {
        self.icon_char(x, y, glyph, size, color);
    }

    /// Would a shape at this box really cover queued text?
    ///
    /// A shape must paint over text that was queued before it, and the two
    /// batches are drawn rects-then-text on every flush. So a flush is only
    /// needed when the new shape actually intersects the queued glyphs.
    /// Without this test every label border or row hairline split the batch,
    /// which is where the 21+ flushes per frame came from (issue #4).
    fn covers_pending_text(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let s = self.scale;
        // A hairline or a rule is decoration, not content: it may sit in the
        // gap between glyphs and never needs a flush (issue #4).
        if h * s < 2.0 || w * s < 2.0 {
            return false;
        }
        let box_ = [x * s, y * s, w * s, h * s];
        [&self.glyphs, &self.ui_glyphs, &self.icon_glyphs]
            .iter()
            .filter_map(|b| b.bounds())
            .any(|b| intersects(b, box_))
    }

    /// Text must paint over the shapes queued before it, and those shapes are
    /// drawn first in a flush. If the over-text batch exists and the new text
    /// would land on it, flush so the order stays honest; otherwise the glyph
    /// simply joins the batch (issue #4).
    fn flush_before_text(&mut self, x: f32, y: f32, text: &str, ui_sized: bool) {
        if self.over_boxes.is_empty() {
            return;
        }
        let s = self.scale;
        let atlas = if ui_sized {
            &self.ui_atlas
        } else {
            &self.atlas
        };
        let w = text.chars().count() as f32 * atlas.cell_w;
        let box_ = [x * s, y * s, w, atlas.cell_h];
        if self.over_boxes.iter().any(|b| intersects(*b, box_)) {
            self.flush_batches();
        }
    }

    fn icon_char(&mut self, x: f32, y: f32, glyph: char, size: f32, color: Color) {
        let Some(g) = self.icon_atlas.get_char(glyph).copied() else {
            // A missing glyph draws nothing rather than a wrong box: a file icon
            // is decoration, not content (v1.2 section 4).
            return;
        };
        // Icons and text live in separate batches, and all three glyph
        // batches are drawn in the same flush after every shape. So an icon is
        // never hidden by text queued before it, and it needs no flush at all:
        // the old rule flushed for every icon that followed a label, which is
        // where most of the remaining draw calls came from (issue #4).
        let s = self.scale;
        let clip = self.current_clip();
        // The bitmap was rasterized at ICON_PX points, so scale it to `size`.
        let k = size / icons::ICON_PX;
        let dx = (x + (size - g.w * k) * 0.5) * s;
        let dy = (y + (size - g.h * k) * 0.5) * s;
        self.icon_glyphs.push(&[
            dx.round(),
            dy.round(),
            (g.w * k * s).round(),
            (g.h * k * s).round(),
            g.u0,
            g.v0,
            g.u1,
            g.v1,
            color[0],
            color[1],
            color[2],
            color[3],
            clip[0],
            clip[1],
            clip[2],
            clip[3],
        ]);
    }

    #[allow(clippy::too_many_arguments)]
    fn shape(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, border: f32, color: Color) {
        if w <= 0.0 || h <= 0.0 || color[3] <= 0.0 {
            return;
        }
        // A shape queued after text must paint over it. When it really does
        // overlap, it goes into the "over text" batch, which is drawn after the
        // glyphs of the same flush: one pair of draw calls for the whole layer
        // instead of one per covering shape (issue #4).
        //
        // The exception is a *wash* — a full alpha fill used as a highlight
        // behind a label. Highlights are drawn before their own text, so a
        // hover pill must stay in the background batch even though the pointer
        // is by definition over a label. Without this the settings page split
        // once per hovered row.
        let over = if color[3] >= 1.0 && is_wash_shaped(w, h) {
            false
        } else {
            self.covers_pending_text(x, y, w, h)
        };
        let s = self.scale;
        let clip = self.current_clip();
        if over {
            // Remember each covering shape separately, so later text only
            // flushes when it really lands on one of them.
            self.over_boxes.push([x * s, y * s, w * s, h * s]);
        }
        let batch = if over {
            &mut self.rects_over
        } else {
            &mut self.rects
        };
        batch.push(&[
            x * s,
            y * s,
            w * s,
            h * s,
            color[0],
            color[1],
            color[2],
            color[3],
            radius * s,
            border * s,
            0.0, // style: plain fill or border
            0.0,
            clip[0],
            clip[1],
            clip[2],
            clip[3],
        ]);
    }

    /// Queue a single line of editor text. `y` is the top of the line box.
    pub fn text(&mut self, x: f32, y: f32, s: &str, color: Color) {
        self.flush_before_text(x, y, s, false);
        let clip = self.current_clip();
        queue_text(
            &self.atlas,
            &mut self.glyphs,
            self.scale,
            self.tab_columns,
            x,
            y,
            s,
            color,
            clip,
        );
    }

    /// Queue a single line of UI (chrome) text at the fixed UI size.
    #[allow(dead_code)] // shell chrome (Phase B+)
    pub fn text_ui(&mut self, x: f32, y: f32, s: &str, color: Color) {
        self.flush_before_text(x, y, s, true);
        let clip = self.current_clip();
        queue_text(
            &self.ui_atlas,
            &mut self.ui_glyphs,
            self.scale,
            self.tab_columns,
            x,
            y,
            s,
            color,
            clip,
        );
    }

    /// X of the pen for character `col` of `line` starting at `x`, in points.
    ///
    /// This repeats exactly what `queue_text` does (round the base, then add
    /// the unrounded advance, then round again), so a caret drawn here lands
    /// on the same pixel column as the glyph it sits in front of. Using
    /// `x + col * cell_w` directly drifts by a pixel or two because the
    /// advance is fractional (9.03 px for the bundled font). Tabs are walked
    /// too, so the caret stays right on lines that contain them.
    pub fn text_pen_x(&self, x: f32, line: &str, col: usize) -> f32 {
        let s = self.scale;
        let cell_w = self.atlas.cell_w;
        let mut pen = (x * s).round();
        let mut c = 0usize;
        for ch in line.chars().take(col) {
            if ch == '\t' {
                let next = (c / self.tab_columns + 1) * self.tab_columns;
                pen += (next - c) as f32 * cell_w;
                c = next;
            } else {
                pen += cell_w;
                c += 1;
            }
        }
        pen.round() / s
    }

    /// Width of `s` in points at the editor size.
    #[allow(dead_code)] // shell chrome (Phase B+)
    pub fn text_width(&self, s: &str) -> f32 {
        s.chars().count() as f32 * self.atlas.cell_w / self.scale
    }

    /// Width of `s` in points at the UI size.
    #[allow(dead_code)] // shell chrome (Phase B+)
    pub fn ui_text_width(&self, s: &str) -> f32 {
        s.chars().count() as f32 * self.ui_atlas.cell_w / self.scale
    }

    fn flush_text(&mut self) {
        if self.glyphs.data.is_empty()
            && self.ui_glyphs.data.is_empty()
            && self.icon_glyphs.data.is_empty()
        {
            return;
        }
        self.stats.flushes += 1;
        unsafe {
            self.gl.use_program(Some(self.text_program));
            self.set_common_uniforms(self.text_program);
            self.gl.active_texture(glow::TEXTURE0);
            let loc = self.gl.get_uniform_location(self.text_program, "u_atlas");
            self.gl.uniform_1_i32(loc.as_ref(), 0);

            if !self.glyphs.data.is_empty() {
                self.gl
                    .bind_texture(glow::TEXTURE_2D, Some(self.atlas.texture));
                self.stats.instances += self.glyphs.len() as u32;
                self.stats.draw_calls += self.glyphs.draw(&self.gl);
            }
            if !self.ui_glyphs.data.is_empty() {
                self.gl
                    .bind_texture(glow::TEXTURE_2D, Some(self.ui_atlas.texture));
                self.stats.instances += self.ui_glyphs.len() as u32;
                self.stats.draw_calls += self.ui_glyphs.draw(&self.gl);
            }
            if !self.icon_glyphs.data.is_empty() {
                self.gl
                    .bind_texture(glow::TEXTURE_2D, Some(self.icon_atlas.texture));
                self.stats.instances += self.icon_glyphs.len() as u32;
                self.stats.draw_calls += self.icon_glyphs.draw(&self.gl);
            }
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }

    /// Shapes first, then glyphs, then the shapes that belong over text: the
    /// normal paint order within a layer, in one flush.
    fn flush_batches(&mut self) {
        // `LUDICROUS_DEBUG_FLUSH=1`: one line per flush, saying which batches
        // were non-empty. A draw-call regression is much easier to trace with
        // this than with the totals alone.
        if std::env::var_os("LUDICROUS_DEBUG_FLUSH").is_some() {
            eprintln!(
                "flush: rects={} glyphs={} ui={} icons={} over={}",
                self.rects.len(),
                self.glyphs.len(),
                self.ui_glyphs.len(),
                self.icon_glyphs.len(),
                self.rects_over.len()
            );
        }
        if !self.rects.data.is_empty() {
            self.stats.flushes += 1;
            self.stats.instances += self.rects.len() as u32;
            unsafe {
                self.gl.use_program(Some(self.rect_program));
                self.set_common_uniforms(self.rect_program);
                self.stats.draw_calls += self.rects.draw(&self.gl);
            }
        }
        self.flush_text();
        if !self.rects_over.data.is_empty() {
            self.stats.flushes += 1;
            self.stats.instances += self.rects_over.len() as u32;
            unsafe {
                self.gl.use_program(Some(self.rect_program));
                self.set_common_uniforms(self.rect_program);
                self.stats.draw_calls += self.rects_over.draw(&self.gl);
            }
        }
        // Everything queued so far is on the screen after this flush, so no
        // box is "pending" any more.
        self.over_boxes.clear();
    }

    /// Upload the viewport, window mask and window-size uniforms for `p`.
    unsafe fn set_common_uniforms(&self, p: glow::Program) {
        unsafe {
            let loc = self.gl.get_uniform_location(p, "u_viewport");
            self.gl.uniform_2_f32(loc.as_ref(), self.width, self.height);
            let loc = self.gl.get_uniform_location(p, "u_window_size");
            self.gl
                .uniform_2_f32(loc.as_ref(), self.window_w, self.window_h);
            let loc = self.gl.get_uniform_location(p, "u_radius");
            self.gl.uniform_1_f32(loc.as_ref(), self.window_radius);
        }
    }

    /// Draw everything queued since `begin_frame`.
    pub fn flush(&mut self) {
        self.flush_batches();
        if std::env::var_os("LUDICROUS_DEBUG_FLUSH").is_some() {
            eprintln!(
                "frame: draws={} flushes={} instances={}",
                self.stats.draw_calls, self.stats.flushes, self.stats.instances
            );
        }
        self.clip.clear();
        // Close the timer query for this frame and pick up the result of an
        // older one. The result is only read when it is already available, so
        // this never stalls the CPU waiting for the GPU.
        unsafe {
            if self.timer_state == 1 {
                self.gl.end_query(glow::TIME_ELAPSED);
                self.timer_state = 2;
            }
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            if let Some(q) = self.timer.take() {
                self.gl.delete_query(q);
            }
        }
        self.rects.destroy(&self.gl);
        self.rects_over.destroy(&self.gl);
        self.glyphs.destroy(&self.gl);
        self.ui_glyphs.destroy(&self.gl);
        self.icon_glyphs.destroy(&self.gl);
    }
}

#[allow(dead_code)] // used by push_clip once the shell clips panels
fn intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

#[allow(clippy::too_many_arguments)]
/// Is this the shape of a row highlight: wide, about one row tall, and not
/// spanning most of the window width?
fn is_wash_shaped(w: f32, h: f32) -> bool {
    (12.0..=40.0).contains(&h) && w >= 40.0
}

/// Do two `[x, y, w, h]` boxes share any pixels?
fn intersects(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

// Nine inputs, each a distinct drawing parameter; a struct here would be
// noise at every call site.
#[allow(clippy::too_many_arguments)]
fn queue_text(
    atlas: &Atlas,
    batch: &mut Batch,
    scale: f32,
    tab_columns: usize,
    x: f32,
    y: f32,
    s: &str,
    color: Color,
    clip: [f32; 4],
) {
    let cell_w = atlas.cell_w;
    let baseline = (y * scale + atlas.ascent).round();
    let mut pen = (x * scale).round();
    let mut col = 0usize;
    for ch in s.chars() {
        // A tab advances to the next tab stop instead of drawing a glyph.
        if ch == '\t' {
            let next = (col / tab_columns + 1) * tab_columns;
            pen += (next - col) as f32 * cell_w;
            col = next;
            continue;
        }
        // Unknown characters (anything outside ASCII) draw as `?` rather than
        // vanishing, and never panic.
        let g = atlas.get(ch).or_else(|| atlas.fallback());
        // Blank glyphs (space, tab stops above) have no bitmap: advance only.
        // Emitting a quad for them would be a zero-size instance at best, and
        // treating them as unknown is what caused issue #2.
        if let Some(g) = g.filter(|g| atlas.has_bitmap(g)) {
            let gx = (pen + g.xoff).round();
            let gy = (baseline - g.yoff).round();
            batch.push(&[
                gx, gy, g.w, g.h, g.u0, g.v0, g.u1, g.v1, color[0], color[1], color[2], color[3],
                clip[0], clip[1], clip[2], clip[3],
            ]);
        }
        pen += cell_w;
        col += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_that_only_touch_do_not_intersect() {
        // Sharing an edge is not covering: a row hairline directly above a
        // label must not count as painting over it (issue #4).
        let a = [0.0, 0.0, 10.0, 10.0];
        assert!(intersects(a, [5.0, 5.0, 10.0, 10.0]), "real overlap");
        assert!(!intersects(a, [10.0, 0.0, 10.0, 10.0]), "touching x");
        assert!(!intersects(a, [0.0, 10.0, 10.0, 10.0]), "touching y");
        assert!(!intersects(a, [20.0, 20.0, 5.0, 5.0]), "far away");
    }

    #[test]
    fn a_wash_is_recognised_by_its_shape() {
        // Row highlights are wide and about one row tall, with full alpha.
        assert!(is_wash_shaped(240.0, 28.0));
        assert!(is_wash_shaped(120.0, 24.0));
        // A hairline is not a wash, a big panel is not a wash, and neither is
        // a tall narrow strip.
        assert!(!is_wash_shaped(600.0, 1.0));
        assert!(!is_wash_shaped(600.0, 300.0));
        assert!(!is_wash_shaped(10.0, 20.0));
    }
}
