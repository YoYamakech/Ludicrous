# Ludicrous v1.1: The Shell

This document extends `LUDICROUS_ARCHITECTURE.md`. Read that file first. Its module rules (section 5), data flow (section 6), and working style (section 13) still apply here.

**Naming, so nothing gets confused:** the reliability items (safe save, quit guard, undo/redo, UTF-8, long lines, selection, auto-indent, mouse click) are in the "v1.1: Reliability & Comfort" section of `LUDICROUS_ARCHITECTURE.md`. This document is the **shell**: window, sidebar, settings, and performance. The two are independent, but finish at least safe save, quit guard, and undo/redo first, because multiple open files make data-loss bugs more likely.

**What changes from v1:** v1 said "no tabs, no file tree." This document lifts that rule **only** for a file-tree sidebar and an open-files list. Tabs, search, and the other items on the "later" list stay out.

---

## 1. Goals

1. **Reclaim screen space.** The developer's screen is small and KDE's window decorations waste too much of it. Ludicrous draws its own title bar, which is thin and holds the window controls.
2. **Rounded, modern window.** Rounded corners, a thin border, and a clean look.
3. **IDE sidebar** in the style of the reference design: a rounded card with a header, grouped sections, hover and active pills, dividers, and a footer. Contents are tailored to code: open files, the project's file tree, settings, and a project card.
4. **Simple settings page** drawn in-app, with a plain config file.
5. **Faster rendering on weak hardware** (Intel HD 4000, Mesa, HDD) by using the GL extensions the driver actually exposes, each one optional, with a fallback.

## 2. Non-goals

No tabs, split views, search, source control panel, build panel, command palette, plugins, themes marketplace, or drag-and-drop. Reserve space for them in the layout but do not build them.

---

## 3. Layout

```
+--------------------------------------------------------------------+
| ●●●  Ludicrous: main.cpp [modified]                      (or ─ □ ✕) |  title bar, 30 px
+-----------+--------------------------------------------------------+
| sidebar   |  gutter | text area                                     |
| 220 px    |                                                        |
| (or rail  |                                                        |
|  48 px,   +--------------------------------------------------------+
|  or none) |  status bar                                            |
+-----------+--------------------------------------------------------+
```

- The window is **one rounded rectangle**. Everything is drawn inside it.
- The title bar spans the full width. Window buttons go on the **right** (like KDE) or the **left** (traffic-light dots like the reference), set by `window_buttons` in config.
- The sidebar has three modes: **expanded** (220 px), **rail** (48 px, icons only), **hidden**.
- **Small-screen behavior** (`sidebar = auto`, the default): when the window's logical width is under 900 px, use rail. Under 600 px, hide it. A manual toggle (Ctrl+B) overrides auto until the window is resized across a threshold.
- Maximized or fullscreen: **square corners, no border**.

### Metrics (all `const` in `ui/theme.rs`)

| Item | Value |
|---|---|
| Window corner radius | 10 px (setting: 0, 6, 10, 14) |
| Window border | 1 px, subtle |
| Title bar height | 30 px |
| Sidebar width (expanded / rail) | 220 / 48 px |
| Row height in sidebar | 28 px |
| Row pill radius | 6 px |
| Padding | 8 px |
| Resize hit area | 5 px inside the window edge |

### Suggested dark palette (all in `theme.rs`, one struct)

| Role | Color |
|---|---|
| Editor background | `#1a1d23` |
| Sidebar background | `#15171c` |
| Border / dividers | `#262a31` |
| Active row pill | `#2b303a` |
| Hover row pill | `#20242b` |
| Text | `#e6e8ec` |
| Muted text | `#8a909c` |
| Accent (cursor, modified dot) | `#f0a030` |

A light theme (like the reference image) is optional: build it only after dark works. Both palettes are values of the same struct.

---

## 4. Custom window decorations

Use `winit` with:

- `with_decorations(false)`
- `with_transparent(true)`
- a minimum inner size of about 480 × 320

The GL config must have an alpha channel: in `glutin`, request `with_alpha_size(8)` and `with_transparency(true)`, and choose a config whose `supports_transparency()` is true. If no such config exists, fall back to an opaque square window and log it. **Never fail to start.**

### 4.1 Rounded corners

