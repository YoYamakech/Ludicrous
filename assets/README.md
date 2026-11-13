# Bundled assets

The editor binary embeds three fonts. None is covered by a project license; each
keeps its own, and all three notices ship inside the release archive.

| File | Font | Used for |
|---|---|---|
| `font.ttf` | DejaVu Sans Mono | Editor text and UI text (the primary font) |
| `icons.ttf` | Lucide icon subset | Chrome: chevrons, gear, window buttons, sidebar toggle, download badge, diagnostics |
| `symbols.ttf` | Nerd Font Symbols Mono (subset) | File-type icons in the tree and open-files list; fallback for private-use-area characters in text |

## Subsetting `symbols.ttf`

The full `SymbolsNerdFontMono-Regular.ttf` is 2.3 MB and 10,397 glyphs, which is
too much to parse at startup on the target laptop. It is cut down at dev time to
exactly the icons `ui/icons_map.rs` uses (plus their composite dependencies),
which comes out at **5.3 KB**.

The codepoints were taken from the font's own glyph names, not guessed:

```sh
# Nerd Fonts v3.3.0, NerdFontsSymbolsOnly.zip
CODES=e7a8,e7a3,e649,e73c,e60c,e8ca,e80b,e691,e73e,e795,e613,f4a5,f413,e615,e628
pyftsubset SymbolsNerdFontMono-Regular.ttf \
  --unicodes=$CODES \
  --output-file=assets/symbols.ttf \
  --layout-features='' --no-hinting --desubroutinize \
  --drop-tables+=GSUB,GPOS,GDEF --name-IDs='*' --recalc-bounds
```

`assets/` unit tests assert that every entry in the icon table has a real glyph in
this subset, so adding an icon to the table without re-subsetting fails the tests
instead of drawing a blank box on screen.

Glyph names and codepoints actually kept:

| Glyph name | Codepoint | Used for |
|---|---|---|
| `dev-rust` | `U+E7A8` | Rust |
| `dev-cplusplus` | `U+E7A3` | C++ |
| `seti-c` | `U+E649` | C |
| `dev-python` | `U+E73C` | Python |
| `seti-javascript` | `U+E60C` | JavaScript |
| `dev-typescript` | `U+E8CA` | TypeScript |
| `dev-json` | `U+E80B` | JSON |
| `seti-config` | `U+E615` | TOML, INI, YAML, `Cargo.toml`, `Makefile`, `CMakeLists.txt` |
| `seti-shell` | `U+E691` | Shell scripts |
| `dev-markdown` | `U+E73E` | Markdown |
| `seti-folder` | `U+E613` | Folders, closed |
| `oct-file_directory` | `U+F413` | Folders, expanded |
| `oct-file` | `U+F4A5` | Default file icon |
| `dev-terminal` | `U+E795` | Kept for the status bar hint (not yet drawn) |

## `font.ttf` — DejaVu Sans Mono

See `FONT_LICENSE.txt`.

## `icons.ttf` — Lucide subset

See `ICONS_LICENSE.txt` (ISC License, some icons derived from Feather, MIT).

## `symbols.ttf` — Nerd Fonts symbols

MIT License, Copyright (c) 2014 Ryan L McIntyre. See `SYMBOLS_LICENSE.txt`; the
individual glyph sets inside Nerd Fonts carry their own upstream notices
(Material Design Icons, Font Awesome, Devicons, Octicons and others), all of
which permit redistribution of subsets.
