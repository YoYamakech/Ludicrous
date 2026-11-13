# Ludicrous v1.2 Spec

Read these first, in this order, and obey them:

1. `LUDICROUS_ARCHITECTURE.md`: module rules (section 5), data flow (section 6), working style (section 13), and the "v1.1: Reliability & Comfort" section.
2. `LUDICROUS_v1.1.md` (the shell spec): window, sidebar, settings, renderer layers, `GlCaps`, config format, and the F3 overlay.

Where this document tightens or corrects something from those files, it says so.

**Status going in:** v1 and the v1.1 shell work: rounded custom window, title bar, sidebar (open files, file tree, settings, project card), settings page, and the GL fast paths. Two known bugs are in sections 3 and 3b.

---

## 0. Hard constraints (unchanged, and they now bind every feature)

| Constraint | Rule |
|---|---|
| Hardware | Intel HD 4000 (Ivy Bridge), Mesa, OpenGL **4.2 core** with individual extensions (see `GlCaps`), spinning HDD, small screen |
| Idle CPU | **0.0%** when nothing is happening. No polling loops, no busy timers |
| Cold start | Time to first visible frame must stay within **+20 ms of v1.1**. Everything new loads lazily (see section 1) |
| Modules | `editor/`, `workspace/`, `lsp/`, `updater/` import **no** `winit`, `glutin`, or `glow`. `ui/` talks only to `Renderer`. `render/` knows nothing about the editor |
| Edits | Every text change goes through `Editor::apply(Command)`. Syntax tree edits, LSP change notifications, snippet state, and undo all hang off that one gate |
| Optional | Every GL extension and every heavy feature has a fallback and can be switched off |

### Build-time budget (the dev machine is the same old laptop)

Heavy dependencies make `cargo build` very slow on that machine. Gate the big features with Cargo features, all on by default, so they can be turned off for fast iteration:

```toml
[features]
default = ["syntax", "lsp", "updater", "dialogs"]
syntax  = ["dep:tree-sitter", ...]   # grammar crates compile C code
lsp     = ["dep:serde", "dep:serde_json", "dep:lsp-types"]
updater = ["dep:ureq", "dep:sha2", ...]
dialogs = ["dep:rfd"]
```

Also set `[profile.dev] opt-level = 1` so debug builds are usable, and keep `lto` and `codegen-units = 1` for release only. Each phase must build with the feature on **and** off.

---

## 1. Cold-start rules for everything new

- The icon font subset, grammars, LSP servers, the file dialog backend, and the update check are all **lazy**. None run before the first frame.
- The update check starts at least **10 seconds** after the first frame, on a background thread.
- LSP servers start only when a file of a supported language is opened (and only if enabled in settings).
- Add a one-line startup timing log, shown in the F3 overlay: time to window, time to first frame. **Record the v1.1 baseline before starting any phase** and compare after each phase.

---

## 2. Cross-cutting plumbing (build this first)

Several features need background threads to wake the UI without polling. Do it once.

```rust
// app.rs
enum UserEvent {
    DialogResult(DialogKind, Vec<PathBuf>),   // from file dialogs
    Lsp(LspEvent),                             // diagnostics, completion results, server status
    Update(UpdateEvent),                       // available / progress / ready / failed
    ParseDone { buffer: BufferId, version: u64 }, // only if highlighting moves to a thread
}
let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
let proxy = event_loop.create_proxy();
```

- Background modules never see `winit`. They take a `Box<dyn Fn(Event) + Send + Sync>` (or a small `Notifier` trait) that `app.rs` builds from the proxy. That keeps the module rule intact.
- Background threads send their data over `std::sync::mpsc`, then call the notifier once. The UI thread drains the channel inside `user_event` and sets `dirty`.
- **No async runtime** (no tokio, no async-std) anywhere. Threads and channels only.
- Rate-limit wake-ups from any stream (download progress, LSP notifications): at most one per 250 ms for progress, and coalesce diagnostics per file.

### Correction: animation and `ControlFlow` (affects section 8)

The feature list said to switch between `ControlFlow::Poll` and `Wait`. **Do not use `Poll`.** It spins the event loop, and on a driver where vsync misbehaves it can pin a core. Use this instead:

- The loop stays on `ControlFlow::Wait` at all times.
- While anything is animating, the redraw handler ends with `window.request_redraw()`. The frame swap (vsync) paces it to the display.
- When the last animation settles, the handler stops requesting redraws. The next thing the loop does is wait, at 0% CPU.
- For things with a delay and no animation (cursor blink, diagnostic hover tooltip delay), use `ControlFlow::WaitUntil(deadline)` with the next deadline, and go back to `Wait` after.

---

## 3. Bug fix: dragging files over the sidebar stalls

**Symptom:** dragging files from the file manager onto the sidebar stalls. The drag only continues once the pointer leaves the sidebar.

