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
    /// True for every codepoint the font actually has. A space rasterizes to
    /// a 0x0 bitmap, which must not be mistaken for "missing": that is what
    /// made every space fall back to `?` (issue #2).
    present: Vec<bool>,
    pub cell_w: f32,
    pub cell_h: f32,
    pub ascent: f32,
    pub texture: glow::Texture,
}

impl Atlas {
    /// The glyph for `c`, or `None` when the font does not have it. A glyph
    /// with a zero-size bitmap (space and other blank characters) is still
    /// returned: it draws nothing, but it is definitely not unknown.
    pub fn get(&self, c: char) -> Option<&Glyph> {
        glyph_for(&self.glyphs, &self.present, c)
    }

    /// True when `c` has a real bitmap worth drawing (not just an advance).
    pub fn has_bitmap(&self, g: &Glyph) -> bool {
        g.w > 0.0 && g.h > 0.0
    }

    /// Glyph to draw for a character the atlas does not have (non-ASCII).
    /// A `?` is better than a blank cell: the user can see that something is
    /// there, and nothing panics on an unknown character.
    pub fn fallback(&self) -> Option<&Glyph> {
        self.get('?')
    }
}

/// CPU side of the atlas: glyph tables plus the packed bitmap, before upload.
pub struct Raster {
    glyphs: Vec<Glyph>,
    present: Vec<bool>,
    pixels: Vec<u8>,
    size: u32,
    cell_w: f32,
    cell_h: f32,
    ascent: f32,
}

/// Look `c` up in a glyph table. Split out from `Atlas` so it can be tested
/// without a GL context.
fn glyph_for<'a>(glyphs: &'a [Glyph], present: &[bool], c: char) -> Option<&'a Glyph> {
    let i = c as usize;
    if present.get(i).copied().unwrap_or(false) {
        glyphs.get(i)
    } else {
        None
    }
}

/// Build the atlas for ASCII 32..=126 at `px` pixels.
pub fn build(gl: &glow::Context, font_bytes: &[u8], px: f32) -> Atlas {
    let r = rasterize(font_bytes, px);
    let Raster {
        glyphs,
        present,
        pixels,
        size,
        cell_w,
        cell_h,
        ascent,
    } = r;
    unsafe {
        let texture = gl.create_texture().expect("texture");
        gl.bind_texture(glow::TEXTURE_2D, Some(texture));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::R8 as i32,
            size as i32,
            size as i32,
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
            present,
            cell_w,
            cell_h,
            ascent,
            texture,
        }
    }
}

/// Rasterize ASCII 32..=126 into one packed R8 bitmap (shelf packing).
fn rasterize(font_bytes: &[u8], px: f32) -> Raster {
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

    const EDGE: u32 = 1024; // square atlas edge in pixels
    let mut pixels = vec![0u8; (EDGE * EDGE) as usize];
    let mut glyphs = vec![empty_glyph(); 128];
    let mut present = vec![false; 128];
    let (mut x, mut y, mut row_h) = (1u32, 1u32, 0u32);

    for code in 32u8..=126 {
        let ch = code as char;
        let (metrics, bitmap) = font.rasterize(ch, px);
        let (w, h) = (metrics.width as u32, metrics.height as u32);
        present[code as usize] = true;
        if w == 0 || h == 0 {
            // Space and friends: no bitmap, but the pen still advances.
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
        if x + w + 1 > EDGE {
            x = 1;
            y += row_h + 1;
            row_h = 0;
        }
        // Copy the glyph bitmap into the atlas, one row at a time.
        for row in 0..h {
            let src = (row * w) as usize;
            let dst = ((y + row) * EDGE + x) as usize;
            pixels[dst..dst + w as usize].copy_from_slice(&bitmap[src..src + w as usize]);
        }
        glyphs[code as usize] = Glyph {
            xoff: metrics.xmin as f32,
            yoff: (metrics.ymin + metrics.height as i32) as f32,
            w: w as f32,
            h: h as f32,
            u0: x as f32 / EDGE as f32,
            v0: y as f32 / EDGE as f32,
            u1: (x + w) as f32 / EDGE as f32,
            v1: (y + h) as f32 / EDGE as f32,
        };
        x += w + 1;
        row_h = row_h.max(h);
    }

    Raster {
        glyphs,
        present,
        pixels,
        size: EDGE,
        cell_w,
        cell_h: line_h.ceil(),
        ascent,
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

#[cfg(test)]
mod tests {
    use super::*;

    const FONT: &[u8] = include_bytes!("../../assets/font.ttf");

    fn lookup(c: char) -> Option<super::Glyph> {
        let r = rasterize(FONT, 15.0);
        glyph_for(&r.glyphs, &r.present, c).copied()
    }

    #[test]
    fn space_is_present_but_has_no_bitmap() {
        // Issue #2: a space has a 0x0 bitmap but must not be "unknown", or it
        // falls back to `?` and every space in the UI shows a question mark.
        let space = lookup(' ').expect("space is a real glyph");
        assert_eq!(space.w, 0.0);
        assert_eq!(space.h, 0.0);
    }

    #[test]
    fn question_mark_and_letters_have_bitmaps() {
        for c in ['?', 'M', 'a', '.'] {
            let g = lookup(c).expect("ASCII glyph");
            assert!(g.w > 0.0 && g.h > 0.0, "{c}");
        }
        // A codepoint outside ASCII is genuinely unknown and falls back.
        assert!(lookup('é').is_none());
    }
}
