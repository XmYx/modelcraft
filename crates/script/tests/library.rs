//! Script libraries: `import "lib/name" as m;` from the scripts folder, sandboxed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use modelcraft_engine::{Command, Document};
use modelcraft_script::library::{MAX_IMPORT_DEPTH, MAX_MODULES, locate, valid_name};
use modelcraft_script::{ErrorKind, Event, Limits, Permissions, RunContext, RunReport, Session};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-lib-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn session(dir: Option<&Path>) -> Session {
    let s = Session::new(Limits::default(), Permissions::none());
    s.set_library(dir.map(Path::to_path_buf));
    s
}

fn run(s: &mut Session, doc: &mut Document, text: &str) -> RunReport {
    s.run(doc, &mut RunContext::default(), "main.rhai", text)
}

fn names(d: &Document) -> Vec<String> {
    d.scene.objects().iter().map(|o| o.name.clone()).collect()
}

const SHAPES: &str = r#"
// A library: functions and constants
export const SIDES = 6;
fn row(n, name) {
    for i in 0..n { move_to(add_primitive("cube", name + i), i * 2, 0, 0); }
}
fn double(x) { x * 2 }
"#;

#[test]
fn a_script_imports_a_library_from_the_scripts_folder() {
    let dir = tmp("basic");
    write(&dir, "lib/shapes.rhai", SHAPES);
    let mut s = session(Some(&dir));
    let mut d = Document::new();
    let r = run(&mut s, &mut d, "import \"lib/shapes\" as shapes; shapes::row(3, \"c\"); print(shapes::double(shapes::SIDES));");
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(names(&d).len(), 3);
    assert_eq!(r.lines[0].text, "12");
    assert_eq!(r.commands, 6, "the library's commands count against the run's limit like the script's own");
    // The same module twice in one run is loaded once; the `.rhai` suffix is optional.
    let r = run(&mut s, &mut d, "import \"lib/shapes\" as a; import \"lib/shapes.rhai\" as b; print(a::double(1) + b::double(2));");
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(r.lines[0].text, "6");
}

#[test]
fn imports_need_a_library_folder_and_say_so() {
    let mut s = session(None);
    let mut d = Document::new();
    let r = run(&mut s, &mut d, "import \"lib/shapes\" as s; s::double(1);");
    let e = r.error.expect("an error");
    assert!(e.message.contains("scripts folder"), "{e}");
    // A folder without the module says which one.
    let dir = tmp("missing");
    let mut s = session(Some(&dir));
    let e = run(&mut s, &mut d, "import \"nope\" as n;").error.expect("an error");
    assert!(e.message.contains("no library \"nope\""), "{e}");
}

#[test]
fn module_names_cannot_leave_the_folder_or_name_odd_things() {
    for bad in [
        "", "../x", "lib/../../x", "/etc/passwd", "a\\b", "C:x", ".hidden", "lib/.hidden", "a//b", "a/", "x.", "a/../b", "na\0me", "a|b", "a*b", "a?b", &"n".repeat(121),
    ] {
        assert!(valid_name(bad).is_err(), "{bad:?}");
    }
    for good in ["lib", "lib/shapes", "my lib/over-there", "a.b/c_d", "ünï/日本"] {
        assert!(valid_name(good).is_ok(), "{good:?}");
    }
    let dir = tmp("escape");
    let outside = tmp("escape-outside");
    write(&outside, "secret.rhai", "fn leak() { 42 }");
    write(&dir, "inside.rhai", "fn ok() { 1 }");
    assert!(locate(&dir, "inside").is_ok());
    assert!(locate(&dir, "../escape-outside/secret").is_err());
    // Every spelling of "up" is refused before the disk is touched, through a whole run too.
    let mut s = session(Some(&dir));
    let mut d = Document::new();
    for name in ["../escape-outside/secret", "lib/../../escape-outside/secret", "/etc/hostname"] {
        let r = run(&mut s, &mut d, &format!("import {name:?} as x; x::leak();"));
        assert!(r.error.is_some(), "{name}");
    }
}