- Clear the framebuffer to `(0, 0, 0, 0)`.
- Use **premultiplied alpha** everywhere: shaders output `vec4(rgb * a, a)` and blending is `glBlendFunc(GL_ONE, GL_ONE_MINUS_SRC_ALPHA)`. The compositor expects premultiplied colors, and non-premultiplied output gives dark fringes at the corners.
- **Window mask:** every fragment shader multiplies its output by a coverage value from a rounded-rectangle signed distance function (SDF) of the whole window. Pass `u_window_size` and `u_radius` as uniforms. This clips all content (the sidebar's and the status bar's corners included) to the rounded window shape with no extra passes. When maximized, `u_radius = 0`.
- Draw the 1 px window border as the same SDF, as an outline.
- Do **not** draw a drop shadow in v1.1. It costs screen space on a small screen.

### 4.2 Title bar

- Left side: sidebar toggle button, then the title text `Ludicrous: <file name>` plus the modified dot.
- Right side (or left, per config): minimize, maximize/restore, close. Close turns red on hover.
- The empty area of the title bar is the **drag region**.

### 4.3 Hit testing

One function, `window.rs::hit_test(pos, size, state) -> Hit`, returns exactly one of:

```rust
enum Hit {
    Content,
    TitleDrag,
    Button(WindowButton),          // Minimize | Maximize | Close
    Resize(ResizeDirection),       // 8 edges and corners
}
```

Behavior:

- Press on `TitleDrag` → `window.drag_window()`. Double-click → toggle maximize.
- Press on `Resize(dir)` → `window.drag_resize_window(dir)`.
- Set the cursor icon (`window.set_cursor(...)`) to the matching resize cursor while hovering an edge.
- While maximized or fullscreen, disable all `Resize` hits.
- Buttons call `set_minimized(true)`, `set_maximized(...)`, and the quit path. **Close goes through the quit guard** (and checks every open buffer for unsaved changes).
- Hover state of buttons only triggers a redraw when the hovered item **changes**.

Test on KDE Plasma under **both Wayland and X11** if both are available. If one of them behaves badly, log it and keep the fallback working.

### 4.4 Small-screen helpers

- F11 toggles fullscreen.
- A setting hides the title text and shows only the buttons.
- Sidebar auto-collapse as described in section 3.

---

## 5. Renderer additions

The renderer from v1 grows. The UI still talks **only** to the `Renderer` API, and `render/` still knows nothing about the editor.

```rust
impl Renderer {
    // existing: cell_size, begin_frame, rect, text, flush

    pub fn rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Color);
    pub fn rounded_outline(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, width: f32, color: Color);
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: Color); // axis-aligned only is fine
    pub fn icon(&mut self, x: f32, y: f32, icon: Icon, size: f32, color: Color);
    pub fn text_ui(&mut self, x: f32, y: f32, s: &str, color: Color);   // smaller UI size
    pub fn text_width(&self, s: &str, ui: bool) -> f32;

    pub fn push_clip(&mut self, x: f32, y: f32, w: f32, h: f32);
    pub fn pop_clip(&mut self);
    pub fn layer(&mut self);       // start a new draw layer
    pub fn set_window_shape(&mut self, w: f32, h: f32, radius: f32);
}
```

Implementation notes:

- **One shader for all rectangles.** Plain, rounded, and outlined rects are the same instanced quad: per-instance position, size, radius, border width, color, and clip rect. The fragment shader computes an SDF with `smoothstep` anti-aliasing. A plain rect is `radius = 0`.
- **Clip rect per instance** (a `vec4` attribute, discard or fade outside it) instead of `glScissor`. This keeps one draw call per layer even when the file tree and the editor both clip.
- **Layers.** Draw order matters once panels overlap. Each layer is drawn as *all its rects, then all its text and icons*. Use at most three layers: (1) editor, (2) sidebar and title bar, (3) overlays (settings, messages). Total draw calls per frame should stay under about 8.
- **Two text sizes** from the same font file: `editor_px` (the setting) and `ui_px` (13 by default). Each size has its own atlas region, both built from ASCII at startup, with other glyphs added on demand.
- **Icons:** embed one small permissively-licensed icon font (e.g. Lucide or Phosphor, **check the license**), subset to the ~12 glyphs used, as a second font in the atlas. Needed icons: folder open, folder closed, file, chevron right, chevron down, gear, sidebar toggle, minimize, maximize, restore, close, dot.
- Rounded rectangles are only a few instances per frame. Don't build a path or tessellation system.

