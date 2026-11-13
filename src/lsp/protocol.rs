// protocol.rs: JSON-RPC framing over the server's stdin/stdout.
//
// LSP frames every message as `Content-Length: N\r\n\r\n<n bytes of JSON>`.
// That is the whole transport; there is no HTTP and no async runtime. Reading
// is incremental because a pipe hands us arbitrary chunks: the reader keeps a
// buffer and pulls one message out whenever it has a whole one.
//
// No types beyond serde_json here: this module only knows how to cut the byte
// stream into messages and how to build the envelope around a body.

use serde_json::{json, Value};

/// One message that arrived from the server.
#[derive(Clone, Debug, PartialEq)]
pub enum Incoming {
    /// A request from the server (it wants an answer). Almost only
    /// `client/registerCapability` and `workspace/configuration` in practice.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// A notification: diagnostics, progress, logs.
    Notification { method: String, params: Value },
    /// The answer to one of our requests (`id` is the number we sent).
    Response {
        id: Value,
        result: Option<Value>,
        error: Option<Value>,
    },
}

impl Incoming {
    /// The method name for notifications and requests, `None` for responses.
    /// Used for the debug log and for dispatch.
    #[allow(dead_code)] // used by tests and the reader's debug path
    pub fn method(&self) -> Option<&str> {
        match self {
            Incoming::Request { method, .. } | Incoming::Notification { method, .. } => {
                Some(method)
            }
            Incoming::Response { .. } => None,
        }
    }
}

/// A request id. Servers accept numbers and strings; we only ever send
/// numbers, but a server may answer with either, so the id is a `Value`.
pub fn request(id: u64, method: &str, params: Value) -> String {
    body(json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
}

pub fn notification(method: &str, params: Value) -> String {
    body(json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    }))
}

pub fn response_ok(id: &Value, result: Value) -> String {
    body(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

pub fn response_error(id: &Value, code: i64, message: &str) -> String {
    body(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message},
    }))
}

/// Frame one JSON body with its `Content-Length` header.
fn body(value: Value) -> String {
    let text = value.to_string();
    format!("Content-Length: {}\r\n\r\n{}", text.len(), text)
}

/// Parse one complete message. Bad JSON is reported, not panicked on: a broken
/// server message must not take the editor down (and one bad line must not
/// wedge the reader, which drops it and carries on).
pub fn parse_message(text: &str) -> Result<Incoming, String> {
    let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let method = value.get("method").and_then(Value::as_str);
    match method {
        Some(method) => {
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            match value.get("id") {
                Some(id) if !id.is_null() => Ok(Incoming::Request {
                    id: id.clone(),
                    method: method.to_string(),
                    params,
                }),
                _ => Ok(Incoming::Notification {
                    method: method.to_string(),
                    params,
                }),
            }
        }
        None => {
            // A response: either `result` or `error`, never both.
            let id = value.get("id").cloned().unwrap_or(Value::Null);
            Ok(Incoming::Response {
                id,
                result: value.get("result").cloned(),
                error: value.get("error").cloned(),
            })
        }
    }
}

/// Incremental frame reader. Feed it bytes, ask for messages.
#[derive(Default)]
pub struct Framer {
    buf: Vec<u8>,
}

/// How large a single message may get before the reader gives up on it. A
/// runaway server (or a corrupt stream) must not grow the buffer forever.
const MAX_MESSAGE: usize = 32 * 1024 * 1024;

