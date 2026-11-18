//! `modelcraft-cli run-script` and `batch-script`: run a script over a project, or over many files,
//! headless. The script sees the same scene and commands the app's Scripts window does; what it
//! changes is saved like `run` does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use modelcraft_engine::Document;
use modelcraft_script::{Limits, LineKind, Permissions, RunContext, RunReport, Session};

pub const USAGE: &str = "  run-script <project|new> <script.rhai> [-o <out>] [options]
  run-script <project|new> -e '<script text>' [-o <out>] [options]
                                    run a script over a project (or an empty scene) and save
  batch-script <script.rhai> <file>... --out-dir DIR [--format modelcraft|glb|obj|stl] [--stop-on-error] [options]
                                    run a script over every file (projects or any importable model)
                                    and save each result in DIR; exit status 1 if any file failed
      --arg NAME=VALUE              give the script an argument, read with arg(\"NAME\", default)
                                    (repeatable)
      --allow-read DIR              let the script read files under DIR (repeatable; the script's
                                    own folder and the project's folder are readable by default)
      --allow-write DIR             let the script write files under DIR (repeatable; default none;
                                    batch-script always lets it write in --out-dir)
      --time-limit SECS             wall-clock limit per run (default 10)
      --max-ops N                   interpreter operation limit (default 10000000)
      --json                        print the report of a run-script as one JSON line on stdout
      --no-save                     run and report, write nothing";

/// Where the script text comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    File(PathBuf),
    Inline(String),
}

/// Options shared by both commands.
#[derive(Clone, Debug, PartialEq)]
pub struct Opts {
    /// The project (`run-script`); empty for `batch-script`.
    pub project: String,
    pub source: Source,
    /// The files of `batch-script`.
    pub files: Vec<String>,
    pub out: Option<String>,
    pub out_dir: Option<PathBuf>,
    pub format: Option<String>,
    pub allow_read: Vec<PathBuf>,
    pub allow_write: Vec<PathBuf>,
    pub args: BTreeMap<String, String>,
    pub limits: Limits,
    pub save: bool,
    pub json: bool,
    pub stop_on_error: bool,
}

fn values(a: &[String], name: &str) -> Vec<String> {
    a.windows(2).filter(|w| w[0] == name).map(|w| w[1].clone()).collect()
}

const VALUE_FLAGS: &[&str] = &["-o", "-e", "--arg", "--allow-read", "--allow-write", "--time-limit", "--max-ops", "--out-dir", "--format"];

/// The arguments that are not flags or flag values, in order.
fn positionals(a: &[String]) -> Vec<&String> {
    let mut v = Vec::new();
    let mut skip = false;
    for x in a {
        if skip {
            skip = false;
        } else if VALUE_FLAGS.contains(&x.as_str()) {
            skip = true;
        } else if !x.starts_with('-') || x == "-" {
            v.push(x);
        }
    }
    v
}

fn common(a: &[String]) -> Result<Opts, String> {
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
    let mut args = BTreeMap::new();
    for kv in values(a, "--arg") {
        let (k, v) = kv.split_once('=').ok_or_else(|| format!("--arg {kv:?}: use NAME=VALUE"))?;
        if k.is_empty() {
            return Err(format!("--arg {kv:?}: the name is empty"));
        }
        args.insert(k.to_string(), v.to_string());
    }
    Ok(Opts {
        project: String::new(),
        source: Source::Inline(String::new()),
        files: Vec::new(),
        out: values(a, "-o").last().cloned(),
        out_dir: values(a, "--out-dir").last().map(PathBuf::from),
        format: values(a, "--format").last().cloned(),
        allow_read: values(a, "--allow-read").into_iter().map(PathBuf::from).collect(),
        allow_write: values(a, "--allow-write").into_iter().map(PathBuf::from).collect(),
        args,
        limits,
        save: !a.iter().any(|x| x == "--no-save"),
        json: a.iter().any(|x| x == "--json"),
        stop_on_error: a.iter().any(|x| x == "--stop-on-error"),
    })
}

pub fn parse(a: &[String]) -> Result<Opts, String> {
    let mut o = common(a)?;
    let pos = positionals(a);
    o.project = pos.first().map(|s| s.to_string()).ok_or("missing project (or `new`)")?;
    o.source = match values(a, "-e").last() {
        Some(code) => Source::Inline(code.clone()),
        None => Source::File(pos.get(1).map(|s| PathBuf::from(s.as_str())).ok_or("missing script file (or -e '<script>')")?),
    };
    Ok(o)
}

pub fn parse_batch(a: &[String]) -> Result<Opts, String> {
    let mut o = common(a)?;
    let pos = positionals(a);
    o.source = Source::File(pos.first().map(|s| PathBuf::from(s.as_str())).ok_or("missing script file")?);
    o.files = pos.iter().skip(1).map(|s| s.to_string()).collect();
    if o.files.is_empty() {
        return Err("missing files to process".into());
    }
    if o.out_dir.is_none() && o.save {
        return Err("missing --out-dir DIR (or --no-save)".into());
    }
    if let Some(f) = &o.format
        && !["modelcraft", "glb", "obj", "stl"].contains(&f.as_str())
    {
        return Err(format!("--format {f:?}: use modelcraft, glb, obj or stl"));
    }
    Ok(o)
}

fn script_dir(o: &Opts) -> PathBuf {
    match &o.source {
        Source::File(p) => p.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")),
        Source::Inline(_) => PathBuf::from("."),
    }
}

/// The permissions of a run: the script's folder and the project's folder to read, plus what was granted.
pub fn permissions(o: &Opts, project: Option<&str>) -> Permissions {
    let dir = script_dir(o);
    let mut p = Permissions::none().with_base(&dir).allow_read(&dir);
    if let Some(project) = project.filter(|p| *p != "new")
        && let Some(pd) = Path::new(project).parent().filter(|p| !p.as_os_str().is_empty())
    {
        p = p.allow_read(pd);
    }
    for d in &o.allow_read {
        p = p.allow_read(d);
    }
    for d in o.allow_write.iter().chain(o.out_dir.iter()) {
        p = p.allow_write(d);
    }
    p
}

fn name_and_text(o: &Opts) -> Result<(String, String), String> {
    match &o.source {
        Source::File(p) => {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
            Ok((p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "script".into()), text))
        }
        Source::Inline(t) => Ok(("<script>".into(), t.clone())),
    }
}

