//! The control channel's requests, answered on a [`Document`]: the same answers whether they come
//! over stdin (`modelcraft-cli control`), the loopback socket of the app or of `modelcraft-cli
//! serve` (craft-control, token-authenticated), or an MCP client (`modelcraft-cli mcp`).
//!
//! | Method | Params | Result |
//! |---|---|---|
//! | `engine.execute` | a [`Command`] (`{"cmd": …}`), or `{"command": {…}}` | the [`crate::Outcome`] |
//! | `engine.commands` | — | `[{name, help}]` |
//! | `document.info` | — | `{path, dirty, revision, objects: [{id, name, type, depth, parent, visible, faces, steps}], undo, redo}` |
//! | `document.revision` | — | `{document_id, revision}` |
//! | `document.save` | `{path?}` | `{path}` |
//! | `document.export` | `{path}` (.glb, .obj, .stl) | `{path}` |
//!
//! Every reply is `{"ok": true, "result": …}` or `{"ok": false, "error": "…"}`.

use std::path::Path;

use serde_json::{Value, json};

use crate::{COMMANDS, Command, Document};

/// Longest request line any channel accepts (a long sculpt stroke is a few MiB).
pub const MAX_LINE: usize = 64 << 20;

/// The methods [`handle`] answers.
pub const METHODS: &[&str] = &["engine.execute", "engine.commands", "document.info", "document.revision", "document.save", "document.export"];

fn ok(result: Value) -> Value {
    json!({"ok": true, "result": result})
}

fn fail(error: impl Into<String>) -> Value {
    json!({"ok": false, "error": error.into()})
}

/// What a request does, for the channel's messages and an MCP client's approval prompts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Reads,
    WritesFile,
    Changes,
}

pub fn effect(method: &str) -> Effect {
    match method {
        "engine.commands" | "document.info" | "document.revision" => Effect::Reads,
        "document.save" | "document.export" => Effect::WritesFile,
        _ => Effect::Changes,
    }
}

/// Answer one request on `doc`.
pub fn handle(doc: &mut Document, method: &str, params: &Value) -> Value {
    match method {
        "engine.execute" => {
            let v = params.get("command").cloned().unwrap_or_else(|| params.clone());
            let cmd = match serde_json::from_value::<Command>(v) {
                Ok(c) => c,
                Err(e) => return fail(format!("not a command: {e} (`engine.commands` lists them)")),
            };
            match doc.execute(cmd) {
                Ok(out) => ok(serde_json::to_value(out).unwrap_or(Value::Null)),
                Err(e) => fail(e.to_string()),
            }
        }
        "engine.commands" => ok(COMMANDS.iter().map(|(name, help)| json!({"name": name, "help": help})).collect()),
        "document.info" => ok(info(doc)),
        "document.revision" => ok(json!({"document_id": document_id(doc), "revision": doc.revision})),
        "document.save" => {
            let path = params.get("path").and_then(Value::as_str).filter(|p| !p.is_empty());
            let r = match path {
                Some(p) => doc.save_as(Path::new(p)),
                None => doc.save(),
            };
            match r {
                Ok(()) => ok(json!({"path": doc.path.as_ref().map(|p| p.display().to_string())})),
                Err(e) => fail(e.to_string()),
            }
        }
        "document.export" => {
            let Some(path) = params.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()) else {
                return fail("document.export needs a path (.glb, .obj or .stl)");
            };
            match doc.export(Path::new(path)) {
                Ok(()) => ok(json!({"path": path})),
                Err(e) => fail(e.to_string()),
            }
        }
        other => fail(format!("unknown method {other:?}; methods: {}", METHODS.join(", "))),
    }
}

/// The document's id for `document.revision`: its file, or `untitled`.
pub fn document_id(doc: &Document) -> String {
    doc.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "untitled".into())
}

/// The object tree with polygon counts and the undo labels.
pub fn info(doc: &mut Document) -> Value {
    doc.sync_sculpt();
    let objects: Vec<Value> = doc
        .scene
        .tree_order()
        .into_iter()
        .filter_map(|(id, depth)| {
            let o = doc.scene.get(id)?.clone();
            let m = doc.eval.evaluate(&doc.scene, id).mesh;
            Some(json!({
                "id": id.0, "name": o.name, "type": o.kind.type_name(), "depth": depth,
                "parent": o.parent.map(|p| p.0), "visible": o.visible,
                "faces": m.face_count(), "steps": o.steps().len(),
            }))
        })
        .collect();
    json!({
        "path": doc.path.as_ref().map(|p| p.display().to_string()),
        "dirty": doc.dirty,
        "revision": doc.revision,
        "objects": objects,
        "undo": doc.undo.labels(),
        "redo": doc.undo.redo_labels(),
    })
}

