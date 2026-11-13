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
    /// Fill of the *selected* segment in a choice row. Deliberately stronger
    /// than `pill_active`: the selected option must be obvious at a glance,
    /// especially in the light theme (issue #6.4).
    pub pill_selected: Color,
    /// 1 px hairline around the selected segment, to lift it off the panel.
    pub pill_selected_border: Color,
    /// Text inside the selected segment: full contrast, never muted.
    pub pill_selected_text: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub gutter_bg: Color,
    pub gutter_text: Color,
    pub status_bg: Color,
    pub status_text: Color,
    pub selection: Color,
    pub close_hover: Color,
    /// Syntax colours. One per `TokenKind`, so the editor picks by kind and
    /// never by string. Tuned to stay legible on both backgrounds.
    pub syn_keyword: Color,
    pub syn_string: Color,
    pub syn_comment: Color,
    pub syn_function: Color,
    pub syn_type: Color,
    pub syn_number: Color,
    pub syn_constant: Color,
    pub syn_property: Color,
    pub syn_operator: Color,
    pub syn_punctuation: Color,
    pub syn_heading: Color,
}

pub const DARK: Palette = Palette {
    bg: rgb(0x1a1d23),
    sidebar_bg: rgb(0x15171c),
    border: rgb(0x262a31),
    pill_active: rgb(0x2b303a),
    pill_hover: rgb(0x20242b),
    pill_selected: rgb(0x3a4150),
    pill_selected_border: rgb(0x4d5768),
    pill_selected_text: rgb(0xf4f6f9),
    text: rgb(0xe6e8ec),
    muted: rgb(0x8a909c),
    accent: rgb(0xf0a030),
    gutter_bg: rgb(0x1f2126),
    gutter_text: rgb(0x616a78),
    status_bg: rgb(0x292d34),
    status_text: rgb(0xb8bec8),
    selection: rgb(0x33475f),
    close_hover: rgb(0xd94f4f),
    // Dark syntax colours: soft on the eyes, distinct at 8 px tall glyphs.
    syn_keyword: rgb(0xc792ea),
    syn_string: rgb(0x9ece6a),
    syn_comment: rgb(0x6b7280),
    syn_function: rgb(0x7aa2f7),
    syn_type: rgb(0x2ac3de),
    syn_number: rgb(0xff9e64),
    syn_constant: rgb(0xe0af68),
    syn_property: rgb(0x73daca),
    syn_operator: rgb(0x89ddff),
    syn_punctuation: rgb(0x9aa5b1),
    syn_heading: rgb(0xf0a030),
};

pub const LIGHT: Palette = Palette {
    bg: rgb(0xf6f7f9),
    sidebar_bg: rgb(0xecedf1),
    border: rgb(0xd3d7de),
    pill_active: rgb(0xdadfe8),
    pill_hover: rgb(0xe3e6ec),
    // A crisp white card with a hairline and near-black text. The old light
    // grey pill with muted text was hard to read (issue #6.4).
    pill_selected: rgb(0xffffff),
    pill_selected_border: rgb(0xb9c0cc),
    pill_selected_text: rgb(0x1a1d23),
    text: rgb(0x1f2329),
    muted: rgb(0x6b7280),
    accent: rgb(0xc9781a),
    gutter_bg: rgb(0xe9ebef),
    gutter_text: rgb(0x9aa1ac),
    status_bg: rgb(0xe4e7ec),
    status_text: rgb(0x414b59),
    selection: rgb(0xbfd3e8),
    close_hover: rgb(0xd94f4f),
    // Light syntax colours: darker versions of the same hues.
    syn_keyword: rgb(0x8a3fb0),
    syn_string: rgb(0x3e7d1f),
    syn_comment: rgb(0x8a909c),
    syn_function: rgb(0x1d5fb8),
    syn_type: rgb(0x0b7285),
    syn_number: rgb(0xb45309),
    syn_constant: rgb(0x8a5a00),
    syn_property: rgb(0x0f766e),
    syn_operator: rgb(0x0e7490),
    syn_punctuation: rgb(0x6b7280),
    syn_heading: rgb(0xc9781a),
};

impl Palette {
    pub fn of(theme: Theme) -> Palette {
        match theme {
            Theme::Dark => DARK,
            Theme::Light => LIGHT,
        }
    }

    /// The colour for a highlighted token. `Unknown` means "no special colour",
    /// which is the normal text colour: an unrecognised capture must never make
    /// text invisible.
    pub fn syntax(&self, kind: crate::editor::highlight::TokenKind) -> Color {
        use crate::editor::highlight::TokenKind as K;
        match kind {
            K::Unknown | K::Variable => self.text,
            K::Keyword => self.syn_keyword,
            K::String => self.syn_string,
            K::Comment => self.syn_comment,
            K::Function => self.syn_function,
            K::Type => self.syn_type,
            K::Number => self.syn_number,
            K::Constant => self.syn_constant,
            K::Property => self.syn_property,
            K::Operator => self.syn_operator,
            K::Punctuation => self.syn_punctuation,
            K::Heading => self.syn_heading,
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
    fn selected_segments_stay_high_contrast_in_both_themes() {
        for (name, p) in [("dark", DARK), ("light", LIGHT)] {
            // The selected fill must differ from the panel background, or the
            // pill disappears (issue #6.4).
            let d =
                |a: Color, b: Color| -> f32 { (0..3).map(|i| (a[i] - b[i]).abs()).sum::<f32>() };
            assert!(d(p.pill_selected, p.bg) > 0.05, "{name}: pill vs bg");
            assert!(
                d(p.pill_selected, p.pill_hover) > 0.05,
                "{name}: pill vs hover"
            );
            // Its text must contrast strongly with the fill it sits on.
            let lum = |c: Color| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            let (a, b) = (lum(p.pill_selected_text), lum(p.pill_selected));
            assert!((a - b).abs() > 0.5, "{name}: selected text contrast");
            // And the border must be visible against the fill.
            assert!(
                d(p.pill_selected_border, p.pill_selected) > 0.05,
                "{name}: border"
            );
        }
    }

    #[test]
    fn palette_lookup_follows_the_theme() {
        assert_eq!(Palette::of(Theme::Light).bg, LIGHT.bg);
        assert_eq!(Palette::of(Theme::Dark).bg, DARK.bg);
    }
}
