//! `modelcraft-cli mcp [project] [--app [--port N]]`: an MCP server on stdio.
//!
//! The protocol (JSON-RPC 2.0, lifecycle, argument checks) is `craft-mcp-server` from craft-libs;
//! the requests are the control channel's (`modelcraft_engine::control`). Without `--app` the
//! server works on its own document (the project, or a new scene); with `--app` every tool call
//! goes to the running ModelCraft window over its token-authenticated loopback channel, so an
//! agent and a person work on the same scene.
//!
//! Tools: `info`, `commands`, `execute` (any engine command, undo and redo included), `save`,
//! `export`.

use std::path::Path;

use craft_mcp_server::{Effect, Info, Server, Tools, object, string, text_result, tool};
use modelcraft_engine::{Document, control};
use serde_json::{Value, json};

pub fn info() -> Info {
    Info { name: "modelcraft", title: "ModelCraft", version: env!("CARGO_PKG_VERSION"), max_line: control::MAX_LINE }
}

/// Where tool calls are answered.
pub enum Backend {
    /// A document of this process.
    Local(Box<Document>, Box<modelcraft_script::channel::ScriptChannel>),
    /// The running app's control channel.
    App(Box<craft_control::client::Connection>),
}

impl Backend {
    fn ask(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match self {
            Backend::Local(d, script) => {
                let r = control::handle_with(d, method, &params, &mut |doc, m, p| script.handle(doc, &mut Vec::new(), m, p));
                if r["ok"] == true { Ok(r["result"].clone()) } else { Err(r["error"].as_str().unwrap_or("failed").to_string()) }
            }
            Backend::App(c) => c.call(craft_control::client::Request::new(method, params)).map_err(|e| e.message().to_string()),
        }
    }
}

pub struct ModelTools {
    pub backend: Backend,
}

/// The tool names and the control method each runs.
const TOOLS: &[(&str, &str)] = &[
    ("info", "document.info"),
    ("commands", "engine.commands"),
    ("execute", "engine.execute"),
    ("save", "document.save"),
    ("export", "document.export"),
    ("script", "script.run"),
];

impl Tools for ModelTools {
    fn instructions(&self) -> String {
        let where_ = match &self.backend {
            Backend::Local(..) => "a document of this server (call `save` to write it)",
            Backend::App(_) => "the scene open in the running ModelCraft window",
        };
        format!(
            "ModelCraft 3D modeling tools on {where_}. Call `info` first: it lists objects with their ids, which commands take. \
             `commands` lists every engine command; `execute` runs one ({{\"cmd\": \"add_primitive\", \"kind\": \"cube\"}}, \
             {{\"cmd\": \"undo\"}}, …) as one undo step."
        )
    }

    fn list(&mut self) -> Vec<Value> {
        let path = |d: &str| string(d);
        vec![
            tool(
                "info",
                "Scene info",
                "Objects (ids, names, types, parents, face counts, history steps) and the undo history, as JSON.",
                object(json!({}), &[]),
                Effect::ReadOnly,
            ),
            tool("commands", "List commands", "Every engine command with its fields.", object(json!({}), &[]), Effect::ReadOnly),
            tool(
                "execute",
                "Run a command",
                "Run one engine command, e.g. {\"cmd\":\"add_step\",\"id\":1,\"op\":{\"op\":\"subdivide\",\"levels\":2}}. Undoable.",
                object(json!({"command": {"type": "object", "description": "The command, with its `cmd` tag"}}), &["command"]),
                Effect::Edits,
            ),
            tool(
                "script",
                "Run a script",
                "Run a Rhai script on the scene (docs/scripting.md): `source` is the script text; `args` an optional object of strings, numbers and booleans read with arg(name, default). Every change is a command and undoable (wrap in undo_group(\"label\", || { ... }) for one step); no file access. Returns ok, value, output lines, command count and, on failure, the error with its line and column.",
                object(
                    json!({"source": string("The script text"), "name": string("A name for error messages (optional)"), "args": {"type": "object", "description": "Arguments for arg(name, default) (optional)"}}),
                    &["source"],
                ),
                Effect::Edits,
            ),
            tool(
                "save",
                "Save",
                "Save the project (atomically), to `path` when given.",
                object(json!({"path": path("Output .modelcraft (optional)")}), &[]),
                Effect::Files,
            ),
            tool("export", "Export", "Export the scene as .glb, .obj or .stl.", object(json!({"path": path("Output file")}), &["path"]), Effect::Files),
        ]
    }

