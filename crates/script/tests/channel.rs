//! `script.run` on the control channel: parameters, results, limits and what a remote script may not do.

use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{Command, Document};
use modelcraft_script::channel::{MAX_SOURCE_BYTES, METHOD, ScriptChannel, report_json};
use serde_json::{Value, json};

fn call(ch: &mut ScriptChannel, doc: &mut Document, sel: &mut Vec<ObjectId>, params: Value) -> Value {
    ch.handle(doc, sel, METHOD, &params).expect("script.run is answered")
}

#[test]
fn only_script_run_is_answered() {
    let mut ch = ScriptChannel::new();
    let mut d = Document::new();
    assert!(ch.handle(&mut d, &mut Vec::new(), "engine.execute", &json!({})).is_none());
    assert!(ch.handle(&mut d, &mut Vec::new(), "script.runn", &json!({})).is_none());
}

#[test]
fn a_script_runs_on_the_scene_and_reports() {
    let mut ch = ScriptChannel::new();
    let mut d = Document::new();
    let mut sel = Vec::new();
    let r = call(&mut ch, &mut d, &mut sel, json!({"source": "let c = add_primitive(\"cube\", \"Agent\"); print(\"made \" + name_of(c)); object_count()"}));
    assert_eq!(r["ok"], true, "{r}");
    let res = &r["result"];
    assert_eq!(res["ok"], true, "{res}");
    assert_eq!(res["value"], "1");
    assert_eq!(res["output"][0]["text"], "made Agent");
    assert_eq!(res["commands"], 1);
    assert_eq!(d.scene.objects()[0].name, "Agent");
    assert_eq!(d.undo.len(), 1, "an undoable change like any command");
}

#[test]
fn arguments_name_and_selection_go_in_and_come_back() {
    let mut ch = ScriptChannel::new();
    let mut d = Document::new();
    let a = d.execute(Command::AddGroup { name: Some("a".into()) }).unwrap().created[0];
    let b = d.execute(Command::AddGroup { name: Some("b".into()) }).unwrap().created[0];
    let mut sel = vec![a];
    let r = call(
        &mut ch,
        &mut d,
        &mut sel,
        json!({"source": "set_selection([2]); print(arg(\"who\", \"nobody\") + arg(\"n\", \"0\") + arg(\"flag\", \"x\")); selection().len()", "name": "pick.rhai", "args": {"who": "me", "n": 3, "flag": true}}),
    );
    let res = &r["result"];
    assert_eq!(res["output"][0]["text"], "me3true", "{res}");
    assert_eq!(res["selection"], json!([b.0]), "the new selection is reported");
    assert_eq!(sel, vec![b], "and handed back to the host");
    // A script that fails names its file, line and column in the result, not in the reply.
    let r = call(&mut ch, &mut d, &mut sel, json!({"source": "let x = 1;\nname_of(404);", "name": "bad.rhai"}));
    assert_eq!(r["ok"], true);
    assert_eq!(r["result"]["ok"], false);
    assert_eq!(r["result"]["error"]["file"], "bad.rhai");
    assert_eq!(r["result"]["error"]["line"], 2);
    assert_eq!(sel, vec![b], "a failed run that did not touch the selection leaves it alone");
}

#[test]
fn bad_requests_are_errors_not_runs() {
    let mut ch = ScriptChannel::new();
    let mut d = Document::new();
    let mut sel = Vec::new();
    for (params, needle) in [
        (json!({}), "needs `source`"),
        (json!({"source": 5}), "needs `source`"),
        (json!({"source": "1", "name": 5}), "`name` must be a string"),
        (json!({"source": "1", "args": [1]}), "`args` must be an object"),
        (json!({"source": "1", "args": {"a": [1]}}), "must be a string, number or boolean"),
        (json!({"source": "1", "args": {"a": "x".repeat(5000)}}), "longer than"),
        (json!({"source": "x".repeat(MAX_SOURCE_BYTES + 1)}), "larger than"),
    ] {
        let r = call(&mut ch, &mut d, &mut sel, params.clone());
        assert_eq!(r["ok"], false, "{params}");
        assert!(r["error"].as_str().unwrap().contains(needle), "{r}");
    }
    let many: serde_json::Map<String, Value> = (0..65).map(|i| (format!("k{i}"), json!("v"))).collect();
    assert_eq!(call(&mut ch, &mut d, &mut sel, json!({"source": "1", "args": many}))["ok"], false);
    assert!(d.scene.is_empty());
}

#[test]
fn a_remote_script_has_no_files_no_library_and_the_usual_limits() {
    let mut ch = ScriptChannel::new();
    let mut d = Document::new();
    let mut sel = Vec::new();
    let r = call(&mut ch, &mut d, &mut sel, json!({"source": "read_text(\"/etc/hostname\")"}));
    assert_eq!(r["result"]["ok"], false, "{r}");
    let r = call(&mut ch, &mut d, &mut sel, json!({"source": "write_text(\"/tmp/mc-agent-should-not-exist.txt\", \"x\")"}));
    assert_eq!(r["result"]["ok"], false, "{r}");
    assert!(!std::path::Path::new("/tmp/mc-agent-should-not-exist.txt").exists());
    let r = call(&mut ch, &mut d, &mut sel, json!({"source": "import \"anything\" as a;"}));
    assert_eq!(r["result"]["ok"], false, "{r}");
    // An endless loop is stopped and the reply says so; the channel keeps working afterwards.
    let t = std::time::Instant::now();
    let r = call(&mut ch, &mut d, &mut sel, json!({"source": "loop { }"}));
    assert_eq!(r["result"]["ok"], false);
    assert!(t.elapsed() < std::time::Duration::from_secs(15));
    assert_eq!(call(&mut ch, &mut d, &mut sel, json!({"source": "1 + 1"}))["result"]["value"], "2");
}

#[test]
fn tools_and_callbacks_a_remote_script_registers_do_not_pile_up() {
    let mut ch = ScriptChannel::new();
    let mut d = Document::new();
    let mut sel = Vec::new();
    for i in 0..200 {
        let r = call(
            &mut ch,
            &mut d,
            &mut sel,
            json!({"source": format!("register_tool(\"T{i}\", || {{ }}); on_scene_changed(|e| {{ }});"), "name": format!("run{i}.rhai")}),
        );
        assert_eq!(r["result"]["ok"], true, "run {i}: {r}");
    }
}

#[test]
fn undo_group_makes_one_step_and_the_report_json_has_its_fields() {
    let mut ch = ScriptChannel::new();
    let mut d = Document::new();
    let mut sel = Vec::new();
    let r = call(&mut ch, &mut d, &mut sel, json!({"source": "undo_group(\"Row\", || { for i in 0..4 { add_primitive(\"cube\"); } });"}));
    assert_eq!(r["result"]["commands"], 4);
    assert_eq!(d.undo.len(), 1);
    for k in ["ok", "value", "output", "commands", "operations", "seconds", "error"] {
        assert!(r["result"].get(k).is_some(), "{k}");
    }
    let mut s = modelcraft_script::Session::new(modelcraft_script::Limits::default(), modelcraft_script::Permissions::none());
    let rep = s.run(&mut Document::new(), &mut modelcraft_script::RunContext::default(), "x", "1");
    assert_eq!(report_json(&rep)["ok"], true);
}