/// Run the script over `doc` and print its console (unless `quiet`). Returns the report; the caller saves.
pub fn run_script(doc: &mut Document, o: &Opts, project: Option<&str>, quiet: bool) -> Result<RunReport, String> {
    let (name, text) = name_and_text(o)?;
    let mut session = Session::new(o.limits.clone(), permissions(o, project));
    session.set_args(o.args.clone());
    session.set_current_file(project.filter(|p| *p != "new").map(str::to_string));
    let mut ctx = RunContext::default();
    let report = session.run(doc, &mut ctx, &name, &text);
    if !quiet {
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
    }
    Ok(report)
}

/// The report as one JSON object.
pub fn report_json(r: &RunReport) -> serde_json::Value {
    let output: Vec<serde_json::Value> = r
        .lines
        .iter()
        .map(|l| serde_json::json!({ "kind": match l.kind { LineKind::Print => "print", LineKind::Debug => "debug", LineKind::Warning => "warning" }, "text": l.text, "line": l.line }))
        .collect();
    let error = r.error.as_ref().map(|e| serde_json::json!({ "file": e.file, "line": e.line, "column": e.column, "message": e.message }));
    serde_json::json!({ "ok": r.error.is_none(), "value": r.value, "output": output, "commands": r.commands, "operations": r.operations, "seconds": r.elapsed.as_secs_f64(), "error": error })
}