    fn call(&mut self, name: &str, args: &Value) -> Value {
        let Some((_, method)) = TOOLS.iter().find(|(t, _)| *t == name) else {
            return text_result(format!("unknown tool {name}"), true);
        };
        match self.backend.ask(method, args.clone()) {
            // A script that failed is a tool error with the full report, so an agent sees the line.
            Ok(v) if name == "script" && v["ok"] == false => text_result(serde_json::to_string_pretty(&v).unwrap_or_default(), true),
            Ok(v) => text_result(serde_json::to_string_pretty(&v).unwrap_or_default(), false),
            Err(e) => text_result(format!("{name}: {e}"), true),
        }
    }
}

/// Serve on stdin and stdout until the client closes.
pub fn serve(backend: Backend) -> Result<(), String> {
    let mut server = Server::new(info(), ModelTools { backend });
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    server.serve(stdin.lock(), stdout.lock()).map_err(|e| e.to_string())
}

/// A connection to the running app on `port`, or the port its discovery file names.
pub fn app_connection(port: Option<u16>, token: Option<&str>) -> Result<craft_control::client::Connection, String> {
    let app = control::app_identity();
    let port = port
        .or_else(|| app.read_current_port())
        .ok_or("no running ModelCraft with its control channel on (start it with --control, or turn on Allow Agents to Control ModelCraft)")?;
    Ok(craft_control::client::Connection::new(app, &format!("127.0.0.1:{port}"), token).with_document("scene"))
}

pub fn local(project: Option<&str>) -> Result<Backend, String> {
    let d = match project {
        Some(p) if Path::new(p).exists() => crate::load(p)?,
        _ => Document::new(),
    };
    Ok(Backend::Local(Box::new(d), Box::new(modelcraft_script::channel::ScriptChannel::new())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpc(s: &mut Server<ModelTools>, v: Value) -> Value {
        serde_json::from_str(&s.handle_line(&v.to_string()).unwrap_or_default()).unwrap_or(Value::Null)
    }

    #[test]
    fn mcp_tools_build_and_list_a_scene() {
        let mut s = Server::new(info(), ModelTools { backend: Backend::Local(Box::default(), Default::default()) });
        let init = rpc(
            &mut s,
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": craft_mcp_server::PROTOCOL_VERSION, "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
        );
        assert!(init["result"]["serverInfo"]["name"] == "modelcraft", "{init}");
        let _ = s.handle_line(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string());
        let list = rpc(&mut s, json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
        assert_eq!(list["result"]["tools"].as_array().map(Vec::len), Some(TOOLS.len()));
        let r = rpc(
            &mut s,
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "execute", "arguments": {"command": {"cmd": "add_primitive", "kind": "cone"}}}}),
        );
        assert!(r["result"]["isError"] != true && r["result"]["content"][0]["text"].as_str().is_some_and(|t| t.contains("created")), "{r}");
        let r = rpc(&mut s, json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "info", "arguments": {}}}));
        assert!(r["result"]["content"][0]["text"].as_str().is_some_and(|t| t.contains("\"Cone\"")), "{r}");
        let r =
            rpc(&mut s, json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "execute", "arguments": {"command": {"cmd": "nope"}}}}));
        assert_eq!(r["result"]["isError"], true, "{r}");
        let r = rpc(&mut s, json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "export", "arguments": {}}}));
        assert!(r.get("error").is_some() || r["result"]["isError"] == true, "a missing path is refused: {r}");
    }
}
