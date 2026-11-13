// client.rs: one language server process, spoken to over stdio.
//
// Three threads, all `std`:
//   * the UI thread sends messages into a channel;
//   * a writer thread copies them into the child's stdin;
//   * a reader thread frames whatever the child writes and hands each message
//     to the notifier, which wakes the event loop.
//
// No async runtime, no polling: the reader thread blocks on `read`, so an idle
// server costs nothing. The notifier is the only way information reaches the UI
// thread, which keeps this module free of winit and of the editor's types.
//
// The client does not know about buffers or files. It speaks in URIs and
// versions, exactly like the protocol, and `lsp::mod` owns the mapping from a
// buffer to a document.

use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::lsp::position::Encoding;
use crate::lsp::protocol::{self, Framer, Incoming};

/// The languages the config can name a server for. Also the key used in the
/// config file (`lsp_rust = rust-analyzer`), so this list is the public name.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Language {
    Rust,
    C,
    Cpp,
    Python,
}

impl Language {
    pub const ALL: [Language; 4] = [Language::Rust, Language::C, Language::Cpp, Language::Python];

    /// Config key suffix and the `languageId` sent to the server.
    pub fn key(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::C => "c",
            Language::Cpp => "cpp",
            Language::Python => "python",
        }
    }

    /// The protocol's `languageId` string.
    pub fn language_id(self) -> &'static str {
        self.key()
    }

    /// The spec's default command for this language (section 7.1). It is only
    /// a default: the config file can replace any of them.
    pub fn default_command(self) -> &'static str {
        match self {
            Language::Rust => "rust-analyzer",
            Language::C | Language::Cpp => "clangd",
            Language::Python => "pyright-langserver --stdio",
        }
    }

    /// Pick a language from a file extension. `None` means "no server", which
    /// is not an error: most files have none.
    pub fn for_path(path: &Path) -> Option<Language> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "rs" => Some(Language::Rust),
            "c" | "h" => Some(Language::C),
            "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => Some(Language::Cpp),
            "py" | "pyi" => Some(Language::Python),
            _ => None,
        }
    }
}

/// One configured server: a program and its arguments, as written in the
/// config file (`pyright-langserver --stdio`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ServerCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl ServerCommand {
    /// Split a config value into program and arguments. An empty value means
    /// "no server for this language". Quoting is deliberately not supported:
    /// the config format is one line per key, and `--stdio` style flags need no
    /// quoting.
    pub fn parse(value: &str) -> Option<ServerCommand> {
        let mut parts = value.split_whitespace();
        let program = parts.next()?.to_string();
        Some(ServerCommand {
            program,
            args: parts.map(str::to_owned).collect(),
        })
    }
}

/// What the client tells the UI thread. Deliberately small and cheap to clone:
/// it is sent across a channel and then dropped after the event loop wakes.
#[derive(Clone, Debug, PartialEq)]
pub enum LspEvent {
    /// `initialize` came back. The encoding is what the server chose, and every
    /// position converted from now on must use it.
    Ready {
        language: Language,
        encoding: Encoding,
        /// Trigger characters the server declared (completion, phase G).
        triggers: Vec<String>,
    },
    /// The server said something went wrong during startup and exited.
    Failed { language: Language, why: String },
    /// The server process ended after a successful start.
    Exited { language: Language },
    /// Diagnostics for one document. The ranges are still in LSP positions;
    /// `lsp::mod` converts them with the buffer it owns.
    Diagnostics {
        language: Language,
        uri: String,
        version: Option<i64>,
        diagnostics: Vec<RawDiagnostic>,
    },
}

/// One diagnostic exactly as published: positions, severity number, message.
#[derive(Clone, Debug, PartialEq)]
pub struct RawDiagnostic {
    pub start: crate::lsp::position::Pos,
    pub end: crate::lsp::position::Pos,
    pub severity: i64,
    pub message: String,
    pub source: Option<String>,
}

/// Everything the writer thread does. Keeping the channel item small means the
/// UI thread never blocks on the pipe.
enum Outgoing {
    Text(String),
}

/// How long a process gets to exit after `exit` before it is killed.
const EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

pub struct Client {
    /// Kept for the F3 overlay and for phase G's per-language dispatch.
    #[allow(dead_code)]
    pub language: Language,
    /// Held in an `Option` so `shutdown` can drop the stdin handle (the child
    /// then sees EOF if it ignores `exit`).
    stdin: Option<Sender<Outgoing>>,
    child: Child,
    reader: Option<std::thread::JoinHandle<()>>,
    writer: Option<std::thread::JoinHandle<()>>,
    /// Set once `initialize` succeeded. Kept for the F3 overlay and for phase
    /// G, which must not ask for a completion from a server still starting.
    pub ready: bool,
    /// Latest request id handed out; shared with the reader thread so it can
    /// match responses (only `initialize` matters in phase F).
    next_id: u64,
}

