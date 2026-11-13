// lsp/mod.rs: one session per language, one document per open file.
//
// This is the layer between "a buffer changed" and "a server was told". It
// owns the lazy start rule (a server starts on the first file of its language),
// the change debounce (about 50 ms, driven by the event loop's `WaitUntil`),
// the conversion of published diagnostics into buffer positions, and the
// restart-once policy. `client.rs` speaks the protocol; nothing here does I/O
// beyond handing work to it.
//
// No winit, no GL: the app passes in a `Notify` closure to wake the event
// loop, and reads the diagnostics back out for drawing.

pub mod client;
pub mod diagnostics;
pub mod position;
pub mod protocol;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::editor::Buffer;
use crate::lsp::client::{Client, Language, LspEvent, RawDiagnostic, ServerCommand};
use crate::lsp::diagnostics::{Diagnostic, Diagnostics};
use crate::lsp::position::{char_to_pos, range_to_chars, Encoding, Pos};

/// How long edits are collected before they are sent as one `didChange`.
/// Roughly one typing burst; short enough that diagnostics feel immediate.
pub const DEBOUNCE: Duration = Duration::from_millis(50);

/// How long to wait before the one allowed restart of a crashed server.
const RESTART_DELAY: Duration = Duration::from_millis(750);

/// The wake-up path to the event loop, built by the app. `mpsc`-free: the
/// client's reader thread calls this directly.
pub type Notify = Arc<dyn Fn(LspEvent) + Send + Sync>;

/// One live server.
struct Server {
    client: Client,
    encoding: Encoding,
    triggers: Vec<String>,
    /// When to try starting it again, after a crash.
    restart_at: Option<Instant>,
}

/// One open document, as the server knows it.
struct Document {
    uri: String,
    language: Language,
    /// Version of the last text the server has.
    version: i64,
    /// Changes queued since then, in order. Positions were computed against
    /// the buffer *at the time of the edit*, so replaying them in order on the
    /// server reproduces the text exactly.
    pending: Vec<(Pos, Pos, String)>,
    /// When the first pending change arrived, for the debounce deadline.
    first_edit: Option<Instant>,
    diagnostics: Diagnostics,
}

pub struct Lsp {
    enabled: bool,
    /// One command per language, from the config file. `None` disables that
    /// language (an empty `lsp_rust =` line).
    commands: HashMap<Language, Option<ServerCommand>>,
    servers: HashMap<Language, Server>,
    docs: HashMap<PathBuf, Document>,
    /// Languages whose server could not be started. Never retried in this
    /// session: a missing binary is not going to appear mid-session.
    unavailable: HashMap<Language, String>,
    /// Crashes per language, across restarts. This lives on the session, not
    /// on the `Server`, because a restarted server is a new object: keeping
    /// the count in the `Server` reset it and a crashing server restarted
    /// forever (found by running the fake crash server).
    crashes: HashMap<Language, u32>,
    /// One status line the app has not shown yet ("clangd not found").
    message: Option<String>,
    /// Workspace root given to a server as `rootUri`. `None` for a loose file.
    root: Option<PathBuf>,
    notify: Notify,
}

/// What the app should do about an event from a server.
#[derive(Debug)]
pub enum Routed {
    /// Nothing to do (a log line, a response we do not use).
    Ignored,
    /// The app should redraw: a server became ready, or moved.
    Changed,
    /// One line for the status bar (already stored in `Lsp::message`, so the
    /// app can read it with the same `take_message` call it uses elsewhere).
    Message,
    /// A server published diagnostics; the app resolves the buffer and calls
    /// `store_diagnostics`.
    Diagnostics {
        uri: String,
        version: Option<i64>,
        items: Vec<RawDiagnostic>,
    },
}

impl Lsp {
    pub fn new(cfg: &Config, notify: Notify) -> Self {
        let mut lsp = Self {
            enabled: cfg.lsp,
            commands: HashMap::new(),
            servers: HashMap::new(),
            docs: HashMap::new(),
            unavailable: HashMap::new(),
            crashes: HashMap::new(),
            message: None,
            root: None,
            notify,
        };
        lsp.configure(cfg);
        lsp
    }

