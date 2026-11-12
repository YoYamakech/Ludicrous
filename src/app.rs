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
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::ModifiersState;
use winit::raw_window_handle::HasWindowHandle;
use winit::window::{Window, WindowId};

use crate::editor::{Buffer, Control, Editor};
use crate::input;
use crate::render::Renderer;
use crate::ui::layout::Layout;
use crate::ui::view;

// The font is compiled into the binary: no file lookup at startup, no missing
// font on a fresh machine.
const FONT: &[u8] = include_bytes!("../assets/font.ttf");
const FONT_PX: f32 = 15.0;

struct GlState {
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    renderer: Renderer,
}

pub struct App {
    path: Option<PathBuf>,
    window: Option<Window>,
    gl: Option<GlState>,
    editor: Option<Editor>,
    mods: ModifiersState,
}

impl App {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            window: None,
            gl: None,
            editor: None,
            mods: ModifiersState::empty(),
        }
    }

    /// Create the window, then the GL context and surface.
    /// The font is loaded only after this succeeds, so the window appears
    /// quickly even on a slow disk.
    fn create_window(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let attributes = Window::default_attributes()
            .with_title("Ludicrous")
            .with_inner_size(LogicalSize::new(1000.0, 660.0));

        let template = ConfigTemplateBuilder::new().with_alpha_size(8);
        let (window, gl_config) = DisplayBuilder::new()
            .with_window_attributes(Some(attributes))
            .build(event_loop, template, |configs| {
                // Pick the config with the most samples; simple and good enough.
                configs
                    .reduce(|best, c| {
                        if c.num_samples() > best.num_samples() {
                            c
                        } else {
                            best
                        }
                    })
                    .expect("no GL configs available")
            })?;
        let window = window.expect("no window created");

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

        let mut renderer = Renderer::new(gl, FONT, FONT_PX);
        renderer.set_scale(window.scale_factor() as f32);

        // Load the file only now: the window is already on screen, so a slow
        // disk or a huge file does not delay the first paint.
        let buffer = match self.path.as_deref() {
            Some(p) => match Buffer::open(p) {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("ludicrous: could not open {}: {e}", p.display());
                    Buffer::empty()
                }
            },
            None => Buffer::empty(),
        };
        self.editor = Some(Editor::new(buffer));

        self.gl = Some(GlState {
            surface,
            context,
            renderer,
        });
        self.window = Some(window);
        Ok(())
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(gl), Some(ed)) =
            (self.window.as_ref(), self.gl.as_mut(), self.editor.as_mut())
        else {
            return;
        };
        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let r = &mut gl.renderer;
        r.set_scale(scale);
        r.begin_frame(size.width, size.height);

        // Layout works in points; the renderer scales internally.
        let (w, h) = (size.width as f32 / scale, size.height as f32 / scale);
        let layout = Layout::new(r, ed.buffer.line_count(), w, h);
        ed.ensure_visible(layout.rows);
        view::draw(r, ed, &layout);

        r.flush();
        let _ = gl.surface.swap_buffers(&gl.context);
        // Draw immediately, then wait for the next event.
        event_loop.set_control_flow(ControlFlow::Wait);
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::ModifiersChanged(m) => self.mods = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                let Some(cmd) = input::key(&event, self.mods) else {
                    return;
                };
                let Some(ed) = self.editor.as_mut() else {
                    return;
                };
                if ed.apply(cmd) == Control::Quit {
                    event_loop.exit();
                    return;
                }
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
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
                let Some(ed) = self.editor.as_mut() else {
                    return;
                };
                ed.apply(input::wheel_lines(lines));
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
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
