//! The command registry as a script API: `run(map)`, one `cmd::<name>(map)` function per entry of
//! [`modelcraft_engine::COMMANDS`] (so a command added to the registry appears here by itself),
//! `commands()` and `undo_group`.

use std::rc::Rc;

use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{COMMANDS, Command, Outcome};
use rhai::{Array, Dynamic, Engine, FnPtr, Map, Module, NativeCallContext};

use super::conv::{id_dyn, map_of, to_json};
use crate::error::{R, rt};
use crate::host::Shared;

/// Commands whose name is a script keyword are exposed under another name.
pub(crate) const RENAMED: &[(&str, &str)] = &[("import", "import_file")];

/// The name of `cmd::<name>` for a registry command.
pub fn script_name(cmd: &str) -> &str {
    RENAMED.iter().find(|(c, _)| *c == cmd).map(|(_, s)| *s).unwrap_or(cmd)
}

pub(crate) fn outcome_map(o: Outcome) -> Map {
    map_of(vec![
        ("created", Dynamic::from_array(o.created.iter().map(|i| id_dyn(*i)).collect::<Array>())),
        ("warnings", Dynamic::from_array(o.warnings.into_iter().map(Dynamic::from).collect::<Array>())),
    ])
}

/// Run a command given as a map (`name` fills in the `cmd` field for `cmd::<name>`).
pub(crate) fn run_map(s: &Shared, name: Option<&str>, m: Map) -> R<Map> {
    let mut v = to_json(&Dynamic::from_map(m))?;
    if let (Some(n), Some(o)) = (name, v.as_object_mut()) {
        o.insert("cmd".into(), serde_json::Value::String(n.to_string()));
    }
    let cmd: Command = serde_json::from_value(v).map_err(|e| rt(format!("not a valid command: {e}")))?;
    Ok(outcome_map(s.exec(cmd)?))
}

/// The first object a command created.
pub(crate) fn created(o: &Outcome) -> R<ObjectId> {
    o.created.first().copied().ok_or_else(|| rt("the command created nothing"))
}

pub(crate) fn register(engine: &mut Engine, s: &Rc<Shared>) {
    let sh = s.clone();
    engine.register_fn("run", move |m: Map| run_map(&sh, None, m));

    engine.register_fn("commands", || -> Array {
        COMMANDS
            .iter()
            .map(|(n, h)| {
                Dynamic::from_map(map_of(vec![
                    ("name", Dynamic::from(n.to_string())),
                    ("script_name", Dynamic::from(script_name(n).to_string())),
                    ("summary", Dynamic::from(h.to_string())),
                ]))
            })
            .collect()
    });

    let mut module = Module::new();
    for (name, _) in COMMANDS {
        let (sh, n) = (s.clone(), name.to_string());
        module.set_native_fn(script_name(name), move |m: Map| run_map(&sh, Some(&n), m));
        let (sh, n) = (s.clone(), name.to_string());
        module.set_native_fn(script_name(name), move || run_map(&sh, Some(&n), Map::new()));
    }
    engine.register_static_module("cmd", module.into());

    let sh = s.clone();
    engine.register_fn("undo_group", move |ctx: NativeCallContext, label: &str, f: FnPtr| -> R<Dynamic> {
        let label: String = label.chars().take(120).collect();
        sh.with_doc(|d| d.begin_group(&label))?;
        let r = f.call_within_context::<Dynamic>(&ctx, ());
        let failed = r.is_err();
        sh.with_doc(|d| {
            d.end_group(failed);
        })?;
        r
    });
}
