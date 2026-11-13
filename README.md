# Ludicrous

A deliberately small, fast code editor — written in Rust with its own
OpenGL 3.3 renderer. No Electron, no wgpu, no Vulkan.

## What it is

- `winit` + `glutin` + `glow` for an undecorated, transparent, GL 3.3 core window
- `ropey` for the text buffer, `fontdue` for glyph rasterisation
- One static unit quad drawn instanced: a rect batch and a glyph batch per layer,
  so a full frame is a handful of draw calls
- Redraws only when something changes (`ControlFlow::Wait`); the caret blinks with
  `WaitUntil`, never a busy loop
- A hand-drawn shell: rounded window, its own title bar, sidebar, settings page

## What it is not

No tabs, split views, search, source control panel, build panel, command palette,
plugins or drag-and-drop. Syntax highlighting, LSP and multi-cursor are not here
either. The point is the rendering path and a tight edit loop.

## Build and run

Needs Rust stable and OpenGL 3.3 drivers.

```sh
cargo run --release                 # empty buffer
cargo run --release -- main.cpp     # one file
cargo run --release -- ~/project    # a folder in the sidebar
```

On Debian/Ubuntu the window needs the usual X11/Wayland/GL development packages:

```sh
sudo apt-get install -y libxkbcommon-dev libx11-dev libxcursor-dev \
  libxrandr-dev libxi-dev libxinerama-dev libwayland-dev \
  libgl1-mesa-dev pkg-config
```

## Keys

| Key | Action |
| --- | --- |
| Arrows / Home / End / PgUp / PgDn | Move |
| Shift + movement | Select |
| Mouse wheel, click | Scroll, place the caret |
| Enter, Backspace, Delete, Tab | Edit (Tab / Shift+Tab indent) |
| Ctrl+S | Save |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo |
| Ctrl+C / Ctrl+X / Ctrl+V | Clipboard |
| Ctrl+Q | Quit (asks again when a file is dirty) |
| Ctrl+B | Toggle the sidebar |
| Ctrl+Shift+E | Focus the file tree |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous open file |
| Ctrl+W | Close the active file |
| Ctrl+, | Open or close the settings page |
| F3 | Debug overlay (GPU frame time, draw calls, active GL paths) |
| F11 | Fullscreen |
| Esc | Back to the editor |

## Settings

The settings page replaces the text area and applies every change live. It is
stored in a plain `key = value` file at `$XDG_CONFIG_HOME/ludicrous/config`
(falls back to `~/.config/ludicrous/config`):

```ini
# Ludicrous config
font_size = 15
tab_width = 4
show_line_numbers = true
cursor_blink = false
window_buttons = right
corner_radius = 10
sidebar = auto
theme = dark
experimental_damage = false
```

Unknown keys and invalid values are ignored, so a hand-broken file still starts
the editor. Window size, the maximized flag and the last sidebar mode live in
`$XDG_STATE_HOME/ludicrous/state` (position is never restored).

## Performance

Optional GL paths are detected by extension name and each has a core 3.3
fallback:

- `GL_ARB_buffer_storage` + `GL_ARB_base_instance`: a persistent 3-region
  instance ring, drawn with `base_instance`, with fences between frames.
- Without them (or with `LUDICROUS_GL_SAFE=1`): `glBufferData` orphaning plus
  `glBufferSubData`.

`F3` shows which path is live along with GPU frame time from
`GL_ARB_timer_query`, draw calls, instances and batch flushes. Frame timing is
measured, not guessed.

## Layout

```text
src/
  app.rs            window, GL context, event wiring
  config.rs         settings + window state (hand-parsed, safe save)
  window.rs         hit testing for buttons, drag and resize
  input.rs          keys -> Command
  editor/           buffer, cursor, commands, undo history
  workspace/        open buffers and the lazy file tree
  ui/               theme, layout, view, sidebar, settings, widgets, overlay
  render/           renderer, batches, glyph atlases, shaders, GL caps
```

## License

The bundled font and the Lucide icon subset keep their own licenses; see
`assets/`.