---

## 6. Workspace (multiple files)

The `App` no longer owns one `Editor`. It owns a `Workspace`:

```rust
pub struct Workspace {
    pub root: Option<PathBuf>,       // project folder, if one was opened
    pub buffers: Vec<Editor>,        // each Editor keeps its own undo history
    pub active: usize,
    pub tree: FileTree,              // model only, see section 7
}
```

- Keep this in `src/workspace/` (no winit, no GL, same rule as `editor/`).
- `Workspace::any_dirty()` feeds the quit guard.
- Opening a file that is already open switches to it instead of opening a second copy.
- Closing a dirty buffer asks first (reuse the "press again to discard" pattern).
- Shortcuts: **Ctrl+Tab** and **Ctrl+Shift+Tab** cycle buffers, **Ctrl+W** closes the active one.
- Command line: `ludicrous file.cpp` opens a file, and `ludicrous some/dir` opens a project folder. With no argument, open an empty buffer.

---

## 7. Sidebar

### 7.1 Structure (expanded)

```
 OPEN FILES                             section label, small, muted
   ▣ main.cpp                ●          active row = pill; ● = modified
   ▣ util.h
 ───────────────────────────
 FILES                                  scrollable; takes all remaining height
   ▾ src
       main.cpp
       util.h
   ▸ include
 ───────────────────────────
 ⚙ Settings                             footer
 [project card]  folder name            path (muted, clipped), git branch if cheap
```

This follows the reference: rounded pill for the active and hovered row, thin dividers between groups, muted section labels, and a card at the bottom.

**Update (v1.2):** the sidebar has **no header row** and the app name `Ludicrous` appears only in the title bar. The project card shows the folder name (or `No folder open`) and the path. See `LUDICROUS_V1.2_SPEC.md`, section 12.

**Reserve, do not build:** a nav group above "OPEN FILES" for future views (Search, Source Control, Run, Problems). Leave the layout code able to take it. Do not render disabled placeholder items.

### 7.2 Rail mode

Icons only, centered, one per slot: files, settings. Hovering an icon shows nothing yet (no tooltips in v1.1). Clicking the files icon expands the sidebar.

### 7.3 File tree: model (`workspace/filetree.rs`, no GL)

- **Lazy loading:** read a directory only when it is expanded the first time. Never scan the whole project at startup, because the developer is on a spinning disk.
- Sort: directories first, then files, both case-insensitive alphabetical.
- Hide by default: names starting with `.`, plus `target`, `build`, `node_modules`, `__pycache__`. Put the ignore list in one `const`.
- Cap one directory at about 2,000 entries and show `… N more` as the last row rather than freezing on huge folders.
- Directory read errors show a muted `(unreadable)` row, never a panic.
- The model exposes a flat `Vec<Row { depth, name, path, is_dir, expanded }>` for the view to draw. Rebuild this list only when something is expanded or collapsed.

### 7.4 Sidebar view (`ui/sidebar.rs`)

- Draw only the visible rows (same rule as the text area). The tree has its own vertical scroll.
- Hover: highlight the row under the mouse, and redraw only when the hovered row changes.
- Click a file → open it in the workspace. Click a folder → expand or collapse. Click a row in OPEN FILES → switch to it.
- Keyboard (when the sidebar has focus): Up/Down move, Enter opens or toggles, Left/Right collapse or expand, Esc returns to the editor. **Ctrl+Shift+E** focuses the file tree.
- Long names are clipped with `…`.

### 7.5 Focus

Add a small `Focus` enum (`Editor`, `Sidebar`, `Settings`) owned by the app. Keyboard input goes to the focused part only. Clicking a part focuses it.

---

## 8. Settings page

Opens from the sidebar footer or **Ctrl+,**. It **replaces the text area** (it is not a separate window, so no overlap is needed). Esc or Ctrl+, goes back.

Only three kinds of control exist: **toggle**, **stepper** (− value +), and **choice** (a segmented row). All are keyboard-usable: Up/Down select a row, Left/Right change the value.

