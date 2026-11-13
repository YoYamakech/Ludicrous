// position.rs: the only place that converts between the editor's char offsets
// and LSP positions.
//
// Three coordinate systems are in play:
//   * the editor counts *characters* (ropey char indexes),
//   * tree-sitter counts bytes,
//   * LSP counts UTF-16 code units by default (a newer server may negotiate
//     UTF-8, which is why the encoding is a parameter, not a constant).
//
// Every conversion lives here, with tests for the cases that break naive code:
// multi-byte characters, emoji (two UTF-16 units for one char), and positions
// past the end of a line.

use crate::editor::buffer::Buffer;

/// Which character unit the server wants in `Position::character`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Encoding {
    /// LSP's default and what nearly every server speaks.
    #[default]
    Utf16,
    /// Opt-in fast path for servers that advertise it (rust-analyzer does).
    Utf8,
}

impl Encoding {
    /// Read the encoding a server selected in its `initialize` result.
    pub fn from_server(name: &str) -> Encoding {
        if name.eq_ignore_ascii_case("utf-8") {
            Encoding::Utf8
        } else {
            Encoding::Utf16
        }
    }

    #[allow(dead_code)] // F3 overlay and phase G diagnostics
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf16 => "utf-16",
            Encoding::Utf8 => "utf-8",
        }
    }

    /// How many units `ch` takes in this encoding.
    fn units(self, ch: char) -> usize {
        match self {
            Encoding::Utf16 => ch.len_utf16(),
            Encoding::Utf8 => ch.len_utf8(),
        }
    }
}

/// An LSP position (zero-based line and character).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Pos {
    pub line: usize,
    pub character: usize,
}

impl Pos {
    pub fn new(line: usize, character: usize) -> Self {
        Self { line, character }
    }
}

/// A character range in the buffer, `end` exclusive. This is what the
/// diagnostics model and the UI use.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CharRange {
    pub start: usize,
    pub end: usize,
}

impl CharRange {
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }
}

/// Convert a buffer char index into an LSP position.
pub fn char_to_pos(buffer: &Buffer, index: usize, enc: Encoding) -> Pos {
    let index = index.min(buffer.rope.len_chars());
    let (line, col) = buffer.line_col_of(index);
    Pos::new(line, col_to_character(buffer, line, col, enc))
}

/// Convert an LSP position into a buffer char index.
///
/// Clamped, never panicking: servers do send `line == line_count` (or beyond)
/// for end-of-file ranges, and a character offset that lands inside a
/// surrogate pair or a multi-byte sequence rounds down to the character start.
pub fn pos_to_char(buffer: &Buffer, pos: Pos, enc: Encoding) -> usize {
    let lines = buffer.line_count();
    if pos.line >= lines {
        return buffer.rope.len_chars();
    }
    let line = buffer.line(pos.line);
    let mut units = 0;
    let mut chars = 0;
    for ch in line.chars() {
        // At the requested unit: done.
        if units >= pos.character {
            break;
        }
        // The request lands *inside* this character (a byte in the middle of a
        // sequence, or between the halves of a surrogate pair). Round down to
        // its start instead of overshooting to the next one.
        if units + enc.units(ch) > pos.character {
            break;
        }
        units += enc.units(ch);
        chars += 1;
    }
    buffer.char_index(pos.line, chars)
}

/// Character offset within one line, measured in the server's units.
pub fn col_to_character(buffer: &Buffer, line: usize, col: usize, enc: Encoding) -> usize {
    let text = buffer.line(line);
    text.chars().take(col).map(|c| enc.units(c)).sum()
}

