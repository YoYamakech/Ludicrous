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
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::raw_window_handle::HasWindowHandle;
use winit::window::{CursorIcon, Fullscreen, Window, WindowId};

use crate::config::{Buttons, Config, SidebarMode, WindowState};
use crate::editor::{Command, Control};
use crate::input;
use crate::render::Renderer;
use crate::ui::cursor::{self, CursorKind};
use crate::ui::focus::Focus;
use crate::ui::hitmap::{HitMap, Rect, Target};
use crate::ui::layout::Layout;
use crate::ui::overlay;
use crate::ui::settings;
use crate::ui::sidebar::{self, SidebarState};
use crate::ui::theme;
use crate::ui::theme::Palette;
use crate::ui::view;
use crate::window::{self, HitState, ResizeDirection, WindowButton};
use crate::workspace::{self, Workspace};

// The updater's app-side glue lives in its own file: it is all about timing and
// threads, which would clutter the event loop code here.
#[cfg(feature = "updater")]
mod update;

// The settings page always has an `UpdateInfo`; without the feature it is the
// fixed default `Check now` row, which the page never renders anyway because
// those rows are `#[cfg]`ed out of `Row::ALL`.
impl App {
    #[cfg(not(feature = "updater"))]
    fn update_info(&self) -> settings::UpdateInfo {
        settings::UpdateInfo::default()
    }
}

// The font is compiled into the binary: no file lookup at startup, no missing
// font on a fresh machine.
const FONT: &[u8] = include_bytes!("../assets/font.ttf");
const ICONS: &[u8] = include_bytes!("../assets/icons.ttf");
// Nerd Font Symbols, subset to the file-type glyphs in `ui/icons_map.rs`.
const SYMBOLS: &[u8] = include_bytes!("../assets/symbols.ttf");

/// Everything a background thread may tell the UI thread.
///
/// The proxy is the *only* wake-up path from a worker: send the payload, then
/// the loop runs `user_event`, which does the small amount of UI work and
/// requests exactly one redraw. No polling, no timers, no async runtime.
#[derive(Debug)]
pub enum UserEvent {
    /// A file or folder dialog finished. `None` means the dialog was
    /// unavailable (no portal) or failed outright; `Some(vec![])` is a cancel.
    DialogResult(
        crate::workspace::DialogKind,
        Option<Vec<std::path::PathBuf>>,
    ),
    /// A background syntax parse finished for `buffer` at `version`.
    ///
    /// The tree is boxed because it is large and this enum is moved around;
    /// `Tree` is `Send` (checked), which is what makes the thread possible.
    /// The updater has news: a new release, progress, or a result.
    #[cfg(feature = "updater")]
    Update(crate::updater::UpdateEvent),
    #[cfg(feature = "syntax")]
    ParseDone {
        buffer: usize,
        version: u64,
        tree: Option<Box<tree_sitter::Tree>>,
    },
}

/// The notifier handed to background modules. `ui/` and `workspace/` never see
/// winit: they receive a `Notify` closure built here.
pub type Proxy = EventLoopProxy<UserEvent>;

struct GlState {
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    renderer: Renderer,
}

pub struct App {
    path: Option<PathBuf>,
    /// Sends messages from background threads to this event loop.
    proxy: Proxy,
    /// The one dialog slot; a second request while one is open is ignored.
    dialogs: crate::workspace::DialogState,
    window: Option<Window>,
    gl: Option<GlState>,
    workspace: Option<Workspace>,
    mods: ModifiersState,
    /// Created on first use: opening the X11/Wayland clipboard at startup
    /// would slow cold start and can fail on a headless machine.
    clipboard: Option<arboard::Clipboard>,
    /// Last mouse position in physical pixels, for click-to-place-cursor.
    cursor_pos: Option<PhysicalPosition<f64>>,
    /// What the mouse is over (from the hit map), so hover feedback only
    /// redraws on change.
    hover: Option<Target>,
    /// Every clickable region for the current layout, front-most last. Clicks,
    /// hover and the mouse cursor all read this one list.
    hitmap: HitMap,
    /// The cursor kind currently set on the window, so `set_cursor` is called
    /// only when the kind really changes.
    current_cursor: CursorKind,
    /// True while files are being dragged over the window. During a drag winit
    /// sends no pointer position, so nothing is hit-tested and the cursor is
    /// left to the OS.
    drop_hover: bool,
    /// Files dropped since the last drain; one batch, one redraw (Phase 2).
    pending_drops: Vec<PathBuf>,
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
    /// The hovered settings control's option under the pointer: for a choice
    /// row, which segment gets the faint wash (issue #6.3).
    settings_hover_seg: Option<usize>,
    /// Window geometry to restore at startup and write back on exit.
    state: WindowState,
    /// Cursor blink phase (only used when `config.cursor_blink` is on).
    blink_on: bool,
    /// When the blink phase flips next.
    blink_at: std::time::Instant,
    /// F3 debug overlay. Closed by default and costs nothing while closed.
    debug: bool,
    /// Start of the current redraw, for the CPU frame time shown in the panel.
    frame_started: std::time::Instant,
    last_cpu_ms: f32,
    /// Process start, for the cold-start numbers (v1.2 section 1). `time to
    /// window` is when the GL surface first exists, `time to first frame` is
    /// the first swap. Both are logged once and shown in the F3 overlay.
    started: std::time::Instant,
    time_to_window_ms: Option<f32>,
    time_to_first_frame_ms: Option<f32>,
    /// How many times `set_cursor` actually reached the window. The spec asks
    /// for a counter that proves it only runs when the kind changes.
    cursor_sets: u32,
    /// Animated sidebar width, hover fade and smooth scroll (v1.2 section 8).
    /// Pure state: it never touches GL, and when nothing moves the loop waits.
    anims: crate::ui::anim::UiAnims,
    /// When the previous frame started, for `dt`. Only read while animating.
    last_frame: std::time::Instant,
    /// `LUDICROUS_DEBUG_ANIM=1`: log the animated values once per frame.
    anim_debug: bool,
    /// Where the update flow is right now (v1.2 section 9). Only the badge and
    /// the Settings rows read it.
    #[cfg(feature = "updater")]
    update: crate::updater::UpdateState,
    /// True once an automatic check has been started this session, so
    /// `about_to_wait` cannot start a second one.
    #[cfg(feature = "updater")]
    update_check_started: bool,
    /// When the first frame was swapped. The automatic update check waits ten
    /// seconds after this, so cold start is never competing with a network
    /// thread.
    #[cfg(feature = "updater")]
    first_frame_at: Option<std::time::Instant>,
    /// Last progress event forwarded to the UI, so a fast download cannot force
    /// a redraw per 64 KB chunk.
    #[cfg(feature = "updater")]
    last_progress_at: std::time::Instant,
}