impl Client {
    /// Start a server for `language`, with the workspace root as `rootUri`
    /// (`None` for a loose file).
    ///
    /// Returns an error when the program is missing or the pipes cannot be
    /// made; the caller turns that into one status bar message and marks the
    /// language unavailable for the session.
    pub fn spawn(
        language: Language,
        command: &ServerCommand,
        root: Option<&Path>,
        notify: Arc<dyn Fn(LspEvent) + Send + Sync>,
    ) -> Result<Client, String> {
        if command.program.is_empty() {
            return Err("no server configured".to_string());
        }
        let mut cmd = Command::new(&command.program);
        cmd.args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The server's own stderr is not our input stream; dropping it
            // keeps the protocol clean. Uncomment to debug a server's logs.
            .stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!("{} not found", command.program),
            _ => format!("{} could not start: {e}", command.program),
        })?;

        let stdin = child.stdin.take().ok_or("no stdin pipe")?;
        let stdout = child.stdout.take().ok_or("no stdout pipe")?;

        let (tx, rx) = mpsc::channel::<Outgoing>();
        let writer = std::thread::Builder::new()
            .name(format!("lsp-write-{}", language.key()))
            .spawn(move || writer_loop(stdin, rx))
            .map_err(|e| e.to_string())?;

        let ready_flag = Arc::new(Mutex::new(false));
        // The reader needs to answer the server's own requests, and stdin is
        // owned by the writer thread: hand it the same channel.
        let replies = tx.clone();
        let reader = std::thread::Builder::new()
            .name(format!("lsp-read-{}", language.key()))
            .spawn(move || reader_loop(language, stdout, notify, ready_flag, replies))
            .map_err(|e| e.to_string())?;

        let mut client = Client {
            language,
            stdin: Some(tx),
            child,
            reader: Some(reader),
            writer: Some(writer),
            ready: false,
            next_id: 0,
        };
        client.initialize(root);
        Ok(client)
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// `initialize`, with the workspace root and the encodings we support.
    fn initialize(&mut self, root: Option<&Path>) {
        let id = self.request_id();
        let root_uri = root.map(path_to_uri);
        let params = json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "Ludicrous", "version": env!("CARGO_PKG_VERSION")},
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    // Phase F only needs diagnostics; the fields here are the
                    // ones a server reads to decide what to send.
                    "publishDiagnostics": {"versionSupport": true},
                    "synchronization": {"dynamicRegistration": false}
                },
                "workspace": {"workspaceFolders": false}
            },
            // Order is a preference: a server that speaks UTF-8 avoids the
            // UTF-16 conversion entirely, and one that does not falls back.
            "positionEncodings": ["utf-8", "utf-16"],
            "workspaceFolders": root_uri.map(|uri| json!([{"uri": uri, "name": "workspace"}]))
        });
        self.send_raw(protocol::request(id, "initialize", params));
        // `initialized` follows immediately; servers accept it before they
        // answer, and clangd wants it to start indexing.
        self.send_raw(protocol::notification("initialized", json!({})));
    }

    fn request_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Queue one framed message for the writer thread.
    fn send_raw(&mut self, text: String) {
        if let Some(tx) = &self.stdin {
            let _ = tx.send(Outgoing::Text(text));
        }
    }

    pub fn did_open(&mut self, uri: &str, language_id: &str, version: i64, text: &str) {
        self.send_raw(protocol::notification(
            "textDocument/didOpen",
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text,
                }
            }),
        ));
    }

    /// One incremental change: a single range replaced by new text. Sending the
    /// whole document instead would be simpler but costs a copy of the file on
    /// every keystroke.
    pub fn did_change_range(
        &mut self,
        uri: &str,
        version: i64,
        start: crate::lsp::position::Pos,
        end: crate::lsp::position::Pos,
        text: &str,
    ) {
        self.send_raw(protocol::notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{
                    "range": {
                        "start": {"line": start.line, "character": start.character},
                        "end": {"line": end.line, "character": end.character},
                    },
                    "text": text,
                }]
            }),
        ));
    }

    pub fn did_save(&mut self, uri: &str, text: Option<&str>) {
        let mut params = json!({"textDocument": {"uri": uri}});
        if let Some(text) = text {
            params["text"] = json!(text);
        }
        self.send_raw(protocol::notification("textDocument/didSave", params));
    }

    pub fn did_close(&mut self, uri: &str) {
        self.send_raw(protocol::notification(
            "textDocument/didClose",
            json!({"textDocument": {"uri": uri}}),
        ));
    }

    /// Ask the server to shut down, then exit, then make sure the process is
    /// gone. Never leaves a zombie: the child is `wait`ed on either way.
    pub fn shutdown(&mut self) {
        if self.is_running() {
            let id = self.request_id();
            self.send_raw(protocol::request(id, "shutdown", Value::Null));
            self.send_raw(protocol::notification("exit", Value::Null));
        }
        // Close stdin so a server ignoring `exit` sees EOF.
        self.stdin = None;
        let deadline = std::time::Instant::now() + EXIT_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                // Still alive after the timeout, or `try_wait` failed: kill.
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        // The reader ends when the pipe closes; it may already be done.
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn writer_loop(mut stdin: ChildStdin, rx: Receiver<Outgoing>) {
    for msg in rx {
        match msg {
            Outgoing::Text(text) => {
                if stdin.write_all(text.as_bytes()).is_err() {
                    return; // the server closed the pipe: nothing left to say
                }
                let _ = stdin.flush();
            }
        }
    }
    // The sender was dropped: the client is shutting down.
    let _ = stdin.flush();
}

fn reader_loop(
    language: Language,
    stdout: std::process::ChildStdout,
    notify: Arc<dyn Fn(LspEvent) + Send + Sync>,
    ready: Arc<Mutex<bool>>,
    replies: Sender<Outgoing>,
) {
    let mut reader = BufReader::new(stdout);
    let mut framer = Framer::new();
    let mut chunk = [0u8; 8192];
    let mut first_error: Option<String> = None;
    loop {
        let n = match reader.read(&mut chunk) {
            Ok(0) => break, // EOF: the server exited
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        framer.feed(&chunk[..n]);
        while let Some(message) = framer.next() {
            match message {
                Ok(Incoming::Response { result, error, .. }) => {
                    if let Some(result) = result {
                        // Only `initialize` is interesting in phase F: it
                        // carries the negotiated encoding and the triggers.
                        if let Some(encoding) = result
                            .get("capabilities")
                            .and_then(|c| c.get("positionEncoding"))
                            .and_then(Value::as_str)
                        {
                            *ready.lock().unwrap() = true;
                            notify(LspEvent::Ready {
                                language,
                                encoding: Encoding::from_server(encoding),
                                triggers: trigger_characters(&result),
                            });
                        }
                    } else if let Some(error) = error {
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("initialize failed");
                        let was_ready = *ready.lock().unwrap();
                        if !was_ready {
                            notify(LspEvent::Failed {
                                language,
                                why: message.to_string(),
                            });
                        }
                    }
                }
                Ok(Incoming::Notification { method, params }) => {
                    if method == "textDocument/publishDiagnostics" {
                        if let Some(event) = diagnostics_event(language, &params) {
                            notify(event);
                        }
                    }
                    // `window/logMessage`, `$/progress` and friends are
                    // ignored in phase F: they are noise on the status bar.
                }
                Ok(Incoming::Request { id, method, .. }) => {
                    // Answer the two requests every server is allowed to make,
                    // so none of them block waiting for a reply.
                    let (result, error) = match method.as_str() {
                        "workspace/configuration" => (json!([null]), None),
                        "client/registerCapability" => (Value::Null, None),
                        other => (
                            Value::Null,
                            Some(format!("ludicrous: unsupported request {other}")),
                        ),
                    };
                    let reply = match error {
                        Some(message) => protocol::response_error(&id, -32601, &message),
                        None => protocol::response_ok(&id, result),
                    };
                    let _ = replies.send(Outgoing::Text(reply));
                }
                Err(e) => {
                    // Log the first bad message only: a broken stream would
                    // otherwise fill the terminal.
                    if first_error.is_none() {
                        eprintln!("ludicrous: lsp {}: {e}", language.key());
                        first_error = Some(e);
                    }
                }
            }
        }
    }
    if *ready.lock().unwrap() {
        notify(LspEvent::Exited { language });
    } else {
        notify(LspEvent::Failed {
            language,
            why: "server exited during startup".to_string(),
        });
    }
}

// Batches are pulled out of the initialize result without a full `lsp-types`
// deserialization: only three fields matter here, and a hand read tolerates a
// server that omits half of them.
fn trigger_characters(result: &Value) -> Vec<String> {
    result
        .get("capabilities")
        .and_then(|c| c.get("completionProvider"))
        .and_then(|c| c.get("triggerCharacters"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Parse one `publishDiagnostics` notification.
fn diagnostics_event(language: Language, params: &Value) -> Option<LspEvent> {
    let uri = params.get("uri")?.as_str()?.to_string();
    let version = params.get("version").and_then(Value::as_i64);
    let list = params.get("diagnostics")?.as_array()?;
    let diagnostics = list
        .iter()
        .filter_map(|d| {
            let range = d.get("range")?;
            let pos = |key: &str| {
                let p = range.get(key)?;
                Some(crate::lsp::position::Pos::new(
                    p.get("line")?.as_u64()? as usize,
                    p.get("character")?.as_u64()? as usize,
                ))
            };
            Some(RawDiagnostic {
                start: pos("start")?,
                end: pos("end")?,
                severity: d.get("severity").and_then(Value::as_i64).unwrap_or(1),
                message: d.get("message")?.as_str()?.to_string(),
                source: d.get("source").and_then(Value::as_str).map(str::to_owned),
            })
        })
        .collect();
    Some(LspEvent::Diagnostics {
        language,
        uri,
        version,
        diagnostics,
    })
}

/// A `file://` URI for an absolute path, with the characters that would
/// otherwise break the URI percent-encoded.
pub fn path_to_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    let text = path.to_string_lossy();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The URI back to a path, for matching a publish to a buffer. Percent
/// sequences are decoded; anything malformed is left as it is.
pub fn uri_to_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).ok()?;
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                bytes.push(b);
                i += 3;
                continue;
            }
        }
        bytes.push(raw[i]);
        i += 1;
    }
    String::from_utf8(bytes).ok().map(std::path::PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_come_from_extensions() {
        let lang = |s: &str| Language::for_path(Path::new(s));
        assert_eq!(lang("main.rs"), Some(Language::Rust));
        assert_eq!(lang("a.h"), Some(Language::C));
        assert_eq!(lang("a.cpp"), Some(Language::Cpp));
        assert_eq!(lang("a.HPP"), Some(Language::Cpp));
        assert_eq!(lang("a.py"), Some(Language::Python));
        assert_eq!(lang("a.txt"), None);
        assert_eq!(lang("Makefile"), None);
    }

    #[test]
    fn default_commands_match_the_spec() {
        assert_eq!(Language::Rust.default_command(), "rust-analyzer");
        assert_eq!(Language::C.default_command(), "clangd");
        assert_eq!(Language::Cpp.default_command(), "clangd");
        assert_eq!(
            ServerCommand::parse(Language::Python.default_command()).unwrap(),
            ServerCommand {
                program: "pyright-langserver".into(),
                args: vec!["--stdio".into()],
            }
        );
    }

    #[test]
    fn a_command_splits_into_program_and_arguments() {
        let c = ServerCommand::parse("clangd --background-index --log=error").unwrap();
        assert_eq!(c.program, "clangd");
        assert_eq!(c.args, ["--background-index", "--log=error"]);
        assert_eq!(ServerCommand::parse("   "), None);
    }

    #[test]
    fn uris_round_trip_paths_with_spaces_and_unicode() {
        let path = Path::new("/tmp/a b/ü.rs");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///tmp/a%20b/%C3%BC.rs");
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
        assert_eq!(uri_to_path("http://x/y"), None);
    }

    #[test]
    fn a_diagnostics_notification_becomes_an_event() {
        let params = json!({
            "uri": "file:///tmp/x.cpp",
            "version": 3,
            "diagnostics": [{
                "range": {"start": {"line": 1, "character": 2},
                          "end": {"line": 1, "character": 5}},
                "severity": 1,
                "source": "clang",
                "message": "expected expression",
            }]
        });
        let event = diagnostics_event(Language::Cpp, &params).unwrap();
        match event {
            LspEvent::Diagnostics {
                uri,
                version,
                diagnostics,
                ..
            } => {
                assert_eq!(uri, "file:///tmp/x.cpp");
                assert_eq!(version, Some(3));
                assert_eq!(diagnostics[0].message, "expected expression");
                assert_eq!(diagnostics[0].source.as_deref(), Some("clang"));
                assert_eq!(diagnostics[0].end.character, 5);
            }
            _ => panic!("expected diagnostics"),
        }
    }

    #[test]
    fn a_malformed_diagnostic_entry_is_skipped_not_fatal() {
        let params = json!({
            "uri": "file:///tmp/x.cpp",
            "diagnostics": [
                {"message": "no range at all"},
                {"range": {"start": {"line": 0, "character": 0},
                           "end": {"line": 0, "character": 1}},
                 "message": "good"},
            ]
        });
        match diagnostics_event(Language::Cpp, &params).unwrap() {
            LspEvent::Diagnostics { diagnostics, .. } => {
                assert_eq!(diagnostics.len(), 1);
                assert_eq!(diagnostics[0].message, "good");
            }
            _ => panic!("expected diagnostics"),
        }
    }

    #[test]
    fn trigger_characters_are_read_from_the_capabilities() {
        let result = json!({"capabilities": {"completionProvider": {
            "triggerCharacters": [".", ":", ">"]
        }}});
        assert_eq!(trigger_characters(&result), [".", ":", ">"]);
        assert!(trigger_characters(&json!({})).is_empty());
    }
}
