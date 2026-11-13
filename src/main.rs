// Ludicrous: a minimal code editor with its own OpenGL renderer.
//
// main.rs only parses the command line and hands control to the event loop.
// Everything else lives in a module with one clear job (see app.rs / editor/ / ui/ / render/).

mod app;
mod config;
mod editor;
mod input;
mod render;
mod ui;
mod window;
mod workspace;

use std::path::PathBuf;

use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // v1 has no file picker: the path comes from the command line.
    // No argument opens an empty buffer.
    let path: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    let event_loop = EventLoop::new()?;
    // No continuous animation: wake up for input, resize, focus and redraw requests only.
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = app::App::new(path);
    // Read `config` and `state` before the window exists: the window size and
    // font come from them. A broken file falls back to defaults, never fails.
    app.load_settings();
    event_loop.run_app(&mut app)?;
    Ok(())
}
