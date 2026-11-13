// icons_map.rs: which file icon and accent a path gets.
//
// Data only: no GL, no winit, no filesystem. The glyphs come from the Nerd Font
// Symbols subset in `assets/symbols.ttf` (see `assets/README.md` for how it was
// cut down), and the codepoints were read from that font's own glyph names
// rather than guessed.
//
// The table is a single `const`, and a unit test checks every entry really has a
// glyph in the bundled subset: adding a row without re-subsetting the font fails
// the test instead of drawing a blank box.

use std::path::Path;

use crate::render::Color;
use crate::ui::theme;

/// A file-type icon: the glyph to draw and the colour it takes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FileIcon {
    pub glyph: char,
    pub accent: Color,
}

/// One table row: extensions (lower-case, no dot) and exact file names that map
/// to a glyph and an accent.
struct Entry {
    glyph: char,
    accent: u32,
    extensions: &'static [&'static str],
    names: &'static [&'static str],
}

// Codepoints are Nerd Fonts v3.3.0 glyph names:
//   dev-rust E7A8, dev-cplusplus E7A3, seti-c E649, dev-python E73C,
//   seti-javascript E60C, dev-typescript E8CA, dev-json E80B, seti-config E615,
//   seti-shell E691, dev-markdown E73E, seti-folder E613,
//   oct-file_directory F413, oct-file F4A5.
const RUST: char = '\u{E7A8}';
const CPP: char = '\u{E7A3}';
const C: char = '\u{E649}';
const PYTHON: char = '\u{E73C}';
const JS: char = '\u{E60C}';
const TS: char = '\u{E8CA}';
const JSON: char = '\u{E80B}';
const CONFIG: char = '\u{E615}';
const SHELL: char = '\u{E691}';
const MARKDOWN: char = '\u{E73E}';
const FOLDER: char = '\u{E613}';
const FOLDER_OPEN: char = '\u{F413}';
const FILE: char = '\u{F4A5}';

/// Every glyph the subset must contain. The test walks this, so a new icon in
/// the table cannot silently point at a glyph that was not subset in.
pub const ALL_GLYPHS: &[char] = &[
    RUST,
    CPP,
    C,
    PYTHON,
    JS,
    TS,
    JSON,
    CONFIG,
    SHELL,
    MARKDOWN,
    FOLDER,
    FOLDER_OPEN,
    FILE,
];

// Accents from the spec, tuned for the dark palette. They are deliberately
// subtle: the icon carries the colour, the file name stays normal text.
const RUST_ACCENT: u32 = 0xdea584;
const C_ACCENT: u32 = 0x6e9ccf;
const CPP_ACCENT: u32 = 0x5b8fd0;
const PYTHON_ACCENT: u32 = 0x4b8bbe;
const JS_ACCENT: u32 = 0xe0c95a;
const TS_ACCENT: u32 = 0x3b82c4;
const JSON_ACCENT: u32 = 0xc9c75a;
const TOML_ACCENT: u32 = 0xb8603a;
const MD_ACCENT: u32 = 0x519aba;
const SH_ACCENT: u32 = 0x7cc65c;

const TABLE: &[Entry] = &[
    Entry {
        glyph: RUST,
        accent: RUST_ACCENT,
        extensions: &["rs"],
        names: &[],
    },
    Entry {
        glyph: CPP,
        accent: CPP_ACCENT,
        extensions: &["cpp", "cc", "cxx", "hpp", "hxx"],
        names: &[],
    },
    Entry {
        glyph: C,
        accent: C_ACCENT,
        extensions: &["c", "h"],
        names: &[],
    },
    Entry {
        glyph: PYTHON,
        accent: PYTHON_ACCENT,
        extensions: &["py", "pyi"],
        names: &[],
    },
    Entry {
        glyph: JS,
        accent: JS_ACCENT,
        extensions: &["js", "mjs", "cjs", "jsx"],
        names: &[],
    },
    Entry {
        glyph: TS,
        accent: TS_ACCENT,
        extensions: &["ts", "tsx"],
        names: &[],
    },
    Entry {
        glyph: JSON,
        accent: JSON_ACCENT,
        extensions: &["json"],
        names: &[],
    },
    Entry {
        glyph: CONFIG,
        accent: TOML_ACCENT,
        extensions: &["toml", "ini", "cfg", "conf", "yaml", "yml"],
        names: &["Cargo.toml", "Makefile", "CMakeLists.txt", "Justfile"],
    },
    Entry {
        glyph: SHELL,
        accent: SH_ACCENT,
        extensions: &["sh", "bash", "zsh", "fish"],
        names: &[],
    },
    Entry {
        glyph: MARKDOWN,
        accent: MD_ACCENT,
        extensions: &["md", "markdown"],
        names: &[],
    },
];