impl Framer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Bytes waiting for a header or a body. Only used by tests and the debug
    /// log; an empty buffer after `next()` means everything was consumed.
    #[allow(dead_code)] // tests and the debug log
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    /// Pull the next complete message out of the buffer, or `None` if more
    /// bytes are needed.
    ///
    /// Errors are returned as `Some(Err(..))` and the offending bytes are
    /// dropped, so the caller can log once and keep reading.
    pub fn next(&mut self) -> Option<Result<Incoming, String>> {
        let header_end = find(&self.buf, b"\r\n\r\n")?;
        let header = std::str::from_utf8(&self.buf[..header_end]).ok()?;
        let mut len: Option<usize> = None;
        for line in header.split("\r\n") {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            if name.trim().eq_ignore_ascii_case("content-length") {
                len = value.trim().parse::<usize>().ok();
            }
        }
        let Some(len) = len else {
            // A header block without a length can never be completed; drop it
            // rather than wait for bytes that will not help.
            self.buf.drain(..header_end + 4);
            return Some(Err("message without Content-Length".to_string()));
        };
        if len > MAX_MESSAGE {
            self.buf.clear();
            return Some(Err(format!("message of {len} bytes is too large")));
        }
        let start = header_end + 4;
        if self.buf.len() < start + len {
            return None; // wait for the rest of the body
        }
        let text = match std::str::from_utf8(&self.buf[start..start + len]) {
            Ok(t) => t,
            Err(_) => {
                self.buf.drain(..start + len);
                return Some(Err("message body is not UTF-8".to_string()));
            }
        };
        let parsed = parse_message(text);
        self.buf.drain(..start + len);
        Some(parsed)
    }
}

/// `haystack` starting at `needle`. `[u8]::windows` is clear enough here and
/// avoids a dependency for four bytes.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_framed_with_its_byte_length() {
        let text = request(7, "initialize", json!({"a": "é"}));
        // `é` is two bytes, so the length is bytes, not characters.
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        assert_eq!(head, format!("Content-Length: {}", body.len()));
        assert!(body.contains("\"id\":7"));
        assert_eq!(
            parse_message(body).unwrap(),
            Incoming::Request {
                id: json!(7),
                method: "initialize".into(),
                params: json!({"a": "é"}),
            }
        );
    }

    #[test]
    fn notifications_requests_and_responses_are_told_apart() {
        let n = parse_message(r#"{"jsonrpc":"2.0","method":"exit"}"#).unwrap();
        assert!(matches!(n, Incoming::Notification { .. }));

        let r = parse_message(r#"{"jsonrpc":"2.0","id":"x","method":"m","params":{}}"#).unwrap();
        assert!(matches!(r, Incoming::Request { .. }));

        let ok = parse_message(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#).unwrap();
        assert_eq!(ok.method(), None);
        match ok {
            Incoming::Response { id, result, error } => {
                assert_eq!(id, json!(1));
                assert!(result.is_some());
                assert!(error.is_none());
            }
            _ => panic!("expected a response"),
        }
    }

    #[test]
    fn messages_arrive_in_arbitrary_chunks() {
        let mut f = Framer::new();
        let a = notification("a", json!({}));
        let b = notification("b", json!({"n": 2}));
        let stream = format!("{a}{b}");
        let bytes = stream.as_bytes();
        // One byte at a time: the worst case a pipe can produce.
        let mut got = Vec::new();
        for i in 0..bytes.len() {
            f.feed(&bytes[i..i + 1]);
            while let Some(m) = f.next() {
                got.push(m.unwrap().method().unwrap().to_string());
            }
        }
        assert_eq!(got, ["a", "b"]);
        assert_eq!(f.buffered(), 0);
    }

    #[test]
    fn two_messages_in_one_read_both_come_out() {
        let mut f = Framer::new();
        f.feed(
            format!(
                "{}{}",
                notification("x", json!({})),
                notification("y", json!({}))
            )
            .as_bytes(),
        );
        assert_eq!(f.next().unwrap().unwrap().method(), Some("x"));
        assert_eq!(f.next().unwrap().unwrap().method(), Some("y"));
        assert!(f.next().is_none());
    }

    #[test]
    fn a_broken_message_is_skipped_not_fatal() {
        let mut f = Framer::new();
        // Not JSON, then a good notification: the reader reports the first and
        // still delivers the second.
        f.feed(b"Content-Length: 4\r\n\r\nnope");
        assert!(f.next().unwrap().is_err());
        f.feed(notification("after", json!({})).as_bytes());
        assert_eq!(f.next().unwrap().unwrap().method(), Some("after"));
    }

    #[test]
    fn a_missing_length_header_does_not_block_the_stream() {
        let mut f = Framer::new();
        f.feed(b"Content-Type: json\r\n\r\n");
        assert!(f.next().unwrap().is_err());
        f.feed(notification("next", json!({})).as_bytes());
        assert_eq!(f.next().unwrap().unwrap().method(), Some("next"));
    }
}
