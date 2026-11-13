// ui: turns editor state into draw calls. It only talks to the Renderer API.
//
// `hitmap` + `cursor` are the single source of truth for what is under the
// pointer: the same regions answer "what does a click do?" and "what should the
// mouse cursor look like?". `ui/` never imports winit, so `app.rs` maps a
// `CursorKind` to winit's icon.
pub mod cursor;
pub mod focus;
pub mod hitmap;
pub mod layout;
pub mod overlay;
pub mod settings;
pub mod sidebar;
pub mod theme;
pub mod view;
pub mod widgets;