impl App {
    pub fn new(path: Option<PathBuf>, proxy: Proxy) -> Self {
        Self {
            path,
            proxy,
            dialogs: crate::workspace::DialogState::default(),
            window: None,
            gl: None,
            workspace: None,
            mods: ModifiersState::empty(),
            clipboard: None,
            cursor_pos: None,
            hover: None,
            hitmap: HitMap::new(),
            current_cursor: CursorKind::Default,
            // Dev aid, in the same spirit as LUDICROUS_GL_SAFE: forces the drop
            // highlight on so the draw path can be checked without a file
            // manager that speaks XDND.
            drop_hover: std::env::var_os("LUDICROUS_DEBUG_DROP").is_some(),
            pending_drops: Vec::new(),
            sidebar_manual: None,
            auto_mode: 0,
            focus: Focus::default(),
            sidebar: SidebarState::default(),
            transparent: false,
            config: Config::default(),
            settings_open: false,
            settings_row: 0,
            settings_hover: None,
            settings_hover_seg: None,
            state: WindowState::default(),
            blink_on: true,
            blink_at: std::time::Instant::now(),
            debug: false,
            frame_started: std::time::Instant::now(),
            last_cpu_ms: 0.0,
            started: std::time::Instant::now(),
            time_to_window_ms: None,
            time_to_first_frame_ms: None,
            cursor_sets: 0,
            anims: crate::ui::anim::UiAnims::default(),
            last_frame: std::time::Instant::now(),
            anim_debug: std::env::var_os("LUDICROUS_DEBUG_ANIM").is_some(),
            #[cfg(feature = "updater")]
            update: crate::updater::UpdateState::default(),
            #[cfg(feature = "updater")]
            update_check_started: false,
            #[cfg(feature = "updater")]
            first_frame_at: None,
            #[cfg(feature = "updater")]
            last_progress_at: std::time::Instant::now(),
        }
    }

    /// Record a cold-start milestone once, and log it. Called at most once per
    /// milestone: later frames must not overwrite the number the spec measures.
    fn timing(&mut self, first_frame: bool) {
        let ms = self.started.elapsed().as_secs_f32() * 1000.0;
        if self.time_to_window_ms.is_none() && !first_frame {
            self.time_to_window_ms = Some(ms);
            eprintln!("ludicrous: time to window {ms:.0} ms");
        } else if self.time_to_first_frame_ms.is_none() && first_frame {
            self.time_to_first_frame_ms = Some(ms);
            eprintln!("ludicrous: time to first frame {ms:.0} ms");
            // The update check is scheduled from here: ten seconds after the
            // frame the user actually sees.
            #[cfg(feature = "updater")]
            {
                self.first_frame_at = Some(std::time::Instant::now());
            }
        }
    }

    /// Read the config and the remembered window geometry. Called before the
    /// window is created, as the spec asks (the file is tiny).
    pub fn load_settings(&mut self) {
        self.config = Config::load();
        self.state = WindowState::load();
        #[cfg(feature = "updater")]
        self.fake_update_from_env();
        // A sidebar mode chosen with Ctrl+B (or a resize) last time wins over
        // `auto`, matching the spec's "last sidebar mode" in the state file.
        if self.config.sidebar == SidebarMode::Auto {
            self.sidebar_manual = match self.state.sidebar {
                Some(SidebarMode::Expanded) => Some(true),
                Some(SidebarMode::Rail) | Some(SidebarMode::Hidden) => Some(false),
                _ => None,
            };
        }
    }

