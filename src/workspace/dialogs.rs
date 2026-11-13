// dialogs.rs: native file dialogs, off the event loop.
//
// The window must never freeze while a dialog is open, so every dialog runs on
// a short-lived thread and reports back through the notifier the app built from
// its event-loop proxy. Nothing here touches winit: `app.rs` owns that.
//
// Backend: `rfd` with `default-features = false, features = ["xdg-portal"]`,
// which talks to the desktop portal over D-Bus. No GTK is linked in and no
// portal traffic happens until the user asks for a dialog, which keeps cold
// start untouched.
//
// A missing or broken portal is normal (headless boxes, minimal sessions): the
// dialog thread fails, the app shows a one-line status message, and everything
// else keeps working. Nothing here may panic or hang.

use std::path::PathBuf;

/// What the user was asked for. Carried back with the result so a late reply
/// can be matched to the request that caused it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogKind {
    /// Ctrl+O: one or more files.
    OpenFiles,
    /// Ctrl+Shift+O or a click on the project card: a folder.
    OpenFolder,
}

impl DialogKind {
    /// Status-bar text when no portal is available. The app shows this instead
    /// of a dialog so the user still knows how to proceed.
    pub fn unavailable_hint(self) -> &'static str {
        match self {
            DialogKind::OpenFiles => "No file dialog available. Start with: ludicrous <path>",
            DialogKind::OpenFolder => "No folder dialog available. Start with: ludicrous <folder>",
        }
    }
}

/// Only one dialog at a time. A second request while one is open is ignored,
/// as the spec asks, so a double-click cannot spawn two portals.
#[derive(Default)]
pub struct DialogState {
    open: Option<DialogKind>,
}

impl DialogState {
    /// Claim the dialog slot. `None` means one is already open.
    pub fn begin(&mut self, kind: DialogKind) -> Option<Request> {
        if self.open.is_some() {
            return None;
        }
        self.open = Some(kind);
        Some(Request { kind })
    }

    /// Mark the reply as handled. A reply for a kind that is not open is
    /// ignored, which keeps a late portal answer from reopening anything.
    pub fn finish(&mut self, kind: DialogKind) -> bool {
        if self.open == Some(kind) {
            self.open = None;
            true
        } else {
            false
        }
    }

    #[cfg(test)] // the app asks `begin`, which answers the same question
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }
}

/// A claimed request; only a `Request` can start a thread, so the one-dialog
/// rule cannot be bypassed by calling the runner directly.
pub struct Request {
    kind: DialogKind,
}

impl Request {
    #[cfg(test)] // the app moves the request into `spawn` and never inspects it
    pub fn kind(&self) -> DialogKind {
        self.kind
    }

    /// Run the dialog on a fresh thread and hand the paths to `done`.
    ///
    /// `done` is what `app.rs` builds from the event-loop proxy, so this module
    /// never needs to know winit exists. An empty result means cancelled or
    /// unavailable; the caller decides which from the backend check below.
    pub fn spawn(self, done: impl FnOnce(DialogKind, Option<Vec<PathBuf>>) + Send + 'static) {
        let kind = self.kind;
        std::thread::spawn(move || {
            let result = run(kind);
            done(kind, result);
        });
    }
}

/// The blocking part, on its own thread. `None` means "no dialog available"
/// (no portal) and `Some(vec![])` means the user cancelled.
#[cfg(feature = "dialogs")]
fn run(kind: DialogKind) -> Option<Vec<PathBuf>> {
    use rfd::FileDialog;
    // A failure here is not an error to report loudly: no portal simply means
    // there is no dialog to show. `FileDialog::pick_file` returns `None` for
    // both cancel and failure, so ask the portal-backed backend to open first?
    // There is no cheap query in rfd 0.18, so the app treats `None` as
    // "cancelled or unavailable" and shows the hint only when nothing opened
    // *and* the portal probe below says so.
    if !portal_available() {
        return None;
    }
    match kind {
        DialogKind::OpenFiles => FileDialog::new().pick_files(),
        DialogKind::OpenFolder => FileDialog::new().pick_folder().map(|p| vec![p]),
    }
}

/// Without the `dialogs` feature the editor still answers Ctrl+O: it just has
/// nothing to show, so the status bar explains the command-line route.
#[cfg(not(feature = "dialogs"))]
fn run(_kind: DialogKind) -> Option<Vec<PathBuf>> {
    None
}

/// Is a desktop portal actually reachable? Checked on the dialog thread, never
/// at startup, because a D-Bus round trip during the first frame is exactly the
/// kind of lazy-init violation the spec forbids.
///
/// The check is deliberately cheap and local: `$DBUS_SESSION_BUS_ADDRESS` is
/// set by every session that has a bus, and the portal service name only
/// resolves over that bus. A missing variable is the common headless case.
#[cfg(feature = "dialogs")]
fn portal_available() -> bool {
    match std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        Ok(addr) => !addr.trim().is_empty(),
        Err(_) => std::env::var_os("XDG_CURRENT_DESKTOP").is_some_and(|d| {
            // Some sessions do not export the bus address but do have a desktop;
            // try anyway rather than refusing a working portal.
            !d.is_empty()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_dialog_can_be_open_at_a_time() {
        let mut state = DialogState::default();
        assert!(!state.is_open());
        assert!(state.begin(DialogKind::OpenFiles).is_some());
        assert!(state.is_open());
        // A second request while one is open is dropped, not queued.
        assert!(state.begin(DialogKind::OpenFiles).is_none());
        assert!(state.begin(DialogKind::OpenFolder).is_none());
        assert!(state.finish(DialogKind::OpenFiles));
        assert!(!state.is_open());
        // The slot is free again.
        assert!(state.begin(DialogKind::OpenFolder).is_some());
    }

    #[test]
    fn a_late_reply_for_a_closed_dialog_is_ignored() {
        let mut state = DialogState::default();
        assert!(state.begin(DialogKind::OpenFiles).is_some());
        assert!(state.finish(DialogKind::OpenFiles));
        // The user opened another kind meanwhile; the stale reply must not
        // close it.
        assert!(state.begin(DialogKind::OpenFolder).is_some());
        assert!(!state.finish(DialogKind::OpenFiles));
        assert!(state.is_open(), "the newer dialog is still open");
    }

    #[test]
    fn every_kind_has_its_own_hint() {
        assert_ne!(
            DialogKind::OpenFiles.unavailable_hint(),
            DialogKind::OpenFolder.unavailable_hint()
        );
        for kind in [DialogKind::OpenFiles, DialogKind::OpenFolder] {
            assert!(kind.unavailable_hint().contains("ludicrous <"));
        }
    }

    #[test]
    fn a_request_carries_its_kind() {
        let mut state = DialogState::default();
        let req = state.begin(DialogKind::OpenFolder).unwrap();
        assert_eq!(req.kind(), DialogKind::OpenFolder);
    }
}
