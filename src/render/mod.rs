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
use crate::render::gl::Batch;

pub mod caps;
pub mod font;
pub mod gl;

pub type Color = [f32; 4]; // RGBA 0.0..=1.0, straight (not premultiplied)

const RECT_VERT: &str = include_str!("shaders/rect.vert");
const RECT_FRAG: &str = include_str!("shaders/rect.frag");
const TEXT_VERT: &str = include_str!("shaders/text.vert");
const TEXT_FRAG: &str = include_str!("shaders/text.frag");

// Generous upper bounds; the buffers are allocated once and never resized.
const MAX_RECTS: usize = 8192;
const MAX_GLYPHS: usize = 32768;

/// Paint order. Switching forces a flush, so `Background` always lands under
/// `Content`, and `Overlay` (tooltips, menus) lands on top of everything.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[allow(dead_code)] // Overlay is used once menus/tooltips land.
pub enum Layer {
    Background = 0,
    Content = 1,
    Overlay = 2,
}

pub struct Renderer {
    gl: glow::Context,
    // Read by the optional fast paths added in Phase F.
    #[allow(dead_code)]
    pub caps: GlCaps,
    rect_program: glow::Program,
    text_program: glow::Program,
    rects: Batch,
    glyphs: Batch,
    ui_glyphs: Batch,
    atlas: Atlas,
    ui_atlas: Atlas,
    // Logical size of the framebuffer, in points (not physical pixels).
    width: f32,
    height: f32,
    // HiDPI: physical pixels per point. Set by the app on resize/scale change.
    scale: f32,
    // Kept so the atlases can be re-rasterized when the scale factor changes.
    font_bytes: Vec<u8>,
    font_px: f32,
    ui_px: f32,
    // Clip stack in physical pixels; each entry is [x, y, w, h].
    clip: Vec<[f32; 4]>,
    layer: Layer,
    // Window shape mask uniforms, in physical pixels.
    window_w: f32,
    window_h: f32,
    window_radius: f32,
}