    /// Push config values into the renderer/window after a live change: font
    /// sizes (which rebuild the atlases) and the window radius.
    fn apply_config(&mut self, old: &Config) {
        let font_changed = old.font_size != self.config.font_size;
        let tab_changed = old.tab_width != self.config.tab_width;
        if let Some(gl) = self.gl.as_mut() {
            if font_changed {
                gl.renderer.set_font_size(self.config.font_size as f32);
            }
            if tab_changed {
                gl.renderer.set_tab_width(self.config.tab_width);
            }
        }
        if tab_changed {
            if let Some(ws) = self.workspace.as_mut() {
                ws.set_indent(self.config.tab_width);
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
        self.state.sidebar = if self.config.sidebar != SidebarMode::Auto {
            Some(self.config.sidebar)
        } else {
            let w = self.state.width;
            match self.sidebar_target(w) {
                w if w >= theme::SIDEBAR_W => Some(SidebarMode::Expanded),
                w if w >= theme::SIDEBAR_RAIL_W => Some(SidebarMode::Rail),
                _ => Some(SidebarMode::Hidden),
            }
        };
        self.state.save();
    }

    /// F11 / the titlebar button: enter borderless fullscreen, or leave it.
    /// One place, so the button and the key cannot disagree (issue #6.8).
    fn toggle_fullscreen(&mut self) {
        if let Some(w) = self.window.as_ref() {
            let next = if w.fullscreen().is_some() {
                None
            } else {
                Some(Fullscreen::Borderless(None))
            };
            w.set_fullscreen(next);
            w.request_redraw();
        }
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
        // sRGB check (section 9.1). A transparent window composites
        // premultiplied alpha, and our shaders output already-encoded colours;
        // asking the hardware to blend in linear space would fringe the
        // rounded corners and glyph edges. So the check is reported and the
        // path stays off unless explicitly requested for testing.
        let srgb =
            gl_config.srgb_capable() && std::env::var("LUDICROUS_SRGB").as_deref() == Ok("1");
        eprintln!(
            "ludicrous: framebuffer sRGB capable={} enabled={}",
            gl_config.srgb_capable(),
            srgb
        );
        let mut surface_builder = SurfaceAttributesBuilder::<WindowSurface>::new();
        if srgb {
            surface_builder = surface_builder.with_srgb(Some(true));
        }
        let surface_attrs = surface_builder.build(raw_handle, w.unwrap(), h.unwrap());
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

        let mut renderer = Renderer::new(gl, FONT, ICONS, SYMBOLS, self.config.font_size as f32);
        renderer.set_tab_width(self.config.tab_width);
        renderer.set_scale(window.scale_factor() as f32);

        // Load the file only now: the window is already on screen, so a slow
        // disk or a huge file does not delay the first paint.
        let mut ws = match self.path.as_deref() {
            Some(p) => match Workspace::open_argument(p) {
                Ok(ws) => ws,
                Err(e) => {
                    eprintln!("ludicrous: {e}");
                    Workspace::new()
                }
            },
            None => Workspace::new(),
        };
        // The saved tab width has to reach the editors opened above, not just
        // ones created later by the settings page.
        ws.set_indent(self.config.tab_width);
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
        // The first frame must show the sidebar already at its width, not
        // sliding open: animations only start when something changes.
        self.snap_sidebar();
        // The surface exists and the context is current: window done.
        self.timing(false);
        Ok(())
    }

    /// Sidebar width in points for a window of `w` points.
    ///
    /// Auto mode collapses to a rail under 900 px and hides under 600 px. A
    /// manual Ctrl+B toggle wins until the window is resized across one of
    /// those thresholds (see `on_resize`).
    /// Where the sidebar is heading, from the config and the window width.
    /// Pure: it says nothing about where the animation currently is.
    fn sidebar_target(&self, w: f32) -> f32 {
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
            self.sidebar_width(),
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

    /// Rebuild the hit map for the current layout, reusing its allocation.
    ///
    /// Called on every mouse event and on every layout change (resize, sidebar
    /// mode, settings open/closed, tree expand/collapse, scroll), never on
    /// idle. Order is back to front: background first, chrome and overlays
    /// last, so the front-most thing wins.
    fn rebuild_hitmap(&mut self) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Some(layout) = self.layout() else {
            return;
        };
        let (Some(ws), Some(gl)) = (self.workspace.as_ref(), self.gl.as_ref()) else {
            return;
        };
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let (w, h) = (size.width as f32 / scale, size.height as f32 / scale);
        let _ = gl;

        let mut map = std::mem::take(&mut self.hitmap);
        map.clear();

        // Base layer: the window background is inert.
        map.push(Rect::new(0.0, 0.0, w, h), Target::None);

        // Editor: gutter first (inert), then the text area (I-beam).
        map.push(
            Rect::new(
                layout.sidebar_w,
                layout.text_y,
                layout.gutter_w,
                layout.height,
            ),
            Target::TextGutter,
        );
        map.push(
            Rect::new(layout.text_x, layout.text_y, layout.text_w, layout.height),
            Target::TextArea,
        );
        map.push(
            Rect::new(
                layout.sidebar_w,
                layout.height - layout.status_h,
                w - layout.sidebar_w,
                layout.status_h,
            ),
            Target::StatusBar,
        );

        // Sidebar rows, exactly where they are drawn.
        if let Some(geo) = self.sidebar_geometry().map(|(_, g)| g) {
            geo.register(
                &mut map,
                ws.file_labels().len(),
                ws.tree.rows(),
                self.sidebar.scroll,
            );
        }

        // Settings page, over the text area.
        if self.settings_open {
            settings::register(&mut map, &layout, &self.config, &self.update_info());
        }

        // Title bar and resize edges, last: the front-most pixels.
        window::chrome_regions(&mut map, (w, h), self.hit_state());

        self.hitmap = map;
    }

    /// Set the mouse cursor from one place: the hit map.
    ///
    /// Called on mouse move, layout changes under a still pointer, focus gain,
    /// and after a drag ends. Never while files are being dragged over the
    /// window (the OS owns the cursor then) and never mid window move/resize.
    fn refresh_cursor(&mut self) {
        if self.drop_hover {
            return;
        }
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let Some(pos) = self.cursor_pos else {
            return;
        };
        let scale = window.scale_factor() as f32;
        let kind = cursor::resolve(window::to_points(pos, scale), &self.hitmap);
        self.set_cursor(kind);
    }

    /// Apply a cursor kind, but only when it differs from what is on screen.
    fn set_cursor(&mut self, kind: CursorKind) {
        if kind == self.current_cursor && !self.drop_hover {
            return;
        }
        self.current_cursor = kind;
        self.cursor_sets = self.cursor_sets.saturating_add(1);
        if let Some(window) = self.window.as_ref() {
            window.set_cursor(cursor_icon(kind));
        }
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
        let expanded = self.sidebar_target(w) >= theme::SIDEBAR_W;
        let rail = self.sidebar_target(w) >= theme::SIDEBAR_RAIL_W && !expanded;
        // Expanded <-> hidden; a rail-mode sidebar toggles its rail instead.
        self.sidebar_manual = Some(if rail { expanded } else { !expanded });
    }

    /// Keyboard handling for the settings page. Returns true when the key was
    /// consumed.
    fn settings_key(
        &mut self,
        event: &winit::event::KeyEvent,
        event_loop: &winit::event_loop::ActiveEventLoop,
    ) -> bool {
        use winit::keyboard::{Key, NamedKey};
        // Only the updater's action row needs the loop; the parameter is
        // uniform so a feature-off build has one code path.
        #[cfg(not(feature = "updater"))]
        let _ = event_loop;
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
                // Enter and Right on the action row press the button; on every
                // other row they step the value.
                #[cfg(feature = "updater")]
                if row == settings::Row::UpdateAction
                    && matches!(event.logical_key, Key::Named(NamedKey::Enter))
                {
                    if self.update_info().enabled {
                        self.run_update_action(event_loop);
                    }
                    return true;
                }
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
        settings::Geometry::new(&l).row_at(x, y)
    }

    /// Which option of the hovered choice row is under the pointer. Read from
    /// the hit map, so it is exactly the piece a click would hit.
    fn settings_segment_at_cursor(&self) -> Option<usize> {
        let pos = self.cursor_pos?;
        let window = self.window.as_ref()?;
        let scale = window.scale_factor() as f32;
        let (x, y) = window::to_points(pos, scale);
        match self.hitmap.target_at((x, y)) {
            Some(Target::SettingsRow { part, .. }) => part,
            _ => None,
        }
    }

    /// Apply one click on a settings control piece. Returns true when the row
    /// under the pointer changed (so the hover wash can follow).
    fn settings_click_part(
        &mut self,
        row: usize,
        part: Option<usize>,
        event_loop: &winit::event_loop::ActiveEventLoop,
    ) -> bool {
        let setting = settings::Row::from_index(row);
        #[cfg(not(feature = "updater"))]
        let _ = event_loop;
        #[cfg(feature = "updater")]
        if setting == settings::Row::UpdateAction {
            // The action row is a button: only an enabled one reacts.
            if part.is_some() && self.update_info().enabled {
                self.run_update_action(event_loop);
            }
            return true;
        }
        let before = self.config.clone();
        let changed = settings::apply_part(&mut self.config, setting, part);
        if changed {
            self.config.save();
            self.apply_after_settings_change();
        }
        let _ = before;
        true
    }

    /// Rebuild anything that depends on a setting after the settings page
    /// changed it (the config has already been mutated and saved).
    /// The layout changed under a still pointer (sidebar mode, settings page,
    /// tree, scroll, resize): the map and the cursor must follow at once.
    fn layout_changed(&mut self) {
        self.update_pointer();
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Ask for a native dialog. Runs on its own thread; the result arrives as
    /// `UserEvent::DialogResult`. With no portal the thread reports
    /// `None` and the status bar explains the command-line route.
    fn request_dialog(&mut self, kind: crate::workspace::DialogKind) {
        let Some(request) = self.dialogs.begin(kind) else {
            return; // one dialog at a time
        };
        let proxy = self.proxy.clone();
        request.spawn(move |kind, paths| {
            // A closed loop or a full queue must not panic a worker thread.
            let _ = proxy.send_event(UserEvent::DialogResult(kind, paths));
        });
    }

    /// Install a finished background parse and redraw once.
    ///
    /// The reply carries the buffer index it was started for. If that buffer has
    /// been closed, or the text changed while parsing, the tree is dropped:
    /// a stale tree would highlight the wrong text.
    #[cfg(feature = "syntax")]
    fn parse_done(&mut self, buffer: usize, version: u64, tree: Option<Box<tree_sitter::Tree>>) {
        let Some(ws) = self.workspace.as_mut() else {
            return;
        };
        let Some(ed) = ws.buffers.get_mut(buffer) else {
            return; // closed while parsing
        };
        let current = ed.buffer.version;
        let Some(h) = ed.highlight.as_mut() else {
            return;
        };
        let installed = h.finish_async(version, tree.map(|t| *t), current);
        if self.anim_debug {
            eprintln!("parse done buffer={buffer} version={version} current={current} installed={installed}");
        }
        // Whether it was installed or turned out stale, one redraw settles it:
        // a stale result made the highlighter re-request a parse next frame.
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
        let _ = installed;
    }

    /// Start a background parse when the active buffer is large.
    ///
    /// Small files parse inline: a few hundred microseconds is cheaper than the
    /// thread hand-off. Called once per frame; the highlighter ignores the call
    /// while a parse for the same version is already in flight.
    #[cfg(feature = "syntax")]
    fn start_async_parse_if_needed(&mut self) {
        let Some(index) = self.workspace.as_ref().map(|ws| ws.active) else {
            return;
        };
        // Build the highlighter before asking whether the file is big: the
        // first frame of a big file must reach this before any parse decision.
        if let Some(ed) = self
            .workspace
            .as_mut()
            .and_then(|ws| ws.buffers.get_mut(index))
        {
            ed.ensure_highlighter();
        }
        self.maybe_parse_async(index);
    }

    #[cfg(feature = "syntax")]
    fn maybe_parse_async(&mut self, buffer_index: usize) {
        let Some(proxy) = Some(self.proxy.clone()) else {
            return;
        };
        let Some(ws) = self.workspace.as_mut() else {
            return;
        };
        let Some(ed) = ws.buffers.get_mut(buffer_index) else {
            return;
        };
        let version = ed.buffer.version;
        let Some(h) = ed.highlight.as_mut() else {
            return;
        };
        if !h.wants_async(&ed.buffer) {
            return; // small: the normal path parses inline
        }
        let started = h.reparse_async(&ed.buffer, version, move |version, tree| {
            // Dev aid: proves the parse ran off the UI thread.
            // A closed loop or a full queue must not panic a worker thread.
            let _ = proxy.send_event(UserEvent::ParseDone {
                buffer: buffer_index,
                version,
                tree: tree.map(Box::new),
            });
        });
        let _ = started;
    }

    /// Apply a dialog result. `None` means the dialog could not be shown.
    fn dialog_result(&mut self, kind: crate::workspace::DialogKind, paths: Option<Vec<PathBuf>>) {
        if !self.dialogs.finish(kind) {
            return; // a stale reply for a dialog that is no longer open
        }
        let Some(paths) = paths else {
            self.status_hint(kind.unavailable_hint());
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
            return;
        };
        if paths.is_empty() {
            return; // cancelled: nothing to say
        }
        self.open_paths(&paths);
    }

    /// Open a batch of paths through the one entry point and report the outcome
    /// in the status bar. A whole drop batch is one call, so it is one redraw.
    fn open_paths(&mut self, paths: &[PathBuf]) {
        let Some(ws) = self.workspace.as_mut() else {
            return;
        };
        let mut message = None;
        for path in paths {
            match ws.open_path(path) {
                crate::workspace::OpenResult::Failed(msg) => message = Some(msg),
                crate::workspace::OpenResult::RootChanged => {
                    message = Some(format!("Opened folder {}", path.display()));
                }
                _ => {}
            }
        }
        // The new root has its own tree and the active buffer may have changed,
        // so the map and the cursor must be refreshed even without a mouse move.
        self.sidebar.scroll = 0.0;
        self.sidebar.selected = 0;
        if let Some(msg) = message {
            self.status_hint(&msg);
        }
        self.layout_changed();
    }

    /// A short-lived status message in the editor's status bar.
    fn status_hint(&mut self, msg: &str) {
        if let Some(ed) = self.workspace.as_mut().and_then(|w| w.active_editor_mut()) {
            ed.status = Some(msg.to_string());
        }
    }

    fn apply_after_settings_change(&mut self) {
        if let Some(gl) = self.gl.as_mut() {
            gl.renderer.set_font_size(self.config.font_size as f32);
            gl.renderer.set_tab_width(self.config.tab_width);
        }
        if let Some(ws) = self.workspace.as_mut() {
            ws.set_indent(self.config.tab_width);
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

    /// The sidebar width to draw and hit-test with: the animation's current
    /// value, which equals the target when nothing is moving.
    fn sidebar_width(&self) -> f32 {
        self.anims.value(crate::ui::anim::Which::Sidebar)
    }

    /// Advance every animation by the frame time. Returns true while anything
    /// is still moving; the caller then asks for exactly one more frame.
    fn step_anims(&mut self) -> bool {
        let now = std::time::Instant::now();
        let dt = crate::ui::anim::dt_seconds(now - self.last_frame);
        self.last_frame = now;
        self.sync_anims();
        // Hover fade: the entered row fades in, the left one fades out.
        let hover_target = self.anims.value(crate::ui::anim::Which::Hover);
        self.sidebar.hover_alpha = hover_target.clamp(0.0, 1.0);
        if self.sidebar.fade_from.is_some() {
            // The fade-out runs on the same eased curve, in reverse.
            self.sidebar.fade_alpha -= dt * 6.0;
            if self.sidebar.fade_alpha <= 0.0 {
                self.sidebar.fade_alpha = 0.0;
                self.sidebar.fade_from = None;
            }
        }
        let moving = self.anims.step(dt);
        moving || self.sidebar.fade_from.is_some()
    }

    /// Is anything in the UI mid-animation right now? Shown in the F3 overlay.
    fn animating(&self) -> bool {
        self.anims.is_moving(crate::ui::anim::Which::Sidebar)
            || self.anims.is_moving(crate::ui::anim::Which::Scroll)
            || self.anims.is_moving(crate::ui::anim::Which::Hover)
            || self.sidebar.fade_from.is_some()
    }

    /// Point the sidebar animation at where it belongs for the current window,
    /// then keep the other animations in step with the state. Called from
    /// `about_to_wait`, so resize, Ctrl+B and a settings change are all caught
    /// without every call site having to remember.
    fn sync_anims(&mut self) {
        self.anims.enabled = self.config.animations;
        let w = self
            .window
            .as_ref()
            .map(|win| {
                let s = win.scale_factor() as f32;
                win.inner_size().width as f32 / s
            })
            .unwrap_or(1000.0);
        let target = self.sidebar_target(w);
        self.anims.set(crate::ui::anim::Which::Sidebar, target);
        // Hover fade: one value for whatever is hovered right now.
        let hover = if self.hover.is_some() { 1.0 } else { 0.0 };
        self.anims.set(crate::ui::anim::Which::Hover, hover);
        // Smooth scrolling chases the editor's whole-line scroll position.
        let lines = self
            .workspace
            .as_ref()
            .and_then(|ws| ws.active_editor())
            .map(|ed| ed.scroll as f32)
            .unwrap_or(0.0);
        self.anims.set(crate::ui::anim::Which::Scroll, lines);
    }

    /// Jump the sidebar straight to its target: used once, when the window is
    /// created, so startup does not play an animation.
    fn snap_sidebar(&mut self) {
        let w = self
            .window
            .as_ref()
            .map(|win| {
                let s = win.scale_factor() as f32;
                win.inner_size().width as f32 / s
            })
            .unwrap_or(1000.0);
        let target = self.sidebar_target(w);
        let anims = &mut self.anims;
        anims.set(crate::ui::anim::Which::Sidebar, target);
        anims.sidebar_w.snap_to_target();
    }

    /// Sidebar width in points, or 0 when it is hidden.
    fn sidebar_width_of_window(&self) -> f32 {
        self.sidebar_width()
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

    /// Recompute everything that depends on where the pointer is.
    ///
    /// One pass: rebuild the map for the current layout, resolve the target,
    /// update the hover highlights, and set the cursor if its kind changed.
    /// Returns true when something the user can see changed, so the caller only
    /// redraws then.
    fn update_pointer(&mut self) -> bool {
        self.rebuild_hitmap();
        let target = match self.cursor_pos {
            Some(pos) => {
                let scale = self
                    .window
                    .as_ref()
                    .map(|w| w.scale_factor() as f32)
                    .unwrap_or(1.0);
                self.hitmap.target_at(window::to_points(pos, scale))
            }
            None => None,
        };

        // The sidebar keeps its highlights in three plain options; nothing else
        // in the UI needs per-row hover state.
        let (mut tree, mut open, mut icon) = (None, None, None);
        match target {
            Some(Target::TreeRow(i)) => tree = Some(i),
            Some(Target::OpenFile(i)) => open = Some(i),
            Some(Target::SidebarSettings) | Some(Target::RailFiles) => icon = Some(0),
            Some(Target::RailSettings) => icon = Some(1),
            _ => {}
        }
        let changed = (
            self.sidebar.hover_tree,
            self.sidebar.hover_open,
            self.sidebar.hover_icon,
            self.hover,
        ) != (tree, open, icon, target);
        // Hover fade: when the hovered sidebar row changes, the row just left
        // becomes the one fading out. Only these two ever animate.
        if changed && tree != self.sidebar.hover_tree {
            self.sidebar.fade_from = self.sidebar.hover_tree.map(Target::TreeRow);
            self.sidebar.fade_alpha = 1.0;
        }
        if changed && open != self.sidebar.hover_open {
            self.sidebar.fade_from = self.sidebar.hover_open.map(Target::OpenFile);
            self.sidebar.fade_alpha = 1.0;
        }
        self.sidebar.hover_tree = tree;
        self.sidebar.hover_open = open;
        self.sidebar.hover_icon = icon;
        self.hover = target;

        self.refresh_cursor();
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
        // CPU frame time: how long it takes to build and submit one frame.
        // Measured with one monotonic read at the end of the frame.
        self.frame_started = std::time::Instant::now();
        // Advance the animations first, so this frame draws the fresh value
        // and the next frame is requested only while something still moves.
        let moving = self.step_anims();
        // Dev aid (like LUDICROUS_GL_SAFE): log the animated width every frame,
        // so a slide can be checked without a video capture tool.
        if self.anim_debug {
            let ed_scroll = self
                .workspace
                .as_ref()
                .and_then(|ws| ws.active_editor())
                .map(|e| e.scroll)
                .unwrap_or(0);
            eprintln!(
                "anim ed_scroll={ed_scroll} sidebar={:.1} scroll={:.2} moving={}",
                self.sidebar_width(),
                self.anims.value(crate::ui::anim::Which::Scroll),
                moving
            );
        }
        // Read the chrome state before borrowing the GL state mutably.
        let title = self.title();
        let hover = self.hover;
        // The animated width, not the target: the sidebar slides.
        let sidebar_w = self.sidebar_width();
        // Read before the GL state is borrowed mutably (it borrows `self`).
        let animating = self.animating();
        let scroll_pos = self.anims.value(crate::ui::anim::Which::Scroll);
        // A big file parses on a thread; until it lands the editor draws plain
        // text, so the frame that opens it is never blocked (v1.2 section 6).
        // This runs before the GL borrows below, because it takes `self`.
        #[cfg(feature = "syntax")]
        self.start_async_parse_if_needed();
        // Both read `self`, so they must be built before the GL borrows below.
        #[cfg(feature = "updater")]
        let settings_update_info = self.update_info();
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
            ed.ensure_highlighter();
        }
        // Highlight only the visible lines. The highlighter caches by version
        // and range, so a blink or a hover redraw reuses these spans.
        let span_first = ws.active_editor().map(|e| e.scroll).unwrap_or(0);
        let spans = match ws.active_editor_mut() {
            Some(ed) => {
                let last = span_first + rows + 2;
                let version = ed.buffer.version;
                let t0 = std::time::Instant::now();
                let spans = match ed.highlight.as_mut() {
                    Some(h) => h.spans_for_lines(&ed.buffer, span_first, last, version),
                    None => Vec::new(),
                };
                if self.anim_debug {
                    eprintln!(
                        "spans {:.2} ms (version {version})",
                        t0.elapsed().as_secs_f32() * 1000.0
                    );
                }
                spans
            }
            None => Vec::new(),
        };
        // Keep the animation chasing the editor's own (clamped) scroll. This is
        // a direct field access so it does not conflict with the `ws` borrow.
        let scroll_now = ws.active_editor().map(|e| e.scroll as f32).unwrap_or(0.0);
        self.anims.set(crate::ui::anim::Which::Scroll, scroll_now);
        // Smooth scrolling: a fractional line offset. Glyphs are placed on
        // whole pixels, so text stays crisp mid-move.
        let scroll_lines = scroll_pos;
        let modified = ws.active_editor().map(|e| e.is_dirty()).unwrap_or(false);
        // Owned, so the borrow of `self.update` ends before the GL borrow.
        #[cfg(feature = "updater")]
        let update_note = self.update.status_line();
        #[cfg(feature = "updater")]
        let update_badge = self.update.badge();
        #[cfg(not(feature = "updater"))]
        let update_note: Option<String> = None;
        #[cfg(not(feature = "updater"))]
        let update_badge = false;
        let palette = Palette::of(self.config.theme);
        let chrome = view::Chrome {
            title: &title,
            modified,
            // Fullscreen counts as "maximized" for drawing: no rounded
            // corners on a screen edge, and the button must offer Restore
            // (issue #6.8).
            maximized: window.is_maximized() || window.fullscreen().is_some(),
            hover,
            focus: self.focus,
            caret: self.blink_on || !self.config.cursor_blink,
            drop_hover: self.drop_hover,
            update_note: update_note.as_deref(),
            update_badge,
            scroll_lines,
            spans: &spans,
            span_first,
            sidebar: self.sidebar.clone(),
            palette,
            settings: self.settings_open,
            radius: self.config.corner_radius as f32,
            buttons_left: self.config.window_buttons == Buttons::Left,
        };
        view::draw(r, ws, &layout, &chrome);
        // The settings page replaces the text area, over the editor layer.
        if self.settings_open {
            // The settings body starts *below* the title bar and is clipped
            // to that band: content may never bleed up over the title or its
            // buttons (issue #6.7).
            r.layer(crate::render::Layer::Overlay);
            let body_y = layout.title_h;
            r.push_clip(layout.sidebar_w, body_y, w - layout.sidebar_w, h - body_y);
            r.rect(
                layout.sidebar_w,
                body_y,
                w - layout.sidebar_w,
                h - body_y,
                palette.bg,
            );
            #[cfg(feature = "updater")]
            let update_info = &settings_update_info;
            #[cfg(not(feature = "updater"))]
            let update_info = &settings::UpdateInfo::default();
            settings::draw(
                r,
                &layout,
                &palette,
                &self.config,
                update_info,
                self.settings_row,
                settings::Pointer {
                    row: self.settings_hover,
                    segment: self.settings_hover_seg,
                },
            );
        }

        // F3 overlay: drawn last so it sits over everything, and skipped
        // entirely (no work at all) when closed.
        if self.debug {
            let upload = r.upload_path();
            let stats = r.stats();
            let gpu_ms = r.gpu_ms();
            let (version_string, renderer_string) = r.gl_strings();
            let version_string = version_string.to_string();
            let renderer_string = renderer_string.to_string();
            let flags = path_flags(r.caps);
            let info = overlay::Info {
                window_ms: self.time_to_window_ms,
                first_frame_ms: self.time_to_first_frame_ms,
                cursor_sets: self.cursor_sets,
                cursor: self.current_cursor.short(),
                regions: self.hitmap.len(),
                animating,
                cpu_ms: self.last_cpu_ms,
                gpu_ms,
                draw_calls: stats.draw_calls,
                instances: stats.instances,
                flushes: stats.flushes,
                upload: upload.label(),
                flags: &flags,
                gl_version: &version_string,
                gl_renderer: &renderer_string,
            };
            r.layer(crate::render::Layer::Overlay);
            let x = (w - 330.0 - 12.0).max(layout.sidebar_w + 8.0);
            overlay::draw(r, &palette, x, layout.title_h + 8.0, &info);
        }

        r.flush();
        let _ = gl.surface.swap_buffers(&gl.context);
        // The borrows of `window`, `gl` and `ws` end here, so the app can step
        // its own state below.
        self.last_cpu_ms = self.frame_started.elapsed().as_secs_f32() * 1000.0;
        self.timing(true);
        // The layout moved under a still pointer, so the hit map and the
        // cursor have to follow: v1.2 section 3b asks for exactly this during
        // the sidebar animation.
        if moving {
            self.update_pointer();
        }
        // Ask for exactly one more frame while anything is still moving. The
        // loop itself stays on `Wait` (never `Poll`): the redraw request is
        // what wakes it, and vsync paces it.
        if moving {
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
        }
        // Wait for the next event, or the blink deadline if blinking is on.
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

/// One compact letter per optional GL path, for the F3 overlay.
fn path_flags(caps: crate::render::caps::GlCaps) -> String {
    let mut s = String::new();
    if caps.safe_mode {
        return "SAFE (all optional paths off)".to_string();
    }
    for (on, letter) in [
        (caps.buffer_storage, 'B'),
        (caps.base_instance, 'I'),
        (caps.texture_storage, 'T'),
        (caps.framebuffer_srgb, 'S'),
        (caps.direct_state_access, 'D'),
        (caps.invalidate_subdata, 'V'),
        (caps.context_flush_control, 'F'),
        (caps.timer_query, 'Q'),
    ] {
        if on {
            s.push(letter);
        }
    }
    if s.is_empty() {
        s.push('-');
    }
    s
}

/// Map a `CursorKind` to winit's icon. This is the only place `ui/` cursor
/// decisions meet winit.
fn cursor_icon(kind: CursorKind) -> CursorIcon {
    match kind {
        CursorKind::Default => CursorIcon::Default,
        CursorKind::Pointer => CursorIcon::Pointer,
        CursorKind::Text => CursorIcon::Text,
        CursorKind::ResizeN => CursorIcon::NsResize,
        CursorKind::ResizeS => CursorIcon::NsResize,
        CursorKind::ResizeE => CursorIcon::EwResize,
        CursorKind::ResizeW => CursorIcon::EwResize,
        CursorKind::ResizeNE => CursorIcon::NeswResize,
        CursorKind::ResizeSW => CursorIcon::NeswResize,
        CursorKind::ResizeNW => CursorIcon::NwseResize,
        CursorKind::ResizeSE => CursorIcon::NwseResize,
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

impl ApplicationHandler<UserEvent> for App {
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
        // A dropped batch is complete once the event queue is empty: all the
        // `DroppedFile` events for one drag arrive together, so processing them
        // here is one call and one redraw instead of one per file. This runs
        // only when something was actually dropped, never on idle.
        if !self.pending_drops.is_empty() {
            let paths = std::mem::take(&mut self.pending_drops);
            self.open_paths(&paths);
        }
        // The update check may want a future wake-up (ten seconds after the
        // first frame). It never polls: it either does nothing or names the one
        // instant to wake at.
        #[cfg(feature = "updater")]
        let wake_at = self.maybe_check_updates();
        #[cfg(not(feature = "updater"))]
        let wake_at: Option<std::time::Instant> = None;
        if !self.config.cursor_blink {
            self.blink_on = true;
            match wake_at {
                Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
                None => event_loop.set_control_flow(ControlFlow::Wait),
            }
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
        // The earlier of the blink flip and the update check: one wake-up, not
        // two, and still `WaitUntil` rather than a poll.
        let at = match wake_at {
            Some(at) => at.min(self.blink_at),
            None => self.blink_at,
        };
        event_loop.set_control_flow(ControlFlow::WaitUntil(at));
    }

    /// Background threads deliver here, on the UI thread. Keep this small: a
    /// message is a payload plus at most one redraw request.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::DialogResult(kind, paths) => self.dialog_result(kind, paths),
            #[cfg(feature = "updater")]
            UserEvent::Update(ev) => self.update_event(ev),
            #[cfg(feature = "syntax")]
            UserEvent::ParseDone {
                buffer,
                version,
                tree,
            } => self.parse_done(buffer, version, tree),
        }
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
                // Global window shortcuts come first, before any focus-specific
                // dispatch. They belong to the window, not to the editor, the
                // tree or the settings page, so they work from all of them
                // (issue #6.5, #6.6). The settings page must not swallow them.
                match event.logical_key {
                    winit::keyboard::Key::Named(winit::keyboard::NamedKey::F3) => {
                        self.debug = !self.debug;
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                        return;
                    }
                    winit::keyboard::Key::Named(winit::keyboard::NamedKey::F11) => {
                        self.toggle_fullscreen();
                        return;
                    }
                    _ => {}
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
                    if self.settings_key(&event, event_loop) {
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
                    let is_o = matches!(&event.logical_key,
                        Key::Character(c) if c.as_str().eq_ignore_ascii_case("o"));
                    if is_o {
                        // Ctrl+Shift+O opens a folder, Ctrl+O files.
                        self.request_dialog(if self.mods.shift_key() {
                            crate::workspace::DialogKind::OpenFolder
                        } else {
                            crate::workspace::DialogKind::OpenFiles
                        });
                        return;
                    }
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
                // A click during a file drag is the drop, handled by the drop
                // events; never start a window drag from here.
                if self.drop_hover {
                    return;
                }
                // One lookup for the whole click: the map already knows what is
                // under the pointer, including the settings page over the text.
                self.rebuild_hitmap();
                let Some(pos) = self.cursor_pos else {
                    return;
                };
                let scale = self
                    .window
                    .as_ref()
                    .map(|w| w.scale_factor() as f32)
                    .unwrap_or(1.0);
                let target = self.hitmap.target_at(window::to_points(pos, scale));

                match target {
                    Some(Target::Resize(dir)) => {
                        if let Some(w) = self.window.as_ref() {
                            let _ = w.drag_resize_window(resize_edge(dir));
                        }
                    }
                    Some(Target::TitleDrag) => {
                        // Empty title bar: drag the window. Double-click to
                        // maximize is handled by the OS-style press count below.
                        if let Some(w) = self.window.as_ref() {
                            let _ = w.drag_window();
                        }
                    }
                    Some(Target::SidebarToggle) => {
                        self.toggle_sidebar();
                        self.layout_changed();
                    }
                    Some(Target::Button(WindowButton::Minimize)) => {
                        if let Some(w) = self.window.as_ref() {
                            w.set_minimized(true);
                        }
                    }
                    Some(Target::Button(WindowButton::Maximize)) => {
                        // In F11 fullscreen the button's job is to leave
                        // fullscreen; `set_maximized` alone would not do it.
                        // Otherwise it is the ordinary maximize toggle.
                        if self
                            .window
                            .as_ref()
                            .map(|w| w.fullscreen().is_some())
                            .unwrap_or(false)
                        {
                            self.toggle_fullscreen();
                        } else if let Some(w) = self.window.as_ref() {
                            w.set_maximized(!w.is_maximized());
                            w.request_redraw();
                        }
                    }
                    Some(Target::Button(WindowButton::Close)) => {
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
                    Some(Target::OpenFile(i)) => {
                        if let Some(ws) = self.workspace.as_mut() {
                            ws.active = i.min(ws.buffers.len().saturating_sub(1));
                        }
                        self.focus = Focus::Editor;
                        self.layout_changed();
                    }
                    Some(Target::TreeRow(i)) => {
                        self.focus = Focus::Sidebar;
                        self.sidebar.selected = i;
                        self.activate_tree_row(i);
                        self.layout_changed();
                    }
                    Some(Target::RailFiles) => {
                        // The files icon expands the sidebar.
                        self.sidebar_manual = Some(true);
                        self.focus = Focus::Sidebar;
                        self.layout_changed();
                    }
                    Some(Target::RailSettings) | Some(Target::SidebarSettings) => {
                        // The sidebar footer opens the settings page, exactly
                        // like Ctrl+, does.
                        self.toggle_settings();
                        self.layout_changed();
                    }
                    Some(Target::ProjectCard) => {
                        // v1.2 section 5.1: clicking the card opens a folder.
                        self.request_dialog(crate::workspace::DialogKind::OpenFolder);
                    }
                    Some(Target::SettingsRow { row, part }) => {
                        self.settings_row = row;
                        self.focus = Focus::Settings;
                        if self.settings_click_part(row, part, event_loop) {
                            self.settings_hover = Some(row);
                        }
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                    }
                    Some(Target::TextArea) => {
                        // Clicking the text area takes keyboard focus back.
                        self.focus = Focus::Editor;
                        let Some(layout) = self.layout() else {
                            return;
                        };
                        let Some(ed) = self.workspace.as_mut().and_then(|w| w.active_editor_mut())
                        else {
                            return;
                        };
                        let (x, y) = window::to_points(pos, scale);
                        if let Some((line, col)) =
                            layout.text_position(x, y, ed.scroll, ed.scroll_x)
                        {
                            ed.apply(Command::Click { line, col });
                            if let Some(w) = self.window.as_ref() {
                                w.request_redraw();
                            }
                        }
                    }
                    // Inert regions and empty space: nothing happens, and the
                    // editor keeps its focus.
                    Some(Target::None)
                    | Some(Target::TextGutter)
                    | Some(Target::StatusBar)
                    | None => {}
                }
            }
            // External file drag: a window-level state, never hit-tested, because
            // winit sends no pointer position during a drag (v1.2 section 3).
            WindowEvent::HoveredFile(_) => {
                if !self.drop_hover {
                    self.drop_hover = true;
                    // Hover and press state from before the drag must not
                    // survive into it.
                    self.sidebar.hover_tree = None;
                    self.sidebar.hover_open = None;
                    self.sidebar.hover_icon = None;
                    self.hover = None;
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::HoveredFileCancelled => {
                if self.drop_hover {
                    self.drop_hover = false;
                    self.refresh_cursor();
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::DroppedFile(path) => {
                // Dropped files arrive one event each; the batch is flushed when
                // the event queue drains (see `about_to_wait`).
                self.pending_drops.push(path);
                self.drop_hover = false;
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = Some(position);
                let changed = self.update_pointer();
                if self.settings_open {
                    let row = self.settings_row_at_cursor();
                    if row != self.settings_hover {
                        self.settings_hover = row;
                        self.settings_hover_seg = self.settings_segment_at_cursor();
                        if let Some(w) = self.window.as_ref() {
                            w.request_redraw();
                        }
                    }
                }
                if changed {
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => {
                // The pointer left the window: no row is hovered any more.
                self.cursor_pos = None;
                self.hover = None;
                self.sidebar.hover_tree = None;
                self.sidebar.hover_open = None;
                self.sidebar.hover_icon = None;
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.settings_open {
                    return; // the page fits; nothing to scroll
                }
                // Over the file tree the wheel scrolls the tree, not the text.
                // The hit map answers without any geometry of its own, and the
                // tree band counts even between rows.
                let over_tree = self.cursor_pos.is_some_and(|pos| {
                    let scale = self
                        .window
                        .as_ref()
                        .map(|w| w.scale_factor() as f32)
                        .unwrap_or(1.0);
                    let (x, y) = window::to_points(pos, scale);
                    match self.sidebar_geometry() {
                        Some((_, geo)) => {
                            x < geo.sidebar_w && y >= geo.tree_y && y < geo.tree_y + geo.tree_h
                        }
                        None => false,
                    }
                });
                if over_tree {
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
                        // Rows moved under a still pointer: re-resolve the
                        // hover and the cursor, as section 3b requires.
                        self.layout_changed();
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