/// A whole LSP range as buffer char indexes. A reversed range (some servers
/// emit them for zero-width diagnostics) is put in order.
pub fn range_to_chars(buffer: &Buffer, start: Pos, end: Pos, enc: Encoding) -> CharRange {
    let a = pos_to_char(buffer, start, enc);
    let b = pos_to_char(buffer, end, enc);
    if a <= b {
        CharRange { start: a, end: b }
    } else {
        CharRange { start: b, end: a }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        Buffer::from_text(text)
    }

    #[test]
    fn ascii_positions_match_columns() {
        let b = buf("hello\nworld\n");
        assert_eq!(char_to_pos(&b, 0, Encoding::Utf16), Pos::new(0, 0));
        assert_eq!(char_to_pos(&b, 7, Encoding::Utf16), Pos::new(1, 1));
        assert_eq!(pos_to_char(&b, Pos::new(1, 3), Encoding::Utf16), 9);
    }

    #[test]
    fn a_multibyte_character_is_one_utf16_unit_but_several_bytes() {
        // "é" is 2 bytes, 1 UTF-16 unit, 1 char.
        let b = buf("éx\n");
        assert_eq!(char_to_pos(&b, 1, Encoding::Utf16), Pos::new(0, 1));
        assert_eq!(char_to_pos(&b, 1, Encoding::Utf8), Pos::new(0, 2));
        assert_eq!(pos_to_char(&b, Pos::new(0, 1), Encoding::Utf16), 1);
        assert_eq!(pos_to_char(&b, Pos::new(0, 2), Encoding::Utf8), 1);
        // An offset in the middle of the sequence rounds down to its start.
        assert_eq!(pos_to_char(&b, Pos::new(0, 1), Encoding::Utf8), 0);
    }

    #[test]
    fn an_emoji_is_two_utf16_units_for_one_char() {
        let b = buf("a😀b\n");
        // chars: a=0, emoji=1, b=2
        assert_eq!(char_to_pos(&b, 1, Encoding::Utf16), Pos::new(0, 1));
        assert_eq!(char_to_pos(&b, 2, Encoding::Utf16), Pos::new(0, 3));
        assert_eq!(char_to_pos(&b, 2, Encoding::Utf8), Pos::new(0, 5));
        assert_eq!(pos_to_char(&b, Pos::new(0, 2), Encoding::Utf16), 1);
        assert_eq!(pos_to_char(&b, Pos::new(0, 3), Encoding::Utf16), 2);
        assert_eq!(pos_to_char(&b, Pos::new(0, 5), Encoding::Utf8), 2);
    }

    #[test]
    fn round_trip_holds_for_every_char_boundary() {
        let b = buf("aé😀z\nsecond 😀😀 line\nlast");
        for enc in [Encoding::Utf16, Encoding::Utf8] {
            for i in 0..=b.rope.len_chars() {
                let pos = char_to_pos(&b, i, enc);
                assert_eq!(pos_to_char(&b, pos, enc), i, "{enc:?} at char {i}");
            }
        }
    }

    #[test]
    fn positions_past_the_end_are_clamped() {
        let b = buf("abc\ndef");
        assert_eq!(pos_to_char(&b, Pos::new(9, 0), Encoding::Utf16), 7);
        assert_eq!(pos_to_char(&b, Pos::new(0, 99), Encoding::Utf16), 3);
        assert_eq!(
            char_to_pos(&b, 9999, Encoding::Utf16),
            Pos::new(1, 3),
            "past the last char is the end of the last line"
        );
    }

    #[test]
    fn a_reversed_range_is_put_in_order() {
        let b = buf("abcdef\n");
        let r = range_to_chars(&b, Pos::new(0, 4), Pos::new(0, 1), Encoding::Utf16);
        assert_eq!(r, CharRange { start: 1, end: 4 });
    }

    #[test]
    fn encoding_comes_from_the_server_string() {
        assert_eq!(Encoding::from_server("utf-8"), Encoding::Utf8);
        assert_eq!(Encoding::from_server("utf-16"), Encoding::Utf16);
        assert_eq!(Encoding::from_server("UTF-8"), Encoding::Utf8);
        // Anything unknown falls back to the protocol default.
        assert_eq!(Encoding::from_server("utf-32"), Encoding::Utf16);
    }
}