I could not inspect the code, so the root cause is **not confirmed**. Likely causes, in order:

1. **Backend mismatch.** As far as I know, `winit` handles external file drag-and-drop on X11 but not on native Wayland. On a KDE Wayland session, a native-Wayland winit window either receives no `HoveredFile` and `DroppedFile` events or refuses the drag, which can look like a stall. **Test first:** run `WINIT_UNIX_BACKEND=x11 ./ludicrous` (the variable name can vary by winit version; check its docs) and repeat the drag. If the stall disappears, this is the cause.
2. **Stale cursor position.** During an external drag, `winit` does **not** deliver `CursorMoved`. If the sidebar's hover or hit-test logic uses the last known cursor position, the sidebar can get stuck in a hover, press, or "drag region" state. For example, a pending `drag_window()` or resize drag may have been started by the last click.
3. **Redraw or layout work triggered per hover event** in the sidebar (rebuilding the file-tree rows or re-clipping text), which blocks the loop during the drag.

**Required fix design (applies regardless of the cause):**

- Treat external drag-and-drop as a **window-level** state. `HoveredFile` sets `drop_hover = true`, and `HoveredFileCancelled` and `DroppedFile` set it to `false`. No region (sidebar or text area) is hit-tested during a drag, because winit gives no position.
- On `HoveredFile`, request **one** redraw (to show the highlight from section 5). On cancel or drop, request one redraw. No other work.
- Clear sidebar hover and pressed state when a drag starts, and also on `CursorLeft` and on window focus loss.
- Never call `drag_window()` or `drag_resize_window()` from a state that began during a file drag.
- If the session is native Wayland and file drops are unsupported, show a one-time status bar hint (`Drag-and-drop needs the X11 backend; use Ctrl+O instead`) and let the app keep working.

**Acceptance:** drag several files across the entire window, including over the sidebar, the title bar, the status bar, and the rounded corners, without stalls. Drop works anywhere. Cancelling with Esc clears the highlight. Idle CPU stays 0% before, during, and after.


---

## 3b. Mouse cursor icons (bug fix, and a rule for all UI)

**Symptom:** hovering the sidebar shows the **text (I-beam) cursor**, including over areas that do nothing. The wanted behavior:

| Where the pointer is | Cursor |
|---|---|
| Non-clickable area (sidebar background, labels, dividers, header) | **Default arrow** |
| Clickable item (sidebar rows, buttons, controls) | **Pointer (hand)** |
| Editor text area | Text (I-beam) |

**Likely cause:** the text cursor is set when the pointer is over the editor and is never reset when the pointer moves to another region, because cursor changes are scattered or only happen on entering the editor. The fix is to make the cursor a **computed result of where the pointer is**, decided in one place.

### Design

- `ui/cursor.rs` owns a `CursorKind` enum and one pure function. `ui/` must not import `winit`, so `app.rs` maps `CursorKind` to `winit`'s `CursorIcon`. (In winit 0.30 this is `window.set_cursor(...)`; older versions use `set_cursor_icon`. Check the version in use.)

```rust
pub enum CursorKind {
    Default, Pointer, Text,
    ResizeN, ResizeS, ResizeE, ResizeW, ResizeNE, ResizeNW, ResizeSE, ResizeSW,
}
pub fn resolve(pos: (f32, f32), hits: &HitMap, window: &WindowState) -> CursorKind;
```

- **One hit map, one source of truth.** Layout code builds a `HitMap` in `ui/hitmap.rs`: a list of regions, front-most first.

```rust
pub struct HitRegion {
    pub rect: Rect,
    pub clip: Rect,            // a region that is scrolled out of view or covered must not be hit
    pub cursor: CursorKind,
    pub target: HitTarget,     // what a click does, and what the hover pill belongs to
}
```

  The **same** map drives click handling, hover highlights, and the cursor. Anything drawn as clickable registers as clickable, so a row can never show a hover pill but a text cursor. The map is rebuilt only when layout changes, reusing its `Vec`, not on every mouse move.

### Region priority (first match wins)

1. **Window resize edges and corners** (not when maximized or fullscreen): the matching resize cursor.
2. **Overlays:** completion popup rows are Pointer, the tooltip is Default.
3. **Title bar:** window buttons and the sidebar toggle are Pointer, the drag area is Default.
4. **Sidebar:** open-file rows, file-tree rows, the Settings row, and the project card are Pointer. Everything else in the sidebar (header, section labels, dividers, empty space) is Default.
5. **Settings page:** toggles, stepper buttons, choice segments, and action rows are Pointer. Labels are Default.
6. **Editor text area:** Text. The gutter is Default.
7. **Status bar:** clickable items (the update badge, diagnostic counters, once they act on click) are Pointer. The rest is Default.
8. **Fallback:** Default.

Disabled or inert items count as non-clickable (Default).

### When to re-resolve

