// editor: pure text model. No windowing, GL or font code in here.
pub mod buffer;
pub mod command;
pub mod cursor;
// Highlighting is compiled only with the `syntax` feature. The stub below keeps
// the rest of the editor compiling and behaving identically with it off.
#[cfg(feature = "syntax")]
pub mod highlight;
#[cfg(not(feature = "syntax"))]
#[path = "highlight/stub.rs"]
pub mod highlight;
pub mod history;
pub mod state;

pub use buffer::Buffer;
pub use command::Command;
pub use state::{Control, Editor};
