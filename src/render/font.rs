// font.rs: rasterize one monospace font into an R8 texture atlas.
// ASCII is rasterized up front (simple shelf packing), so drawing text is just
// glyph lookups and one texture upload.

use fontdue::{Font, FontSettings};
use glow::HasContext;

#[derive(Clone, Copy)]
pub struct Glyph {
    /// Offset from the pen position (left edge of the cell) to the bitmap.
    pub xoff: f32,
    /// Offset up from the baseline to the top of the bitmap.
    pub yoff: f32,
    pub w: f32,
    pub h: f32,
    /// Atlas coordinates, 0..1, top-left to bottom-right.
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

pub struct Atlas {
    pub glyphs: Vec<Glyph>, // indexed by byte value; empty glyph for unsupported
    pub cell_w: f32,
    pub cell_h: f32,
    pub ascent: f32,
    pub texture: glow::Texture,
}

impl Atlas {
    pub fn get(&self, c: char) -> Option<&Glyph> {
        let i = c as usize;
        if i < self.glyphs.len() && self.glyphs[i].w > 0.0 {
            Some(&self.glyphs[i])
        } else {
            None
        }
    }
}

/// Build the atlas for ASCII 32..=126 at `px` pixels.
pub fn build(gl: &glow::Context, font_bytes: &[u8], px: f32) -> Atlas {
    let font = Font::from_bytes(font_bytes, FontSettings::default()).expect("failed to parse font");
    let ascent = font
        .horizontal_line_metrics(px)
        .map(|m| m.ascent)
        .unwrap_or(px * 0.8);
    let line_h = font
        .horizontal_line_metrics(px)
        .map(|m| m.new_line_size)
        .unwrap_or(px * 1.2);
    let cell_w = font.metrics('M', px).advance_width.ceil();

    const ATLAS: u32 = 1024;
    let mut pixels = vec![0u8; (ATLAS * ATLAS) as usize];
    let mut glyphs = vec![empty_glyph(); 128];
    let (mut x, mut y, mut row_h) = (1u32, 1u32, 0u32);

    for code in 32u8..=126 {
        let ch = code as char;
        let (metrics, bitmap) = font.rasterize(ch, px);
        let (w, h) = (metrics.width as u32, metrics.height as u32);
        if w == 0 || h == 0 {
            glyphs[code as usize] = Glyph {
                w: 0.0,
                h: 0.0,
                xoff: 0.0,
                yoff: 0.0,
                u0: 0.0,
                v0: 0.0,
                u1: 0.0,
                v1: 0.0,
            };
            continue;
        }
        if x + w + 1 > ATLAS {
            x = 1;
            y += row_h + 1;
            row_h = 0;
        }
        // Copy the glyph bitmap into the atlas, one row at a time.
        for row in 0..h {
            let src = (row * w) as usize;
            let dst = ((y + row) * ATLAS + x) as usize;
            pixels[dst..dst + w as usize].copy_from_slice(&bitmap[src..src + w as usize]);
        }
        glyphs[code as usize] = Glyph {
            xoff: metrics.xmin as f32,
            yoff: (metrics.ymin + metrics.height as i32) as f32,
            w: w as f32,
            h: h as f32,
            u0: x as f32 / ATLAS as f32,
            v0: y as f32 / ATLAS as f32,
            u1: (x + w) as f32 / ATLAS as f32,
            v1: (y + h) as f32 / ATLAS as f32,
        };
        x += w + 1;
        row_h = row_h.max(h);
    }

    // Upload as a single-channel texture; the shader treats it as alpha.
    unsafe {
        let texture = gl.create_texture().expect("texture");
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::R8 as i32,
            ATLAS as i32,
            ATLAS as i32,
            0,
            glow::RED,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&pixels)),
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.bind_texture(glow::TEXTURE_2D, None);

        Atlas {
            glyphs,
            cell_w,
            cell_h: line_h.ceil(),
            ascent,
            texture,
        }
    }
}

fn empty_glyph() -> Glyph {
    Glyph {
        xoff: 0.0,
        yoff: 0.0,
        w: 0.0,
        h: 0.0,
        u0: 0.0,
        v0: 0.0,
        u1: 0.0,
        v1: 0.0,
    }
}
