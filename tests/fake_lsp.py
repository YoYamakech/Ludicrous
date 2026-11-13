#!/usr/bin/env python3
"""A tiny fake language server, for testing the Ludicrous LSP client.

Speaks the real protocol (Content-Length framing over stdio) but all it does is:
  * answer `initialize` with utf-16 and a trigger character list;
  * on `didOpen`/`didChange`/`didSave`, publish one diagnostic per line that
    contains the text `BAD` (error) or `warn` (warning);
  * answer `shutdown` and exit on `exit`.

That is enough to check framing, position conversion (it folds a non-ASCII
prefix into the range), the debounce, the hover tooltip and process cleanup,
without one gigabyte of clang.
"""
import json
import sys
import threading

VERSION = 1


def read_message(fin):
    headers = {}
    while True:
        line = fin.readline()
        if not line:
            return None
        if line in (b"\r\n", b"\n"):
            break
        name, _, value = line.decode().partition(":")
        headers[name.strip().lower()] = value.strip()
    length = int(headers.get("content-length", "0"))
    body = fin.read(length)
    return json.loads(body)


def send(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(data))
    sys.stdout.buffer.write(data)
    sys.stdout.buffer.flush()


lock = threading.Lock()
documents = {}


def publish(uri):
    text = documents.get(uri, "")
    diags = []
    for lineno, line in enumerate(text.split("\n")):
        for needle, severity, message in (
            ("BAD", 1, "deliberate error: expected expression"),
            ("warn", 2, "this is only a warning, and its message is long enough "
                        "to need clipping with an ellipsis at sixty-four columns"),
        ):
            col = line.find(needle)
            if col < 0:
                continue
            # Range in UTF-16 units, like a real server: count the units before
            # the needle so a non-ASCII prefix lands on the right characters.
            start = len(line[:col].encode("utf-16-le")) // 2
            end = start + len(needle)
            diags.append({
                "range": {
                    "start": {"line": lineno, "character": start},
                    "end": {"line": lineno, "character": end},
                },
                "severity": severity,
                "source": "fakeserver",
                "message": message,
            })
    send({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {"uri": uri, "version": VERSION, "diagnostics": diags},
    })


def main():
    global VERSION
    log = open("/tmp/fake-lsp.log", "a", buffering=1)
    log.write("fake server started\n")
    fin = sys.stdin.buffer
    while True:
        msg = read_message(fin)
        if msg is None:
            log.write("stdin closed\n")
            return
        log.write("recv %s\n" % msg.get("method", "response id=%s" % msg.get("id")))
        method = msg.get("method")
        if method == "initialize":
            send({
                "jsonrpc": "2.0",
                "id": msg["id"],
                "result": {
                    "capabilities": {
                        "positionEncoding": "utf-16",
                        "completionProvider": {"triggerCharacters": [".", ":"]},
                        "textDocumentSync": {"openClose": True, "change": 2},
                    },
                    "serverInfo": {"name": "fakeserver", "version": "1"},
                },
            })
        elif method == "shutdown":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
        elif method == "exit":
            log.write("exit: leaving\n")
            return
        elif method == "textDocument/didOpen":
            uri = msg["params"]["textDocument"]["uri"]
            documents[uri] = msg["params"]["textDocument"]["text"]
            VERSION = msg["params"]["textDocument"]["version"]
            publish(uri)
        elif method == "textDocument/didChange":
            params = msg["params"]
            uri = params["textDocument"]["uri"]
            VERSION = params["textDocument"]["version"]
            text = documents.get(uri, "")
            # Apply the single range the client sends, in UTF-16 units.
            for change in params["contentChanges"]:
                if "range" not in change:
                    text = change["text"]
                    continue
                r = change["range"]
                lines = text.split("\n")

                def offset(pos):
                    line, char = pos["line"], pos["character"]
                    return sum(len(l) + 1 for l in lines[:line]) + char

                start = offset(r["start"])
                end = offset(r["end"])
                text = text[:start] + change["text"] + text[end:]
            documents[uri] = text
            publish(uri)
        elif method == "textDocument/didSave":
            publish(msg["params"]["textDocument"]["uri"])
        elif method == "textDocument/didClose":
            uri = msg["params"]["textDocument"]["uri"]
            documents.pop(uri, None)
            send({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {"uri": uri, "diagnostics": []},
            })
        elif "id" in msg:
            # Something we do not implement: answer so the client never blocks.
            send({"jsonrpc": "2.0", "id": msg["id"], "result": None})


main()
