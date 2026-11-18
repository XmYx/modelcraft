//! `modelcraft-cli macro-script`: write a recorded list of commands (a JSON-lines file as `run`
//! reads it, or a crash journal) as a macro script, by replaying it on an empty scene to learn
//! which objects each command creates. The script runs with `run-script` or in the app.

use std::path::Path;

use modelcraft_engine::Command;
use modelcraft_script::macros::{macro_script, record_commands};

pub const USAGE: &str = "  macro-script <commands.jsonl|project.journal> [--name NAME] [-o <out.rhai>]
                                    turn one JSON command per line (or a crash journal) into a
                                    script that repeats them as one undo step; prints to stdout
                                    without -o";

/// The commands of a JSON-lines file or a journal (`{\"seq\":…,\"cmd\":{…}}` lines).
pub fn read_commands(text: &str) -> Result<Vec<Command>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") || line.starts_with('#') {
            continue;
        }
        let v: serde_json::Value = serde_json::from_str(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        // A journal line wraps the command; a plain line is the command.
        let v = match v.get("cmd") {
            Some(inner) if inner.is_object() => inner.clone(),
            _ => v,
        };
        out.push(serde_json::from_value(v).map_err(|e| format!("line {}: not a command: {e}", i + 1))?);
        if out.len() > modelcraft_engine::MAX_RECORDED_COMMANDS {
            return Err(format!("more than {} commands", modelcraft_engine::MAX_RECORDED_COMMANDS));
        }
    }
    Ok(out)
}

pub fn run(a: &[String]) -> Result<(), String> {
    let input = crate::arg(a, 0, "a commands file")?;
    let name = crate::flag(a, "--name")
        .unwrap_or_else(|| Path::new(input).file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| "macro".into()));
    let meta = std::fs::metadata(input).map_err(|e| format!("{input}: {e}"))?;
    if meta.len() > 64 << 20 {
        return Err(format!("{input} is larger than 64 MiB"));
    }
    let text = std::fs::read_to_string(input).map_err(|e| format!("{input}: {e}"))?;
    let commands = read_commands(&text)?;
    let steps = record_commands(&commands)?;
    let script = macro_script(&name, &steps, &[]).map_err(|e| e.to_string())?;
    match crate::flag(a, "-o") {
        Some(out) => craft_atomic::atomic_write(Path::new(&out), script.as_bytes()).map_err(|e| format!("{out}: {e}")),
        None => {
            print!("{script}");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_plain_lines_and_journal_lines() {
        let plain = "{\"cmd\":\"add_group\",\"name\":\"A\"}\n\n// a comment\n{\"cmd\":\"add_primitive\",\"kind\":\"cube\"}\n";
        assert_eq!(read_commands(plain).map(|c| c.len()), Ok(2));
        let journal = "{\"seq\":1,\"at\":\"2026-11-18T00:00:00Z\",\"cmd\":{\"cmd\":\"add_group\",\"name\":\"A\"}}\n";
        assert_eq!(read_commands(journal).map(|c| c.len()), Ok(1));
        assert!(read_commands("{\"cmd\":\"teleport\"}").unwrap_err().contains("line 1"));
        assert!(read_commands("not json").is_err());
    }

    #[test]
    fn a_command_file_becomes_a_script_that_repeats_it() {
        use modelcraft_engine::Document;
        use modelcraft_script::{Limits, Permissions, RunContext, Session};
        let dir = std::env::temp_dir().join(format!("mc-macro-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("steps.jsonl");
        std::fs::write(
            &input,
            "{\"cmd\":\"add_primitive\",\"kind\":\"cube\",\"name\":\"A\"}\n{\"cmd\":\"add_primitive\",\"kind\":\"sphere\",\"name\":\"B\"}\n{\"cmd\":\"reparent\",\"id\":2,\"parent\":1}\n",
        )
        .unwrap();
        let out = dir.join("steps.rhai");
        run(&[input.display().to_string(), "--name".into(), "Pair".into(), "-o".into(), out.display().to_string()]).unwrap();
        let text = std::fs::read_to_string(&out).unwrap();
        assert!(text.starts_with("// Macro: Pair (3 commands)"), "{text}");
        // The script repeats the commands on a scene that already has an object (other ids).
        let mut doc = Document::new();
        doc.execute(Command::AddGroup { name: Some("Existing".into()) }).unwrap();
        let mut s = Session::new(Limits::default(), Permissions::none());
        let r = s.run(&mut doc, &mut RunContext::default(), "steps.rhai", &text);
        assert!(r.ok(), "{:?}", r.error);
        let b = doc.scene.objects().iter().find(|o| o.name == "B").map(|o| o.parent);
        assert!(b.flatten().is_some(), "B sits under A");
        assert_eq!(doc.undo.len(), 2, "the macro is one undo step");
        // Missing and empty inputs are errors, not panics.
        assert!(run(&[dir.join("none.jsonl").display().to_string()]).is_err());
        std::fs::write(dir.join("empty.jsonl"), "\n").unwrap();
        assert!(run(&[dir.join("empty.jsonl").display().to_string()]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
