// icons.rs: a second, tiny atlas for the shell's icons.
//
// The icons come from Lucide (ISC licence, see assets/ICONS_LICENSE.txt),
// subset to the glyphs below. They are rasterized once into the same kind of
// R8 texture as the text atlas, so `Renderer::icon` is just another quad.

use fontdue::{Font, FontSettings};
use glow::HasContext;

/// Icons the shell draws. The names match Lucide's icon names (kebab-cased).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Folder,
    FolderOpen,
    File,
    ChevronRight,
    ChevronDown,
    Gear,
    Sidebar,
    Minimize,
    Maximize,
    Restore,
    Close,
    Dot,
    Files,
}

impl Icon {
    /// Lucide codepoint for each icon. Kept next to the enum so adding an icon
    /// is one line here plus one name in the atlas builder.
    pub fn ch(self) -> char {
        let code = match self {
            Icon::Folder => 0xE0D7,
            Icon::FolderOpen => 0xE247,
            Icon::File => 0xE0C0,
            Icon::ChevronRight => 0xE06F,
            Icon::ChevronDown => 0xE06D,
            Icon::Gear => 0xE154,
            Icon::Sidebar => 0xE12A,
            Icon::Minimize => 0xE11C,
            Icon::Maximize => 0xE167,
            Icon::Restore => 0xE09E,
            Icon::Close => 0xE1B2,
            Icon::Dot => 0xE076,
            Icon::Files => 0xE0CF,
        };
        char::from_u32(code).unwrap_or(' ')
    }

    pub const ALL: [Icon; 13] = [
        Icon::Folder,
        Icon::FolderOpen,
        Icon::File,
        Icon::ChevronRight,
        Icon::ChevronDown,
        Icon::Gear,
        Icon::Sidebar,
        Icon::Minimize,
        Icon::Maximize,
        Icon::Restore,
        Icon::Close,
        Icon::Dot,
        Icon::Files,
    ];
}

/// Icon bitmaps are rasterized at this size in points, then scaled to the
/// requested draw size. One size keeps the atlas small and is sharp enough at
/// the 12-16 px the shell asks for.
pub const ICON_PX: f32 = 16.0;

#[derive(Clone, Copy)]
pub struct IconGlyph {
    pub w: f32,
    pub h: f32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

pub struct IconAtlas {
    /// (codepoint, bitmap) pairs; a linear scan over 13 entries is faster than
    /// any map.
    glyphs: Vec<(char, IconGlyph)>,
    pub texture: glow::Texture,
}

impl IconAtlas {
    pub fn get(&self, icon: Icon) -> Option<&IconGlyph> {
        let ch = icon.ch();
        self.glyphs.iter().find(|(c, _)| *c == ch).map(|(_, g)| g)
    }
}

/// Rasterize the icon subset into one small texture. `px` is the em size in
/// physical pixels (ICON_PX * scale).
pub fn build(gl: &glow::Context, font_bytes: &[u8], px: f32) -> IconAtlas {
    let font =
        Font::from_bytes(font_bytes, FontSettings::default()).expect("failed to parse icons");

    const ATLAS: u32 = 128;
    let mut pixels = vec![0u8; (ATLAS * ATLAS) as usize];
    let mut glyphs = Vec::with_capacity(Icon::ALL.len());
    let (mut x, mut y, mut row_h) = (1u32, 1u32, 0u32);

    for icon in Icon::ALL {
        let (metrics, bitmap) = font.rasterize(icon.ch(), px);
        let (w, h) = (metrics.width as u32, metrics.height as u32);
        if w == 0 || h == 0 {
            continue;
        }
        if x + w + 1 > ATLAS {
            x = 1;
            y += row_h + 1;
            row_h = 0;
        }
        for row in 0..h {
            let src = (row * w) as usize;
            let dst = ((y + row) * ATLAS + x) as usize;
            pixels[dst..dst + w as usize].copy_from_slice(&bitmap[src..src + w as usize]);
        }
        glyphs.push((
            icon.ch(),
            IconGlyph {
                w: w as f32,
                h: h as f32,
                u0: x as f32 / ATLAS as f32,
                v0: y as f32 / ATLAS as f32,
                u1: (x + w) as f32 / ATLAS as f32,
                v1: (y + h) as f32 / ATLAS as f32,
            },
        ));
        x += w + 1;
        row_h = row_h.max(h);
    }

    unsafe {
        let texture = gl.create_texture().expect("icon texture");
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
        IconAtlas { glyphs, texture }
    }
}
