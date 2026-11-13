// workspace: the open buffers and the project tree. No winit/GL (see spec 5.1).

pub mod dialogs;
pub mod filetree;
pub mod model;

#[allow(unused_imports)]
pub use dialogs::{DialogKind, DialogState};
#[allow(unused_imports)] // consumed by the sidebar in Phase D
pub use filetree::FileTree;
pub use model::{buffer_name, root_name, OpenResult, Workspace};
