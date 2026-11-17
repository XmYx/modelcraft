//! `modelcraft-cli run-script`: run a script over a project, headless. The script sees the same
//! scene and commands the app's script editor does; what it changes is saved like `run` does.

use std::path::{Path, PathBuf};
use std::time::Duration;

use modelcraft_engine::Document;
use modelcraft_script::{Limits, LineKind, Permissions, RunContext, RunReport, Session};

pub const USAGE: &str = "  run-script <project|new> <script.rhai> [-o <out>] [options]
                                    run a script over a project (or an empty scene) and save
      --allow-read DIR              let the script read files under DIR (repeatable; the script's
                                    own folder and the project's folder are readable by default)
      --allow-write DIR             let the script write files under DIR (repeatable; default none)
      --time-limit SECS             wall-clock limit (default 10)
      --max-ops N                   interpreter operation limit (default 10000000)
      --no-save                     run and report, write nothing";

/// Options of one `run-script`.
#[derive(Clone, Debug, PartialEq)]
pub struct Opts {
    pub project: String,
    pub script: PathBuf,
    pub out: Option<String>,
    pub allow_read: Vec<PathBuf>,
    pub allow_write: Vec<PathBuf>,
    pub limits: Limits,
    pub save: bool,
}

fn values(a: &[String], name: &str) -> Vec<String> {
    a.windows(2).filter(|w| w[0] == name).map(|w| w[1].clone()).collect()
}

pub fn parse(a: &[String]) -> Result<Opts, String> {
    let project = a.first().filter(|p| !p.starts_with('-')).ok_or("missing project (or `new`)")?.clone();
    let script = a.get(1).filter(|p| !p.starts_with('-')).ok_or("missing script file")?.into();
    let mut limits = Limits::default();
    if let Some(v) = values(a, "--time-limit").last() {
        let s: f64 = v.parse().map_err(|_| format!("--time-limit: {v:?} is not a number of seconds"))?;
        if !s.is_finite() || s <= 0.0 || s > 86_400.0 {
            return Err("--time-limit must be between 0 and 86400 seconds".into());
        }
        limits.max_time = Duration::from_secs_f64(s);
    }
    if let Some(v) = values(a, "--max-ops").last() {
        limits.max_operations = v.parse().map_err(|_| format!("--max-ops: {v:?} is not a number"))?;
    }
    Ok(Opts {
        project,
        script,
        out: values(a, "-o").last().cloned(),
        allow_read: values(a, "--allow-read").into_iter().map(PathBuf::from).collect(),
        allow_write: values(a, "--allow-write").into_iter().map(PathBuf::from).collect(),
        limits,
        save: !a.iter().any(|x| x == "--no-save"),
    })
}

/// The permissions a run gets: its own folder and the project's folder to read, plus what was granted.
pub fn permissions(o: &Opts) -> Permissions {
    let dir = o.script.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let mut p = Permissions::none().with_base(dir).allow_read(dir);
    if o.project != "new"
        && let Some(pd) = Path::new(&o.project).parent().filter(|p| !p.as_os_str().is_empty())
    {
        p = p.allow_read(pd);
    }
    for d in &o.allow_read {
        p = p.allow_read(d);
    }
    for d in &o.allow_write {
        p = p.allow_write(d);
    }
    p
}

/// Run the script and print its console. Returns the report; the caller saves.
pub fn run_script(doc: &mut Document, o: &Opts) -> Result<RunReport, String> {
    let text = std::fs::read_to_string(&o.script).map_err(|e| format!("{}: {e}", o.script.display()))?;
    let name = o.script.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "script".into());
    let mut session = Session::new(o.limits.clone(), permissions(o));
    let mut ctx = RunContext::default();
    let report = session.run(doc, &mut ctx, &name, &text);
    for l in &report.lines {
        match l.kind {
            LineKind::Print => println!("{}", l.text),
            LineKind::Debug => match l.line {
                Some(n) => eprintln!("{name}:{n}: {}", l.text),
                None => eprintln!("{}", l.text),
            },
            LineKind::Warning => eprintln!("warning: {}", l.text),
        }
    }
    Ok(report)
}

pub fn run(a: &[String], load: fn(&str) -> Result<Document, String>, save: fn(&mut Document, &str) -> Result<(), String>) -> Result<(), String> {
    let o = parse(a)?;
    let mut d = if o.project != "new" && Path::new(&o.project).exists() { load(&o.project)? } else { Document::new() };
    let report = run_script(&mut d, &o)?;
    if let Some(e) = &report.error {
        return Err(e.to_string());
    }
    eprintln!("{} commands, {} operations, {:.2} s", report.commands, report.operations, report.elapsed.as_secs_f64());
    if o.save {
        let target =
            o.out.clone().or_else(|| (o.project != "new").then(|| o.project.clone())).ok_or("the script ran on an empty scene: give -o <out> to save it")?;
        save(&mut d, &target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mc-cli-script-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn options_parse_and_reject_nonsense() {
        let o = parse(&s(&[
            "a.modelcraft",
            "x.rhai",
            "-o",
            "b.modelcraft",
            "--allow-write",
            "out",
            "--allow-read",
            "in",
            "--allow-read",
            "more",
            "--time-limit",
            "2.5",
            "--max-ops",
            "99",
        ]))
        .unwrap();
        assert_eq!((o.project.as_str(), o.out.as_deref(), o.allow_read.len(), o.allow_write.len()), ("a.modelcraft", Some("b.modelcraft"), 2, 1));
        assert_eq!((o.limits.max_time, o.limits.max_operations, o.save), (Duration::from_millis(2500), 99, true));
        assert!(!parse(&s(&["new", "x.rhai", "--no-save"])).unwrap().save);
        assert!(parse(&s(&["new"])).is_err());
        assert!(parse(&s(&[])).is_err());
        assert!(parse(&s(&["new", "x.rhai", "--time-limit", "soon"])).is_err());
        assert!(parse(&s(&["new", "x.rhai", "--time-limit", "-1"])).is_err());
        assert!(parse(&s(&["new", "x.rhai", "--max-ops", "many"])).is_err());
    }

    #[test]
    fn a_script_runs_over_a_document_and_respects_the_grants() {
        let dir = tmp("run");
        let script = dir.join("make.rhai");
        std::fs::write(&script, "let c = add_primitive(\"cube\", \"Made\");\nwrite_text(\"out/log.txt\", \"hi\");\nprint(\"ok\");\nobject_count()").unwrap();
        let _ = std::fs::create_dir_all(dir.join("out"));
        let mut o = parse(&s(&["new", script.to_str().unwrap()])).unwrap();
        // Writing is refused until granted.
        let mut d = Document::new();
        let r = run_script(&mut d, &o).unwrap();
        assert!(r.error.is_some_and(|e| e.message.contains("may not use writing")));
        o.allow_write.push(dir.join("out"));
        let mut d = Document::new();
        let r = run_script(&mut d, &o).unwrap();
        assert!(r.ok(), "{:?}", r.error);
        assert_eq!(r.value.as_deref(), Some("1"));
        assert_eq!(d.scene.len(), 1);
        assert_eq!(std::fs::read_to_string(dir.join("out/log.txt")).unwrap(), "hi");
        assert!(run_script(&mut d, &Opts { script: dir.join("missing.rhai"), ..o }).is_err());
    }
}
