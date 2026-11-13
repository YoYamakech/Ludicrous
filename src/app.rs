// app.rs owns the window, the GL context and the editor.
// It is the only module that receives winit callbacks; it forwards key input
// to input.rs which returns Commands, and asks ui/view.rs to draw.

use std::num::NonZeroU32;
use std::path::PathBuf;

use glutin::config::{ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::DisplayBuilder;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::ModifiersState;
use winit::raw_window_handle::HasWindowHandle;
use winit::window::{CursorIcon, Fullscreen, Window, WindowId};

use crate::config::{Buttons, Config, SidebarMode, WindowState};
use crate::editor::{Command, Control};
use crate::input;
use crate::render::Renderer;
use crate::ui::focus::Focus;
use crate::ui::layout::Layout;
use crate::ui::settings;
use crate::ui::sidebar::{self, SidebarState};
use crate::ui::theme;
use crate::ui::theme::Palette;
use crate::ui::view;
use crate::window::{self, Hit, HitState, ResizeDirection, WindowButton};
use crate::workspace::{self, Workspace};

// The font is compiled into the binary: no file lookup at startup, no missing
// font on a fresh machine.
const FONT: &[u8] = include_bytes!("../assets/font.ttf");
const ICONS: &[u8] = include_bytes!("../assets/icons.ttf");

struct GlState {
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    renderer: Renderer,
}

pub struct App {
    path: Option<PathBuf>,
    window: Option<Window>,
    gl: Option<GlState>,
    workspace: Option<Workspace>,
    mods: ModifiersState,
    /// Created on first use: opening the X11/Wayland clipboard at startup
    /// would slow cold start and can fail on a headless machine.
    clipboard: Option<arboard::Clipboard>,
    /// Last mouse position in physical pixels, for click-to-place-cursor.
    cursor_pos: Option<PhysicalPosition<f64>>,
    /// What the mouse is over, so hover feedback only redraws on change.
    hover: Option<Hit>,
    /// Sidebar mode. `None` means "auto" (collapse on narrow windows).
    sidebar_manual: Option<bool>,
    /// Auto sidebar mode last seen, so a resize that crosses a breakpoint can
    /// drop a manual Ctrl+B choice.
    auto_mode: u8,
    /// Keyboard focus: the editor or the file tree.
    focus: Focus,
    /// Sidebar hover/scroll/selection state, kept between frames.
    sidebar: SidebarState,
    /// True when the GL config actually has an alpha channel. Without it the
    /// window is opaque and square, and we say so once at startup.
    transparent: bool,
    /// User settings, loaded before the window exists and saved on change.
    config: Config,
    /// True while the settings page replaces the text area.
    settings_open: bool,
    /// Selected settings row, for keyboard navigation.
    settings_row: usize,
    /// Settings row under the mouse (for the hover wash).
    settings_hover: Option<usize>,
    /// Window geometry to restore at startup and write back on exit.
    state: WindowState,
    /// Cursor blink phase (only used when `config.cursor_blink` is on).
    blink_on: bool,
    /// When the blink phase flips next.
    blink_at: std::time::Instant,
}

impl App {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            window: None,
            gl: None,
            workspace: None,
            mods: ModifiersState::empty(),
            clipboard: None,
            cursor_pos: None,
            hover: None,
            sidebar_manual: None,
            auto_mode: 0,
            focus: Focus::default(),
            sidebar: SidebarState::default(),
            transparent: false,
            config: Config::default(),
            settings_open: false,
            settings_row: 0,
            settings_hover: None,
            state: WindowState::default(),
            blink_on: true,
            blink_at: std::time::Instant::now(),
        }
    }

    /// Read the config and the remembered window geometry. Called before the
    /// window is created, as the spec asks (the file is tiny).
    pub fn load_settings(&mut self) {
        self.config = Config::load();
        self.state = WindowState::load();
    }

    /// Push config values into the renderer/window after a live change: font
    /// sizes (which rebuild the atlases) and the window radius.
    fn apply_config(&mut self, old: &Config) {
        let font_changed = old.font_size != self.config.font_size;
        if let Some(gl) = self.gl.as_mut() {
            if font_changed {
                gl.renderer.set_font_size(self.config.font_size as f32);
            }
        }
        // Saving is deliberate: only when a value actually changed.
        if old != &self.config {
            self.config.save();
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
        }
    }

    /// Change one setting from the UI, save it, and redraw. Returns true when
    /// the config actually changed.
    fn change_config(&mut self, f: impl FnOnce(&mut Config) -> bool) -> bool {
        let old = self.config.clone();
        let changed = f(&mut self.config);
        if changed {
            self.apply_config(&old);
            // Some settings affect window-level state immediately.
            if old.sidebar != self.config.sidebar {
                self.sidebar_manual = match self.config.sidebar {
                    SidebarMode::Auto => None,
                    SidebarMode::Expanded => Some(true),
                    SidebarMode::Rail | SidebarMode::Hidden => Some(false),
                };
            }
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
        }
        changed
    }

    /// Remember the window size and maximized flag, then leave.
    fn remember_state(&mut self) {
        if let Some(w) = self.window.as_ref() {
            let size = w.inner_size();
            let scale = w.scale_factor() as f32;
            self.state.width = size.width as f32 / scale;
            self.state.height = size.height as f32 / scale;
            self.state.maximized = w.is_maximized();
        }
        self.state.save();
    }

    fn toggle_settings(&mut self) {
        self.settings_open = !self.settings_open;
        self.focus = if self.settings_open {
            Focus::Settings
        } else {
            Focus::Editor
        };
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Copy the current selection to the system clipboard. Failures become a
    /// status message, never a panic.
    fn copy_selection(&mut self) {
        let Some(ed) = self.workspace.as_mut().and_then(|w| w.active_editor_mut()) else {
            return;
        };
        let Some(text) = ed.selected_text() else {
            ed.status = Some("Nothing selected".to_string());
            return;
        };
        if self.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => self.clipboard = Some(c),
                Err(e) => {
                    ed.status = Some(format!("Clipboard unavailable: {e}"));
                    return;
                }
            }
        }
        if let Some(c) = self.clipboard.as_mut() {
            if let Err(e) = c.set_text(text) {
                ed.status = Some(format!("Copy failed: {e}"));
            }
        }
    }

    /// Read the clipboard and hand it to the editor as one paste command.
    fn paste(&mut self) {
        let Some(ed) = self.workspace.as_mut().and_then(|w| w.active_editor_mut()) else {
            return;
        };
        if self.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => self.clipboard = Some(c),
                Err(e) => {
                    ed.status = Some(format!("Clipboard unavailable: {e}"));
                    return;
                }
            }
        }
        let text = match self.clipboard.as_mut().map(|c| c.get_text()) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                ed.status = Some(format!("Paste failed: {e}"));
                return;
            }
            None => return,
        };
        ed.apply(Command::Paste(text));
    }

    /// Create the window, then the GL context and surface.
    /// The font is loaded only after this succeeds, so the window appears
    /// quickly even on a slow disk.
    fn create_window(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // The shell draws its own title bar and rounded corners, so the OS
        // decorations are off and the window is transparent where the rounded
        // shape does not cover it.
        let attributes = Window::default_attributes()
            .with_title("Ludicrous")
            .with_inner_size(LogicalSize::new(self.state.width, self.state.height))
            .with_min_inner_size(LogicalSize::new(theme::MIN_W, theme::MIN_H))
            .with_decorations(false)
            .with_transparent(true);

        // Ask for an alpha channel and transparency. If the driver cannot do
        // it we still start: the window is then opaque and square.
        let template = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .with_transparency(true);
        let (window, gl_config) = DisplayBuilder::new()
            .with_window_attributes(Some(attributes))
            .build(event_loop, template, |configs| {
                // Prefer a config that really supports transparency, then the
                // one with the most samples.
                configs
                    .reduce(|best, c| {
                        let best_t = best.supports_transparency().unwrap_or(false);
                        let c_t = c.supports_transparency().unwrap_or(false);
                        if c_t != best_t {
                            if c_t {
                                c
                            } else {
                                best
                            }
                        } else if c.num_samples() > best.num_samples() {
                            c
                        } else {
                            best
                        }
                    })
                    .expect("no GL configs available")
            })?;
        let window = window.expect("no window created");
        self.transparent = gl_config.supports_transparency().unwrap_or(false);
        if !self.transparent {
            eprintln!("ludicrous: no transparent GL config; using an opaque square window");
        }

        let size = window.inner_size();
        let (w, h) = (
            NonZeroU32::new(size.width.max(1)),
            NonZeroU32::new(size.height.max(1)),
        );

        let raw_handle = window.window_handle()?.as_raw();
        let surface_attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            raw_handle,
            w.unwrap(),
            h.unwrap(),
        );
        let surface = unsafe {
            gl_config
                .display()
                .create_window_surface(&gl_config, &surface_attrs)?
        };

        let context_attrs = ContextAttributesBuilder::new().build(Some(raw_handle));
        let not_current = unsafe {
            gl_config
                .display()
                .create_context(&gl_config, &context_attrs)?
        };
        let context = not_current.make_current(&surface)?;

        // vsync: don't burn CPU; we only draw when something changes anyway.
        let _ =
            surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()));

        // Load GL function pointers. glow::Context::from_loader_function is the standard way.
        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|s| {
                gl_config.display().get_proc_address(s) as *const _
            })
        };

        let mut renderer = Renderer::new(gl, FONT, ICONS, self.config.font_size as f32);
        renderer.set_scale(window.scale_factor() as f32);

        // Load the file only now: the window is already on screen, so a slow
        // disk or a huge file does not delay the first paint.
        let ws = match self.path.as_deref() {
            Some(p) => match Workspace::open_argument(p) {
                Ok(ws) => ws,
                Err(e) => {
                    eprintln!("ludicrous: {e}");
                    Workspace::new()
                }
            },
            None => Workspace::new(),
        };
        self.workspace = Some(ws);

        self.gl = Some(GlState {
            surface,
            context,
            renderer,
        });
        // Restore the remembered maximized state. Position is never restored:
        // Wayland does not allow it.
        if self.state.maximized {
            window.set_maximized(true);
        }
        self.window = Some(window);
        Ok(())
    }

    /// Sidebar width in points for a window of `w` points.
    ///
    /// Auto mode collapses to a rail under 900 px and hides under 600 px. A
    /// manual Ctrl+B toggle wins until the window is resized across one of
    /// those thresholds (see `on_resize`).
    fn sidebar_width(&self, w: f32) -> f32 {
        // The configured mode wins; `Auto` falls back to the width thresholds.
        match self.config.sidebar {
            SidebarMode::Expanded => return theme::SIDEBAR_W,
            SidebarMode::Rail => return theme::SIDEBAR_RAIL_W,
            SidebarMode::Hidden => return 0.0,
            SidebarMode::Auto => {}
        }
        let expanded = match self.sidebar_manual {
            Some(show) => show,
            None => w >= 900.0,
        };
        if expanded {
            theme::SIDEBAR_W
        } else if w >= 600.0 {
            theme::SIDEBAR_RAIL_W
        } else {
            0.0
        }
    }

    /// Build the current layout from the window size. Used by both drawing and
    /// hit testing, so a click always lands where the text was drawn.
    fn layout(&self) -> Option<Layout> {
        let window = self.window.as_ref()?;
        let gl = self.gl.as_ref()?;
        let ed = self.workspace.as_ref()?.active_editor()?;
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let (w, h) = (size.width as f32 / scale, size.height as f32 / scale);
        Some(Layout::new(
            &gl.renderer,
            ed.buffer.line_count(),
            w,
            h,
            self.sidebar_width(w),
        ))
    }

    /// Title text for the title bar.
    fn title(&self) -> String {
        let Some(ws) = self.workspace.as_ref() else {
            return "Ludicrous".to_string();
        };
        match ws.active_editor() {
            Some(ed) if ed.path().is_some() => {
                format!("Ludicrous: {}", workspace::buffer_name(ed))
            }
            _ => "Ludicrous".to_string(),
        }
    }

    fn hit_state(&self) -> HitState {
        let maximized = self
            .window
            .as_ref()
            .map(|w| w.is_maximized())
            .unwrap_or(false);
        let fullscreen = self
            .window
            .as_ref()
            .map(|w| w.fullscreen().is_some())
            .unwrap_or(false);
        HitState {
            maximized,
            fullscreen,
            buttons_left: self.config.window_buttons == Buttons::Left,
        }
    }

    /// What is under the last known cursor position, in points.
    fn hit_at_cursor(&self) -> Option<Hit> {
        let pos = self.cursor_pos?;
        let window = self.window.as_ref()?;
        let scale = window.scale_factor() as f32;
        let size = window.inner_size();
        let (w, h) = (size.width as f32 / scale, size.height as f32 / scale);
        Some(window::hit_test(
            window::to_points(pos, scale),
            (w, h),
            self.hit_state(),
        ))
    }

    /// Update the OS cursor icon for the current hover target.
    fn update_cursor_icon(&self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let icon = match self.hover {
            Some(Hit::Resize(dir)) => Some(match dir {
                ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
                ResizeDirection::East | ResizeDirection::West => CursorIcon::EwResize,
                ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
                ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::NwseResize,
            }),
            Some(Hit::TitleDrag) => Some(CursorIcon::Default),
            _ => Some(CursorIcon::Text),
        };
        window.set_cursor(icon.unwrap_or(CursorIcon::Default));
    }

    /// Toggle the sidebar, remembering the choice until the next resize across
    /// a breakpoint.
    fn toggle_sidebar(&mut self) {
        let w = self
            .window
            .as_ref()
            .map(|win| {
                let s = win.scale_factor() as f32;
                win.inner_size().width as f32 / s
            })
            .unwrap_or(1000.0);
        let expanded = self.sidebar_width(w) >= theme::SIDEBAR_W;
        let rail = self.sidebar_width(w) >= theme::SIDEBAR_RAIL_W && !expanded;
        // Expanded <-> hidden; a rail-mode sidebar toggles its rail instead.
        self.sidebar_manual = Some(if rail { expanded } else { !expanded });
    }

    /// Keyboard handling for the settings page. Returns true when the key was
    /// consumed.
    fn settings_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        use winit::keyboard::{Key, NamedKey};
        match event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.settings_open = false;
                self.focus = Focus::Editor;
                self.settings_hover = None;
            }
            Key::Named(NamedKey::ArrowDown) => {
                self.settings_row = (self.settings_row + 1).min(settings::Row::COUNT - 1);
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.settings_row = self.settings_row.saturating_sub(1);
            }
            Key::Named(NamedKey::ArrowRight) | Key::Named(NamedKey::Enter) => {
                let row = settings::Row::from_index(self.settings_row);
                self.change_config(|c| settings::step(c, row, 1));
            }
            Key::Named(NamedKey::ArrowLeft) => {
                let row = settings::Row::from_index(self.settings_row);
                self.change_config(|c| settings::step(c, row, -1));
            }
            Key::Named(NamedKey::Tab) => {
                self.settings_row = (self.settings_row + 1) % settings::Row::COUNT;
            }
            _ => return false,
        }
        true
    }

    /// The settings layout for the current window, or None.
    fn settings_geometry(&self) -> Option<Layout> {
        self.layout()
    }

    /// Settings row under the mouse.
    fn settings_row_at_cursor(&self) -> Option<usize> {
        let pos = self.cursor_pos?;
        let window = self.window.as_ref()?;
        let scale = window.scale_factor() as f32;
        let (x, y) = window::to_points(pos, scale);
        let l = self.settings_geometry()?;
        settings::Geometry::new(&l).hit(x, y).map(|(row, _)| row)
    }

    /// Handle a click while the settings page is open. Returns true when the
    /// click landed on the page (and was consumed).
    fn settings_click(&mut self) -> bool {
        let Some(pos) = self.cursor_pos else {
            return false;
        };
        let Some(window) = self.window.as_ref() else {
            return false;
        };
        let scale = window.scale_factor() as f32;
        let (x, y) = window::to_points(pos, scale);
        let Some(l) = self.settings_geometry() else {
            return false;
        };
        // Only clicks inside the page's panel are consumed; the sidebar and
        // title bar stay usable.
        if x < l.sidebar_w {
            return false;
        }
        let Some((row, changed)) = settings::click(&mut self.config, &l, x, y) else {
            // Blank space below the rows: leave the page open but unfocus.
            return true;
        };
        self.settings_row = row;
        self.focus = Focus::Settings;
        if changed {
            let old = self.config.clone();
            // `click` already mutated the config, so persist and rebuild.
            let _ = &old;
            self.config.save();
            self.apply_after_settings_change();
        }
        true
    }

    /// Rebuild anything that depends on a setting after the settings page
    /// changed it (the config has already been mutated and saved).
    fn apply_after_settings_change(&mut self) {
        if let Some(gl) = self.gl.as_mut() {
            gl.renderer.set_font_size(self.config.font_size as f32);
        }
        self.sidebar_manual = match self.config.sidebar {
            SidebarMode::Auto => None,
            SidebarMode::Expanded => Some(true),
            SidebarMode::Rail | SidebarMode::Hidden => Some(false),
        };
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Sidebar width in points, or 0 when it is hidden.
    fn sidebar_width_of_window(&self) -> f32 {
        self.window
            .as_ref()
            .map(|w| {
                let s = w.scale_factor() as f32;
                self.sidebar_width(w.inner_size().width as f32 / s)
            })
            .unwrap_or(0.0)
    }

    /// Sidebar geometry for the current window, or `None` when it is hidden.
    fn sidebar_geometry(&self) -> Option<(Layout, sidebar::Geometry)> {
        let l = self.layout()?;
        if l.sidebar_w <= 0.0 {
            return None;
        }
        let open = self.workspace.as_ref()?.file_labels().len();
        let geo = sidebar::Geometry::from_layout(&l, open);
        Some((l, geo))
    }

    /// What the mouse is over inside the sidebar.
    fn sidebar_hit(&self) -> Option<sidebar::Hit> {
        let pos = self.cursor_pos?;
        let window = self.window.as_ref()?;
        let scale = window.scale_factor() as f32;
        let (x, y) = window::to_points(pos, scale);
        let (_l, geo) = self.sidebar_geometry()?;
        let ws = self.workspace.as_ref()?;
        Some(geo.hit(
            x,
            y,
            ws.file_labels().len(),
            ws.tree.rows().len(),
            self.sidebar.scroll,
        ))
    }

    /// Refresh the hover flags from the mouse position. Returns true when
    /// something changed, so the caller only redraws on real changes.
    fn update_sidebar_hover(&mut self) -> bool {
        let hit = self.sidebar_hit();
        let (mut tree, mut open, mut icon) = (None, None, None);
        match hit {
            Some(sidebar::Hit::TreeRow(i)) => tree = Some(i),
            Some(sidebar::Hit::OpenFile(i)) => open = Some(i),
            Some(sidebar::Hit::Settings) => icon = Some(0),
            Some(sidebar::Hit::RailFiles) => icon = Some(0),
            Some(sidebar::Hit::RailSettings) => icon = Some(1),
            _ => {}
        }
        let changed = (
            self.sidebar.hover_tree,
            self.sidebar.hover_open,
            self.sidebar.hover_icon,
        ) != (tree, open, icon);
        self.sidebar.hover_tree = tree;
        self.sidebar.hover_open = open;
        self.sidebar.hover_icon = icon;
        changed
    }

    /// Activate a tree row: open a file, or expand/collapse a directory.
    fn activate_tree_row(&mut self, index: usize) {
        let Some(ws) = self.workspace.as_mut() else {
            return;
        };
        let Some(row) = ws.tree.rows().get(index).cloned() else {
            return;
        };
        if row.unreadable || row.more > 0 {
            return;
        }
        if row.is_dir {
            ws.tree.toggle(&row.path);
            // Collapsing can remove rows above the selection; keep it valid.
            let n = ws.tree.rows().len();
            self.sidebar.selected = self.sidebar.selected.min(n.saturating_sub(1));
            self.update_sidebar_scroll();
            return;
        }
        match ws.open_file(&row.path) {
            Ok(_) => self.focus = Focus::Editor,
            Err(e) => {
                if let Some(ed) = ws.active_editor_mut() {
                    ed.status = Some(format!("Could not open: {e}"));
                }
            }
        }
    }

    /// Keep the selected tree row inside the viewport.
    fn update_sidebar_scroll(&mut self) {
        let Some((_, geo)) = self.sidebar_geometry() else {
            return;
        };
        let row_top = self.sidebar.selected as f32 * theme::ROW_H;
        let row_bottom = row_top + theme::ROW_H;
        if row_top < self.sidebar.scroll {
            self.sidebar.scroll = row_top;
        } else if row_bottom > self.sidebar.scroll + geo.tree_h {
            self.sidebar.scroll = row_bottom - geo.tree_h;
        }
        let rows = self
            .workspace
            .as_ref()
            .map(|w| w.tree.rows().len())
            .unwrap_or(0);
        let max = sidebar::max_scroll(geo.tree_h, rows);
        self.sidebar.scroll = self.sidebar.scroll.clamp(0.0, max);
    }

    /// Keyboard input while the sidebar has focus. Returns true when the key
    /// was consumed.
    fn sidebar_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let Some(ws) = self.workspace.as_ref() else {
            return false;
        };
        let rows = ws.tree.rows().len();
        match event.logical_key {
            Key::Named(NamedKey::Escape) => {
                self.focus = Focus::Editor;
            }
            Key::Named(NamedKey::ArrowDown) => {
                if rows > 0 {
                    self.sidebar.selected = (self.sidebar.selected + 1).min(rows - 1);
                    self.update_sidebar_scroll();
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.sidebar.selected = self.sidebar.selected.saturating_sub(1);
                self.update_sidebar_scroll();
            }
            Key::Named(NamedKey::Enter) => {
                let i = self.sidebar.selected;
                self.activate_tree_row(i);
            }
            Key::Named(NamedKey::ArrowRight) => {
                // Expand a collapsed folder, or step into an expanded one.
                let i = self.sidebar.selected;
                let Some(row) = ws.tree.rows().get(i).cloned() else {
                    return true;
                };
                if row.is_dir && !row.expanded {
                    self.activate_tree_row(i);
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                let i = self.sidebar.selected;
                let Some(row) = ws.tree.rows().get(i).cloned() else {
                    return true;
                };
                if row.is_dir && row.expanded {
                    self.activate_tree_row(i);
                } else if row.depth > 0 {
                    // Jump to the parent row: the nearest earlier row that is
                    // one level shallower.
                    if let Some(ws) = self.workspace.as_ref() {
                        let rows = ws.tree.rows();
                        for j in (0..i).rev() {
                            if rows[j].depth + 1 == row.depth && rows[j].is_dir {
                                self.sidebar.selected = j;
                                break;
                            }
                        }
                    }
                    self.update_sidebar_scroll();
                }
            }
            _ => return false,
        }
        true
    }

    /// Which mode auto would pick for `w` points: 0 hidden, 1 rail, 2 expanded.
    fn auto_sidebar_mode(w: f32) -> u8 {
        if w >= 900.0 {
            2
        } else if w >= 600.0 {
            1
        } else {
            0
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        // Read the chrome state before borrowing the GL state mutably.
        let title = self.title();
        let hover = self.hover;
        let sidebar_w = self
            .window
            .as_ref()
            .map(|w| {
                let s = w.scale_factor() as f32;
                self.sidebar_width(w.inner_size().width as f32 / s)
            })
            .unwrap_or(0.0);
        let (Some(window), Some(gl), Some(ws)) = (
            self.window.as_ref(),
            self.gl.as_mut(),
            self.workspace.as_mut(),
        ) else {
            return;
        };
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let r = &mut gl.renderer;
        r.set_scale(scale);
        r.begin_frame(size.width, size.height);

        // Layout works in points; the renderer scales internally.
        let (w, h) = (size.width as f32 / scale, size.height as f32 / scale);
        let line_count = ws
            .active_editor()
            .map(|e| e.buffer.line_count())
            .unwrap_or(1);
        let layout = Layout::with_numbers(
            r,
            line_count,
            w,
            h,
            sidebar_w,
            self.config.show_line_numbers,
        );
        let (rows, cols) = (layout.rows, layout.cols);
        if let Some(ed) = ws.active_editor_mut() {
            ed.ensure_visible(rows, cols);
        }
        let modified = ws.active_editor().map(|e| e.is_dirty()).unwrap_or(false);
        let palette = Palette::of(self.config.theme);
        let chrome = view::Chrome {
            title: &title,
            modified,
            maximized: window.is_maximized(),
            hover,
            focus: self.focus,
            caret: self.blink_on || !self.config.cursor_blink,
            sidebar: self.sidebar.clone(),
            palette,
            settings: self.settings_open,
            radius: self.config.corner_radius as f32,
            buttons_left: self.config.window_buttons == Buttons::Left,
        };
        view::draw(r, ws, &layout, &chrome);
        // The settings page replaces the text area, over the editor layer.
        if self.settings_open {
            r.layer(crate::render::Layer::Overlay);
            r.rect(layout.sidebar_w, 0.0, w - layout.sidebar_w, h, palette.bg);
            settings::draw(
                r,
                &layout,
                &palette,
                &self.config,
                self.settings_row,
                self.settings_hover,
            );
        }

        r.flush();
        let _ = gl.surface.swap_buffers(&gl.context);
        // Draw immediately, then wait for the next event.
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

/// winit's resize directions, mapped from our own enum.
fn resize_edge(dir: ResizeDirection) -> winit::window::ResizeDirection {
    use winit::window::ResizeDirection as W;
    match dir {
        ResizeDirection::North => W::North,
        ResizeDirection::South => W::South,
        ResizeDirection::East => W::East,
        ResizeDirection::West => W::West,
        ResizeDirection::NorthEast => W::NorthEast,
        ResizeDirection::NorthWest => W::NorthWest,
        ResizeDirection::SouthEast => W::SouthEast,
        ResizeDirection::SouthWest => W::SouthWest,
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            if let Err(e) = self.create_window(event_loop) {
                eprintln!("ludicrous: could not create window: {e}");
                event_loop.exit();
            }
        }
    }

    /// Blink the caret without ever busy-waiting: when blinking is on we ask
    /// for a wake-up at the next flip, otherwise we sleep until input.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if !self.config.cursor_blink {
            self.blink_on = true;
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let now = std::time::Instant::now();
        if now >= self.blink_at {
            self.blink_on = !self.blink_on;
            self.blink_at = now + std::time::Duration::from_millis(530);
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.blink_at));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                // The close button goes through the same guard as Ctrl+Q, and
                // every open buffer must be clean before we exit.
                let quit = match self.workspace.as_mut() {
                    Some(ws) => ws.request_quit() == Control::Quit,
                    None => true,
                };
                if quit {
                    self.remember_state();
                    event_loop.exit();
                } else if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                // Ctrl+, opens or closes the settings page.
                if self.mods.control_key()
                    && matches!(&event.logical_key,
                        winit::keyboard::Key::Character(c) if c.as_str() == ",")
                {
                    self.toggle_settings();
                    return;
                }
                // While the settings page is open it owns the keyboard: it
                // replaces the text area, so the editor must not see these.
                if self.settings_open {
                    if self.settings_key(&event) {
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                        return;
                    }
                    // Control shortcuts (save, quit, tab cycling) still pass
                    // through below; plain keys are swallowed by the page.
                    if !self.mods.control_key() {
                        return;
                    }
                }
                // Ctrl+Shift+E focuses the file tree (wherever focus was).
                if self.mods.control_key()
                    && self.mods.shift_key()
                    && matches!(&event.logical_key,
                        winit::keyboard::Key::Character(c) if c.as_str().eq_ignore_ascii_case("e"))
                {
                    if self.sidebar_width_of_window() > 0.0 {
                        self.focus = Focus::Sidebar;
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                    }
                    return;
                }
                // With the sidebar focused, its own keys win over the editor's.
                if self.focus == Focus::Sidebar && self.sidebar_width_of_window() > 0.0 {
                    if self.sidebar_key(&event) {
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                        return;
                    }
                    // Typing belongs to the focused part: plain keys do nothing
                    // in the tree instead of leaking into the editor. Control
                    // shortcuts (save, quit, cycling) still work.
                    if !self.mods.control_key() {
                        return;
                    }
                }
                // Shell shortcuts that are not editor commands.
                if matches!(
                    event.logical_key,
                    winit::keyboard::Key::Named(winit::keyboard::NamedKey::F11)
                ) {
                    if let Some(w) = self.window.as_ref() {
                        let next = if w.fullscreen().is_some() {
                            None
                        } else {
                            Some(Fullscreen::Borderless(None))
                        };
                        w.set_fullscreen(next);
                        w.request_redraw();
                    }
                    return;
                }
                if self.mods.control_key()
                    && matches!(&event.logical_key,
                        winit::keyboard::Key::Character(c) if c.as_str().eq_ignore_ascii_case("b"))
                {
                    self.toggle_sidebar();
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                    return;
                }
                // Ctrl+Tab / Ctrl+Shift+Tab cycle the open buffers.
                if self.mods.control_key() {
                    use winit::keyboard::{Key, NamedKey};
                    let is_tab = matches!(&event.logical_key, Key::Named(NamedKey::Tab));
                    let is_w = matches!(&event.logical_key,
                        Key::Character(c) if c.as_str().eq_ignore_ascii_case("w"));
                    if is_tab {
                        if let Some(ws) = self.workspace.as_mut() {
                            ws.cycle(!self.mods.shift_key());
                        }
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                        return;
                    }
                    if is_w {
                        if let Some(ws) = self.workspace.as_mut() {
                            // close_active only returns Quit for the guard on a
                            // dirty buffer; closing never exits the window.
                            let _ = ws.close_active();
                        }
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                        return;
                    }
                }
                // Clipboard commands need the app, not just the editor.
                if input::is_paste(&event, self.mods) {
                    self.paste();
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                    return;
                }
                let Some(cmd) = input::key(&event, self.mods) else {
                    return;
                };
                // Quit is a workspace question, not an editor one: the guard
                // has to walk every dirty buffer.
                if cmd == Command::Quit {
                    let quit = match self.workspace.as_mut() {
                        Some(ws) => ws.request_quit() == Control::Quit,
                        None => true,
                    };
                    if quit {
                        self.remember_state();
                        event_loop.exit();
                    } else if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                    return;
                }
                // Copy/Cut read the selection before the editor changes it.
                if matches!(cmd, Command::Copy | Command::Cut) {
                    self.copy_selection();
                }
                let Some(ed) = self.workspace.as_mut().and_then(|w| w.active_editor_mut()) else {
                    return;
                };
                if ed.apply(cmd) == Control::Quit {
                    self.remember_state();
                    event_loop.exit();
                    return;
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if state != ElementState::Pressed || button != MouseButton::Left {
                    return;
                }
                // The settings page sits over the editing area; a click there
                // never reaches the editor or the sidebar.
                if self.settings_open {
                    let handled = self.settings_click();
                    if handled {
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                        return;
                    }
                }
                match self.sidebar_hit() {
                    Some(sidebar::Hit::Empty) => {}
                    Some(sidebar_hit) => {
                        match sidebar_hit {
                            sidebar::Hit::OpenFile(i) => {
                                if let Some(ws) = self.workspace.as_mut() {
                                    ws.active = i.min(ws.buffers.len().saturating_sub(1));
                                }
                                self.focus = Focus::Editor;
                            }
                            sidebar::Hit::TreeRow(i) => {
                                self.focus = Focus::Sidebar;
                                self.sidebar.selected = i;
                                self.activate_tree_row(i);
                            }
                            sidebar::Hit::RailFiles => {
                                // The files icon expands the sidebar.
                                self.sidebar_manual = Some(true);
                                self.focus = Focus::Sidebar;
                            }
                            sidebar::Hit::RailSettings | sidebar::Hit::Settings => {
                                // The sidebar footer opens the settings page,
                                // exactly like Ctrl+, does.
                                self.toggle_settings();
                            }
                            sidebar::Hit::Header | sidebar::Hit::Empty => {
                                self.focus = Focus::Editor;
                            }
                        }
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                        return;
                    }
                    None => {}
                }
                let Some(hit) = self.hit_at_cursor() else {
                    return;
                };
                match hit {
                    Hit::Resize(dir) => {
                        if let Some(w) = self.window.as_ref() {
                            let _ = w.drag_resize_window(resize_edge(dir));
                        }
                    }
                    Hit::TitleDrag => {
                        if let Some(w) = self.window.as_ref() {
                            let _ = w.drag_window();
                        }
                    }
                    Hit::SidebarToggle => {
                        self.toggle_sidebar();
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                    }
                    Hit::Button(WindowButton::Minimize) => {
                        if let Some(w) = self.window.as_ref() {
                            w.set_minimized(true);
                        }
                    }
                    Hit::Button(WindowButton::Maximize) => {
                        if let Some(w) = self.window.as_ref() {
                            w.set_maximized(!w.is_maximized());
                        }
                    }
                    Hit::Button(WindowButton::Close) => {
                        // Same guard as Ctrl+Q and the OS close request.
                        let quit = match self.workspace.as_mut() {
                            Some(ws) => ws.request_quit() == Control::Quit,
                            None => true,
                        };
                        if quit {
                            self.remember_state();
                            event_loop.exit();
                        } else if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                    }
                    Hit::Content => {
                        // Clicking the text area takes keyboard focus back.
                        self.focus = Focus::Editor;
                        let Some(layout) = self.layout() else {
                            return;
                        };
                        let Some(ed) = self.workspace.as_mut().and_then(|w| w.active_editor_mut())
                        else {
                            return;
                        };
                        let scale = self
                            .window
                            .as_ref()
                            .map(|w| w.scale_factor() as f32)
                            .unwrap_or(1.0);
                        let Some(pos) = self.cursor_pos else {
                            return;
                        };
                        let (x, y) = window::to_points(pos, scale);
                        // Clicks outside the text area (gutter, status bar) do
                        // nothing.
                        if let Some((line, col)) =
                            layout.text_position(x, y, ed.scroll, ed.scroll_x)
                        {
                            ed.apply(Command::Click { line, col });
                            if let Some(w) = self.window.as_ref() {
                                w.request_redraw();
                            }
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = Some(position);
                let sidebar_changed = self.update_sidebar_hover();
                if self.settings_open {
                    let row = self.settings_row_at_cursor();
                    if row != self.settings_hover {
                        self.settings_hover = row;
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                    }
                }
                // Hover feedback: only redraw when the target actually changes.
                let hit = self.hit_at_cursor();
                if sidebar_changed {
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
                if hit != self.hover {
                    self.hover = hit;
                    self.update_cursor_icon();
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if self.hover.is_some() {
                    self.hover = None;
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.settings_open {
                    return; // the page fits; nothing to scroll
                }
                // Over the file tree the wheel scrolls the tree, not the text.
                if matches!(
                    self.sidebar_hit(),
                    Some(sidebar::Hit::TreeRow(_)) | Some(sidebar::Hit::Header)
                ) {
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y * theme::ROW_H,
                        MouseScrollDelta::PixelDelta(p) => p.y as f32,
                    };
                    if let Some((_, geo)) = self.sidebar_geometry() {
                        let rows = self
                            .workspace
                            .as_ref()
                            .map(|w| w.tree.rows().len())
                            .unwrap_or(0);
                        let max = sidebar::max_scroll(geo.tree_h, rows);
                        self.sidebar.scroll = (self.sidebar.scroll - dy).clamp(0.0, max);
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                        return;
                    }
                }
                // Convert pixel deltas to lines using the current cell height.
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => {
                        let cell_h = self
                            .gl
                            .as_ref()
                            .map(|g| g.renderer.cell_size().1)
                            .unwrap_or(16.0);
                        p.y as f32 / cell_h
                    }
                };
                let Some(ed) = self.workspace.as_mut().and_then(|w| w.active_editor_mut()) else {
                    return;
                };
                ed.apply(input::wheel_lines(lines));
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                // A manual sidebar toggle only lasts until the window is
                // resized across one of the auto-collapse thresholds.
                let scale = self
                    .window
                    .as_ref()
                    .map(|w| w.scale_factor() as f32)
                    .unwrap_or(1.0);
                let w_pts = size.width as f32 / scale;
                self.state.width = w_pts;
                self.state.height = size.height as f32 / scale;
                self.state.maximized = self
                    .window
                    .as_ref()
                    .map(|w| w.is_maximized())
                    .unwrap_or(false);
                // A manual Ctrl+B choice lasts only until the window is
                // resized across one of the auto-collapse breakpoints.
                let mode = Self::auto_sidebar_mode(w_pts);
                if mode != self.auto_mode {
                    self.sidebar_manual = None;
                    self.auto_mode = mode;
                }
                if let Some(gl) = self.gl.as_mut() {
                    if let (Some(w), Some(h)) =
                        (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                    {
                        gl.surface.resize(&gl.context, w, h);
                    }
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}