pub fn run(a: &[String], load: fn(&str) -> Result<Document, String>, save: fn(&mut Document, &str) -> Result<(), String>) -> Result<(), String> {
    let o = parse(a)?;
    let mut d = if o.project != "new" && Path::new(&o.project).exists() { load(&o.project)? } else { Document::new() };
    let report = run_script(&mut d, &o, Some(&o.project), o.json)?;
    if o.json {
        println!("{}", report_json(&report));
    }
    if let Some(e) = &report.error {
        return Err(e.to_string());
    }
    if !o.json {
        eprintln!("{} commands, {} operations, {:.2} s", report.commands, report.operations, report.elapsed.as_secs_f64());
    }
    if o.save {
        let target =
            o.out.clone().or_else(|| (o.project != "new").then(|| o.project.clone())).ok_or("the script ran on an empty scene: give -o <out> to save it")?;
        save(&mut d, &target)?;
    }
    Ok(())
}

/// Where the result for `input` goes.
pub fn output_path(dir: &Path, input: &str, format: Option<&str>) -> PathBuf {
    let p = Path::new(input);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    let ext = match format {
        Some(f) => f.to_string(),
        None if p.extension().is_some_and(|e| e == modelcraft_engine::formats::project::EXTENSION) => "modelcraft".to_string(),
        None => "modelcraft".to_string(),
    };
    dir.join(format!("{stem}.{ext}"))
}

/// `path`, or `path` with `_2`, `_3`... added to its name until it is not in `taken`; the result is added to `taken`.
pub fn unique_output(path: PathBuf, taken: &mut std::collections::HashSet<PathBuf>) -> PathBuf {
    let mut candidate = path.clone();
    let mut n = 2u32;
    while !taken.insert(candidate.clone()) {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let name = match path.extension() {
            Some(e) => format!("{stem}_{n}.{}", e.to_string_lossy()),
            None => format!("{stem}_{n}"),
        };
        candidate = path.with_file_name(name);
        n += 1;
    }
    candidate
}

