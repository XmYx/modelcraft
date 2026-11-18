//! Recorded macros: a [`modelcraft_engine::Recorded`] step list written as a script.
//!
//! The script is ordinary Rhai over the command registry (`cmd::<name>(#{ ... })`), so a macro can
//! be read, edited, run from the Scripts window or the command line, and saved as a user tool. It
//! runs as one undo step. Objects are named so it works on other scenes:
//!
//! - an object the macro created is bound to a variable (`let r1 = cmd::add_primitive(...).created;`)
//!   and later commands use `r1[0]`;
//! - an object that existed when recording started and was selected then is `sel[i]`, the i-th
//!   object of the selection the macro runs with (the script checks that enough are selected);
//! - any other object keeps its number and only matches a scene that has it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{COMMANDS, Command, Document, Recorded};
use serde_json::{Map, Value};

use crate::api::script_name;

/// Fields of a command that hold object ids (a number, a list of numbers or null). Only these are
/// rewritten, at the top level of a command.
const ID_KEYS: &[&str] =
    &["id", "ids", "parent", "input", "target", "mesh", "joint", "joints", "sources", "driver", "end", "goal", "pole", "up_object", "objects", "from"];

/// Longest macro script written, in bytes (a long sculpt stroke can be large).
pub const MAX_MACRO_BYTES: usize = 8 << 20;

/// Why a macro could not be written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MacroError {
    TooLarge,
    Empty,
}

impl std::fmt::Display for MacroError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MacroError::TooLarge => write!(f, "the macro is larger than {} MiB", MAX_MACRO_BYTES >> 20),
            MacroError::Empty => write!(f, "nothing was recorded"),
        }
    }
}

impl std::error::Error for MacroError {}

/// Write `steps` (recorded while `selection` was selected) as a script named `name`.
pub fn macro_script(name: &str, steps: &[Recorded], selection: &[ObjectId]) -> Result<String, MacroError> {
    struct Item<'a> {
        label: Option<&'a str>,
        name: String,
        fields: Map<String, Value>,
        created: &'a [ObjectId],
    }
    let mut items: Vec<Item> = Vec::new();
    for step in steps {
        for (i, rc) in step.commands.iter().enumerate() {
            let Ok(Value::Object(mut fields)) = serde_json::to_value(&rc.command) else { continue };
            let Some(Value::String(cname)) = fields.remove("cmd") else { continue };
            items.push(Item { label: if i == 0 { step.label.as_deref() } else { None }, name: cname, fields, created: &rc.created });
        }
    }
    if items.is_empty() {
        return Err(MacroError::Empty);
    }
    let clean: String = one_line(name).chars().take(80).collect();
    // Which ids are referenced: a created object only gets a variable when something uses it.
    let mut referenced: BTreeSet<u64> = BTreeSet::new();
    for it in &items {
        for k in ID_KEYS {
            collect_ids(it.fields.get(*k), &mut referenced);
        }
    }
    let sel_index: BTreeMap<u64, usize> = selection.iter().enumerate().map(|(i, id)| (id.0, i)).collect();
    let mut bound: BTreeMap<u64, String> = BTreeMap::new();
    let mut body = String::new();
    let mut counter = 0usize;
    let mut sel_needed = 0usize;
    let mut unmapped: BTreeSet<u64> = BTreeSet::new();
    for it in &items {
        if let Some(l) = it.label {
            let _ = writeln!(body, "    // {}", one_line(l));
        }
        let mut fields = Vec::new();
        for (k, v) in &it.fields {
            let lit = if ID_KEYS.contains(&k.as_str()) { id_literal(v, &bound, &sel_index, &mut sel_needed, &mut unmapped) } else { literal(v) };
            fields.push(format!("{}: {}", key(k), lit));
        }
        // Registry commands have a `cmd::` function; the others (animation, rig, simulation,
        // groups) go through `run` with the command's name in the map.
        let call = if COMMANDS.iter().any(|(n, _)| *n == it.name) {
            let f = script_name(&it.name);
            if fields.is_empty() { format!("cmd::{f}()") } else { format!("cmd::{f}(#{{ {} }})", fields.join(", ")) }
        } else {
            fields.insert(0, format!("cmd: {}", string(&it.name)));
            format!("run(#{{ {} }})", fields.join(", "))
        };
        let wanted: Vec<(usize, u64)> = it.created.iter().enumerate().filter(|(_, id)| referenced.contains(&id.0)).map(|(i, id)| (i, id.0)).collect();
        if wanted.is_empty() {
            let _ = writeln!(body, "    {call};");
        } else {
            counter += 1;
            let var = format!("r{counter}");
            let _ = writeln!(body, "    let {var} = {call}.created;");
            for (i, id) in wanted {
                bound.insert(id, format!("{var}[{i}]"));
            }
        }
        if body.len() > MAX_MACRO_BYTES {
            return Err(MacroError::TooLarge);
        }
    }
    let mut out = String::new();
    let _ = writeln!(out, "// Macro: {clean} ({} commands), recorded in ModelCraft. Edit it freely; it runs as one undo step.", items.len());
    if sel_needed > 0 {
        let _ = writeln!(out, "// It works on the objects that were selected when it was recorded: select {sel_needed} first, in the same order.");
    }
    if !unmapped.is_empty() {
        let list: Vec<String> = unmapped.iter().map(u64::to_string).collect();
        let _ = writeln!(
            out,
            "// Objects {} were not selected when recording started; their numbers are kept and only match a scene that has them.",
            list.join(", ")
        );
    }
    let _ = writeln!(out, "let sel = selection();");
    if sel_needed > 0 {
        let _ = writeln!(
            out,
            "if sel.len() < {sel_needed} {{ throw \"select {sel_needed} object(s) first: this macro works on the objects that were selected when it was recorded\"; }}"
        );
    }
    let _ = writeln!(out, "undo_group({}, || {{", string(&format!("Macro: {clean}")));
    out.push_str(&body);
    out.push_str("});\n");
    if out.len() > MAX_MACRO_BYTES {
        return Err(MacroError::TooLarge);
    }
    Ok(out)
}