#[cfg(unix)]
#[test]
fn a_link_pointing_out_of_the_folder_is_not_followed() {
    let dir = tmp("symlink");
    let outside = tmp("symlink-outside");
    write(&outside, "secret.rhai", "fn leak() { 42 }");
    std::os::unix::fs::symlink(outside.join("secret.rhai"), dir.join("linked.rhai")).unwrap();
    std::os::unix::fs::symlink(&outside, dir.join("dir-link")).unwrap();
    let mut s = session(Some(&dir));
    let mut d = Document::new();
    for text in ["import \"linked\" as l; l::leak();", "import \"dir-link/secret\" as l; l::leak();"] {
        let e = run(&mut s, &mut d, text).error.expect("refused");
        assert!(e.message.contains("outside the scripts folder"), "{e}");
    }
    // A link that stays inside the folder is fine.
    write(&dir, "real.rhai", "fn ok() { 7 }");
    std::os::unix::fs::symlink(dir.join("real.rhai"), dir.join("alias.rhai")).unwrap();
    assert!(run(&mut s, &mut d, "import \"alias\" as a; print(a::ok());").ok());
}

#[test]
fn libraries_run_under_the_same_limits_and_permissions_as_the_script() {
    let dir = tmp("limits");
    write(&dir, "spin.rhai", "loop { }");
    write(&dir, "reader.rhai", "fn read() { read_text(\"/etc/hostname\") }");
    write(&dir, "flood.rhai", "fn flood() { for i in 0..100 { add_group(\"g\"); } }");
    write(&dir, "huge.rhai", &format!("// {}\nfn f() {{ 1 }}", "x".repeat(5 << 20)));
    let mut s = Session::new(Limits { max_operations: 100_000, max_commands: 50, ..Limits::default() }, Permissions::none());
    s.set_library(Some(dir.clone()));
    let mut d = Document::new();
    // An endless module body stops at the operation limit.
    let t = Instant::now();
    let e = run(&mut s, &mut d, "import \"spin\" as s;").error.expect("stopped");
    assert_eq!(e.kind, ErrorKind::Limit, "{e}");
    assert!(t.elapsed() < Duration::from_secs(5));
    // Files stay behind the script's permissions.
    let e = run(&mut s, &mut d, "import \"reader\" as r; r::read();").error.expect("denied");
    assert!(e.message.to_lowercase().contains("permission") || e.message.contains("not allowed") || e.message.contains("read"), "{e}");
    // The command limit is the run's, not the library's.
    let e = run(&mut s, &mut d, "import \"flood\" as f; f::flood();").error.expect("stopped");
    assert_eq!(e.kind, ErrorKind::Limit, "{e}");
    assert_eq!(d.scene.len(), 50);
    // A library over the file size limit is refused.
    let e = run(&mut s, &mut d, "import \"huge\" as h;").error.expect("refused");
    assert!(e.message.contains("regular file"), "{e}");
}

#[test]
fn cycles_depth_and_module_counts_are_bounded() {
    let dir = tmp("bounds");
    write(&dir, "a.rhai", "import \"b\" as b; fn f() { 1 }");
    write(&dir, "b.rhai", "import \"a\" as a; fn g() { 2 }");
    write(&dir, "self.rhai", "import \"self\" as me;");
    let mut s = session(Some(&dir));
    let mut d = Document::new();
    let e = run(&mut s, &mut d, "import \"a\" as a;").error.expect("cycle");
    assert!(e.message.contains("cycle"), "{e}");
    let e = run(&mut s, &mut d, "import \"self\" as me;").error.expect("cycle");
    assert!(e.message.contains("cycle"), "{e}");
    // A chain just inside and just outside the depth limit.
    for i in 0..=MAX_IMPORT_DEPTH + 2 {
        let body = if i == MAX_IMPORT_DEPTH + 2 { "fn leaf() { 1 }".to_string() } else { format!("import \"chain{}\" as next; fn leaf() {{ 1 }}", i + 1) };
        write(&dir, &format!("chain{i}.rhai"), &body);
    }
    let e = run(&mut s, &mut d, "import \"chain0\" as c;").error.expect("too deep");
    assert!(e.message.contains("deep"), "{e}");
    assert!(run(&mut s, &mut d, &format!("import \"chain{}\" as c; print(c::leaf());", 3)).ok());
    // Too many modules in one run.
    for i in 0..=MAX_MODULES {
        write(&dir, &format!("m{i}.rhai"), "fn f() { 1 }");
    }
    let many: String = (0..=MAX_MODULES).map(|i| format!("import \"m{i}\" as m{i};\n")).collect();
    let e = run(&mut s, &mut d, &many).error.expect("too many");
    assert!(e.message.contains("at most"), "{e}");
    // And the next run starts again from zero.
    assert!(run(&mut s, &mut d, "import \"m0\" as m; print(m::f());").ok());
}