/// The control protocol as craft-control's server needs it.
pub struct Protocol;

impl craft_control::server::Protocol for Protocol {
    fn summary(&self, method: &str, params: &Value) -> craft_control::server::Summary {
        let command = match method {
            "engine.execute" => {
                params.get("command").unwrap_or(params).get("cmd").and_then(Value::as_str).and_then(|c| COMMANDS.iter().find(|(n, _)| *n == c)).map(|(n, _)| *n)
            }
            _ => None,
        };
        let what = match command {
            Some(c) => format!("`{method}` ({c})"),
            None => format!("`{method}`"),
        };
        let effect = match effect(method) {
            Effect::Reads => craft_control::Effect::ReadsOnly,
            Effect::WritesFile => craft_control::Effect::WritesFile,
            Effect::Changes => craft_control::Effect::Changes,
        };
        craft_control::server::Summary { what, effect, command, steps: None, long: matches!(method, "document.save" | "document.export") }
    }
    fn document(&self) -> &'static str {
        "scene"
    }
    fn change_event(&self) -> &'static str {
        "scene.changed"
    }
    fn revision_method(&self) -> &'static str {
        "document.revision"
    }
    fn docs(&self) -> &'static str {
        "docs/cli.md (Control channel)"
    }
    fn capabilities(&self) -> Value {
        json!({"methods": METHODS, "commands": COMMANDS.iter().map(|(n, _)| *n).collect::<Vec<_>>()})
    }
}

/// Where ModelCraft's control token files live and how its environment is named.
pub fn app_identity() -> craft_control::auth::App {
    craft_control::auth::App { name: "ModelCraft", data_dir, env, utc, default_port: DEFAULT_PORT }
}

/// The port tried first (then the next [`craft_control::auth::PORT_TRIES`]).
pub const DEFAULT_PORT: u16 = 28_730;

fn data_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
        .or_else(|| std::env::var_os("APPDATA").map(std::path::PathBuf::from))?;
    Some(base.join("modelcraft"))
}

fn env(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(format!("MODELCRAFT_{name}"))
}

fn utc(secs: u64) -> String {
    chrono::DateTime::from_timestamp(i64::try_from(secs).unwrap_or(i64::MAX), 0).map(|t| t.to_rfc3339()).unwrap_or_default()
}

/// A listening control channel: the socket's port and the token file to remove at exit.
pub struct Listening {
    pub port: u16,
    pub token: String,
    pub token_file: Option<std::path::PathBuf>,
    pub requests: std::sync::mpsc::Receiver<craft_control::ControlRequest>,
    stop: Option<craft_control::server::Stop>,
}