/// Icon for a path. Files open in the tree pass `is_dir` and their expansion
/// state; anything unknown gets the default file icon in the muted colour.
pub fn icon_for(path: &Path, is_dir: bool, expanded: bool) -> FileIcon {
    if is_dir {
        return FileIcon {
            glyph: if expanded { FOLDER_OPEN } else { FOLDER },
            accent: if expanded {
                theme::rgb(0x8a9bb0)
            } else {
                theme::rgb(0x7d8799)
            },
        };
    }

    // Exact names win: `Cargo.toml` should look like a manifest, not like any
    // other TOML file. Compared case-sensitively, which is how they are spelled.
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        for entry in TABLE {
            if entry.names.contains(&name) {
                return FileIcon {
                    glyph: entry.glyph,
                    accent: theme::rgb(entry.accent),
                };
            }
        }
    }

    // Then the extension, lower-cased so `Main.RS` still matches.
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext = ext.to_ascii_lowercase();
        for entry in TABLE {
            if entry.extensions.contains(&ext.as_str()) {
                return FileIcon {
                    glyph: entry.glyph,
                    accent: theme::rgb(entry.accent),
                };
            }
        }
    }

    FileIcon {
        glyph: FILE,
        accent: theme::rgb(0x8a9bb0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> &Path {
        Path::new(s)
    }

    #[test]
    fn the_listed_types_map_to_their_own_glyph() {
        let cases = [
            ("main.rs", RUST),
            ("a.cpp", CPP),
            ("a.cc", CPP),
            ("a.cxx", CPP),
            ("a.hpp", CPP),
            ("a.c", C),
            ("a.h", C),
            ("a.py", PYTHON),
            ("a.js", JS),
            ("a.ts", TS),
            ("a.json", JSON),
            ("a.toml", CONFIG),
            ("a.md", MARKDOWN),
            ("run.sh", SHELL),
        ];
        for (name, want) in cases {
            assert_eq!(icon_for(path(name), false, false).glyph, want, "{name}");
        }
    }

    #[test]
    fn extensions_are_case_insensitive() {
        assert_eq!(icon_for(path("Main.RS"), false, false).glyph, RUST);
        assert_eq!(icon_for(path("App.TSX"), false, false).glyph, TS);
    }

    #[test]
    fn well_known_names_beat_a_generic_extension() {
        // Cargo.toml is a manifest, and Makefile has no extension at all.
        assert_eq!(icon_for(path("Cargo.toml"), false, false).glyph, CONFIG);
        assert_eq!(icon_for(path("Makefile"), false, false).glyph, CONFIG);
        assert_eq!(icon_for(path("CMakeLists.txt"), false, false).glyph, CONFIG);
    }

    #[test]
    fn unknown_types_fall_back_to_the_default_file_icon() {
        let unknown = icon_for(path("thing.qqq"), false, false);
        assert_eq!(unknown.glyph, FILE);
        assert_eq!(icon_for(path("noext"), false, false).glyph, FILE);
        assert_eq!(icon_for(path(""), false, false).glyph, FILE);
    }

    #[test]
    fn folders_use_closed_and_open_glyphs() {
        assert_eq!(icon_for(path("src"), true, false).glyph, FOLDER);
        assert_eq!(icon_for(path("src"), true, true).glyph, FOLDER_OPEN);
        // Expansion state must not change a file's icon.
        assert_eq!(
            icon_for(path("main.rs"), false, true).glyph,
            icon_for(path("main.rs"), false, false).glyph
        );
    }

    #[test]
    fn an_unknown_type_and_a_folder_never_share_a_glyph() {
        assert_ne!(
            icon_for(path("a.qqq"), false, false).glyph,
            icon_for(path("a"), true, false).glyph
        );
    }

    #[test]
    fn every_table_glyph_exists_in_the_bundled_subset() {
        // This is the check the spec asks for: it catches an icon added to the
        // table without re-subsetting `assets/symbols.ttf`.
        let bytes: &[u8] = include_bytes!("../../assets/symbols.ttf");
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
            .expect("bundled symbols font must parse");
        for &glyph in ALL_GLYPHS {
            assert_ne!(
                font.lookup_glyph_index(glyph),
                0,
                "U+{:04X} is missing from assets/symbols.ttf; re-run the pyftsubset \
                 command in assets/README.md",
                glyph as u32
            );
        }
    }

    #[test]
    fn every_table_row_is_covered_by_the_glyph_check() {
        for entry in TABLE {
            assert!(
                ALL_GLYPHS.contains(&entry.glyph),
                "table glyph U+{:04X} is not in ALL_GLYPHS",
                entry.glyph as u32
            );
            assert!(
                !entry.extensions.is_empty() || !entry.names.is_empty(),
                "a table row must match something"
            );
        }
    }
}