| Setting | Control | Values | Default |
|---|---|---|---|
| Font size | stepper | 10 to 28 | 15 |
| Tab width | choice | 2 / 4 / 8 | 4 |
| Show line numbers | toggle | on / off | on |
| Cursor blink | toggle | on / off | off |
| Window buttons | choice | left / right | right |
| Corner radius | choice | 0 / 6 / 10 / 14 | 10 |
| Sidebar | choice | auto / expanded / rail / hidden | auto |
| Theme | choice | dark / light | dark |

Changes apply **live** (no Apply button).

### Config file

- Path: `$XDG_CONFIG_HOME/ludicrous/config` (fall back to `~/.config/ludicrous/config`).
- Format: plain `key = value` lines with `#` comments. **Parse it by hand** (about 50 lines). No `serde` or `toml` dependency.
- Unknown keys are ignored. Missing or invalid values fall back to defaults. A broken config must never stop the editor from starting.
- Save with the same **safe save** routine as files (temp file + rename), and only when a setting actually changes.
- Read it once at startup, before creating the window (it is one tiny file).

```
# Ludicrous config
font_size = 15
tab_width = 4
show_line_numbers = true
cursor_blink = false
window_buttons = right
corner_radius = 10
sidebar = auto
theme = dark
```

---

## 9. OpenGL extensions and performance

**Baseline:** OpenGL 3.3 core. **Nothing else is required.** Every extension below is an optional fast path with a fallback.

### 9.1 Capabilities detection

Create `render/caps.rs` with a `GlCaps` struct, filled **once** at startup from the GL version and the extension list (`glGetStringi(GL_EXTENSIONS, i)`). Log what was found and what was chosen. Fast paths are selected from the struct and never by scattering `if` checks through the renderer.

The environment variable **`LUDICROUS_GL_SAFE=1`** turns off every optional path. This is the first thing to try if anything looks wrong. The developer should also confirm what the driver exposes with `glxinfo -B` and `glxinfo | grep -i <name>` before relying on anything, since availability depends on the Mesa version.

### 9.2 Paths

