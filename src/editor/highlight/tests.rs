// Tests for the highlighter: query building, capture mapping, byte/char
// conversion and the cache. Kept out of `mod.rs` so that file stays readable.

use super::*;
use crate::editor::buffer::{Buffer, LineEnding};
use ropey::Rope;

fn buf(text: &str) -> Buffer {
    Buffer {
        rope: Rope::from_str(text),
        path: None,
        modified: false,
        line_ending: LineEnding::Lf,
        version: 1,
        last_edit: None,
    }
}

fn with_path(text: &str, path: &str) -> Buffer {
    let mut b = buf(text);
    b.path = Some(std::path::PathBuf::from(path));
    b
}

#[test]
fn languages_are_picked_by_extension() {
    let cases = [
        ("a.rs", Some(Lang::Rust)),
        ("a.c", Some(Lang::C)),
        ("a.h", Some(Lang::C)),
        ("a.cpp", Some(Lang::Cpp)),
        ("a.hpp", Some(Lang::Cpp)),
        ("a.py", Some(Lang::Python)),
        ("a.json", Some(Lang::Json)),
        ("a.toml", Some(Lang::Toml)),
        ("a.md", Some(Lang::Markdown)),
        ("a.txt", None),
        ("Makefile", None),
        ("A.RS", Some(Lang::Rust)),
    ];
    for (name, want) in cases {
        assert_eq!(Lang::for_path(Path::new(name)), want, "{name}");
    }
}

#[test]
fn every_language_builds_a_query() {
    // A grammar update that breaks a query must show up here, not as a
    // silently colourless file.
    for lang in [
        Lang::Rust,
        Lang::C,
        Lang::Cpp,
        Lang::Python,
        Lang::Json,
        Lang::Toml,
        Lang::Markdown,
    ] {
        assert!(query_for(lang).is_some(), "{lang:?} has no usable query");
    }
}

#[test]
fn every_language_parses_a_small_file() {
    let samples = [
        (Lang::Rust, "fn main() { let x = 1; }\n"),
        (Lang::C, "int main(void) { return 0; }\n"),
        (Lang::Cpp, "int main() { return 0; }\n"),
        (Lang::Python, "def f(x):\n    return x\n"),
        (Lang::Json, "{\"a\": 1}\n"),
        (Lang::Toml, "a = 1\n"),
        (Lang::Markdown, "# Title\n\ntext\n"),
    ];
    for (lang, text) in samples {
        let b = buf(text);
        let mut h = Highlighter::new(lang).expect("highlighter");
        let spans = h.spans_for_lines(&b, 0, b.line_count(), 1);
        assert_eq!(spans.len(), b.line_count());
        assert!(
            spans.iter().any(|row| !row.is_empty()),
            "{lang:?} produced no spans for {text:?}"
        );
    }
}

#[test]
fn a_rust_keyword_and_string_are_found() {
    let b = buf("fn main() {\n    let s = \"hi\";\n}\n");
    let mut h = Highlighter::new(Lang::Rust).unwrap();
    let spans = h.spans_for_lines(&b, 0, 3, 1);
    let kinds: Vec<_> = spans.iter().flatten().map(|s| s.kind).collect();
    assert!(kinds.contains(&TokenKind::Keyword), "{kinds:?}");
    assert!(kinds.contains(&TokenKind::String), "{kinds:?}");
    // `fn` sits at columns 0..2.
    let first = spans[0]
        .iter()
        .find(|s| s.kind == TokenKind::Keyword)
        .unwrap();
    assert_eq!((first.start_col, first.end_col), (0, 2));
}

#[test]
fn markdown_style_captures_are_understood() {
    // The block grammar names captures by node type, with the meaning last.
    assert_eq!(TokenKind::from_capture("text.title"), TokenKind::Heading);
    assert_eq!(TokenKind::from_capture("text.literal"), TokenKind::String);
    assert_eq!(
        TokenKind::from_capture("punctuation.special"),
        TokenKind::Punctuation
    );
    assert_eq!(TokenKind::from_capture("text.uri"), TokenKind::Unknown);
    assert_eq!(TokenKind::from_capture("text"), TokenKind::Unknown);
}

