# Ludicrous: Architecture & Build Spec

Ludicrous is a fast, minimal code editor written in **Rust** with its **own OpenGL renderer**. It is tailored to one developer's machine and workflow. Read this whole file before writing any code, and follow the build order at the bottom.

## 1. Goals

- **Fast cold start on a slow machine.** The target is an Intel HD 4000 (Ivy Bridge) laptop with Mesa on Linux, with a spinning hard disk. A visible window should appear in under a second.
- **Small and dependency-light.** Few crates, a small binary, one font.
- **Learnable.** The developer is learning C++ and plans to rewrite Ludicrous in C++ later. Keep the code simple, readable, and split into clear modules. Avoid clever abstractions, macros, and heavy generics, and comment the non-obvious parts.
- **Redraw only on change.** No continuous render loop.

## 2. Non-goals for v1

Do not build any of these yet: tabs, a file tree, multi-cursor, selections, search/replace, syntax highlighting, LSP, plugins, or settings UI. A "later" list is allowed, but no code for it.

## 3. Technical constraints

- **Language:** Rust (stable).
- **Graphics:** OpenGL **3.3 core profile** (the HD 4000 on Mesa tops out around 4.2). No Vulkan or wgpu.
- **Window and context:** `winit` + `glutin` + `glutin-winit`. GL function loading with `glow`.
- **Text buffer:** `ropey`.
- **Fonts:** `fontdue` for rasterization. Load **one monospace font file**, rasterize ASCII at startup into a glyph atlas, and add other glyphs on demand later.
- **Crate versions:** check current versions on crates.io. `glutin-winit` must match the `winit` version in use.
- **Optional extension:** `ARB_buffer_storage` for streaming vertex data. Check availability at runtime and fall back to plain `glBufferSubData`. Do not require it.

Release profile in `Cargo.toml`:

```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

## 4. Project layout

```
ludicrous/
  Cargo.toml
  assets/
    font.ttf              # one monospace font
  src/
    main.rs               # parse args, create App, run the event loop
    app.rs                # owns window + renderer + editor; handles winit events
    input.rs              # winit key/mouse event -> Command
    editor/
      mod.rs
      command.rs          # enum Command
      buffer.rs           # wraps ropey::Rope
      cursor.rs           # line/col position and movement
      state.rs            # Editor struct + apply(Command)
    ui/
      mod.rs
      layout.rs           # computes rects: gutter, text area, status bar
      view.rs             # Editor + layout -> renderer calls
    render/
      mod.rs              # Renderer: the only API the UI uses
      gl.rs               # glow context, shader programs, vertex batches
      font.rs             # load font, rasterize ASCII, build glyph atlas
      shaders/
        rect.vert
        rect.frag
        text.vert
        text.frag
```

## 5. Module rules (important)

1. **`editor/` must not import winit, glutin, glow, or anything from `render/` or `ui/`.** It is plain Rust: it receives `Command`s and holds text and cursor state. This keeps it unit-testable and easy to port to C++.
2. **`ui/` talks only to the `Renderer` API.** It never calls GL directly.
3. **`render/` knows nothing about the editor.** It draws rectangles and text it is told to draw.
4. **`input.rs` is the only place that translates raw window events into `Command`s.**

## 6. Data flow

```
winit event
  -> input.rs         (translate to Command)
  -> Editor::apply()  (mutate buffer / cursor / scroll; set dirty = true)
  -> window.request_redraw()
  -> view.draw(&editor, &mut renderer)
  -> renderer.flush() (upload batched vertices, issue 1-2 draw calls)
  -> swap buffers
```

Only redraw on: key press, mouse wheel, window resize, window focus, and cursor blink (if blinking is enabled). Otherwise, wait for events (`ControlFlow::Wait`).

## 7. Renderer API

This is the entire surface the UI may use.

```rust
pub type Color = [f32; 4]; // RGBA, 0.0..=1.0

impl Renderer {
    pub fn new(gl: glow::Context, font_bytes: &[u8], font_px: f32) -> Self;

    /// Monospace cell size in pixels: (width, height).
    pub fn cell_size(&self) -> (f32, f32);

    pub fn begin_frame(&mut self, width: u32, height: u32);
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color);
    pub fn text(&mut self, x: f32, y: f32, s: &str, color: Color);
    pub fn flush(&mut self);
}
```

Implementation notes:

- `rect` and `text` only **append** to CPU-side vertex arrays. `flush` uploads and draws.
- Draw all rectangles in one call and all glyph quads in one call (instancing via `glDrawArraysInstanced`, core in GL 3.3, is preferred).
- Text is monospace, so a glyph's position is `x + column * cell_width`. No shaping is needed for v1.
- Handle HiDPI by multiplying by the window scale factor.

## 8. Editor model

```rust
pub enum Command {
    Insert(char),
    Newline,
    Backspace,
    Delete,
    Left, Right, Up, Down,
    Home, End,
    PageUp, PageDown,
    Scroll(f32),   // mouse wheel, in lines
    Save,
    Quit,
}