- On `CursorMoved`, `CursorEntered`, and window focus gained.
- **Without any mouse movement**, using the last known pointer position, whenever layout changes under a still pointer: sidebar expand, collapse, or animation frames, opening or closing the settings page or the completion popup, expanding or collapsing a tree folder, a window resize, and wheel scrolling of the tree or the editor.
- During a text-selection drag that started in the editor, keep Text even if the pointer leaves the text area. After the button is released, re-resolve.
- During a window move or resize started by the title bar or an edge, do not touch the cursor. After it ends, re-resolve.
- During an **external file drag**, the OS owns the cursor. Do not set it.

### Efficiency

Keep `current_cursor` in `App` and call `set_cursor` **only when the resolved kind differs** from it. Hit-testing is a linear scan over the visible regions, which only runs on mouse events and layout changes, so idle CPU stays 0%.

**Acceptance:**

- [ ] Sidebar background, labels, header, and dividers show the arrow.
- [ ] Every clickable sidebar row, button, and control shows the hand.
- [ ] Editor text shows the I-beam. The gutter and status bar show the arrow.
- [ ] Moving quickly from the editor into the sidebar and back changes the cursor immediately each time.
- [ ] Scrolling the file tree with the wheel while the pointer is still updates the cursor as rows move under it.
- [ ] The cursor updates correctly during and after the sidebar expand and collapse animation, and after opening and closing settings.
- [ ] Window edges show resize cursors and maximize removes them.
- [ ] A debug counter confirms `set_cursor` is called only when the kind changes.

---

## 4. Feature 1: Nerd Font file icons

### Corrections to the feature list

- **Do not embed the full `SymbolsNerdFontMono-Regular.ttf`.** It is multiple megabytes. Parsing it at startup costs time and memory, and the binary pages it in from the HDD. **Subset it at dev time** to only the glyphs used (a few KB), the same way the Lucide font was subset. Keep the subsetting command in `assets/README.md` (e.g. with `pyftsubset` from `fonttools`).
- **Check the license** of each glyph set included in the Nerd Fonts symbols file and ship the required notice with the project.
- **Do not guess codepoints.** Take them from the official Nerd Fonts cheat sheet, and put all of them in **one** `const` table. A build-time or startup test must check that every table entry exists in the subset (`font.lookup_glyph_index(ch) != 0`), and fall back to the default file glyph if one is missing.

### Font stack

| Role | Font | Used for |
|---|---|---|
| Primary | Existing monospace | Editor text and UI text |
| UI icons | Existing Lucide subset | Chrome: chevrons, gear, window buttons, sidebar toggle, download badge, diagnostics |
| File icons | Nerd Font Symbols Mono (subset) | File-type icons in the tree and open-files list. Also the fallback for private-use-area characters inside text |

