// main.rs: parse the command line, build the event loop, hand control to App.
//
// The event loop is generic over `UserEvent` from the start. Background work
// (dialogs now, the update checker and LSP later) never touches winit: each
// thread gets a notifier built from `EventLoopProxy`, sends its payload through
// that one channel, and the UI thread drains it in `App::user_event`. That is
// the only way a background thread can wake the loop, so idle CPU stays at zero
// and no polling loop is needed anywhere (v1.2 section 2).

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

use app::UserEvent;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // v1 had no file picker: the path comes from the command line. That is
    // still the most reliable route, and it is what the dialogs' fallback
    // message points at. No argument opens an empty buffer.
    let path: Option<PathBuf> = std::env::args_os().nth(1).map(PathBuf::from);

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    // No continuous animation: wake up for input, resize, focus, a redraw
    // request, or a message from a background thread.
    event_loop.set_control_flow(ControlFlow::Wait);

    let proxy = event_loop.create_proxy();
    let mut app = app::App::new(path, proxy);
    // Read `config` and `state` before the window exists: the window size and
    // font come from them. A broken file falls back to defaults, never fails.
    app.load_settings();
    event_loop.run_app(&mut app)?;
    Ok(())
}