pub struct Editor {
    pub buffer: Buffer,      // ropey::Rope + file path
    pub cursor: Cursor,      // line, col (col is a char index; remember desired col for Up/Down)
    pub scroll_y: usize,     // first visible line
    pub dirty: bool,         // unsaved changes
}
```

- `Buffer`: `load(path)`, `save()`, `insert_char(line, col, c)`, `delete_char(...)`, `line(n) -> RopeSlice`, `line_count()`.
- `Cursor` movement clamps to line length. Up and Down keep the "desired column".
- `Editor::apply(Command)` is the only way state changes. Keep scroll adjusted so the cursor is always visible.

## 9. UI layout (v1)

```
+-------+--------------------------------------+
| gutter|            text area                 |
| line  |                                      |
| nums  |                                      |
+-------+--------------------------------------+
| status bar: filename  [modified]   Ln 12, Col 5 |
+------------------------------------------------+
```

- **Gutter:** line numbers, dimmer color, width based on the digit count of the last line.
- **Text area:** only draw lines that are visible (from `scroll_y` for as many rows as fit). Never lay out the whole file.
- **Cursor:** a thin rectangle at `(col * cell_w, row * cell_h)`.
- **Status bar:** one rectangle across the bottom plus text.
- **Theme:** hard-code one dark theme as `const` colors in `ui/view.rs`. Keep it ugly but readable.

## 10. Startup behavior

- File path comes from the command line (`ludicrous main.cpp`). No file picker in v1. With no argument, open an empty buffer.
- Create the window and draw the first frame **before** doing anything slow. Load the font and build the atlas right after the window exists, not before.
- Keep startup free of directory scans or config parsing that could hit the disk.

## 11. Build order

Complete and test each step before starting the next. At each step the program must compile and run.

1. **Window.** `main.rs` and `app.rs`: create the window and GL 3.3 core context, clear to a dark color, redraw only on events, and handle resize and close.
2. **Renderer, rectangles.** Rect shader and batching. Draw a few hard-coded rectangles.
3. **Renderer, text.** Load the font, build the glyph atlas, and draw a hard-coded string with `text()`.
4. **Buffer and view.** Load the file from argv into `ropey`, draw the visible lines, and add scrolling (mouse wheel, PageUp/PageDown).
5. **Editing.** `input.rs`, `Command`, `Editor::apply`, the cursor rectangle, typing, backspace, enter, and arrow keys.
6. **Save.** Ctrl+S writes the file back and clears `dirty`.
7. **Status bar and gutter.**

After step 7, stop. v1 is done. Use it to write real C++ before adding features.

## 12. Later list (do not build yet)

- Syntax highlighting (`tree-sitter`)
- Build/run command with a panel that jumps to compiler error lines
- Selections, copy/paste, undo/redo
- Search
- Tabs and a file tree
- clangd integration

## 13. Working style for the AI

- Make one build-order step at a time, and show the files changed.
- Explain non-obvious Rust choices in short comments. The developer wants to understand the code.
- Prefer simple, explicit code over abstractions.
- If a decision isn't covered here, pick the simplest option and say what you chose.

---

# v1.1: Reliability & Comfort

**Status:** v1 (build-order steps 1 to 7) is done and working: window, text rendering, gutter, cursor, status bar, and a modified flag. v1.1 makes the editor safe to use for real work. The module rules in section 5 and the working style in section 13 still apply.

Build **one item at a time, in this order**. Compile and test after each. Items 1 to 5 are reliability and come first. Items 6 to 8 are comfort.

## Rule for all edits: one gate

Every change to text must go through `Editor::apply(Command)`. Undo/redo, the dirty flag, and cursor fixes all depend on this. Do not mutate `Buffer` from anywhere else (input, UI, or app code).

## 1. Safe save

- Write to a temp file in the **same directory** as the target (e.g. `.name.ludicrous-tmp`), flush and sync it, then `rename` it over the original. A crash mid-save must never corrupt the original file.
- Preserve the original file's permissions where possible.
- `Save` returns a `Result`. On failure, show a message in the status bar (e.g. `Save failed: <reason>`) and keep `dirty = true`. Never fail silently and never panic.
- With no file path (`[No Name]`), Ctrl+S should show "No file name" for now. A save-as prompt is not part of v1.1.

## 2. Quit guard

- If `dirty` is true when the user quits (Ctrl+Q or closing the window), do **not** exit. Show `Unsaved changes: press Ctrl+Q again to discard` in the status bar.
- A second Ctrl+Q within about 3 seconds quits. Any other key cancels the warning.
- If `dirty` is false, quit immediately.
- The window close button follows the same guard.

## 3. Undo / redo

- Ctrl+Z undoes, Ctrl+Shift+Z or Ctrl+Y redoes.
- Store history as **inverse operations**, not full buffer snapshots:

```rust
enum Edit {
    Insert { pos: usize, text: String },  // pos = char index in the rope
    Delete { pos: usize, text: String },  // text = what was removed
}
struct HistoryEntry {
    edits: Vec<Edit>,
    cursor_before: Cursor,
    cursor_after: Cursor,
}
```

- **Group** consecutive typed characters into a single entry. Start a new group on: a newline, a cursor movement, a pause of about 1 second, or a switch between inserting and deleting.
- Any new edit clears the redo stack.
- Undo and redo restore the cursor position.
- Track the "saved" position in the history so `dirty` becomes false again if the user undoes back to the saved state.
- Cap history at a reasonable size (e.g. 1000 entries).

## 4. UTF-8 safety

- The rope is indexed by **char**. The cursor column is a **char index**, never a byte offset. Never slice strings by raw byte positions.
- The glyph atlas only has ASCII at startup. For any character that has no glyph, draw a visible placeholder (a box or `?`). **Never panic** on unknown characters.
- Treat a tab as 4 columns wide for display, or convert Tab to spaces on insert (see item 7). Be consistent.
- Handle `\r\n` line endings when loading. Preserve the original line ending style on save.
- If the file is not valid UTF-8, refuse to open it with a clear message instead of corrupting it.
- Add unit tests in `editor/` for: inserting and deleting multi-byte characters, and moving the cursor across them.

## 5. Long lines

- Add horizontal scrolling: `scroll_x` (in columns), adjusted so the cursor is always visible.
- Clip drawing to the text area. Text must never draw over the gutter or the status bar (use a GL scissor rectangle or clamp the quads).
- Home and End move to the start and end of the line.
- Draw only the visible columns of each line, so a 100,000-character line does not slow rendering.

## 6. Selection + clipboard

- Add `anchor: Option<Cursor>` to `Editor`. The selection is the range between the anchor and the cursor.
- Shift+Arrows, Shift+Home, and Shift+End extend the selection. Any non-shift movement clears it.
- Draw selected text with a highlight rectangle behind it (a `rect()` call before the `text()` call).
- Typing, Backspace, or Delete with a selection replaces or removes it as one undoable step.
- Ctrl+A selects all. Ctrl+C, Ctrl+X, and Ctrl+V use the system clipboard (`arboard` crate, or the simplest option that works on Linux X11 and Wayland). Paste must be one undo step.
- Clipboard failures show a status bar message instead of crashing.

## 7. Auto-indent and Tab

- On Enter, copy the previous line's leading whitespace to the new line.
- After a line ending in `{`, add one extra indent level. This is optional, so keep it simple.
- Tab inserts spaces up to the next multiple of 4. Shift+Tab removes up to one indent level from the start of the line.
- Make the indent width a single `const` so it is easy to change.

## 8. Mouse click to place the cursor

- A left click moves the cursor to the clicked position: `col = (x - text_area_x + scroll_x * cell_w) / cell_w` and `line = scroll_y + (y / cell_h)`, clamped to the buffer and the line length.
- Clicking in the gutter or the status bar does nothing for now.
- Click and drag to select is **not** part of v1.1.

## Acceptance checklist (test by hand after each item)

- [ ] Save, then kill the process during a save of a large file, and the original stays intact.
- [ ] Ctrl+Q with unsaved changes warns, and a second Ctrl+Q quits.
- [ ] Type some text, undo it all, redo it all, and the cursor lands in the right places.
- [ ] Paste text containing `é`, `日本`, and an emoji without a panic.
- [ ] A 5,000-character line scrolls horizontally and stays fast.
- [ ] Select a few lines, cut, paste elsewhere, undo, and the buffer is back to the original.
- [ ] Enter after an indented line keeps the indent.
- [ ] Clicking past the end of a short line puts the cursor at the end of that line.

## Still on the later list

Syntax highlighting, a build/run command with a panel that jumps to compiler errors, search, tabs, a file tree, and clangd integration. Do not start these during v1.1.


---

# Cross-cutting rule: mouse cursor icons

The mouse cursor is **computed from where the pointer is**, never set ad hoc.

- The UI builds a `HitMap` (regions in front-to-back order, each with a rect, a clip rect, a `CursorKind`, and a click target). Clicks, hover highlights, and the cursor all read the **same** map.
- `ui/cursor.rs::resolve(pos, hitmap, window_state)` returns a `CursorKind`: **Default** (arrow) on non-clickable areas, **Pointer** (hand) on clickable items, **Text** (I-beam) only in the editor text area, and resize cursors on window edges.
- `app.rs` maps `CursorKind` to the windowing library's cursor type and calls `set_cursor` **only when the kind changes**.
- Re-resolve on mouse move and also whenever layout changes under a still pointer (scrolling, animations, opening or closing panels).
- Details and acceptance checks are in `LUDICROUS_V1.2_SPEC.md`, section 3b. This rule applies to every new clickable element: if it is clickable, it registers a region.
