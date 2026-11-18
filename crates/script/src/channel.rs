//! `script.run` for the control channel: the window's socket, `modelcraft-cli serve`, `control`
//! and `mcp` (and with them any agent) can run a script on the open scene.
//!
//! ```text
//! {"method": "script.run", "params": {"source": "add_primitive(\"cube\")", "name": "agent.rhai", "args": {"n": "3"}}}
//! → {"ok": true, "result": {"ok": true, "value": null, "output": [...], "commands": 1, "operations": 12, "seconds": 0.0, "error": null, "selection": [1]}}
//! ```
//!
//! The script sees the scene and the selection the host passes in and runs like any other script:
//! one engine command per change, undoable (put `undo_group` around it for one step), with the
//! default [`Limits`]. It gets no file access at all and no script library, whatever the user has
//! allowed their own scripts: what an agent may do is what its commands already allowed, plus
//! computation. Tools and callbacks it registers are dropped when it ends. A script that fails
//! is a successful reply with `result.ok == false` and the error with its line and column.

use modelcraft_engine::Document;
use modelcraft_engine::scene::ObjectId;
use serde_json::{Value, json};

use crate::{LineKind, Limits, Permissions, RunContext, RunReport, Session};

/// The control method.
pub const METHOD: &str = "script.run";
/// The longest script a request may carry.
pub const MAX_SOURCE_BYTES: usize = 1 << 20;
const MAX_ARGS: usize = 64;
const MAX_ARG_BYTES: usize = 4096;

/// One script session for the requests of a channel.
pub struct ScriptChannel {
    session: Session,
}

impl Default for ScriptChannel {
    fn default() -> Self {
        ScriptChannel::new()
    }
}

impl ScriptChannel {
    pub fn new() -> Self {
        ScriptChannel { session: Session::new(Limits::default(), Permissions::none()) }
    }

    /// Answer `method` if it is `script.run`; `None` for any other method. `selection` is what the
    /// script reads and may change.
    pub fn handle(&mut self, doc: &mut Document, selection: &mut Vec<ObjectId>, method: &str, params: &Value) -> Option<Value> {
        if method != METHOD {
            return None;
        }
        Some(match self.run(doc, selection, params) {
            Ok(result) => json!({"ok": true, "result": result}),
            Err(e) => json!({"ok": false, "error": e}),
        })
    }

    fn run(&mut self, doc: &mut Document, selection: &mut Vec<ObjectId>, params: &Value) -> Result<Value, String> {
        let source = params.get("source").and_then(Value::as_str).ok_or("script.run needs `source`, the script text")?;
        if source.len() > MAX_SOURCE_BYTES {
            return Err(format!("the script is larger than {} MiB", MAX_SOURCE_BYTES >> 20));
        }
        let name: String = match params.get("name") {
            None | Some(Value::Null) => "agent.rhai".into(),
            Some(Value::String(s)) => s.chars().filter(|c| !c.is_control()).take(80).collect(),
            Some(_) => return Err("`name` must be a string".into()),
        };
        let mut args = std::collections::BTreeMap::new();
        match params.get("args") {
            None | Some(Value::Null) => {}
            Some(Value::Object(m)) => {
                if m.len() > MAX_ARGS {
                    return Err(format!("at most {MAX_ARGS} args"));
                }
                for (k, v) in m {
                    let text = match v {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => return Err(format!("arg {k:?} must be a string, number or boolean")),
                    };
                    if text.len() > MAX_ARG_BYTES || k.len() > MAX_ARG_BYTES {
                        return Err(format!("arg {k:?} is longer than {MAX_ARG_BYTES} bytes"));
                    }
                    args.insert(k.clone(), text);
                }
            }
            Some(_) => return Err("`args` must be an object of strings, numbers and booleans".into()),
        }
        self.session.set_args(args);
        self.session.set_current_file(doc.path.as_ref().map(|p| p.display().to_string()));
        let mut ctx = RunContext { selection: selection.clone() };
        let report = self.session.run(doc, &mut ctx, &name, source);
        self.session.clear_registered();
        let mut result = report_json(&report);
        if let Some(sel) = &report.selection {
            *selection = sel.clone();
        }
        if let (Some(o), Some(sel)) = (result.as_object_mut(), &report.selection) {
            o.insert("selection".into(), json!(sel.iter().map(|i| i.0).collect::<Vec<_>>()));
        }
        Ok(result)
    }
}

/// The report of a run as one JSON object: `ok`, `value`, `output` (kind, text, line), `commands`,
/// `operations`, `seconds` and `error` (file, line, column, message).
pub fn report_json(r: &RunReport) -> Value {
    let output: Vec<Value> = r
        .lines
        .iter()
        .map(|l| json!({ "kind": match l.kind { LineKind::Print => "print", LineKind::Debug => "debug", LineKind::Warning => "warning" }, "text": l.text, "line": l.line }))
        .collect();
    let error = r.error.as_ref().map(|e| json!({ "file": e.file, "line": e.line, "column": e.column, "message": e.message }));
    json!({ "ok": r.error.is_none(), "value": r.value, "output": output, "commands": r.commands, "operations": r.operations, "seconds": r.elapsed.as_secs_f64(), "error": error })
}
