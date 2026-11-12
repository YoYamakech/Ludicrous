// editor: pure text model. No windowing, GL or font code in here.
pub mod buffer;
pub mod command;
pub mod cursor;
pub mod history;
pub mod state;

pub use buffer::Buffer;
pub use command::Command;
pub use state::{Control, Editor};