pub fn batch(a: &[String], load: fn(&str) -> Result<Document, String>, save: fn(&mut Document, &str) -> Result<(), String>) -> Result<(), String> {
    let o = parse_batch(a)?;
    if let Some(d) = &o.out_dir {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let mut failed = 0usize;
    // Two inputs with the same file name (a/part.obj, b/part.obj) must not overwrite each other's result.
    let mut taken: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for f in &o.files {
        let result = (|| -> Result<(String, u64), String> {
            let mut d = load(f)?;
            let report = run_script(&mut d, &o, Some(f), false)?;
            if let Some(e) = &report.error {
                return Err(e.to_string());
            }
            let target = match &o.out_dir {
                Some(dir) if o.save => {
                    let t = unique_output(output_path(dir, f, o.format.as_deref()), &mut taken);
                    save(&mut d, &t.to_string_lossy())?;
                    t.display().to_string()
                }
                _ => "(not saved)".to_string(),
            };
            Ok((target, report.commands))
        })();
        match result {
            Ok((t, n)) => println!("ok      {f} -> {t} ({n} commands)"),
            Err(e) => {
                failed += 1;
                println!("FAILED  {f}: {e}");
                if o.stop_on_error {
                    break;
                }
            }
        }
    }
    println!("{} of {} files processed", o.files.len().min(o.files.len() - failed.min(o.files.len())), o.files.len());
    if failed == 0 { Ok(()) } else { Err(format!("{failed} files failed")) }
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
            "--arg",
            "count=7",
            "--arg",
            "kind=cube",
            "--json",
        ]))
        .unwrap();
        assert_eq!((o.project.as_str(), o.out.as_deref(), o.allow_read.len(), o.allow_write.len()), ("a.modelcraft", Some("b.modelcraft"), 2, 1));
        assert_eq!((o.limits.max_time, o.limits.max_operations, o.save, o.json), (Duration::from_millis(2500), 99, true, true));
        assert_eq!(o.args.get("count").map(String::as_str), Some("7"));
        assert_eq!(o.source, Source::File("x.rhai".into()));
        assert!(!parse(&s(&["new", "x.rhai", "--no-save"])).unwrap().save);
        assert_eq!(parse(&s(&["new", "-e", "1 + 1"])).unwrap().source, Source::Inline("1 + 1".into()));
        for bad in [
            &["new"][..],
            &[],
            &["new", "x.rhai", "--time-limit", "soon"],
            &["new", "x.rhai", "--time-limit", "-1"],
            &["new", "x.rhai", "--max-ops", "many"],
            &["new", "x.rhai", "--arg", "novalue"],
            &["new", "x.rhai", "--arg", "=1"],
        ] {
            assert!(parse(&s(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn batch_options() {
        let o = parse_batch(&s(&["fix.rhai", "a.glb", "b.obj", "--out-dir", "out", "--format", "glb", "--stop-on-error"])).unwrap();
        assert_eq!((o.files.len(), o.format.as_deref(), o.stop_on_error), (2, Some("glb"), true));
        assert!(parse_batch(&s(&["fix.rhai", "a.glb"])).is_err(), "needs --out-dir");
        assert!(parse_batch(&s(&["fix.rhai", "--out-dir", "out"])).is_err(), "needs files");
        assert!(parse_batch(&s(&["fix.rhai", "a.glb", "--out-dir", "o", "--format", "fbx"])).is_err());
        assert!(parse_batch(&s(&["fix.rhai", "a.glb", "--no-save"])).is_ok());
        assert_eq!(output_path(Path::new("o"), "dir/Chair.glb", Some("obj")), PathBuf::from("o/Chair.obj"));
        assert_eq!(output_path(Path::new("o"), "x/scene.modelcraft", None), PathBuf::from("o/scene.modelcraft"));
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
        let r = run_script(&mut d, &o, Some("new"), true).unwrap();
        assert!(r.error.is_some_and(|e| e.message.contains("may not use writing")));
        o.allow_write.push(dir.join("out"));
        let mut d = Document::new();
        let r = run_script(&mut d, &o, Some("new"), true).unwrap();
        assert!(r.ok(), "{:?}", r.error);
        assert_eq!(r.value.as_deref(), Some("1"));
        assert_eq!(d.scene.len(), 1);
        assert_eq!(std::fs::read_to_string(dir.join("out/log.txt")).unwrap(), "hi");
        assert!(run_script(&mut d, &Opts { source: Source::File(dir.join("missing.rhai")), ..o }, None, true).is_err());
    }

    #[test]
    fn arguments_inline_scripts_and_the_json_report() {
        let o = parse(&s(&[
            "new",
            "-e",
            "add_primitive(arg(\"kind\", \"cube\"), \"A\"); print(arg(\"n\", 0) + 1); current_file()",
            "--arg",
            "kind=sphere",
            "--arg",
            "n=41",
        ]))
        .unwrap();
        let mut d = Document::new();
        let r = run_script(&mut d, &o, Some("scene.modelcraft"), true).unwrap();
        assert!(r.ok(), "{:?}", r.error);
        assert_eq!(r.value.as_deref(), Some("scene.modelcraft"));
        assert_eq!(r.lines[0].text, "42");
        let j = report_json(&r);
        assert_eq!((j["ok"].as_bool(), j["commands"].as_u64(), j["output"][0]["text"].as_str()), (Some(true), Some(1), Some("42")));
        assert!(j["error"].is_null());
        assert_eq!(d.scene.objects()[0].name, "A");
        let bad = parse(&s(&["new", "-e", "let a = 1;\nname_of(5);"])).unwrap();
        let r = run_script(&mut Document::new(), &bad, None, true).unwrap();
        let j = report_json(&r);
        assert_eq!((j["ok"].as_bool(), j["error"]["line"].as_u64(), j["error"]["file"].as_str()), (Some(false), Some(2), Some("<script>")));
    }

    #[test]
    fn two_inputs_with_one_file_name_do_not_overwrite_each_other() {
        let mut taken = std::collections::HashSet::new();
        let first = unique_output(PathBuf::from("out/part.glb"), &mut taken);
        let second = unique_output(PathBuf::from("out/part.glb"), &mut taken);
        let third = unique_output(PathBuf::from("out/part.glb"), &mut taken);
        let other = unique_output(PathBuf::from("out/other.glb"), &mut taken);
        assert_eq!(
            (first, second, third, other),
            (PathBuf::from("out/part.glb"), PathBuf::from("out/part_2.glb"), PathBuf::from("out/part_3.glb"), PathBuf::from("out/other.glb"))
        );
        // End to end: two projects named alike in different folders.
        let dir = tmp("clash");
        let script = dir.join("tag.rhai");
        std::fs::write(&script, "add_group(\"Tagged\");").unwrap();
        let mut d = Document::new();
        d.execute(modelcraft_engine::Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap();
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        d.save_as(&a.join("part.modelcraft")).unwrap();
        d.detach_journal();
        d.execute(modelcraft_engine::Command::AddGroup { name: Some("only in b".into()) }).unwrap();
        d.save_as(&b.join("part.modelcraft")).unwrap();
        d.detach_journal();
        let load = |p: &str| Document::open(Path::new(p)).map_err(|e| e.to_string());
        let save = |d: &mut Document, p: &str| d.save_as(Path::new(p)).map_err(|e| e.to_string());
        let out = dir.join("out");
        batch(
            &s(&[
                script.to_str().unwrap(),
                a.join("part.modelcraft").to_str().unwrap(),
                b.join("part.modelcraft").to_str().unwrap(),
                "--out-dir",
                out.to_str().unwrap(),
            ]),
            load,
            save,
        )
        .unwrap();
        assert_eq!(Document::open(&out.join("part.modelcraft")).unwrap().scene.len(), 2);
        assert_eq!(Document::open(&out.join("part_2.modelcraft")).unwrap().scene.len(), 3, "the second input kept its own result");
    }

    #[test]
    fn batch_processes_every_file_and_reports_failures() {
        let dir = tmp("batch");
        let script = dir.join("tag.rhai");
        std::fs::write(&script, "if arg(\"fail\", false) { throw \"asked to fail\"; } add_group(\"Tagged\");").unwrap();
        let mut a = Document::new();
        a.execute(modelcraft_engine::Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap();
        let (p1, p2) = (dir.join("one.modelcraft"), dir.join("two.modelcraft"));
        a.save_as(&p1).unwrap();
        a.detach_journal();
        a.save_as(&p2).unwrap();
        a.detach_journal();
        let load = |p: &str| Document::open(Path::new(p)).map_err(|e| e.to_string());
        let save = |d: &mut Document, p: &str| d.save_as(Path::new(p)).map_err(|e| e.to_string());
        let out = dir.join("out");
        let args = s(&[script.to_str().unwrap(), p1.to_str().unwrap(), p2.to_str().unwrap(), "--out-dir", out.to_str().unwrap()]);
        batch(&args, load, save).unwrap();
        for n in ["one", "two"] {
            let d = Document::open(&out.join(format!("{n}.modelcraft"))).unwrap();
            assert_eq!(d.scene.len(), 2, "{n}: the cube and the group the script added");
        }
        // A failing script fails every file, and the exit status says so.
        let mut failing = args.clone();
        failing.extend(s(&["--arg", "fail=true", "--out-dir", out.join("bad").to_str().unwrap()]));
        assert!(batch(&failing, load, save).is_err());
        assert!(!out.join("bad/one.modelcraft").exists());
    }
}