#[test]
fn markdown_headings_get_the_heading_colour() {
    let b = buf("# Title\n\ntext\n");
    let mut h = Highlighter::new(Lang::Markdown).unwrap();
    let spans = h.spans_for_lines(&b, 0, 3, 1);
    let kinds: Vec<_> = spans.iter().flatten().map(|s| s.kind).collect();
    assert!(kinds.contains(&TokenKind::Heading), "{kinds:?}");
}

#[test]
fn captures_map_to_a_small_kind_set() {
    assert_eq!(TokenKind::from_capture("keyword"), TokenKind::Keyword);
    assert_eq!(
        TokenKind::from_capture("keyword.control"),
        TokenKind::Keyword
    );
    assert_eq!(TokenKind::from_capture("string.special"), TokenKind::String);
    assert_eq!(TokenKind::from_capture("comment"), TokenKind::Comment);
    assert_eq!(
        TokenKind::from_capture("punctuation.bracket"),
        TokenKind::Punctuation
    );
    assert_eq!(
        TokenKind::from_capture("function.method"),
        TokenKind::Function
    );
    // Unknown names must be safe, not a panic and not a wrong colour.
    assert_eq!(TokenKind::from_capture("nonsense"), TokenKind::Unknown);
    assert_eq!(TokenKind::from_capture(""), TokenKind::Unknown);
}

#[test]
fn multibyte_text_keeps_columns_in_characters() {
    // "é" is two bytes; the string starts after it, so a byte column would
    // be wrong by one if the conversion were missing.
    let b = buf("// é comment\nlet s = \"é\";\n");
    let mut h = Highlighter::new(Lang::Rust).unwrap();
    let spans = h.spans_for_lines(&b, 0, 2, 1);
    let string = spans[1]
        .iter()
        .find(|s| s.kind == TokenKind::String)
        .expect("string span");
    // The line is `let s = "é";` so the string is chars 8..=10.
    assert_eq!(string.start_col, 8);
    assert_eq!(string.end_col, 11, "the closing quote is included");
    // And the comment covers all 12 characters of `// é comment`, not its 13
    // bytes.
    let comment = spans[0]
        .iter()
        .find(|s| s.kind == TokenKind::Comment)
        .expect("comment span");
    assert_eq!((comment.start_col, comment.end_col), (0, 12));
}

#[test]
fn char_to_byte_matches_the_rope() {
    let b = buf("aé\nb\n");
    // chars: a(1) é(2) \n(1) b(1) \n(1) -> bytes 1,3,4,5,6
    assert_eq!(char_to_byte(&b, 0), 0);
    assert_eq!(char_to_byte(&b, 1), 1);
    assert_eq!(char_to_byte(&b, 2), 3);
    assert_eq!(char_to_byte(&b, 3), 4);
    assert_eq!(char_to_byte(&b, 4), 5);
    // Out of range clamps instead of panicking.
    assert_eq!(char_to_byte(&b, 99), 6);
}

#[test]
fn an_edit_converts_to_byte_and_point_offsets() {
    // Insert an "é" at char 1 of "ab\ncd\n" -> "aéb\ncd\n".
    let b = buf("aéb\ncd\n");
    let edit = Edit {
        start_char: 1,
        old_chars: 0,
        new_chars: 1,
    };
    let ie = edit.input_edit(&b).expect("edit");
    assert_eq!(ie.start_byte, 1);
    assert_eq!(ie.new_end_byte, 3, "é is two bytes");
    assert_eq!(ie.start_position, Point::new(0, 1));
    // The new end is three bytes into line 0, which is its length.
    assert_eq!(ie.new_end_position, Point::new(0, 3));
}

