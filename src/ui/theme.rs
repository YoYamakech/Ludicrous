// theme.rs: every colour and metric the UI uses, in one place.
//
// Metrics are plain consts. Colours come from a `Palette`, so switching theme
// (dark/light) is one value passed down instead of a global. The dark palette
// is the shell one from the spec; the light palette is its inverse, with the
// same accent. Editor colours live here too, so the text area follows the
// theme.

use crate::config::Theme;
use crate::render::Color;

/// `#rrggbb` to a renderer colour. Alpha is 1.0.
pub const fn rgb(hex: u32) -> Color {
    [
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
        1.0,
    ]
}

/// All colours for one theme.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Color,
    pub sidebar_bg: Color,
    pub border: Color,
    pub pill_active: Color,
    pub pill_hover: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub gutter_bg: Color,
    pub gutter_text: Color,
    pub status_bg: Color,
    pub status_text: Color,
    pub selection: Color,
    pub close_hover: Color,
}

pub const DARK: Palette = Palette {
    bg: rgb(0x1a1d23),
    sidebar_bg: rgb(0x15171c),
    border: rgb(0x262a31),
    pill_active: rgb(0x2b303a),
    pill_hover: rgb(0x20242b),
    text: rgb(0xe6e8ec),
    muted: rgb(0x8a909c),
    accent: rgb(0xf0a030),
    gutter_bg: rgb(0x1f2126),
    gutter_text: rgb(0x616a78),
    status_bg: rgb(0x292d34),
    status_text: rgb(0xb8bec8),
    selection: rgb(0x33475f),
    close_hover: rgb(0xd94f4f),
};

pub const LIGHT: Palette = Palette {
    bg: rgb(0xf6f7f9),
    sidebar_bg: rgb(0xecedf1),
    border: rgb(0xd3d7de),
    pill_active: rgb(0xdadfe8),
    pill_hover: rgb(0xe3e6ec),
    text: rgb(0x1f2329),
    muted: rgb(0x6b7280),
    accent: rgb(0xc9781a),
    gutter_bg: rgb(0xe9ebef),
    gutter_text: rgb(0x9aa1ac),
    status_bg: rgb(0xe4e7ec),
    status_text: rgb(0x414b59),
    selection: rgb(0xbfd3e8),
    close_hover: rgb(0xd94f4f),
};

impl Palette {
    pub fn of(theme: Theme) -> Palette {
        match theme {
            Theme::Dark => DARK,
            Theme::Light => LIGHT,
        }
    }
}

// Metrics (spec section 3). Not theme dependent.
pub const TITLE_H: f32 = 30.0;
pub const SIDEBAR_W: f32 = 220.0;
pub const SIDEBAR_RAIL_W: f32 = 48.0;
pub const ROW_H: f32 = 28.0;
pub const PILL_RADIUS: f32 = 6.0;
pub const PAD: f32 = 8.0;
pub const MIN_W: f32 = 480.0;
pub const MIN_H: f32 = 320.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palettes_differ_in_background_and_text() {
        assert_ne!(DARK.bg, LIGHT.bg);
        assert_ne!(DARK.text, LIGHT.text);
        // Dark text must be brighter than its background, light the reverse.
        const { assert!(DARK.text[0] > DARK.bg[0]) };
        const { assert!(LIGHT.text[0] < LIGHT.bg[0]) };
    }

    #[test]
    fn palette_lookup_follows_the_theme() {
        assert_eq!(Palette::of(Theme::Light).bg, LIGHT.bg);
        assert_eq!(Palette::of(Theme::Dark).bg, DARK.bg);
    }
}