| Feature | Extension or core version | Used for | Fallback |
|---|---|---|---|
| Instanced drawing | core 3.3 (`glDrawArraysInstanced`) | All rects and glyphs: one instance per quad | none, required |
| Persistent mapped buffer (**confirmed available on the developer's machine, primary path**) | `ARB_buffer_storage` (core 4.4) | Streaming per-frame instance data with no copy; use a ring of 3 regions | Buffer orphaning: `glInvalidateBufferData` if `ARB_invalidate_subdata` is present, otherwise `glBufferData(NULL)`, then `glBufferSubData` |
| Immutable atlas storage | `ARB_texture_storage` (core 4.2) | Glyph atlas allocation | `glTexImage2D` |
| Timer queries | `ARB_timer_query` (core 3.3) | GPU frame time in the debug overlay | CPU timing only |
| Debug output | `KHR_debug` / `ARB_debug_output` | GL error messages and object labels, **debug builds only** | none, skip |
| Shader binary cache | `ARB_get_program_binary` (core 4.1) | Skip shader compilation on startup | compile from source |
| Swap with damage | `EGL_KHR_swap_buffers_with_damage` / `EGL_EXT_buffer_age` | Telling the compositor which area changed | normal swap |

Notes on each:

- **Buffer storage ring.** Persistently map one buffer with `GL_MAP_WRITE_BIT | GL_MAP_PERSISTENT_BIT | GL_MAP_COHERENT_BIT`. Split it into 3 regions and write frame N into region `N % 3`. Use a fence (`glFenceSync`) per region so the CPU never writes data the GPU is still reading. If the code is not confident about the synchronization, use the fallback. Correctness beats speed.
- **Shader binary cache.** Mesa already caches compiled shaders on disk, so the gain is small. Do this **last**, and drop it if it adds any complexity. Cache files go in `$XDG_CACHE_HOME/ludicrous/` and are keyed by the GL renderer string and driver version. If loading fails, compile from source.
- **Damage and buffer age.** Treat this as experimental. With a transparent window and premultiplied alpha, partial redraws are easy to get wrong. Ship it only if it is stable, and keep it behind a config key `experimental_damage = false`.
### 9.2b What the developer's driver exposes

The developer reports that `ARB_buffer_storage` and a range of OpenGL 4.3 to 4.6 extensions are available. Rules for using that:

- **Extension names being exposed does not mean the context is a higher version.** The driver may report a 4.2 context and expose later features as individual extensions. Detect each extension **by name** in `GlCaps`, never by assuming "4.5 means everything in 4.5."
- Request a **core profile** context and let the driver give the highest version it offers. Log the version string and the full list of extensions that Ludicrous checks for.
- Keep shaders at `#version 330 core`. If a shader variant needs a later feature, put it behind the matching `#extension ... : require` line and a separate shader file, chosen only when `GlCaps` says it is present. The 330 variant must always exist.

Which extensions are worth using for this workload (a few draw calls, lots of small per-frame data):

1. `ARB_buffer_storage`: the primary streaming path, as in the table above. Implement this first.
2. `ARB_timer_query` and `KHR_debug`: for measuring and for catching GL errors. Use these from the start of the performance phase.
3. `ARB_invalidate_subdata`: use `glInvalidateBufferData` in the fallback path instead of `glBufferData(NULL)`. It is a small change.
4. `ARB_direct_state_access` (core 4.5): optional. It removes bind-to-edit churn and makes the renderer code tidier. Do **not** rewrite working code for it. Use it only in new code, and only if the extension is present, with the classic bind path as the fallback.
5. `ARB_shader_storage_buffer_object` (core 4.3): possible "vertex pulling" (reading instance data from a storage buffer instead of vertex attributes). Skip it in v1.1. Try it only if the F3 overlay shows the instance upload is still a bottleneck after the buffer-storage ring.

**Not worth it here:** multi-draw indirect, compute shaders, bindless textures, and sparse resources. The renderer issues under 8 draw calls per frame, so there is nothing for them to speed up, and they add driver risk on an old iGPU.

The honest expectation: on a UI this small, the biggest wins come from redrawing only on events, batching, and avoiding per-frame allocations. The extensions mostly reduce driver overhead. Use the F3 overlay to confirm each one actually helps, and keep the fallback if it doesn't.

### 9.3 Free performance (no extensions)

- Redraw only on events (already true in v1). Hover changes redraw only when the hovered item changes. Cursor blink is off by default.
- One shader program for rects, one for glyphs. Bind each once per layer.
- Keep the per-frame vertex data in `Vec`s that are reused (`clear()`, never reallocate).
- Only build the layout and instances for **visible** rows (editor lines, tree rows).
- `SwapInterval`: use vsync (1). Do not render faster than the display.

### 9.4 Debug overlay (F3)

A small panel in a corner, drawn with the normal renderer, showing: GPU frame time (if `ARB_timer_query` is active, otherwise CPU frame time), draw calls, instances drawn, which extension paths are active, and the GL renderer string. It exists so performance work is measured, not guessed. It must cost nothing when closed.

---

## 10. Persistence (small)

A second tiny file at `$XDG_STATE_HOME/ludicrous/state` (fall back to `~/.local/state/ludicrous/state`) stores: window width and height, maximized flag, and the last sidebar mode. Restore the size at startup. Do **not** restore the position (Wayland does not allow it). Same `key = value` format and the same safe-save routine. A missing or broken file is ignored.

---

## 11. Shortcuts

| Key | Action |
|---|---|
| Ctrl+B | Toggle sidebar (expanded ↔ hidden; rail if that is the configured mode) |
| Ctrl+Shift+E | Focus the file tree |
| Ctrl+, | Open or close settings |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous open file |
| Ctrl+W | Close active file |
| F11 | Fullscreen |
| F3 | Debug overlay |
| Esc | Return focus to the editor (from the sidebar or settings) |

All earlier shortcuts (Ctrl+S, Ctrl+Q, Ctrl+Z, and so on) keep working.

---

## 12. Module changes

```
src/
  workspace/
    mod.rs
    workspace.rs      # Workspace { root, buffers, active, tree }
    filetree.rs       # lazy directory model (no GL, no winit)
  config.rs           # Config + state: parse by hand, safe save, defaults
  window.rs           # Hit enum, hit_test, drag/resize handling, button state
  ui/
    theme.rs          # palettes + metrics, built from Config
    titlebar.rs
    sidebar.rs
    settings.rs
    widgets.rs        # toggle, stepper, choice, scrollable list
    focus.rs          # Focus enum
    overlay.rs        # F3 debug overlay
  render/
    caps.rs           # GlCaps: version + extension detection
    icons.rs          # icon font glyphs -> atlas
    shaders/
      rect.vert / rect.frag      # rounded-rect SDF + window mask + clip
      text.vert / text.frag      # glyphs + window mask + clip
```

The rules from the architecture file still hold:

1. `editor/` and `workspace/` import no winit, glutin, or glow.
2. `ui/` talks to the `Renderer` only.
3. `render/` knows nothing about the editor or workspace.
4. `input.rs` and `window.rs` are the only places raw window events become commands or hits.

---

## 13. Build order

Do these one at a time. The program must compile, run, and still pass the earlier acceptance checks after every phase.

**Phase A: renderer groundwork**
1. `GlCaps` detection and logging, plus `LUDICROUS_GL_SAFE`.
2. Rounded-rect SDF shader, per-instance clip rect, layers, and the window mask. Test with a demo scene of a few rounded rects before touching the editor.

**Phase B: window**
3. Undecorated, transparent window with rounded corners and a border (opaque fallback when no alpha config exists).
4. Title bar with title text, window buttons, drag, double-click maximize, and edge resize with cursors. Corners become square when maximized.

**Phase C: workspace**
5. Refactor `App` to own a `Workspace` with one buffer. No visible change. Add `any_dirty()` to the quit guard.
6. Open-file switching (Ctrl+Tab), Ctrl+W, and the command-line folder or file argument.

**Phase D: sidebar**
7. Icon font in the atlas, two text sizes, and the theme struct.
8. Sidebar shell: expanded, rail, and hidden modes, auto-collapse, header, footer, and the project card. Hover and active pills.
9. OPEN FILES list.
10. File tree model, then the tree view with lazy expansion, scroll, click to open, and keyboard navigation.

**Phase E: settings**
11. `config.rs`: parse, defaults, safe save.
12. Settings page with the three control types. Apply changes live.

**Phase F: performance**
13. F3 overlay with timer queries.
14. `ARB_buffer_storage` ring with the orphaning fallback. Measure against the fallback using the overlay and keep whichever wins.
15. Optional: shader binary cache, then experimental damage.
16. State persistence (section 10).

---

## 14. Acceptance checklist (test by hand)

- [ ] With no alpha-capable GL config, the window still opens (square, opaque) and logs why.
- [ ] Corners are smooth with no dark fringes against both a light and a dark desktop wallpaper.
- [ ] The title bar drags the window, double-click maximizes, and all 8 edges and corners resize with the right cursor.
- [ ] Maximized: square corners, no border, no resize cursors.
- [ ] Close with an unsaved buffer warns, and so does closing while a **different** buffer is the dirty one.
- [ ] `window_buttons = left` and `right` both lay out correctly.
- [ ] Sidebar: expanded, rail, and hidden all work, and shrinking the window to under 900 px, then under 600 px, auto-collapses it.
- [ ] Opening a folder with 10,000 files does not freeze (lazy loading, entry cap).
- [ ] Unreadable directories show `(unreadable)`.
- [ ] Clicking a file opens it, and clicking it again does not open a duplicate.
- [ ] Every setting changes the UI live, survives a restart, and a hand-broken config file still starts the editor.
- [ ] Idle CPU use is ~0% (no redraw loop, cursor blink off).
- [ ] `LUDICROUS_GL_SAFE=1` runs correctly with every optional path off, and the F3 overlay shows which paths are active.
- [ ] No GL errors in debug builds (`KHR_debug` output is clean).

## 15. Working style for the AI

- One build-order step at a time. Show the files changed and say how to test the step.
- Never require an extension. Detect it, use it if present, and fall back.
- Prefer simple, explicit code. Comment non-obvious Rust and any GL synchronization.
- If something here is ambiguous or turns out wrong on the real machine (for example, transparency on a particular compositor), say so and pick the simplest working option instead of guessing silently.
