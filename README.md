# Ludicrous

A deliberately small, fast code editor for C++ — written in Rust with its own
OpenGL 3.3 renderer. No Electron, no wgpu, no Vulkan.

## What it is

- ~2.5k lines of Rust across 16 source files
- `winit` + `glutin` + `glow` for a GL 3.3 core window
- `ropey` for the text buffer, `fontdue` for glyph rasterisation
- One static unit quad drawn instanced: a rect batch and a glyph batch, so a
  full frame is 1–2 draw calls
- Redraws only when something changes (`ControlFlow::Wait`)

## What it is not

Not an IDE yet. No tabs, file tree, selections, multi-cursor, search, syntax
highlighting, LSP, plugins, settings UI, or undo/redo. The point is the
rendering path and a tight edit loop, not features.

## Build and run

Needs Rust stable and OpenGL 3.3 drivers.

```sh
cargo run --release            # empty buffer
cargo run --release -- main.cpp
```

## Keys

| Key | Action |
| --- | --- |
| Arrows / Home / End / PgUp / PgDn | Move |
| Ctrl+Left / Ctrl+Right | Word left / right |
| Enter, Backspace, Delete, Tab | Edit |
| Ctrl+S | Save |
| Ctrl+Q | Quit |

## Layout

```
src/main.rs        entry point, event loop
src/app.rs         window + GL setup, redraw
src/input.rs       winit events -> Command (the only translator)
src/editor/        buffer, cursor, commands, state
src/ui/            layout geometry and drawing
src/render/        Renderer API, GL objects, glyph atlas, shaders
```

Module boundaries are enforced by convention: `editor/` never touches winit,
glutin, glow, `render/` or `ui/`; `ui/` only calls the `Renderer` API; `render/`
knows nothing about editing.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

CI runs those on `ubuntu-24.04` plus a release build uploaded as
`ludicrous-linux-x86_64`.

## License

DejaVu Sans Mono is bundled under its own permissive license.
