// app/lsp.rs: the event loop's side of the language server client.
//
// Everything here is glue: open and close documents when the active buffer
// changes, turn `Editor::last_change` into a debounced `didChange`, carry a
// published diagnostic list back to the buffer it belongs to, and hand the
// diagnostics store back to the view. The model lives in `lsp/`, which knows
// nothing about winit.

use std::time::Instant;

use crate::app::{App, UserEvent};
use crate::lsp::client::LspEvent;
use crate::lsp::Routed;

/// How long the pointer must rest on a diagnostic before the tooltip shows.
const HOVER_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// Longest tooltip line in characters; longer messages are cut with `…`.
const TOOLTIP_COLS: usize = 64;

/// The first line of a message, clipped. Newlines are dropped: a tooltip is one
/// paragraph, and clang messages often carry several.
fn first_line(message: &str, cols: usize) -> String {
    let line = message.lines().next().unwrap_or("").trim();
    if line.chars().count() <= cols {
        return line.to_string();
    }
    let mut out: String = line.chars().take(cols.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// A resting pointer over a diagnostic, waiting for the 300 ms tooltip.
#[derive(Clone, Copy, Debug)]
pub struct DiagHover {
    /// Character range of the diagnostic, so a republish or movement can
    /// cancel the tooltip.
    pub start: usize,
    pub end: usize,
    /// When the pointer came to rest here.
    pub since: Instant,
    /// True once the delay has passed and the tooltip is visible.
    pub shown: bool,
}

/// The event loop's side of the client: documents, edits and diagnostics.
impl App {
    /// Build the client, if the feature is on and the settings allow it. Runs
    /// once, after the config is loaded and before the first frame.
    pub fn init_lsp(&mut self) {
        let proxy = self.proxy.clone();
        // The notifier is the only wake-up path: a reader thread sends, the
        // loop wakes, `user_event` does the small UI work.
        let notify: crate::lsp::Notify = std::sync::Arc::new(move |event: LspEvent| {
            let _ = proxy.send_event(UserEvent::Lsp(event));
        });
        self.lsp = Some(crate::lsp::Lsp::new(&self.config, notify));
        self.sync_lsp_root();
    }

    /// Keep the server's workspace root in step with the open folder.
    pub fn sync_lsp_root(&mut self) {
        let Some(lsp) = self.lsp.as_mut() else {
            return;
        };
        let root = self.workspace.as_ref().and_then(|ws| ws.root.clone());
        lsp.set_root(root.as_deref());
    }

    /// Tell the server about the active buffer, if it is not already open, and
    /// clear the document of any buffer that was closed.
    ///
    /// Called on every frame; cheap because it only looks at the active path.
    pub fn sync_lsp_documents(&mut self) {
        // Collect first, then open: `open` needs `&mut self.lsp` while the
        // text comes from `&self.workspace`.
        let Some(ws) = self.workspace.as_ref() else {
            return;
        };
        let docs: Vec<(std::path::PathBuf, String)> = ws
            .buffers
            .iter()
            .filter_map(|b| {
                let path = b.path()?;
                // A huge buffer is not sent to a server: a 32 MB file would be
                // copied into a JSON string on every open.
                if b.buffer.rope.len_bytes() > 4 * 1024 * 1024 {
                    return None;
                }
                Some((path.to_path_buf(), b.buffer.rope.to_string()))
            })
            .collect();
        let Some(lsp) = self.lsp.as_mut() else {
            return;
        };
        if !lsp.enabled() {
            return;
        }
        for (path, text) in docs {
            // `open` ignores a document it already knows about.
            lsp.open(&path, &text);
        }
    }

    /// Report the edit that the last `apply` made, if any, and let the pending
    /// ones go out once the debounce has passed.
    pub fn pump_lsp(&mut self, now: Instant) {
        let Some(lsp) = self.lsp.as_mut() else {
            return;
        };
        if !lsp.enabled() {
            return;
        }
        // The edit record is taken from whichever buffer the user just typed
        // in: the active one. A background parse or a programmatic change (the
        // settings page, a reload) does not touch it.
        if let Some(ws) = self.workspace.as_mut() {
            for i in 0..ws.buffers.len() {
                let Some(change) = ws.buffers[i].last_change.take() else {
                    continue;
                };
                let (start, removed, added) = change;
                let path = ws.buffers[i].path().map(|p| p.to_path_buf());
                if let Some(path) = path {
                    let buffer = &ws.buffers[i].buffer;
                    lsp.changed(&path, buffer, start, removed, added);
                }
            }
        }
        if lsp.pump(now) {
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
        }
    }

    /// When the next debounce or restart is due, so the loop can `WaitUntil`.
    pub fn lsp_deadline(&self) -> Option<Instant> {
        self.lsp.as_ref().and_then(|l| l.next_deadline())
    }

    /// A server event arrived on the UI thread.
    pub fn lsp_event(&mut self, event: LspEvent) {
        let Some(lsp) = self.lsp.as_mut() else {
            return;
        };
        let routed = lsp.handle(event);
        match routed {
            Routed::Ignored => {}
            Routed::Changed => {
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            Routed::Message => {
                // The LSP layer already stored the message; showing it here
                // keeps the status bar path identical to every other hint.
                if let Some(message) = lsp.take_message() {
                    self.status_hint(&message);
                }
            }
            Routed::Diagnostics {
                uri,
                version,
                items,
            } => {
                // Find the buffer the URI names: the document store is keyed by
                // path, so a buffer that is open here is the one to convert
                // against.
                let Some(path) = crate::lsp::client::uri_to_path(&uri) else {
                    return;
                };
                let text = self
                    .workspace
                    .as_ref()
                    .and_then(|ws| ws.buffers.iter().find(|b| b.path() == Some(path.as_path())))
                    .map(|b| b.buffer.rope.to_string());
                let Some(text) = text else {
                    return; // the file was closed while the publish was in flight
                };
                let buffer = crate::editor::Buffer::from_text(&text);
                if lsp.store_diagnostics(&uri, version, items, &buffer) {
                    // A tooltip for the old list must not survive a republish.
                    self.diag_hover = None;
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
        }
    }

    /// Diagnostics for the active buffer, for drawing. Empty when there is no
    /// server or the feature is off.
    pub fn diagnostics(&self) -> &crate::lsp::diagnostics::Diagnostics {
        static EMPTY: std::sync::OnceLock<crate::lsp::diagnostics::Diagnostics> =
            std::sync::OnceLock::new();
        let empty = EMPTY.get_or_init(crate::lsp::diagnostics::Diagnostics::default);
        let (Some(lsp), Some(ws)) = (self.lsp.as_ref(), self.workspace.as_ref()) else {
            return empty;
        };
        match ws.active_editor().and_then(|ed| ed.path()) {
            Some(path) => lsp.diagnostics(path),
            None => empty,
        }
    }

    /// A copy of the active file's diagnostics.
    ///
    /// The view borrows the list for a whole frame, and the frame already holds
    /// `&mut self.gl` by then, so a snapshot is the only shape that compiles.
    /// A file has a handful of diagnostics, so the copy is a few `String`s.
    pub fn diagnostics_snapshot(&self) -> crate::lsp::diagnostics::Diagnostics {
        self.diagnostics().clone()
    }

    /// A buffer was closed: tell its server, and drop the diagnostics.
    pub fn lsp_close(&mut self, path: &std::path::Path) {
        if let Some(lsp) = self.lsp.as_mut() {
            lsp.close(path);
        }
        if self.diag_hover.is_some() {
            self.diag_hover = None;
        }
    }

    /// A buffer was written to disk: the server may want to re-check it.
    pub fn lsp_saved(&mut self, path: &std::path::Path, text: &str) {
        if let Some(lsp) = self.lsp.as_mut() {
            lsp.saved(path, text);
        }
    }

    /// Stop every server. Called from the quit path before the loop exits, so
    /// no child process outlives the editor.
    pub fn shutdown_lsp(&mut self) {
        if let Some(lsp) = self.lsp.as_mut() {
            lsp.shutdown();
        }
    }

    /// Re-read the LSP toggle and the server commands after the settings page
    /// changed them. Turning servers off stops them now.
    pub fn reconfigure_lsp(&mut self) {
        let config = self.config.clone();
        if let Some(lsp) = self.lsp.as_mut() {
            lsp.configure(&config);
        }
    }

    /// Character index under the pointer in the text area, if it is there.
    pub fn diag_char_under_pointer(&self) -> Option<usize> {
        let pos = self.cursor_pos?;
        let scale = self.window.as_ref()?.scale_factor() as f32;
        let (x, y) = crate::window::to_points(pos, scale);
        let layout = self.layout()?;
        let ed = self.workspace.as_ref()?.active_editor()?;
        if self.settings_open || self.focus != crate::ui::focus::Focus::Editor {
            return None;
        }
        let (line, col) = layout.text_position(x, y, ed.scroll, ed.scroll_x)?;
        if line >= ed.buffer.line_count() {
            return None;
        }
        Some(
            ed.buffer
                .char_index(line, col.min(ed.buffer.line_len(line))),
        )
    }

    /// Keep the tooltip delay honest: a pointer resting on a diagnostic for
    /// 300 ms shows the message; moving anywhere else drops it at once.
    ///
    /// This is the only periodic part of the diagnostic UI, and it does nothing
    /// unless the pointer is over the text area.
    pub fn step_diag_hover(&mut self, now: Instant) {
        let Some(path) = self
            .workspace
            .as_ref()
            .and_then(|ws| ws.active_editor())
            .and_then(|ed| ed.path())
            .map(|p| p.to_path_buf())
        else {
            self.diag_hover = None;
            return;
        };
        let Some(at) = self.diag_char_under_pointer() else {
            self.diag_hover = None;
            return;
        };
        // Find the range first, so the tooltip only exists for a real hit.
        let range = {
            let Some(lsp) = self.lsp.as_ref() else {
                self.diag_hover = None;
                return;
            };
            let Some(buffer) = self
                .workspace
                .as_ref()
                .and_then(|ws| ws.active_editor())
                .map(|ed| &ed.buffer)
            else {
                self.diag_hover = None;
                return;
            };
            lsp.diagnostics(&path).at(buffer, at).map(|d| d.range)
        };
        let Some(range) = range else {
            self.diag_hover = None;
            return;
        };
        match self.diag_hover {
            Some(h) if h.start == range.start && h.end == range.end => {
                if !h.shown && now.duration_since(h.since) >= HOVER_DELAY {
                    self.diag_hover = Some(DiagHover { shown: true, ..h });
                    if let Some(w) = self.window.as_ref() {
                        w.request_redraw();
                    }
                }
            }
            // A different diagnostic: restart the clock, hide the old tooltip.
            _ => {
                self.diag_hover = Some(DiagHover {
                    start: range.start,
                    end: range.end,
                    since: now,
                    shown: false,
                });
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
        }
    }

    /// When the tooltip is due, so the loop can `WaitUntil` exactly then.
    pub fn diag_hover_deadline(&self) -> Option<Instant> {
        self.diag_hover
            .filter(|h| !h.shown)
            .map(|h| h.since + HOVER_DELAY)
    }

    /// The tooltip to draw: text, position and severity, once the delay has
    /// passed. The first line of the message only, clipped to 64 characters.
    pub fn diag_tooltip(&self) -> Option<crate::ui::view::DiagTooltip> {
        let hover = self.diag_hover.filter(|h| h.shown)?;
        let path = self
            .workspace
            .as_ref()
            .and_then(|ws| ws.active_editor())
            .and_then(|ed| ed.path())?;
        let buffer = &self.workspace.as_ref()?.active_editor()?.buffer;
        let lsp = self.lsp.as_ref()?;
        let d = lsp.diagnostics(path).at(buffer, hover.start)?;
        let text = first_line(&d.message, TOOLTIP_COLS);
        // Anchor under the pointer, then let the view clamp it into the window.
        let pos = self.cursor_pos?;
        let scale = self.window.as_ref()?.scale_factor() as f32;
        let (x, y) = crate::window::to_points(pos, scale);
        Some(crate::ui::view::DiagTooltip {
            x: x + 12.0,
            y: y + 18.0,
            text,
            severity: d.severity,
        })
    }

    /// One line of server state for the F3 overlay: language, the negotiated
    /// encoding and whether `initialize` has finished.
    pub fn lsp_status(&self) -> Option<String> {
        let lsp = self.lsp.as_ref()?;
        if !lsp.enabled() {
            return Some("disabled".to_string());
        }
        let servers = lsp.server_status();
        if servers.is_empty() {
            return Some("no server started".to_string());
        }
        Some(servers.join(", "))
    }

    /// Show the LSP layer's pending message (a missing server, a crash) once.
    pub fn take_lsp_message(&mut self) -> Option<String> {
        self.lsp.as_mut().and_then(|l| l.take_message())
    }
}