#[test]
fn errors_in_a_library_name_the_library() {
    let dir = tmp("errors");
    write(&dir, "broken.rhai", "fn f( { }");
    write(&dir, "throws.rhai", "fn f() { throw \"from the library\"; }");
    let mut s = session(Some(&dir));
    let mut d = Document::new();
    let e = run(&mut s, &mut d, "import \"broken\" as b;").error.expect("syntax error");
    assert!(e.to_string().contains("broken"), "{e}");
    let e = run(&mut s, &mut d, "import \"throws\" as t; t::f();").error.expect("thrown");
    assert!(e.message.contains("from the library"), "{e}");
}

#[test]
fn edits_to_a_library_show_up_in_the_next_run() {
    let dir = tmp("edit");
    write(&dir, "v.rhai", "fn version() { 1 }");
    let mut s = session(Some(&dir));
    let mut d = Document::new();
    assert_eq!(run(&mut s, &mut d, "import \"v\" as v; print(v::version());").lines[0].text, "1");
    write(&dir, "v.rhai", "fn version() { 2 }");
    assert_eq!(run(&mut s, &mut d, "import \"v\" as v; print(v::version());").lines[0].text, "2");
}

#[test]
fn tools_and_callbacks_can_call_library_functions_after_the_script_ended() {
    let dir = tmp("callbacks");
    write(&dir, "naming.rhai", "fn label(n) { \"made-\" + n }");
    let mut s = session(Some(&dir));
    let mut d = Document::new();
    let mut ctx = RunContext::default();
    let r = s.run(
        &mut d,
        &mut ctx,
        "tool.rhai",
        "import \"naming\" as naming; register_tool(\"Make One\", || { add_group(naming::label(1)); }); on_selection_changed(|e| { add_group(naming::label(e.selection.len())); });",
    );
    assert!(r.ok(), "{:?}", r.error);
    let r = s.run_tool(&mut d, &mut ctx, "Make One");
    assert!(r.ok(), "{:?}", r.error);
    let reports = s.fire(&mut d, &mut ctx, &Event::SelectionChanged);
    assert!(reports[0].ok(), "{:?}", reports[0].error);
    assert_eq!(names(&d), vec!["made-1".to_string(), "made-0".to_string()]);
}

#[test]
fn the_library_folder_can_be_changed_or_removed() {
    let (a, b) = (tmp("swap-a"), tmp("swap-b"));
    write(&a, "who.rhai", "fn me() { \"a\" }");
    write(&b, "who.rhai", "fn me() { \"b\" }");
    let mut s = session(Some(&a));
    let mut d = Document::new();
    assert_eq!(run(&mut s, &mut d, "import \"who\" as w; print(w::me());").lines[0].text, "a");
    s.set_library(Some(b));
    assert_eq!(run(&mut s, &mut d, "import \"who\" as w; print(w::me());").lines[0].text, "b");
    s.set_library(None);
    assert!(run(&mut s, &mut d, "import \"who\" as w;").error.is_some());
    s.set_library(Some(PathBuf::from("/definitely/not/a/folder")));
    assert!(run(&mut s, &mut d, "import \"who\" as w;").error.is_some());
    // The document is untouched by all the failed imports.
    assert!(d.scene.is_empty());
    let _ = d.execute(Command::Undo);
}

#[test]
fn the_import_scanner_ignores_comments_and_strings() {
    use modelcraft_script::library::imports;
    let src = "// import \"nope\" as no;\nimport \"lib/a\" as a;\nlet s = \"import \\\"x\\\" as y;\";\n/* import \"c\" as c; */\nimport  \"lib/b c\"   as  bc ;\nimport lib as l;\nimport \"back\\\\slash\" as bs;";
    assert_eq!(imports(src), vec![("lib/a".to_string(), "a".to_string()), ("lib/b c".to_string(), "bc".to_string())]);
    assert!(imports("").is_empty());
    assert!(imports("import").is_empty());
}

#[test]
fn an_alias_cannot_replace_the_engines_own_namespaces() {
    let dir = tmp("alias");
    write(&dir, "evil.rhai", "fn add_primitive_is_mine() { 1 }");
    let mut s = session(Some(&dir));
    let mut d = Document::new();
    // Importing a library as `cmd` works inside that script, but the session does not keep it:
    // the command namespace stays the engine's.
    assert!(run(&mut s, &mut d, "import \"evil\" as cmd;").ok());
    let r = run(&mut s, &mut d, "cmd::add_group(#{ name: \"still works\" });");
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(names(&d), vec!["still works".to_string()]);
}
