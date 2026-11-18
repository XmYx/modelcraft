//! A host can answer more control methods than the engine; `script.run` is announced by the engine.

use modelcraft_engine::Document;
use modelcraft_engine::control::{METHODS, effect, handle, handle_with};
use serde_json::{Value, json};

#[test]
fn script_run_is_listed_and_a_change_for_approval_prompts() {
    assert!(METHODS.contains(&"script.run"));
    assert_eq!(effect("script.run"), modelcraft_engine::control::Effect::Changes);
}

#[test]
fn without_a_host_the_engine_says_what_is_missing() {
    let mut d = Document::new();
    let r = handle(&mut d, "script.run", &json!({"source": "1"}));
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().contains("scripting host"), "{r}");
}

#[test]
fn an_extension_takes_its_methods_and_leaves_the_rest_to_the_engine() {
    let mut d = Document::new();
    let mut seen: Vec<String> = Vec::new();
    let mut ext = |doc: &mut Document, method: &str, params: &Value| -> Option<Value> {
        seen.push(method.to_string());
        (method == "script.run").then(|| json!({"ok": true, "result": {"objects": doc.scene.len(), "echo": params["source"]}}))
    };
    let r = handle_with(&mut d, "script.run", &json!({"source": "hi"}), &mut ext);
    assert_eq!(r["result"]["echo"], "hi");
    let r = handle_with(&mut d, "engine.execute", &json!({"cmd": "add_group"}), &mut ext);
    assert_eq!(r["ok"], true, "{r}");
    let r = handle_with(&mut d, "teleport", &json!({}), &mut ext);
    assert_eq!(r["ok"], false);
    assert_eq!(seen, vec!["script.run", "engine.execute", "teleport"], "the extension sees every request first");
    assert_eq!(d.scene.len(), 1);
}