#[test]
fn an_edit_across_a_newline_gets_the_right_points() {
    let b = buf("ab\ncd\n");
    // Delete "\nc" (chars 2..4): points start at line 0 col 2, end at line
    // 1 col 1.
    let edit = Edit {
        start_char: 2,
        old_chars: 2,
        new_chars: 0,
    };
    let ie = edit.input_edit(&b).unwrap();
    assert_eq!(ie.start_byte, 2);
    assert_eq!(ie.old_end_byte, 4);
    assert_eq!(ie.start_position, Point::new(0, 2));
    assert_eq!(ie.old_end_position, Point::new(1, 1));
}

#[test]
fn an_out_of_range_edit_is_clamped_not_panicking() {
    let b = buf("short\n");
    let edit = Edit {
        start_char: 900,
        old_chars: 10,
        new_chars: 5,
    };
    let ie = edit.input_edit(&b).unwrap();
    assert!(ie.start_byte <= b.rope.len_bytes());
    assert_eq!(ie.old_end_byte, b.rope.len_bytes());
}

#[test]
fn editing_updates_the_tree_incrementally() {
    let mut b = buf("let x = 1;\n");
    let mut h = Highlighter::new(Lang::Rust).unwrap();
    let before = h.spans_for_lines(&b, 0, 1, 1);
    assert!(!before[0].is_empty());

    // Append a line: the tree must know about it after `edit` + `reparse`.
    b.rope.insert(11, "let y = \"s\";\n");
    let edit = Edit {
        start_char: 11,
        old_chars: 0,
        new_chars: 12,
    };
    h.edit(&edit.input_edit(&b).unwrap());
    let spans = h.spans_for_lines(&b, 0, 2, 2);
    assert_eq!(spans.len(), 2);
    let kinds: Vec<_> = spans[1].iter().map(|s| s.kind).collect();
    assert!(kinds.contains(&TokenKind::String), "{kinds:?}");
}

#[test]
fn the_cache_is_reused_for_the_same_version_and_range() {
    let b = buf("fn main() {}\n");
    let mut h = Highlighter::new(Lang::Rust).unwrap();
    let a = h.spans_for_lines(&b, 0, 1, 7);
    let reparsed = h.reparse(&b, 7);
    assert!(!reparsed, "same version means no parse");
    let c = h.spans_for_lines(&b, 0, 1, 7);
    assert_eq!(a, c);
}

#[test]
fn only_the_requested_lines_get_spans() {
    let text = "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n";
    let b = buf(text);
    let mut h = Highlighter::new(Lang::Rust).unwrap();
    let spans = h.spans_for_lines(&b, 2, 3, 1);
    assert_eq!(spans.len(), 1, "one line was asked for");
    // Line 2 is `fn c() {}`: the keyword must be there.
    assert!(spans[0].iter().any(|s| s.kind == TokenKind::Keyword));
}

#[test]
fn a_line_range_past_the_end_is_clamped() {
    let b = buf("one\ntwo\n");
    let mut h = Highlighter::new(Lang::Rust).unwrap();
    let spans = h.spans_for_lines(&b, 0, 99, 1);
    // `ropey` counts the trailing newline as the start of a third, empty line.
    assert_eq!(spans.len(), b.line_count());
    assert!(spans.len() >= 2);
}

#[test]
fn a_large_file_is_not_highlighted() {
    let mut b = with_path("x", "big.rs");
    b.rope = Rope::from_str(&"a".repeat(MAX_HIGHLIGHT_BYTES + 1));
    assert_eq!(lang_for_buffer(&b), None);
    let small = with_path("fn main() {}\n", "small.rs");
    assert_eq!(lang_for_buffer(&small), Some(Lang::Rust));
    // An unnamed buffer has no language: [No Name] gets no colours.
    assert_eq!(lang_for_buffer(&buf("fn main() {}")), None);
}