impl Listening {
    /// Stop listening and remove the token and discovery files.
    pub fn close(mut self) {
        self.cleanup();
    }
    fn cleanup(&mut self) {
        if let Some(s) = self.stop.take() {
            s.stop();
        }
        if let Some(f) = self.token_file.take() {
            craft_control::auth::remove_token_file(&f, &self.token);
            app_identity().remove_current_port(self.port, &self.token);
        }
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Open the loopback channel: `port` exactly, or the default port and the next free ones. The
/// token is `token` when given (tests), else `MODELCRAFT_CONTROL_TOKEN`, else made fresh and
/// written to a file only the user can read. `wake` is called when a request arrives.
pub fn listen(port: Option<u16>, token: Option<String>, wake: impl Fn() + Send + Sync + 'static) -> std::io::Result<Listening> {
    use craft_control::auth;
    let app = app_identity();
    let listener = match port {
        Some(p) => std::net::TcpListener::bind(("127.0.0.1", p))?,
        None => auth::bind_with_fallback(DEFAULT_PORT, auth::PORT_TRIES)?,
    };
    let port = listener.local_addr()?.port();
    let (token, token_file) = match token.filter(|t| t.len() >= auth::MIN_TOKEN_LEN).or_else(|| app.env_token()) {
        Some(t) => (t, None),
        None => {
            let t = craft_control::server::new_token().ok_or_else(|| std::io::Error::other("no random token"))?;
            let f = app.token_path(port).ok_or_else(|| std::io::Error::other("no data folder for the token file"))?;
            app.write_token_file(&f, &t)?;
            let _ = app.write_current_port(port);
            (t, Some(f))
        }
    };
    let hub = std::sync::Arc::new(craft_control::server::Hub::new(Protocol, wake, MAX_LINE));
    let (tx, rx) = std::sync::mpsc::channel();
    let stop = hub.listen_stoppable(listener, token.clone(), tx, false)?;
    Ok(Listening { port, token, token_file, requests: rx, stop: Some(stop) })
}

/// Answer every request waiting on `l` (call it each frame, or in a loop).
pub fn answer_pending(doc: &mut Document, l: &Listening) -> usize {
    let mut n = 0;
    while let Ok(req) = l.requests.try_recv() {
        answer(doc, req);
        n += 1;
    }
    n
}

/// Answer one request.
pub fn answer(doc: &mut Document, req: craft_control::ControlRequest) {
    req.started.store(true, std::sync::atomic::Ordering::Relaxed);
    let reply = handle(doc, &req.method, &req.params);
    let _ = req.reply.send(reply);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    fn requests_answer_on_the_document() {
        let mut d = Document::new();
        let r = handle(&mut d, "engine.execute", &json!({"cmd": "add_primitive", "kind": "cube"}));
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["result"]["created"][0], 1);
        let r = handle(&mut d, "engine.execute", &json!({"command": {"cmd": "add_step", "id": 1, "op": {"op": "subdivide", "levels": 1}}}));
        assert_eq!(r["ok"], true, "{r}");
        let info = handle(&mut d, "document.info", &json!({}));
        assert_eq!(info["result"]["objects"][0]["faces"], 24);
        assert_eq!(info["result"]["undo"].as_array().map(Vec::len), Some(2));
        let rev = handle(&mut d, "document.revision", &json!({}))["result"]["revision"].as_u64();
        assert_eq!(rev, Some(2));
        assert_eq!(handle(&mut d, "engine.execute", &json!({"cmd": "undo"}))["ok"], true);
        assert_eq!(handle(&mut d, "document.info", &json!({}))["result"]["redo"][0], "Subdivide");
        // Errors are replies, not failures.
        assert_eq!(handle(&mut d, "engine.execute", &json!({"cmd": "teleport"}))["ok"], false);
        assert_eq!(handle(&mut d, "engine.execute", &json!({"cmd": "delete", "ids": [99]}))["ok"], false);
        assert_eq!(handle(&mut d, "document.export", &json!({}))["ok"], false);
        assert_eq!(handle(&mut d, "document.save", &json!({}))["ok"], false, "untitled and no path");
        assert_eq!(handle(&mut d, "nope", &json!({}))["ok"], false);
        assert!(handle(&mut d, "engine.commands", &json!({}))["result"].as_array().is_some_and(|a| a.len() == COMMANDS.len()));
    }

    #[test]
    fn the_socket_needs_the_token_and_answers_commands() {
        let dir = std::env::temp_dir().join(format!("mc-control-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let token = "0123456789abcdef0123456789abcdef0123456789abcdef";
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let hub = std::sync::Arc::new(craft_control::server::Hub::new(Protocol, || {}, MAX_LINE));
        let (tx, rx) = std::sync::mpsc::channel();
        hub.listen(listener, token.into(), tx, false).unwrap();
        let worker = std::thread::spawn(move || {
            let mut d = Document::new();
            for _ in 0..2 {
                if let Ok(req) = rx.recv_timeout(std::time::Duration::from_secs(10)) {
                    answer(&mut d, req);
                }
            }
            d.scene.len()
        });
        let ask = |s: &mut std::net::TcpStream, v: Value| -> Value {
            writeln!(s, "{v}").unwrap();
            let mut line = String::new();
            BufReader::new(s.try_clone().unwrap()).read_line(&mut line).unwrap();
            serde_json::from_str(&line).unwrap()
        };
        let mut wrong = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let r = ask(&mut wrong, json!({"id": 1, "method": "document.info", "token": "nope"}));
        assert_eq!(r["ok"], false, "a wrong token is refused: {r}");
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let r = ask(&mut s, json!({"id": 1, "method": "engine.execute", "params": {"cmd": "add_primitive", "kind": "torus"}, "token": token}));
        assert_eq!(r["ok"], true, "{r}");
        let r = ask(&mut s, json!({"id": 2, "method": "document.info"}));
        assert_eq!(r["result"]["objects"][0]["name"], "Torus", "{r}");
        assert_eq!(worker.join().unwrap(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
