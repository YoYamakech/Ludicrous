// stub.rs: the `lsp` feature is off.
//
// The UI still asks for diagnostics (squiggles, gutter markers, counts), so the
// two model files that need no JSON and no process are compiled as they are:
// they are pure text maths and load nothing. Everything that would talk to a
// server is simply absent, and the app never builds a session.

// `dead_code`: with the feature off nothing converts a position (there is no
// server to send one to), but the drawing code still needs the diagnostic
// ranges, so the modules stay whole rather than growing a `cfg` per item.
#[allow(dead_code)]
#[path = "position.rs"]
pub mod position;

#[allow(dead_code)]
#[path = "diagnostics.rs"]
pub mod diagnostics;
