//! The MCP `script` tool runs a script on the server's document.

use craft_mcp_server::Server;
use serde_json::{Value, json};

use crate::mcp::{Backend, ModelTools, info};

fn rpc(s: &mut Server<ModelTools>, v: Value) -> Value {
    serde_json::from_str(&s.handle_line(&v.to_string()).unwrap_or_default()).unwrap_or(Value::Null)
}

#[test]
fn the_script_tool_runs_scripts_and_reports_failures_as_tool_errors() {
    let mut s = Server::new(info(), ModelTools { backend: Backend::Local(Box::default(), Default::default()) });
    let _ = rpc(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": craft_mcp_server::PROTOCOL_VERSION, "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
    );
    let _ = s.handle_line(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string());
    let list = rpc(&mut s, json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"script"), "{names:?}");
    let call = |s: &mut Server<ModelTools>, id: u64, args: Value| rpc(s, json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": "script", "arguments": args}}));
    let r = call(&mut s, 3, json!({"source": "for i in 0..3 { add_primitive(\"cube\", \"c\" + i); } print(\"done \" + arg(\"who\", \"?\")); object_count()", "args": {"who": "agent"}}));
    assert_ne!(r["result"]["isError"], true, "{r}");
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("done agent") && text.contains("\"value\": \"3\""), "{text}");
    let info = rpc(&mut s, json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "info", "arguments": {}}}));
    assert!(info["result"]["content"][0]["text"].as_str().unwrap().contains("\"c2\""), "{info}");
    // A script that fails is an error result that still carries the report with its line.
    let r = call(&mut s, 5, json!({"source": "let x = 1;\nname_of(404);"}));
    assert_eq!(r["result"]["isError"], true, "{r}");
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("\"line\": 2"), "{r}");
    // A request without a script is refused.
    let r = call(&mut s, 6, json!({}));
    assert!(r.get("error").is_some() || r["result"]["isError"] == true, "{r}");
}