    /// Re-read the toggle and the server commands. Called when the settings
    /// page changes and once at startup.
    pub fn configure(&mut self, cfg: &Config) {
        self.commands.clear();
        for lang in Language::ALL {
            let configured = cfg.lsp_command(lang.key());
            // `Some(Some(cmd))` for a normal line, `Some(None)` when the line
            // exists but the program comes from the default, `None` when the
            // file never mentions it.
            self.commands.insert(
                lang,
                configured
                    .map(|s| ServerCommand::parse(&s))
                    .unwrap_or_else(|| ServerCommand::parse(lang.default_command())),
            );
        }
        let was = self.enabled;
        self.enabled = cfg.lsp;
        if was && !self.enabled {
            // Turning servers off stops them now, not at quit.
            self.shutdown();
            self.docs.clear();
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Set the folder a server should treat as its workspace. Called when a
    /// project is opened; servers started earlier keep the root they had.
    pub fn set_root(&mut self, root: Option<&Path>) {
        let new = root.map(Path::to_path_buf);
        if new != self.root {
            // A different workspace invalidates the running servers' view, so
            // stop them; they restart on the next file of their language.
            self.shutdown();
            self.docs.clear();
            self.unavailable.clear();
            // A new workspace is a new chance for a server that failed here.
            self.crashes.clear();
            self.root = new;
        }
    }

    /// The one-line message the status bar should show now, if any.
    pub fn take_message(&mut self) -> Option<String> {
        self.message.take()
    }

    /// A file was opened (or became active for the first time): tell its
    /// server about it, starting that server if this is its first file.
    pub fn open(&mut self, path: &Path, text: &str) {
        if !self.enabled || self.docs.contains_key(path) {
            return;
        }
        let Some(language) = Language::for_path(path) else {
            return;
        };
        if self.unavailable.contains_key(&language) {
            return;
        }
        if !self.servers.contains_key(&language) && self.start(language).is_err() {
            return; // start() already recorded the message
        }
        let Some(server) = self.servers.get_mut(&language) else {
            return;
        };
        let uri = client::path_to_uri(path);
        let version = 1;
        server
            .client
            .did_open(&uri, language.language_id(), version, text);
        self.docs.insert(
            path.to_path_buf(),
            Document {
                uri,
                language,
                version,
                pending: Vec::new(),
                first_edit: None,
                diagnostics: Diagnostics::default(),
            },
        );
    }

    /// The document was closed: clear its diagnostics and stop tracking it.
    pub fn close(&mut self, path: &Path) {
        let Some(doc) = self.docs.remove(path) else {
            return;
        };
        if let Some(server) = self.servers.get_mut(&doc.language) {
            server.client.did_close(&doc.uri);
        }
    }

    /// One text change, already applied to `buffer`.
    ///
    /// `start` and `removed` are character offsets in the *pre-edit* buffer and
    /// `added` counts the inserted characters, exactly what
    /// `Buffer::last_edit` records. Positions are converted here, using the
    /// post-edit buffer, which is the only state where the inserted text is
    /// where the server will put it.
    pub fn changed(
        &mut self,
        path: &Path,
        buffer: &Buffer,
        start: usize,
        removed: usize,
        added: usize,
    ) {
        let Some(doc) = self.docs.get_mut(path) else {
            return;
        };
        let enc = self
            .servers
            .get(&doc.language)
            .map(|s| s.encoding)
            .unwrap_or_default();
        let from = char_to_pos(buffer, start, enc);
        let to = char_to_pos(buffer, start + added, enc);
        let inserted = buffer
            .rope
            .slice(start..(start + added).min(buffer.rope.len_chars()))
            .to_string();
        doc.pending.push((from, to, inserted));
        doc.first_edit.get_or_insert_with(Instant::now);
        // Keep the squiggles on the right characters while the server has not
        // republished yet (spec 7.3: shift on edit).
        doc.diagnostics.shift(start, removed, added);
    }

    /// The document was written to disk: send `didSave` immediately, and any
    /// pending change first so the server sees the saved text.
    pub fn saved(&mut self, path: &Path, text: &str) {
        let Some(doc) = self.docs.get_mut(path) else {
            return;
        };
        let Some(server) = self.servers.get_mut(&doc.language) else {
            return;
        };
        if !doc.pending.is_empty() {
            flush_document(&mut server.client, doc);
        }
        server.client.did_save(&doc.uri, Some(text));
    }

    /// When the next debounce or restart is due. `None` means the event loop
    /// can `Wait` instead of `WaitUntil`.
    pub fn next_deadline(&self) -> Option<Instant> {
        let pending = self
            .docs
            .values()
            .filter_map(|d| d.first_edit.map(|t| t + DEBOUNCE));
        let restarts = self.servers.values().filter_map(|s| s.restart_at);
        pending.chain(restarts).min()
    }

    /// Do whatever is due at `now`: send debounced changes, restart servers.
    /// Returns true when a redraw is needed (a restart changed the status).
    pub fn pump(&mut self, now: Instant) -> bool {
        let mut changed = false;
        // Restart first: a restarted server has no documents yet, and the
        // reopen below will hand it the current text.
        let restarting: Vec<Language> = self
            .servers
            .iter()
            .filter(|(_, s)| s.restart_at.is_some_and(|t| t <= now))
            .map(|(l, _)| *l)
            .collect();
        for language in restarting {
            self.servers.remove(&language);
            if self.start(language).is_ok() {
                changed = true;
                self.reopen_documents(language);
            } else {
                changed = true;
            }
        }

        let due: Vec<PathBuf> = self
            .docs
            .iter()
            .filter(|(_, d)| d.first_edit.is_some_and(|t| t + DEBOUNCE <= now))
            .map(|(p, _)| p.clone())
            .collect();
        for path in due {
            let Some(doc) = self.docs.get_mut(&path) else {
                continue;
            };
            if let Some(server) = self.servers.get_mut(&doc.language) {
                flush_document(&mut server.client, doc);
            }
        }
        changed
    }

    fn reopen_documents(&mut self, language: Language) {
        // `open` needs the text, which only the app has. So this just drops the
        // stale entries: the app reopens the active buffer on its next frame.
        self.docs.retain(|_, d| d.language != language);
    }

    /// Start one server. Records the message and the unavailable flag itself,
    /// so every caller behaves the same.
    fn start(&mut self, language: Language) -> Result<(), ()> {
        let command = match self.commands.get(&language).and_then(|c| c.clone()) {
            Some(cmd) => cmd,
            None => {
                self.unavailable
                    .insert(language, "no server configured".into());
                return Err(());
            }
        };
        let notify = self.notify.clone();
        match Client::spawn(language, &command, self.root_hint(), notify) {
            Ok(client) => {
                self.servers.insert(
                    language,
                    Server {
                        client,
                        encoding: Encoding::default(),
                        triggers: Vec::new(),
                        restart_at: None,
                    },
                );
                Ok(())
            }
            Err(why) => {
                let message = format!("{}: {why}", language.key());
                self.message = Some(message.clone());
                self.unavailable.insert(language, message);
                Err(())
            }
        }
    }

    /// The workspace root. The app sets it when a folder is opened; it is
    /// only a hint for the server's `rootUri`.
    fn root_hint(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Route one event from a server. Diagnostics are handed back to the app,
    /// which owns the buffers.
    pub fn handle(&mut self, event: LspEvent) -> Routed {
        match event {
            LspEvent::Ready {
                language,
                encoding,
                triggers,
            } => {
                if let Some(server) = self.servers.get_mut(&language) {
                    server.encoding = encoding;
                    server.triggers = triggers;
                }
                Routed::Changed
            }
            LspEvent::Failed { language, why } => {
                self.unavailable.insert(language, why.clone());
                self.servers.remove(&language);
                self.message = Some(format!("{}: {why}", language.key()));
                Routed::Message
            }
            LspEvent::Exited { language } => {
                if !self.servers.contains_key(&language) {
                    return Routed::Ignored;
                }
                let crashes = self.crashes.entry(language).or_insert(0);
                // One retry per session (section 7.1). A second crash means the
                // server cannot run here; say so once and stop.
                if *crashes >= 1 {
                    self.servers.remove(&language);
                    self.unavailable.insert(language, "server stopped".into());
                    self.message = Some(format!("{} stopped", language.key()));
                    return Routed::Message;
                }
                *crashes += 1;
                if let Some(server) = self.servers.get_mut(&language) {
                    server.restart_at = Some(Instant::now() + RESTART_DELAY);
                }
                Routed::Changed
            }
            LspEvent::Diagnostics {
                language: _,
                uri,
                version,
                diagnostics,
            } => Routed::Diagnostics {
                uri,
                version,
                items: diagnostics,
            },
        }
    }

    /// Store one publish, converting its positions with `buffer` (the file the
    /// app matched to `uri`). Returns true when anything changed.
    pub fn store_diagnostics(
        &mut self,
        uri: &str,
        version: Option<i64>,
        items: Vec<RawDiagnostic>,
        buffer: &Buffer,
    ) -> bool {
        let Some(doc) = self.docs.values_mut().find(|d| d.uri == uri) else {
            return false;
        };
        // A publish for an older version than we have is stale: ignore it,
        // or the squiggles jump back to where they were two edits ago.
        if let Some(v) = version {
            if v < doc.version {
                return false;
            }
        }
        let enc = self
            .servers
            .get(&doc.language)
            .map(|s| s.encoding)
            .unwrap_or_default();
        let converted: Vec<Diagnostic> = items
            .into_iter()
            .map(|raw| Diagnostic {
                range: range_to_chars(buffer, raw.start, raw.end, enc),
                severity: diagnostics::Severity::from_lsp(raw.severity),
                message: raw.message,
                source: raw.source,
            })
            .collect();
        let changed = converted.len() != doc.diagnostics.items().len()
            || converted
                .iter()
                .zip(doc.diagnostics.items())
                .any(|(a, b)| a != b);
        doc.diagnostics.replace(converted);
        changed
    }

    /// Diagnostics for one file. Empty when the file is not tracked (no
    /// server, or the LSP feature is off).
    pub fn diagnostics(&self, path: &Path) -> &Diagnostics {
        static EMPTY: std::sync::OnceLock<Diagnostics> = std::sync::OnceLock::new();
        match self.docs.get(path) {
            Some(doc) => &doc.diagnostics,
            None => EMPTY.get_or_init(Diagnostics::default),
        }
    }

    /// One short string per running server, for the F3 overlay.
    pub fn server_status(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .servers
            .iter()
            .map(|(lang, server)| {
                format!(
                    "{}: {} {}",
                    lang.key(),
                    server.encoding.label(),
                    if server.client.ready {
                        "ready"
                    } else {
                        "starting"
                    }
                )
            })
            .collect();
        out.sort();
        out
    }

    /// Stop every server. Idempotent; called from the quit path.
    pub fn shutdown(&mut self) {
        for (_, mut server) in self.servers.drain() {
            server.client.shutdown();
        }
    }
}

impl Drop for Lsp {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Send every queued change for one document as a single notification and
/// bump its version.
fn flush_document(client: &mut Client, doc: &mut Document) {
    if doc.pending.is_empty() {
        return;
    }
    doc.version += 1;
    let version = doc.version;
    let uri = doc.uri.clone();
    // One `didChange` per queued change: LSP allows several contentChanges in
    // one notification, and the writer thread sends them back to back anyway.
    // Splitting keeps the client API to the single-range case it needs.
    for (from, to, text) in doc.pending.drain(..) {
        client.did_change_range(&uri, version, from, to, &text);
    }
    doc.first_edit = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lsp(notify: Notify) -> Lsp {
        Lsp::new(&Config::default(), notify)
    }

    fn no_notify() -> Notify {
        Arc::new(|_| {})
    }

    #[test]
    fn a_file_without_a_language_is_not_tracked() {
        let mut l = lsp(no_notify());
        l.open(Path::new("/tmp/notes.txt"), "hello");
        assert!(l.docs.is_empty());
        assert!(l.diagnostics(Path::new("/tmp/notes.txt")).is_empty());
    }

    #[test]
    fn a_missing_server_sets_one_message_and_is_not_retried() {
        let mut l = lsp(no_notify());
        // Point rust at a binary that cannot exist, keeping the test offline.
        l.commands.insert(
            Language::Rust,
            Some(ServerCommand {
                program: "/nonexistent/rust-analyzer".into(),
                args: vec![],
            }),
        );
        l.open(Path::new("/tmp/a.rs"), "fn main() {}");
        assert!(l.docs.is_empty(), "no document without a server");
        let msg = l.take_message().expect("one status message");
        assert!(msg.contains("rust"));
        assert!(l.unavailable.contains_key(&Language::Rust));
        assert!(l.take_message().is_none(), "the message is shown once");
        // A second file of the same language does not try again.
        l.open(Path::new("/tmp/b.rs"), "fn main() {}");
        assert!(l.take_message().is_none());
        assert!(l.servers.is_empty());
    }

    #[test]
    fn an_edit_queues_a_change_and_shifts_the_squiggles() {
        let mut l = lsp(no_notify());
        let buffer = Buffer::from_text("let x = 1;\n");
        let path = Path::new("/tmp/x.rs");
        // Pretend the document is open without starting a server.
        l.docs.insert(
            path.to_path_buf(),
            Document {
                uri: "file:///tmp/x.rs".into(),
                language: Language::Rust,
                version: 1,
                pending: Vec::new(),
                first_edit: None,
                diagnostics: Diagnostics::default(),
            },
        );
        l.changed(path, &buffer, 4, 0, 1);
        let doc = &l.docs[path];
        assert_eq!(doc.pending.len(), 1);
        assert_eq!(doc.pending[0].2, "x");
        assert!(doc.first_edit.is_some());
        assert!(l.next_deadline().is_some());
    }

    #[test]
    fn a_stale_publish_is_ignored() {
        let mut l = lsp(no_notify());
        let path = Path::new("/tmp/x.rs");
        let buffer = Buffer::from_text("abc\n");
        l.docs.insert(
            path.to_path_buf(),
            Document {
                uri: "file:///tmp/x.rs".into(),
                language: Language::Rust,
                version: 7,
                pending: Vec::new(),
                first_edit: None,
                diagnostics: Diagnostics::default(),
            },
        );
        let raw = RawDiagnostic {
            start: Pos::new(0, 0),
            end: Pos::new(0, 1),
            severity: 1,
            message: "old".into(),
            source: None,
        };
        assert!(!l.store_diagnostics("file:///tmp/x.rs", Some(3), vec![raw.clone()], &buffer));
        assert!(l.diagnostics(path).is_empty());
        assert!(l.store_diagnostics("file:///tmp/x.rs", Some(9), vec![raw], &buffer));
        assert_eq!(l.diagnostics(path).counts(), (1, 0));
    }

    #[test]
    fn a_crash_gets_one_restart_then_gives_up() {
        let mut l = lsp(no_notify());
        l.servers.insert(
            Language::Cpp,
            Server {
                // A client needs a process; use `true`, which exits at once.
                client: Client::spawn(
                    Language::Cpp,
                    &ServerCommand {
                        program: "true".into(),
                        args: vec![],
                    },
                    None,
                    no_notify(),
                )
                .unwrap(),
                encoding: Encoding::default(),
                triggers: Vec::new(),
                restart_at: None,
            },
        );
        assert!(matches!(
            l.handle(LspEvent::Exited {
                language: Language::Cpp
            }),
            Routed::Changed
        ));
        // The count lives on the session, so a restarted server cannot reset it.
        assert_eq!(l.crashes[&Language::Cpp], 1);
        assert!(matches!(
            l.handle(LspEvent::Exited {
                language: Language::Cpp
            }),
            Routed::Message
        ));
        assert!(!l.servers.contains_key(&Language::Cpp));
    }
}