impl Renderer {
    /// `font_px` is the editor font size in points; the atlas is rasterized at
    /// `font_px * scale` so text stays sharp on HiDPI screens.
    pub fn new(gl: glow::Context, font_bytes: &[u8], font_px: f32) -> Self {
        let caps = GlCaps::detect(&gl);
        let rect_program = gl::program(&gl, RECT_VERT, RECT_FRAG);
        let text_program = gl::program(&gl, TEXT_VERT, TEXT_FRAG);

        // Instance layouts must match the shaders:
        // shape: rect(4) color(4) params(4: radius, border, -, -) clip(4)
        let rects = Batch::new(&gl, &[(1, 4), (2, 4), (3, 4), (4, 4)], MAX_RECTS);
        // glyph: rect(4) uv(4) color(4) clip(4)
        let glyphs = Batch::new(&gl, &[(1, 4), (2, 4), (3, 4), (4, 4)], MAX_GLYPHS);
        let ui_glyphs = Batch::new(&gl, &[(1, 4), (2, 4), (3, 4), (4, 4)], MAX_GLYPHS);

        let scale = 1.0;
        let ui_px = 13.0;
        let atlas = font::build(&gl, font_bytes, font_px * scale);
        let ui_atlas = font::build(&gl, font_bytes, ui_px * scale);

        unsafe {
            gl.enable(glow::BLEND);
            // Premultiplied alpha: our shaders output vec4(rgb * a, a), and
            // this blend function composites that correctly over transparency
            // (and over the desktop when the window is translucent).
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
        }

        Self {
            gl,
            caps,
            rect_program,
            text_program,
            rects,
            glyphs,
            ui_glyphs,
            atlas,
            ui_atlas,
            width: 1.0,
            height: 1.0,
            scale,
            font_bytes: font_bytes.to_vec(),
            font_px,
            ui_px,
            clip: Vec::new(),
            layer: Layer::Background,
            window_w: 1.0,
            window_h: 1.0,
            window_radius: 0.0,
        }
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
        }
        self.atlas = font::build(&self.gl, &self.font_bytes, self.font_px * scale);
        self.ui_atlas = font::build(&self.gl, &self.font_bytes, self.ui_px * scale);
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
        unsafe {
            self.gl.viewport(0, 0, w as i32, h as i32);
            // Transparent clear: the shell draws its own rounded background,
            // and the compositor shows the desktop outside that shape.
            self.gl.clear_color(0.0, 0.0, 0.0, 0.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        self.rects.data.clear();
        self.glyphs.data.clear();
        self.ui_glyphs.data.clear();
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

    /// Placeholder icon: until the embedded icon font lands (Phase D), draw a
    /// muted rounded square so layout work can proceed.
    #[allow(dead_code)] // shell chrome (Phase D)
    pub fn icon(&mut self, x: f32, y: f32, size: f32, color: Color) {
        self.rounded_rect(
            x + size * 0.2,
            y + size * 0.2,
            size * 0.6,
            size * 0.6,
            1.5,
            color,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn shape(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, border: f32, color: Color) {
        if w <= 0.0 || h <= 0.0 || color[3] <= 0.0 {
            return;
        }
        // A shape queued after text must paint over it. Flush *both* batches:
        // rects are only uploaded at flush time, so flushing the glyphs alone
        // would let an earlier background rect land on top of the text.
        if !self.glyphs.data.is_empty() || !self.ui_glyphs.data.is_empty() {
            self.flush_batches();
        }
        let s = self.scale;
        let clip = self.current_clip();
        self.rects.push(&[
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
            0.0,
            0.0,
            clip[0],
            clip[1],
            clip[2],
            clip[3],
        ]);
    }

    /// Queue a single line of editor text. `y` is the top of the line box.
    pub fn text(&mut self, x: f32, y: f32, s: &str, color: Color) {
        let clip = self.current_clip();
        queue_text(
            &self.atlas,
            &mut self.glyphs,
            self.scale,
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
        let clip = self.current_clip();
        queue_text(
            &self.ui_atlas,
            &mut self.ui_glyphs,
            self.scale,
            x,
            y,
            s,
            color,
            clip,
        );
    }

    /// X of the pen for character `col` of a line starting at `x`, in points.
    ///
    /// This repeats exactly what `queue_text` does (round the base, then add
    /// the unrounded advance, then round again), so a caret drawn here lands
    /// on the same pixel column as the glyph it sits in front of. Using
    /// `x + col * cell_w` directly drifts by a pixel or two because the
    /// advance is fractional (9.03 px for the bundled font).
    pub fn text_pen_x(&self, x: f32, col: usize) -> f32 {
        let s = self.scale;
        let base = (x * s).round();
        ((base + col as f32 * self.atlas.cell_w).round()) / s
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
        if self.glyphs.data.is_empty() && self.ui_glyphs.data.is_empty() {
            return;
        }
        unsafe {
            self.gl.use_program(Some(self.text_program));
            self.set_common_uniforms(self.text_program);
            self.gl.active_texture(glow::TEXTURE0);
            let loc = self.gl.get_uniform_location(self.text_program, "u_atlas");
            self.gl.uniform_1_i32(loc.as_ref(), 0);

            if !self.glyphs.data.is_empty() {
                self.gl
                    .bind_texture(glow::TEXTURE_2D, Some(self.atlas.texture));
                self.glyphs.draw(&self.gl);
            }
            if !self.ui_glyphs.data.is_empty() {
                self.gl
                    .bind_texture(glow::TEXTURE_2D, Some(self.ui_atlas.texture));
                self.ui_glyphs.draw(&self.gl);
            }
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }

    /// Shapes first, then glyphs: the normal paint order within a layer.
    fn flush_batches(&mut self) {
        if !self.rects.data.is_empty() {
            unsafe {
                self.gl.use_program(Some(self.rect_program));
                self.set_common_uniforms(self.rect_program);
                self.rects.draw(&self.gl);
            }
        }
        self.flush_text();
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
        self.clip.clear();
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
fn queue_text(
    atlas: &Atlas,
    batch: &mut Batch,
    scale: f32,
    x: f32,
    y: f32,
    s: &str,
    color: Color,
    clip: [f32; 4],
) {
    let cell_w = atlas.cell_w;
    let baseline = (y * scale + atlas.ascent).round();
    let mut pen = (x * scale).round();
    for ch in s.chars() {
        if let Some(g) = atlas.get(ch) {
            let gx = (pen + g.xoff).round();
            let gy = (baseline - g.yoff).round();
            batch.push(&[
                gx, gy, g.w, g.h, g.u0, g.v0, g.u1, g.v1, color[0], color[1], color[2], color[3],
                clip[0], clip[1], clip[2], clip[3],
            ]);
        }
        pen += cell_w;
    }
}
