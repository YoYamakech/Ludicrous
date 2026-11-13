// theme.rs: every colour and metric the UI uses, in one place.
// The shell palette is the one from the spec; the editor colours are the v1
// dark theme, kept so the text area still looks the same.

use crate::render::Color;

/// `#rrggbb` to a renderer colour. Alpha is 1.0; use `with_alpha` for panels.
pub const fn rgb(hex: u32) -> Color {
    [
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
        1.0,
    ]
}

// Shell palette (spec section 3).
pub const BG: Color = rgb(0x1a1d23);
pub const SIDEBAR_BG: Color = rgb(0x15171c);
pub const BORDER: Color = rgb(0x262a31);
pub const PILL_ACTIVE: Color = rgb(0x2b303a);
pub const PILL_HOVER: Color = rgb(0x20242b);
pub const TEXT: Color = rgb(0xe6e8ec);
pub const MUTED: Color = rgb(0x8a909c);
pub const ACCENT: Color = rgb(0xf0a030);

// Editor colours (v1 theme, unchanged).
pub const GUTTER_BG: Color = rgb(0x1f2126);
pub const GUTTER_TEXT: Color = rgb(0x616a78);
pub const CURSOR: Color = ACCENT;
pub const STATUS_BG: Color = rgb(0x292d34);
pub const STATUS_TEXT: Color = rgb(0xb8bec8);
pub const MODIFIED: Color = ACCENT;
pub const ERROR: Color = rgb(0xf27366);
pub const SELECTION: Color = rgb(0x33475f);

// Window chrome.
pub const CLOSE_HOVER: Color = rgb(0xd94f4f);
pub const TITLE_TEXT: Color = TEXT;

// Metrics (spec section 3).
pub const WINDOW_RADIUS: f32 = 10.0;
pub const TITLE_H: f32 = 30.0;
pub const SIDEBAR_W: f32 = 220.0;
pub const SIDEBAR_RAIL_W: f32 = 48.0;
pub const ROW_H: f32 = 28.0;
pub const PILL_RADIUS: f32 = 6.0;
pub const PAD: f32 = 8.0;
pub const MIN_W: f32 = 480.0;
pub const MIN_H: f32 = 320.0;