/// Record `commands` as a macro by running them on a new document (so the objects they create
/// are known), for turning a journal or a list of commands into a script.
pub fn record_commands(commands: &[Command]) -> Result<Vec<Recorded>, String> {
    let mut doc = Document::new();
    doc.start_recording();
    for (i, c) in commands.iter().enumerate() {
        doc.execute(c.clone()).map_err(|e| format!("command {}: {e}", i + 1))?;
    }
    Ok(doc.stop_recording().map(|r| r.into_steps()).unwrap_or_default())
}

fn one_line(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

fn collect_ids(v: Option<&Value>, into: &mut BTreeSet<u64>) {
    match v {
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_u64() {
                into.insert(i);
            }
        }
        Some(Value::Array(a)) => {
            for x in a {
                collect_ids(Some(x), into);
            }
        }
        _ => {}
    }
}

fn id_literal(v: &Value, bound: &BTreeMap<u64, String>, sel: &BTreeMap<u64, usize>, sel_needed: &mut usize, unmapped: &mut BTreeSet<u64>) -> String {
    match v {
        Value::Number(n) => match n.as_u64() {
            Some(i) => {
                if let Some(b) = bound.get(&i) {
                    b.clone()
                } else if let Some(s) = sel.get(&i) {
                    *sel_needed = (*sel_needed).max(s + 1);
                    format!("sel[{s}]")
                } else {
                    unmapped.insert(i);
                    i.to_string()
                }
            }
            None => literal(v),
        },
        Value::Array(a) => format!("[{}]", a.iter().map(|x| id_literal(x, bound, sel, sel_needed, unmapped)).collect::<Vec<_>>().join(", ")),
        other => literal(other),
    }
}

/// A map key: a plain name stays bare, anything else is quoted.
fn key(k: &str) -> String {
    let mut chars = k.chars();
    let plain = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain && !is_keyword(k) { k.to_string() } else { string(k) }
}

fn is_keyword(k: &str) -> bool {
    matches!(
        k,
        "true"
            | "false"
            | "let"
            | "const"
            | "if"
            | "else"
            | "switch"
            | "do"
            | "while"
            | "until"
            | "loop"
            | "for"
            | "in"
            | "continue"
            | "break"
            | "return"
            | "throw"
            | "try"
            | "catch"
            | "fn"
            | "private"
            | "import"
            | "export"
            | "as"
            | "global"
            | "this"
            | "is"
            | "type_of"
            | "print"
            | "debug"
            | "eval"
    )
}

fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON value as a Rhai literal (`null` is `()`).
fn literal(v: &Value) -> String {
    match v {
        Value::Null => "()".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                // Rhai integers are signed 64-bit; a larger one is not a valid script number.
                if i64::try_from(u).is_ok() { u.to_string() } else { "0".into() }
            } else {
                let f = n.as_f64().unwrap_or(0.0);
                // A float always has a point, so it stays a float in the script.
                let s = format!("{f:?}");
                if s.contains('e') || s.contains('E') { format!("{f:.12}") } else { s }
            }
        }
        Value::String(s) => string(s),
        Value::Array(a) => format!("[{}]", a.iter().map(literal).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => {
            if m.is_empty() {
                "#{}".into()
            } else {
                format!("#{{ {} }}", m.iter().map(|(k, v)| format!("{}: {}", key(k), literal(v))).collect::<Vec<_>>().join(", "))
            }
        }
    }
}
