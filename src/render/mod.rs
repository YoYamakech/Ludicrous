// render/mod.rs: the only drawing API the UI is allowed to use.
// It knows nothing about the editor or the filesystem.
//
// Everything is batched: `rect` and `text` only append instance data, and
// `flush` issues one instanced draw call for all rectangles and one for all
// glyphs. That keeps the CPU/GPU chatter to a minimum on weak GPUs.

use glow::HasContext;

use crate::render::font::Atlas;
use crate::render::gl::Batch;

pub mod font;
pub mod gl;

pub type Color = [f32; 4]; // RGBA 0.0..=1.0

const RECT_VERT: &str = include_str!("shaders/rect.vert");
const RECT_FRAG: &str = include_str!("shaders/rect.frag");
const TEXT_VERT: &str = include_str!("shaders/text.vert");
const TEXT_FRAG: &str = include_str!("shaders/text.frag");

// Generous upper bounds; the buffers are allocated once and never resized.
const MAX_RECTS: usize = 4096;
const MAX_GLYPHS: usize = 16384;

pub struct Renderer {
    gl: glow::Context,
    rect_program: glow::Program,
    text_program: glow::Program,
    rects: Batch,
    glyphs: Batch,
    atlas: Atlas,
    // Logical size of the framebuffer, in points (not physical pixels).
    width: f32,
    height: f32,
    // HiDPI: physical pixels per point. Set by the app on resize/scale change.
    scale: f32,
    // Kept so the atlas can be re-rasterized when the scale factor changes.
    font_bytes: Vec<u8>,
    font_px: f32,
}

impl Renderer {
    /// `font_px` is the font size in points; the atlas is rasterized at
    /// `font_px * scale` so text stays sharp on HiDPI screens.
    pub fn new(gl: glow::Context, font_bytes: &[u8], font_px: f32) -> Self {
        let rect_program = gl::program(&gl, RECT_VERT, RECT_FRAG);
        let text_program = gl::program(&gl, TEXT_VERT, TEXT_FRAG);

        // rect instance: x, y, w, h, r, g, b, a
        let rects = Batch::new(&gl, &[(1, 4), (2, 4)], MAX_RECTS);
        // glyph instance: x, y, w, h, u0, v0, u1, v1, r, g, b, a
        let glyphs = Batch::new(&gl, &[(1, 4), (2, 4), (3, 4)], MAX_GLYPHS);

        let scale = 1.0;
        let atlas = font::build(&gl, font_bytes, font_px * scale);

        unsafe {
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
        }

        Self {
            gl,
            rect_program,
            text_program,
            rects,
            glyphs,
            atlas,
            width: 1.0,
            height: 1.0,
            scale,
            font_bytes: font_bytes.to_vec(),
            font_px,
        }
    }

    /// Size of one character cell in points: (width, height).
    pub fn cell_size(&self) -> (f32, f32) {
        (
            self.atlas.cell_w / self.scale,
            self.atlas.cell_h / self.scale,
        )
    }

    /// Update the HiDPI factor. Re-rasterizes the atlas at the new density so
    /// glyphs stay sharp when the window moves to a different monitor.
    pub fn set_scale(&mut self, scale: f32) {
        if (scale - self.scale).abs() < 0.01 {
            return;
        }
        self.scale = scale;
        unsafe {
            self.gl.delete_texture(self.atlas.texture);
        }
        self.atlas = font::build(&self.gl, &self.font_bytes, self.font_px * scale);
    }

    /// Start a frame: set the viewport, clear, and reset the batches.
    /// `w`/`h` are physical pixels.
    pub fn begin_frame(&mut self, w: u32, h: u32) {
        self.width = w as f32;
        self.height = h as f32;
        unsafe {
            self.gl.viewport(0, 0, w as i32, h as i32);
            self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        self.rects.data.clear();
        self.glyphs.data.clear();
    }

    /// Queue a filled rectangle. Coordinates are in points, top-left origin.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let s = self.scale;
        self.rects.push(&[
            x * s,
            y * s,
            w * s,
            h * s,
            color[0],
            color[1],
            color[2],
            color[3],
        ]);
    }

    /// Queue a single line of text. `y` is the top of the line box, in points.
    pub fn text(&mut self, x: f32, y: f32, s: &str, color: Color) {
        let scale = self.scale;
        let cell_w = self.atlas.cell_w;
        let baseline = (y * scale + self.atlas.ascent).round();
        let mut pen = (x * scale).round();
        for ch in s.chars() {
            if let Some(g) = self.atlas.get(ch) {
                let gx = (pen + g.xoff).round();
                let gy = (baseline - g.yoff).round();
                self.glyphs.push(&[
                    gx, gy, g.w, g.h, g.u0, g.v0, g.u1, g.v1, color[0], color[1], color[2],
                    color[3],
                ]);
            }
            pen += cell_w;
        }
    }

    /// Draw everything queued since `begin_frame`, in two instanced calls.
    pub fn flush(&mut self) {
        unsafe {
            self.gl.use_program(Some(self.rect_program));
            let loc = self
                .gl
                .get_uniform_location(self.rect_program, "u_viewport");
            self.gl.uniform_2_f32(loc.as_ref(), self.width, self.height);
            self.rects.draw(&self.gl);

            self.gl.use_program(Some(self.text_program));
            let loc = self
                .gl
                .get_uniform_location(self.text_program, "u_viewport");
            self.gl.uniform_2_f32(loc.as_ref(), self.width, self.height);
            self.gl.active_texture(glow::TEXTURE0);
            self.gl
                .bind_texture(glow::TEXTURE_2D, Some(self.atlas.texture));
            let loc = self.gl.get_uniform_location(self.text_program, "u_atlas");
            self.gl.uniform_1_i32(loc.as_ref(), 0);
            self.glyphs.draw(&self.gl);
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }
    }
}