Glyph lookup order for text: primary → Nerd symbols → placeholder box (as in v1.1's UTF-8 rule). Rasterize on demand into the atlas, as for other non-ASCII glyphs.

### Mapping (`ui/icons_map.rs`)

```rust
pub struct FileIcon { pub glyph: char, pub accent: Color }
pub fn icon_for(path: &Path, is_dir: bool, expanded: bool) -> FileIcon
```

Cover: `.rs`, `.c`, `.cpp` (also `.cc`, `.cxx`, `.hpp`), `.h`, `.py`, `.js`, `.ts`, `.json`, `.toml`, `.md`, `.sh`, folders (closed and open), and a default file icon. Match by lowercase extension, with a small exact-name table for well-known files (`Cargo.toml`, `Makefile`, `CMakeLists.txt`).

Subtle accent colors (suggestions, all in `theme.rs`, tuned to sit well on the dark palette):

| Type | Accent |
|---|---|
| Rust | `#dea584` |
| C | `#6e9ccf` |
| C++ | `#5b8fd0` |
| Python | `#4b8bbe` |
| JavaScript | `#e0c95a` |
| TypeScript | `#3b82c4` |
| JSON | `#c9c75a` |
| TOML | `#b8603a` |
| Markdown | `#519aba` |
| Shell | `#7cc65c` |
| Folder / default | muted text color |

Rule: the icon takes the accent color, and the file name stays the normal text color. Selected and hovered rows keep the accent.

**Acceptance:** the tree and the open-files list show the right icons. An unknown extension shows the default icon. Binary size grows by well under 100 KB.

---

## 5. Features 2 and 3: file and folder opener, drag-and-drop

### 5.1 Native dialogs (`rfd`)

**Corrections to the feature list:**

- `rfd` defaults to a GTK3 backend on some versions, which is heavy to link and slow to start. Configure it with **`default-features = false`** and the **XDG desktop portal** backend (the feature name depends on the version, so check the crate docs for the version in use). KDE provides a portal backend, so this stays lightweight.
- **Never block the event loop.** Run the dialog on a short-lived thread (use the async API with `pollster`, or the blocking API inside the thread). Send the result back as `UserEvent::DialogResult`.
- **Lazy init:** nothing portal-related happens until the user asks for a dialog.
- **Portal missing or fails:** do not crash and do not hang. Show `No file dialog available. Start with: ludicrous <path>` in the status bar.
- Only one dialog at a time. A second request while one is open is ignored.

**Triggers:**

| Action | Trigger |
|---|---|
| Open file(s) | Ctrl+O |
| Open folder | Ctrl+Shift+O, or clicking the project card in the sidebar |

New code goes in `workspace/dialogs.rs` (a thin wrapper) and the open logic stays in `Workspace`:

```rust
impl Workspace {
    pub fn open_path(&mut self, path: &Path) -> OpenResult;   // file -> buffer, dir -> set root
}
```

Dialogs, drag-and-drop, the command line, and file-tree clicks **all** call `open_path`. Rules inside it:

- A file that is already open just becomes the active buffer.
- A directory sets the workspace root and rebuilds the tree lazily (no full scan). Open buffers are **not** closed.
- Refuse files larger than a const cap (suggest 32 MB), non-UTF-8 files, and unreadable paths with a clear status bar message. Never panic.
- Opening several files at once opens them in order and activates the last one.

### 5.2 Drag-and-drop

Handle `WindowEvent::HoveredFile`, `HoveredFileCancelled`, and `DroppedFile`, following the design in section 3. Details:

- `winit` sends one event **per file**. Collect `DroppedFile` events that arrive in the same event-loop iteration (or within about 50 ms) and process them as one batch, so 5 dropped files do not cause 5 redraws and 5 activations.
- Check `path.is_dir()` to decide between "open file" and "switch root". If a batch contains directories, use the first directory as the new root and open any files too.
- **Highlight:** while `drop_hover` is true, draw a 2 px accent-colored rounded outline around the whole window (same SDF outline primitive as the window border), plus an optional dimmed overlay with `Drop to open`. Remove it immediately on cancel or drop.

**Acceptance:** Ctrl+O and Ctrl+Shift+O work on KDE. With the portal unavailable, the status bar message appears and the editor keeps working. Dropping a file opens it, dropping a folder changes the root, and dropping a mix works. The highlight appears and disappears correctly. Idle CPU is 0%.

---

## 6. Feature 4: syntax highlighting (`tree-sitter`)

Lives in `editor/highlight.rs`. No `winit` or `glow`.

### Design

```rust
pub struct Highlighter { /* Parser, Option<Tree>, Language, Query, version */ }

pub struct Span { pub start_col: u32, pub end_col: u32, pub kind: TokenKind }

impl Highlighter {
    pub fn new(lang: Lang) -> Option<Self>;
    pub fn edit(&mut self, e: &InputEdit);       // called from Editor::apply
    pub fn reparse(&mut self, buffer: &Buffer);  // incremental
    pub fn spans_for_lines(&mut self, buffer: &Buffer, first: usize, last: usize) -> Vec<Vec<Span>>;
}
```

- **Languages:** Rust, C, C++, Python, JSON, TOML, Markdown. Pick the language by extension.
- **Markdown:** `tree-sitter-md` has separate block and inline grammars. For v1.2, highlight the **block** grammar only (headings, code fences, list markers, quotes). Inline emphasis and injected languages in code fences are out of scope.
- **Crate versions:** `tree-sitter` and each grammar crate must be compatible versions, and the language constant API changes between versions. Follow each grammar crate's own README for the current API and pin exact versions in `Cargo.toml`.
- **Lazy per language:** create the `Language`, `Parser`, and `Query` the first time a file of that type is opened, not at startup.
- **Incremental edits:** `Editor::apply` builds a `tree_sitter::InputEdit` for every change (start byte, old end byte, new end byte, and the matching row and column points) and calls `Highlighter::edit`. `ropey` is char-indexed and tree-sitter is **byte**-indexed, so convert with `char_to_byte`. Write unit tests for multi-byte characters.
- **Query only what is visible:** run the highlights query with `QueryCursor::set_byte_range` over the visible lines only (plus a small margin). Never run it on the whole file per frame.
- **Cache:** reuse the spans until the buffer version or the visible range changes. A cursor blink or hover redraw must not re-run the query.
- **Large files:** above a const size (suggest 2 MB) skip highlighting, show `Highlighting off (large file)` in the status bar, and keep editing fast. Initial parses above a smaller size (suggest 256 KB) happen on a background thread, delivering `ParseDone` through the proxy.
- **Mapping:** map capture names (`keyword`, `string`, `comment`, `function`, `type`, `number`, `constant`, `operator`, `punctuation`, `variable`, `property`, ...) to a small `TokenKind` enum with a safe default for unknown captures. Colors come from the theme palette in `ui/theme.rs`, never from `editor/`.
- **Rendering:** `ui/view.rs` draws each visible line as runs of text, one `text()` call per run, same color within a run. All runs go into the same glyph batch, so there is no extra draw call.

**Acceptance:** the 7 languages highlight correctly. Typing stays instant in a 5,000-line file. A file over the large-file cap opens without highlighting and without freezing. No new draw calls per frame. Idle CPU is 0%.

---

## 7. Feature 5: LSP client, diagnostics, autocomplete, snippets

This is the biggest feature. Build it in **two phases** (F: protocol and diagnostics, G: completion and snippets). Everything is behind the `lsp` Cargo feature and the `lsp` setting.

### 7.1 Architecture (`src/lsp/`, no `winit`)

```
lsp/
  mod.rs
  client.rs        # spawn the server process, reader + writer threads, request ids
  protocol.rs      # "Content-Length" framing, serde_json glue
  position.rs      # offset conversion (see 7.2)
  diagnostics.rs   # per-file diagnostic store (a model)
  completion.rs    # completion items, fuzzy filter, isIncomplete handling
```

- One child process per language, spoken to over stdio. One reader thread parses messages, one writer thread sends them. Both use `std::sync::mpsc`. After handling a message, the reader calls the notifier so the UI thread wakes. **No async runtime.**
- `lsp-types` and `serde_json` are allowed here (and in `updater/`) only. The config parser stays hand-written.
- **Servers are configured, not hard-coded:**

```
lsp = true
lsp_rust = rust-analyzer
lsp_c = clangd
lsp_cpp = clangd
lsp_python = pyright-langserver --stdio
```

- **Lazy start:** a server starts only when the first file of its language opens. If the command is not on `PATH`, show one status bar message (`clangd not found`), mark that language as unavailable for the session, and do not retry in a loop.
- **Lifecycle:** `initialize` (with the workspace root as `rootUri`, or null for a loose file), `initialized`, then `textDocument/didOpen`, incremental `didChange` (with version numbers, debounced about 50 ms), `didSave`, and `didClose`. On quit, send `shutdown` and `exit`, then kill the child after a short timeout. Never leave zombie processes. If a server crashes, show `Language server stopped` and restart it **once** with a delay. After a second crash, stop trying.
- **Resource honesty:** the "0% idle CPU" rule applies to **Ludicrous**, not to the language servers. `rust-analyzer` can use a lot of RAM and CPU while indexing, which matters on an old laptop. That is why the `lsp` toggle exists, and why servers are lazy.
- `clangd` note: for a single loose `.cpp` file, `clangd` works with default flags. For a project, it wants a `compile_commands.json` (from CMake, or Bear for plain Makefiles). Mention this in a status or docs note; do not try to generate it.

### 7.2 Position encoding (a known bug trap)

LSP positions are **UTF-16 code units** by default. The editor uses **char indexes** (ropey) and tree-sitter uses **bytes**. In `initialize`, request `positionEncodings: ["utf-8", "utf-16"]` and use whatever the server selects. All conversion lives in `lsp/position.rs` with unit tests for multi-byte characters and emoji. No other code converts positions.

### 7.3 Phase F: diagnostics

- `publishDiagnostics` is stored per file in a `diagnostics.rs` model (no GL). Replace a file's list wholesale on each publish. Clear it on close.
- **Squiggles:** add a **squiggle style** to the existing rect shader (an extra per-instance attribute selecting a wavy underline), so there is no new program and no extra draw call. Renderer method: `squiggle(x, y, w, color)`. Red for errors, yellow for warnings.
- **Gutter markers:** a small colored bar or dot at the left of the line number for the worst severity on that line.
- **Status bar:** error and warning counts using Lucide icons (`circle-x`, `triangle-alert`), e.g. `✖ 2  ⚠ 5`, for the active file.
- **Hover message:** when the mouse rests on a diagnostic range for about 300 ms, show a small tooltip in the overlay layer. Use `WaitUntil` for the delay (section 2), and cancel it when the mouse moves. Clip the tooltip to the window. The message text is clipped to a const width with `…` and shows the full message for the first line only.
- Diagnostic ranges must shift correctly while the user types before the server republishes. Apply simple range adjustment on edits in `Editor::apply`, or drop squiggles for the edited line until the next publish. Choose the simpler option and say which.

**Acceptance:** opening a C++ file with a deliberate error (e.g. `x + y = value;`) shows a squiggle, a gutter marker, and a status bar count within about a second of `clangd` starting. Hover shows the message. Fixing the error clears it. Missing server: no crash, one status message. Quit leaves no stray server processes.

### 7.4 Phase G: autocomplete and snippets

**Triggering** (correction: use the server's own trigger characters, not hard-coded ones):

- Read `completionProvider.triggerCharacters` from the server's capabilities. Servers declare their own (usually `.`, and `:` or `>` for `::` and `->`). Trigger the request when one of them is typed.
- **Ctrl+Space** requests completion manually.
- Re-request while typing a word only when the server returned `isIncomplete = true`; otherwise filter the existing list on the client.

**Popup** (`ui/completion_popup.rs`, overlay layer):

- Shows up to 10 rows, each with a kind icon (a few Lucide glyphs mapped from `CompletionItemKind`), the label, and a muted detail text. Scrolls if more.
- Placed under the cursor, flipped above it when there is no room below. Clipped to the window.
- **Fuzzy filtering** on the client: a simple subsequence scorer that favors prefix and word-boundary matches, with stable ordering. Keep it in `lsp/completion.rs` (no GL) and unit test it.
- **Keys while open:** Up and Down move, **Tab or Enter accept**, Esc closes, any non-word character closes it (and may accept if the server marks it a commit character; skip commit characters in v1.2).
- Accepting an item applies `textEdit` (or the label), plus any `additionalTextEdits` (for example auto-imports), as **one undo step**.

**Snippets** (`editor/snippet.rs`, no GL):

- Support items with `insertTextFormat = Snippet`. Parse a **subset**: tabstops `$1`, placeholders `${1:text}`, the final stop `$0`, and escaped `\$`. Choices, regex transforms, and variables fall back to inserting the plain default text, with no crash.
- After insertion, enter a **snippet session**: the first tab stop is selected (placeholder text selected so typing replaces it). The session tracks tab stop ranges and adjusts them on edits inside the session.
- **Tab** moves to the next stop, **Shift+Tab** to the previous. After `$0`, or on Esc, or when the cursor leaves all stops, the session ends.
- **Key priority for Tab**, in order: completion popup accept → next snippet stop → normal indent.
- The whole snippet insertion (with its selections) is one undo step.

**Acceptance:** in a C++ file, typing `std::` offers completions, `Tab` accepts, and a function-call snippet puts the cursor on the first parameter with Tab moving through the rest. Undo reverses the whole insertion in one step. Typing continues to feel instant while the popup is open. No CPU use when the popup is idle.

---

## 8. Feature 6: UI animations

Uses the corrected loop from section 2. New file `ui/anim.rs` (pure Rust, no GL).

```rust
pub struct Anim { value: f32, target: f32 }
impl Anim {
    pub fn set_target(&mut self, t: f32);
    pub fn step(&mut self, dt: f32, rate: f32) -> bool;   // returns "still moving"
}
// frame-rate independent exponential approach:
// value += (target - value) * (1.0 - (-rate * dt).exp());
// snap to target and report finished when |target - value| < 0.1 px (or 0.003 for 0..1 values)
```

- `dt` comes from `Instant`, **capped at 50 ms** so a stalled frame does not jump.
- The app keeps `animating = anims.iter().any(moving)`. If true, the redraw handler requests another redraw. If false, it does not.
- **What animates** (each is optional via `animations = true | false`, and off means instant):
  1. **Sidebar width** between expanded, rail, and hidden (about 150 ms).
  2. **Smooth scrolling:** keep `scroll_target` and a fractional `scroll_pos`. The text area draws with a fractional vertical offset (round to whole pixels when drawing glyphs, to avoid blurry text), so draw one extra line above and below. The tree and the completion list use the same mechanism.
  3. **Hover fades:** animate only the currently hovered row and the one just left, not a state per row.
- Layout and clipping are recomputed each frame while animating, which is cheap at this size.
- Add a Settings toggle `Animations`. The F3 overlay shows `animating: yes/no`.

**Acceptance:** `top -H -p $(pgrep ludicrous)` shows **0.0%** while idle, including right after an animation ends. During an animation, frame time stays within vsync and CPU use is low. With `animations = false`, everything is instant and still correct.

---

## 9. Feature 7: GitHub auto-updater (`src/updater/`, no `winit`)

Behind the `updater` Cargo feature and the `check_updates` setting. Disabled automatically in debug builds, and when the executable path is inside a `target/` directory.

### Corrections and additions to the feature list

- **Integrity, not just transport.** Replacing your own executable with something downloaded is risky. The minimum is: HTTPS only, restricted to GitHub hosts, and a **SHA-256 checksum** published in the same release (`SHA256SUMS`), verified with the `sha2` crate. **If the checksum is missing or does not match, discard the download and fail closed.** Strongly recommended: also sign the release with an ed25519 key (for example `minisign`) and embed the **public key** in the binary, because a checksum hosted next to the binary does not help if the release itself is replaced.
- **Package-managed installs.** If the directory holding the executable is not writable (for example `/usr/bin`), do not self-update. Instead offer `Open release page`, which runs `xdg-open <url>`.
- **HTTP client:** `ureq` (blocking) on a background thread. It avoids tokio and reqwest. It is still the heaviest new dependency, which is why it is feature-gated.

### Flow

1. **Check** (background thread, at least 10 s after the first frame, at most once per 24 h; the last check time lives in the state file from v1.1): `GET https://api.github.com/repos/<owner>/<repo>/releases/latest` with a `User-Agent: ludicrous/<version>` header (GitHub requires one) and short timeouts (connect 10 s, total 30 s). Constant: `UPDATE_REPO = "YoYamakech/Ludicrous"` (**confirm this value**). Compare `tag_name` (strip a leading `v`) with `env!("CARGO_PKG_VERSION")` using semver. Silently ignore any parse or network failure. Never show an error for a failed background check.
2. **Notify:** when a newer version exists, send `UpdateEvent::Available { version, notes_url }` through the proxy. The UI shows a small Lucide `download` icon badge in the status bar and a row in Settings (`Update available: v1.2.1 · Download and install`). A manual `Check now` row in Settings forces a check.
3. **Download** (only on the user's click): pick the asset by name convention (see below), stream it to a temp file **in the same directory as the executable** (`.ludicrous.new`), with a size cap (suggest 100 MB) and timeouts. Report progress to the UI at most 4 times per second. Cancel cleanly if the user quits.
4. **Verify:** SHA-256 against `SHA256SUMS` (and the signature, if implemented). On failure, delete the temp file and show `Update failed: checksum mismatch`.
5. **Swap:** set the executable bit, `fsync`, then `rename` the temp file over `std::env::current_exe()`. On Linux, renaming over a running binary is safe, because the running process keeps the old file. Keep one copy of the previous binary as a backup in `$XDG_STATE_HOME/ludicrous/ludicrous.prev`.
6. **Restart:** show `Updated to v1.2.1. Restart to apply  [Restart] [Later]`. **Restart goes through the quit guard** (unsaved buffers are checked), then spawns the new executable with the same arguments and exits. Never restart automatically.

### Release convention (needed for the updater to work)

- Tag releases `vMAJOR.MINOR.PATCH`.
- Attach one asset per platform named `ludicrous-<version>-x86_64-linux` (a raw binary, or a `.tar.gz` if the AI prefers), plus `SHA256SUMS`.
- Build on an environment with an **older glibc** than the target machines, so the binary runs on the developer's older distro. A GitHub Actions workflow that builds with the release profile and uploads the assets is a good follow-up task, but not part of this phase.

### Privacy

The only request is to GitHub. It sends no identifying data beyond the standard `User-Agent`. Say this in Settings next to the toggle.

**Acceptance:** with a newer fake release in a test repo, the badge appears, download and verify work, a corrupted download is rejected, the swap leaves a working executable, and restart goes through the quit guard. In a read-only install directory, only `Open release page` is offered. With the network off, nothing is shown and nothing blocks. Idle CPU stays 0%.

---

## 10. Settings and config additions

New rows on the settings page (the v1.1 control types are toggle, stepper, choice; add one more, an **action row** for buttons like `Check now`):

| Setting | Control | Default |
|---|---|---|
| Animations | toggle | on |
| Language servers | toggle | on |
| Syntax highlighting | toggle | on |
| Check for updates | toggle | on |
| Check now | action | n/a |

Config keys added: `animations`, `lsp`, `syntax`, `check_updates`, `lsp_rust`, `lsp_c`, `lsp_cpp`, `lsp_python`. The server commands are config-file only, not shown in the settings page. State file: add `last_update_check` (a Unix timestamp).

## 11. Shortcuts added

| Key | Action |
|---|---|
| Ctrl+O | Open file(s) |
| Ctrl+Shift+O | Open folder |
| Ctrl+Space | Request completion |
| Tab / Shift+Tab | Next / previous snippet stop (after popup accept, before indent) |

---

## 12. UI cleanup (required) and polish (optional)

### Required: the name `Ludicrous` appears once

The app name shows **once** in the UI, in the **title bar** (the custom window decoration). It currently also shows in the sidebar header and in the project card. This section **overrides** the sidebar structure in `LUDICROUS_v1.1.md`, section 7.1.

- **Title bar:** unchanged. Keep its text exactly as it is now.
- **Remove the sidebar header row** (the orange mark and the app name). The sidebar starts directly with the OPEN FILES section. This also gives back about 36 px of vertical space, which matters on a small screen.
- **Project card:** the first line is the project folder name, or `No folder open` when there is none. The second line is the path (muted, clipped from the **left** with `…` so the end of the path stays visible) or the git branch if that is cheap to get. With no folder open, the second line is the muted hint `Ctrl+Shift+O to open one`. The card never shows the app name.
- **Rail mode** and the **settings page** do not show the app name either.
- The app name appears in no other widget text or tooltip. Setting the OS-level window title with `window.set_title("Ludicrous")` is fine, since it only shows in task switchers.
- **Check:** searching `ui/` for the string literal `"Ludicrous"` finds exactly one UI use, in the title bar code.

### Optional polish (only if time allows, after the phases below)

- Show a muted hint in the empty FILES section: `Ctrl+Shift+O to open a folder`.
- Give the Settings row in the sidebar the same hover pill as the other rows.

## 13. Module changes

```
src/
  app.rs                      # + UserEvent, drop/hover state
  editor/
    highlight.rs              # tree-sitter wrapper
    snippet.rs                # snippet parser + tab-stop session
  lsp/                        # client, protocol, position, diagnostics, completion
  updater/                    # check, download, verify, swap
  workspace/
    dialogs.rs                # rfd wrapper
  ui/
    anim.rs
    cursor.rs                 # CursorKind + resolve(pos, hitmap, state)
    hitmap.rs                 # HitRegion list built by layout: clicks, hover, cursor
    icons_map.rs              # extension -> glyph + accent
    completion_popup.rs
    diagnostics_view.rs       # squiggles, gutter markers, tooltip
  render/
    font.rs                   # fallback chain: primary -> Nerd symbols -> placeholder
    shaders/rect.*            # + squiggle style attribute
```

## 14. Build order

One phase at a time. After every phase: builds with features on and off, v1.1 acceptance checks still pass, idle CPU is 0%, and the cold-start numbers are logged and compared to baseline.

0. **Baseline and bug.** Record v1.1 cold-start timing. Add Cargo features. Add `UserEvent` plumbing. Fix the sidebar drag stall (section 3). Fix the wrong mouse cursors with the `HitMap` and `resolve` design (section 3b). Apply the single-name UI cleanup (section 12).
1. **Icons.** Subset the Nerd font, the fallback chain, the mapping table, and the tree and open-files rendering (section 4).
2. **Open paths.** `Workspace::open_path`, then dialogs (Ctrl+O, Ctrl+Shift+O, project card click), then drag-and-drop with the highlight (section 5).
3. **Animations.** `anim.rs`, the corrected redraw loop, then the sidebar, scrolling, and hover fades (section 8). This comes before the LSP popups because they reuse the scrolling code.
4. **Syntax highlighting.** Rust and C/C++ first, then Python, JSON, TOML, Markdown (section 6).
5. **Updater** (section 9). It is independent and smaller than LSP, so finish it first.
6. **LSP phase F.** Process, protocol, position encoding, diagnostics, squiggles, status bar counters, hover (section 7.1 to 7.3).
7. **LSP phase G.** Completion popup, fuzzy filter, snippets (section 7.4).

*Why this order:* it follows the earlier list but delivers visible wins first, puts the animation changes before the code that needs them, and leaves the riskiest piece (LSP) for last, so a problem there never blocks the rest.

## 15. Acceptance checklist (whole release)

- [ ] Dragging files across the whole window, sidebar included, never stalls. Drop works anywhere.
- [ ] Mouse cursor is correct everywhere: arrow on non-clickable areas, hand on clickable items, I-beam in editor text, resize cursors on edges (section 3b).
- [ ] `Ludicrous` appears once in the UI (the title bar). The sidebar has no header row, and the project card shows the folder name or `No folder open` (section 12).
- [ ] Icons correct for all listed types. Unknown types show the default. A missing glyph never crashes.
- [ ] Ctrl+O and Ctrl+Shift+O work. With no portal, a status message appears and nothing hangs.
- [ ] Drop file, drop folder, and drop a mix all behave correctly.
- [ ] Highlighting works for all 7 languages and is skipped gracefully for huge files.
- [ ] clangd diagnostics show squiggles, markers, counters, and hover text. Autocomplete and snippets work with one undo step per insertion.
- [ ] A missing language server produces one message and no retry loop. Quit leaves no server processes behind.
- [ ] Animations settle, and idle CPU is 0.0% afterwards. `animations = false` works.
- [ ] Updater: detects, downloads, verifies, swaps, and restarts through the quit guard. A bad checksum is rejected. A read-only install offers only the release page.
- [ ] Cold start is within +20 ms of the v1.1 baseline with every feature compiled in.
- [ ] `LUDICROUS_GL_SAFE=1` still works, and the F3 overlay shows active paths, `animating`, and startup timings.

## 16. Still on the later list

Search and replace, tabs, source control panel, build and run panel with error jumping, go-to-definition and rename, formatting, code actions, signature help, multi-cursor, and the other languages.

## 17. Working style for the AI

- One phase at a time. Show the files changed and say how to test the phase.
- Everything new is lazy and optional. If something threatens the 0% idle or cold-start rules, stop and report it instead of working around it.
- Prefer simple, explicit code. Comment non-obvious Rust, GL synchronization, and every encoding conversion.
- If this document is wrong or ambiguous on the real machine (for example, the `winit` backend, the `rfd` portal feature name, or a grammar crate's API), say so and pick the simplest working option instead of guessing silently.
