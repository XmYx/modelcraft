//! Behaviour of the script API and its sandbox. Scripts run against real documents.

use std::time::Duration;

use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{Command, Document};

use crate::{ErrorKind, Event, EventKind, Limits, Permissions, RunContext, RunReport, Session};

fn run_with(session: &mut Session, doc: &mut Document, src: &str) -> RunReport {
    let mut ctx = RunContext::default();
    session.run(doc, &mut ctx, "test.rhai", src)
}

fn run(doc: &mut Document, src: &str) -> RunReport {
    run_with(&mut Session::new(Limits::default(), Permissions::none()), doc, src)
}

fn ok(src: &str) -> (Document, RunReport) {
    let mut d = Document::new();
    let r = run(&mut d, src);
    assert!(r.ok(), "{src}\n-> {}", r.error.as_ref().map(ToString::to_string).unwrap_or_default());
    (d, r)
}

fn fails(src: &str) -> String {
    let mut d = Document::new();
    let r = run(&mut d, src);
    r.error.unwrap_or_else(|| panic!("expected an error from: {src}")).to_string()
}

fn json(d: &Document) -> String {
    serde_json::to_string(&d.scene).unwrap_or_default()
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mc-script-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    std::fs::canonicalize(&d).unwrap_or(d)
}

#[test]
fn a_script_builds_a_scene_and_returns_a_value() {
    let (d, r) = ok(r#"
        let a = add_primitive("cube", "Box");
        let b = add_primitive("sphere");
        move_to(b, 2, 0.5, 0);
        print("made " + a + " and " + b);
        object_count()
    "#);
    assert_eq!(d.scene.len(), 2);
    assert_eq!(r.value.as_deref(), Some("2"));
    assert_eq!(r.lines.len(), 1);
    assert!(r.lines[0].text.starts_with("made "));
    assert_eq!(r.commands, 3);
    assert!(r.operations > 0);
    let sphere = d.scene.objects().iter().find(|o| o.name == "Sphere").map(|o| o.transform.translation);
    assert_eq!(sphere, Some(glam::Vec3::new(2.0, 0.5, 0.0)));
}

#[test]
fn every_registry_command_is_callable_and_matches_the_json_form() {
    let (d, r) = ok(r#"
        let a = cmd::add_primitive(#{ kind: "cube", name: "A" });
        let g = run(#{ cmd: "add_group" });
        cmd::rename(#{ id: a.created[0], name: "B" });
        cmd::set_transform(#{ id: a.created[0], transform: #{ translation: [1, 2, 3], rotation: [0, 0, 0, 1], scale: [1, 1, 1] } });
        cmd::undo();
        cmd::redo();
        g.created.len() + a.warnings.len()
    "#);
    assert_eq!(r.value.as_deref(), Some("1"));
    let cube = d.scene.objects().iter().find(|o| o.name == "B");
    assert_eq!(cube.map(|o| o.transform.translation), Some(glam::Vec3::new(1.0, 2.0, 3.0)));
    // The registry is the single source: every command is exposed, `import` under a safe name.
    let names: Vec<String> = ok("commands().map(|c| c.script_name)").1.value.iter().cloned().collect();
    assert!(names[0].contains("import_file") && names[0].contains("add_mesh") && names[0].contains("batch"), "{names:?}");
    assert!(fails("cmd::add_primitive(#{ kind: \"teapot\" })").contains("teapot"));
    assert!(fails("run(#{ cmd: \"nope\" })").contains("not a valid command"));
    assert!(fails("run(#{ id: 1 })").contains("cmd"));
    assert!(fails("cmd::add_primitive(#{ kind: 5 })").contains("not a valid command"));
}

#[test]
fn script_commands_are_journaled_and_undoable_like_any_other() {
    let dir = tmp("journal");
    let jp = dir.join("j.jsonl");
    let mut d = Document::new();
    d.attach_journal(&jp).unwrap();
    let r = run(&mut d, r#"let a = add_primitive("cube"); rename(a, "Renamed"); undo_group("two", || { add_group("G1"); add_group("G2"); });"#);
    assert!(r.ok());
    d.sync_journal();
    let logged = std::fs::read_to_string(&jp).unwrap();
    assert_eq!(logged.lines().count(), 4, "{logged}");
    assert_eq!(d.undo.len(), 3, "the two groups are one step");
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.scene.len(), 1);
}

#[test]
fn queries_see_the_scene() {
    let (_, r) = ok(r#"
        let g = add_group("Rig");
        let a = add_primitive("cube", "Leg 1");
        let b = add_primitive("cube", "Leg 2");
        add_primitive("sphere", "Ball");
        reparent(a, g); reparent(b, g);
        let legs = find_all("Leg*");
        let info = object_info(a);
        let out = [
            legs.len(), children(g).len(), parent(a) == g, parent(g) == (),
            find("Ball") != (), find("nothing") == (), exists(a), exists(9999), exists(-3),
            name_of(b), kind_of(g), kind_of(a), info.name, info.vertices, info.faces,
            roots().len(), object_ids().len(), object_count(),
        ];
        out.map(|x| x.to_string()).reduce(|s, x| s + "," + x, "")
    "#);
    assert_eq!(r.value.as_deref(), Some(",2,2,true,true,true,true,true,false,false,Leg 2,Group,Mesh,Leg 1,8,6,2,4,4"));
}

#[test]
fn attributes_read_and_write_through_commands() {
    let (d, r) = ok(r#"
        let c = add_primitive("cylinder", "Pipe");
        set_attr(c, "translation", [1, 2, 3]);
        set_attr(c, "rotation_euler", [0, 90, 0]);
        set_attr(c, "scale", [2, 2, 2]);
        set_attr(c, "visible", false);
        set_attr(c, "material", #{ roughness: 0.25 });
        set_attr(c, "primitive", #{ radius: 0.5 });
        set_attr(c, "name", "Pipe 2");
        let cage = cmd::add_smooth_cage(#{ input: c, levels: 1 }).created[0];
        set_attr(cage, "levels", 3);
        [get_attr(c, "translation"), get_attr(c, "rotation_euler")[1].round(), get_attr(c, "material").roughness, get_attr(c, "primitive").radius, get_attr(c, "primitive").kind, attributes(cage), get_attr(cage, "levels")].to_string()
    "#);
    let o = d.scene.objects().iter().find(|o| o.name == "Pipe 2").unwrap();
    assert!(!o.visible && o.transform.scale == glam::Vec3::splat(2.0));
    assert!(r.value.unwrap().contains(
        "[[1.0, 2.0, 3.0], 90.0, 0.25, 0.5, \"cylinder\", [\"name\", \"visible\", \"translation\", \"rotation_euler\", \"scale\", \"material\", \"levels\"], 3]"
    ));
    assert!(fails(r#"let c = add_primitive("cube"); get_attr(c, "radius")"#).contains("no attribute"));
    assert!(fails(r#"let c = add_primitive("cube"); set_attr(c, "scale", [1, 2])"#).contains("[x, y, z]"));
    assert!(fails(r#"let c = add_primitive("cube"); set_attr(c, "material", #{ roughness: "x" })"#).contains("not a valid material"));
    assert!(fails(r#"let c = add_primitive("cube"); set_attr(c, "levels", 2)"#).contains("no attribute"));
    assert!(fails("name_of(77)").contains("no object 77"));
    assert!(fails("name_of(-1)").contains("not an object id"));
}

#[test]
fn selection_is_read_and_written_through_the_run_context() {
    let mut d = Document::new();
    let a = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    let b = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    let mut s = Session::new(Limits::default(), Permissions::none());
    let mut ctx = RunContext { selection: vec![a] };
    let r = s.run(&mut d, &mut ctx, "t", "let first = selection(); select(2); select(2); [first.len(), selection().len(), is_selected(2)].to_string()");
    assert_eq!(r.value.as_deref(), Some("[1, 2, true]"));
    assert_eq!(r.selection, Some(vec![a, b]));
    assert_eq!(ctx.selection, vec![a, b]);
    // Reading does not report a change; invalid ids are refused.
    let r = s.run(&mut d, &mut ctx, "t", "selection().len()");
    assert_eq!(r.selection, None);
    let r = s.run(&mut d, &mut ctx, "t", "set_selection([1, 99])");
    assert!(r.error.is_some_and(|e| e.message.contains("no object 99")));
    assert_eq!(ctx.selection, vec![a, b], "a refused change leaves the selection");
    let r = s.run(&mut d, &mut ctx, "t", "clear_selection(); deselect(1)");
    assert_eq!(r.selection, Some(vec![]));
}

#[test]
fn the_dependency_graph_shows_what_evaluation_reads() {
    let (_, r) = ok(r#"
        let c = add_primitive("cube");
        let bend = cmd::add_deformer(#{ kind: "bend", target: c }).created[0];
        let cage = cmd::add_smooth_cage(#{ input: c, levels: 1 }).created[0];
        let e = graph_edges().filter(|x| x.kind != "transform");
        [dependencies(c), dependents(c), dependencies(cage), dependents(bend), e.len()].to_string()
    "#);
    assert_eq!(
        r.value.as_deref(),
        Some("[[2, 3], [2, 3], [1], [1], 2]"),
        "the deformer and the cage read the mesh; the mesh has no inputs of its own besides them"
    );
}

#[test]
fn history_nodes_evaluate_step_by_step() {
    let (_, r) = ok(r#"
        let c = add_primitive("cube");
        cmd::add_step(#{ id: c, op: #{ op: "subdivide", levels: 1 } });
        cmd::add_step(#{ id: c, op: #{ op: "triangulate" } });
        cmd::enable_step(#{ id: c, index: 0, enabled: true });
        let base = evaluate(c).face_count();
        let s = steps(c);
        [s.len(), s[0].name, s[1].enabled, evaluate_step(c, 0).face_count(), evaluate_step(c, 1).face_count(), base, evaluation_errors(c).len()].to_string()
    "#);
    let v = r.value.unwrap();
    assert!(v.starts_with("[2, \"Subdivide\", true, 24, 48, 48, 0]"), "{v}");
    assert!(fails(r#"let c = add_primitive("cube"); evaluate_step(c, 0)"#).contains("step 0 is out of range"));
    // A step whose selection no longer fits is reported, not fatal.
    let (_, r) = ok(
        r#"let c = add_primitive("cube"); let o = cmd::add_step(#{ id: c, op: #{ op: "extrude", faces: [99], distance: 1.0 } }); [o.warnings.len(), evaluation_errors(c)[0].step].to_string()"#,
    );
    assert_eq!(r.value.as_deref(), Some("[1, 0]"));
}

#[test]
fn mesh_handles_are_bounds_checked() {
    let (_, r) = ok(r#"
        let m = evaluate(add_primitive("cube"));
        let b = m.bounds();
        let st = m.stats();
        [m.vertex_count(), m.face_count(), m.edge_count(), m.triangle_count(), m.face(0).len(), m.is_closed(), st.closed,
         m.positions().len(), m.positions_range(6, 100).len(), m.faces().len(), m.faces_range(0, 2).len(), m.indices().len(),
         m.triangle(0).len(), m.position(0).len(), b.max[0] - b.min[0] > 0.0, st.area > 0.0, st.volume > 0.0,
         m.face_normal(0).len(), m.face_center(5).len()].to_string()
    "#);
    assert_eq!(r.value.as_deref(), Some("[8, 6, 12, 12, 4, true, true, 8, 2, 6, 2, 36, 3, 3, true, true, true, 3, 3]"));
    for bad in [
        "m.position(8)",
        "m.position(-1)",
        "m.face(6)",
        "m.face_normal(99)",
        "m.triangle(12)",
        "m.positions_range(9, 1)",
        "m.positions_range(-1, 1)",
        "m.faces_range(0, -5)",
    ] {
        let e = fails(&format!(r#"let m = evaluate(add_primitive("cube")); {bad}"#));
        assert!(e.contains("out of range") || e.contains("negative") || e.contains("past the end"), "{bad}: {e}");
    }
    // A group has an empty mesh and no bounds; an open mesh has no volume.
    let (_, r) = ok(
        r#"let m = evaluate(add_group()); let p = evaluate(add_primitive("plane")); [m.vertex_count(), m.bounds() == (), p.is_closed(), p.stats().volume == ()].to_string()"#,
    );
    assert_eq!(r.value.as_deref(), Some("[0, true, false, true]"));
    assert!(fails(r#"evaluate(add_primitive("plane")).volume()"#).contains("closed"));
}

#[test]
fn big_arrays_are_refused_not_built() {
    let mut d = Document::new();
    let mut s = Session::new(Limits { max_array_len: 100, ..Limits::default() }, Permissions::none());
    let r = run_with(&mut s, &mut d, r#"let m = evaluate(add_primitive("sphere")); m.positions().len()"#);
    assert!(r.error.is_some_and(|e| e.message.contains("positions_range")));
    let r = run_with(
        &mut s,
        &mut d,
        r#"let m = evaluate(add_primitive("sphere")); m.positions_range(0, 20).len() + m.faces_range(0, 10).len() + m.vertex_count()"#,
    );
    assert!(r.ok(), "{:?}", r.error);
    let r = run_with(&mut s, &mut d, r#"let m = evaluate(add_primitive("sphere")); m.positions_range(0, 30)"#);
    assert!(r.error.is_some_and(|e| e.message.contains("nested ones count")));
    let r = run_with(&mut s, &mut d, "object_ids().len()");
    assert!(r.ok());
}

#[test]
fn building_and_editing_meshes() {
    let (d, r) = ok(r#"
        let b = mesh_builder();
        let v0 = b.vertex(0, 0, 0);
        let v1 = b.vertex(1, 0, 0);
        let v2 = b.vertex(1, 1, 0);
        let v3 = b.vertex(0, 1, 0);
        b.quad(v0, v1, v2, v3);
        b.add_box([2, 0, 0], [3, 1, 1]);
        let id = b.build("Thing");
        let m = evaluate(id);
        let e = edit_positions(id);
        e.set(0, -1, -1, -1);
        e.translate_all(0, 0, 10);
        e.commit();
        [b.vertex_count(), b.face_count(), m.vertex_count(), m.face_count(), evaluate(id).position(0), e.len(), e.get(1)].to_string()
    "#);
    assert_eq!(r.value.as_deref(), Some("[12, 7, 12, 7, [-1.0, -1.0, 9.0], 12, [1.0, 0.0, 10.0]]"));
    assert_eq!(d.undo.len(), 2);
    for (bad, want) in [
        ("b.face([0, 1, 5])", "vertex 5"),
        ("b.face([0, 1])", "three vertices"),
        ("b.face([0, 1, \"x\"])", "list of vertex numbers"),
        ("b.triangle(0, 1, -1)", "negative"),
        ("b.vertex(0, 0, \"x\")", "expected a number"),
        ("b.build(\"x\")", "at least one face"),
        ("let e = edit_positions(add_primitive(\"cube\"));", "collapse"),
        ("let e = edit_positions(add_group());", "only mesh"),
        ("let e = edit_positions(b.build(\"y\"));", "at least one face"),
    ] {
        let e = fails(&format!("let b = mesh_builder(); b.vertex(0,0,0); b.vertex(1,0,0); b.vertex(0,1,0); {bad}"));
        assert!(e.contains(want), "{bad}: {e}");
    }
    let e = fails(
        r#"let b = mesh_builder(); b.vertex(0,0,0); b.vertex(1,0,0); b.vertex(0,1,0); b.triangle(0,1,2); let e = edit_positions(b.build("T")); e.set(3, 0, 0, 0)"#,
    );
    assert!(e.contains("vertex 3 is out of range"), "{e}");
    let e = fails(
        r#"let b = mesh_builder(); b.vertex(0,0,0); b.vertex(1,0,0); b.vertex(0,1,0); b.triangle(0,1,2); let e = edit_positions(b.build("T")); e.get(-2)"#,
    );
    assert!(e.contains("out of range"), "{e}");
    // Cloning an evaluated mesh into a new object.
    let (d, _) = ok(r#"let m = evaluate(add_primitive("cone")); let b = m.to_builder(); b.add_box([0,0,0],[1,1,1]); b.build("Copy")"#);
    assert!(d.scene.objects().iter().any(|o| o.name == "Copy"));
}

#[test]
fn an_undo_group_is_one_step_and_rolls_back_on_error() {
    let mut d = Document::new();
    let r = run(
        &mut d,
        r#"
        undo_group("Row", || {
            for i in 0..5 { let c = add_primitive("cube"); move_to(c, i, 0, 0); }
        });
        undo_group("Outer", || { undo_group("Inner", || { add_group("x"); }); add_group("y"); });
    "#,
    );
    assert!(r.ok());
    assert_eq!(d.undo.labels(), vec!["Row".to_string(), "Outer".to_string()]);
    assert_eq!(d.scene.len(), 7);
    let before = json(&d);
    let steps = d.undo.len();
    let r = run(
        &mut d,
        r#"
        undo_group("Bad", || { add_primitive("cube"); add_primitive("cube"); throw "boom"; });
    "#,
    );
    let e = r.error.unwrap();
    assert!(e.message.contains("boom") && e.line == Some(2), "{e}");
    assert_eq!(json(&d), before, "the failed group left nothing behind");
    assert_eq!(d.undo.len(), steps);
    assert!(!d.undo.can_redo());
    // A refused command inside the group rolls the earlier ones back too.
    let r = run(&mut d, r#"undo_group("Bad", || { add_group("a"); delete(9999); });"#);
    assert!(r.error.is_some());
    assert_eq!(json(&d), before);
    // Undo and redo are refused inside a group; a script's own undo works outside one.
    let r = run(&mut d, "undo_group(\"x\", || { cmd::undo(); });");
    assert!(r.error.is_some_and(|e| e.message.contains("undo group")));
    let r = run(&mut d, "cmd::undo(); object_count()");
    assert_eq!(r.value.as_deref(), Some("5"));
    assert_eq!(d.group_depth(), 0);
}

#[test]
fn the_document_comes_back_even_when_the_script_fails() {
    let mut d = Document::new();
    let r = run(&mut d, r#"add_primitive("cube"); add_primitive("cube"); throw "stop"; add_primitive("cube")"#);
    assert!(r.error.is_some());
    assert_eq!(d.scene.len(), 2, "what ran before the error stays");
    let r = run(&mut d, "let x = ;");
    assert_eq!(r.error.as_ref().map(|e| e.kind), Some(ErrorKind::Syntax));
    assert_eq!(d.scene.len(), 2);
}

// ---- sandbox ----

fn limited(l: Limits) -> (Session, Document) {
    (Session::new(l, Permissions::none()), Document::new())
}

#[test]
fn an_endless_loop_stops_at_the_operation_limit() {
    let (mut s, mut d) = limited(Limits { max_operations: 100_000, ..Limits::default() });
    let t = std::time::Instant::now();
    let r = run_with(&mut s, &mut d, "loop { }");
    let e = r.error.unwrap();
    assert_eq!(e.kind, ErrorKind::Limit);
    assert!(e.message.contains("too many operations"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(5));
    // The session is usable afterwards.
    assert!(run_with(&mut s, &mut d, "1 + 1").ok());
}

#[test]
fn a_slow_script_stops_at_the_time_limit() {
    let (mut s, mut d) = limited(Limits { max_operations: u64::MAX / 2, max_time: Duration::from_millis(150), ..Limits::default() });
    let t = std::time::Instant::now();
    let r = run_with(&mut s, &mut d, "let i = 0; while true { i += 1; }");
    let e = r.error.unwrap();
    assert_eq!(e.kind, ErrorKind::Limit);
    assert!(e.message.contains("longer than"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    // Slow commands are stopped between commands too.
    let r = run_with(&mut s, &mut d, "loop { add_group(); }");
    assert_eq!(r.error.map(|e| e.kind), Some(ErrorKind::Limit));
}

#[test]
fn deep_recursion_and_huge_values_are_refused() {
    let e = fails("fn f(n) { f(n + 1) } f(0)");
    assert!(e.contains("nests"), "{e}");
    let e = fails(&format!("let x = {}1{}; x", "(".repeat(2000), ")".repeat(2000)));
    assert!(e.contains("complexity"), "{e}");
    let e = fails(r#"let s = "x"; loop { s += s; }"#);
    assert!(e.contains("too large"), "{e}");
    let (mut s, mut d) = limited(Limits { max_array_len: 1000, ..Limits::default() });
    let r = run_with(&mut s, &mut d, "let a = []; loop { a.push(1); }");
    assert!(r.error.is_some_and(|e| e.message.contains("too large")));
    // Nested containers count together.
    let r = run_with(&mut s, &mut d, "let a = []; for i in 0..900 { a.push(1); } let o = []; loop { o.push(a); }");
    assert!(r.error.is_some_and(|e| e.message.contains("too large")));
    // Adding keys to a map is not measured by the engine as it goes; the operation limit bounds
    // it (see the memory figures in docs/scripting.md).
    let r = run_with(&mut s, &mut d, "let m = #{}; let i = 0; loop { m[i.to_string()] = i; i += 1; }");
    assert_eq!(r.error.map(|e| e.kind), Some(ErrorKind::Limit));
    let e = fails("let b = blob(2000000000); b.len()");
    assert!(e.contains("too large"), "{e}");
}

#[test]
fn commands_and_geometry_are_budgeted() {
    let (mut s, mut d) = limited(Limits { max_commands: 5, ..Limits::default() });
    let r = run_with(&mut s, &mut d, "for i in 0..10 { add_group(); }");
    assert!(r.error.is_some_and(|e| e.message.contains("more than 5 commands")));
    assert_eq!(d.scene.len(), 5);
    let (mut s, mut d) = limited(Limits { max_native_items: 1000, ..Limits::default() });
    let r = run_with(&mut s, &mut d, "let b = mesh_builder(); for i in 0..5000 { b.vertex(i, 0, 0); }");
    assert!(r.error.is_some_and(|e| e.message.contains("more geometry")));
    let r =
        run_with(&mut s, &mut d, r#"let m = evaluate(add_primitive("sphere", "S")); let a = m.to_builder(); let b = m.to_builder(); let c = m.to_builder();"#);
    assert!(r.error.is_some_and(|e| e.message.contains("more geometry")), "copies of a big mesh are charged");
    // The budget is per run.
    let r = run_with(&mut s, &mut d, "let b = mesh_builder(); for i in 0..500 { b.vertex(i, 0, 0); }");
    assert!(r.ok());
}

#[test]
fn output_is_capped() {
    let (mut s, mut d) = limited(Limits { max_output_lines: 10, ..Limits::default() });
    let r = run_with(&mut s, &mut d, "for i in 0..1000 { print(\"line \" + i); }");
    assert!(r.ok());
    assert_eq!(r.lines.len(), 11);
    assert!(r.lines[10].text.contains("cut"));
    let r = run_with(&mut s, &mut d, "debug(5); add_primitive(\"cube\", \"A\"); add_primitive(\"cube\", \"A\"); 1");
    assert_eq!(r.lines[0].line, Some(1));
}

#[test]
fn scripts_have_no_file_or_system_access_by_default() {
    // No file functions work without a grant, whatever the path.
    for src in [
        r#"read_text("/etc/passwd")"#,
        r#"read_text("a.txt")"#,
        r#"write_text("a.txt", "x")"#,
        r#"list_dir(".")"#,
        r#"file_exists("/")"#,
        r#"add_primitive("cube"); export_scene("out.glb")"#,
    ] {
        let e = fails(src);
        assert!(e.contains("may not use"), "{src}: {e}");
    }
    // Commands that name files go through the same check.
    let e = fails(r#"cmd::import_file(#{ path: "/etc/hostname" })"#);
    assert!(e.contains("may not use reading"), "{e}");
    let e = fails(r#"cmd::batch(#{ commands: [#{ cmd: "import", path: "x.obj" }] })"#);
    assert!(e.contains("may not use reading"), "{e}");
    // No eval, no modules, nothing that reaches the OS: those names simply do not exist.
    assert!(fails("eval(\"1 + 1\")").contains("eval' is disabled"));
    for src in ["system(\"ls\")", "exec(\"ls\")", "env()", "open(\"x\")", "http_get(\"http://example.com\")"] {
        assert!(fails(src).contains("Function not found"), "{src}");
    }
    assert!(fails("std::fs::read(\"x\")").contains("Undefined module"));
    assert!(fails(r#"import "other" as o; o::x()"#).contains("scripts folder"), "with no library folder every import fails");
}

#[test]
fn granted_folders_work_and_nothing_else_does() {
    let dir = tmp("granted");
    let outside = tmp("not-granted");
    std::fs::write(dir.join("in.txt"), "hello").unwrap();
    std::fs::write(outside.join("secret.txt"), "no").unwrap();
    let perms = Permissions::none().with_base(&dir).allow_read(&dir);
    let mut s = Session::new(Limits::default(), perms);
    let mut d = Document::new();
    let r = run_with(&mut s, &mut d, r#"[read_text("in.txt"), file_exists("in.txt"), file_exists("nope.txt"), list_dir(".")].to_string()"#);
    assert_eq!(r.value.as_deref(), Some("[\"hello\", true, false, [\"in.txt\"]]"), "{:?}", r.error);
    for src in [
        format!(r#"read_text("{}")"#, outside.join("secret.txt").display()),
        r#"read_text("../not-granted/secret.txt")"#.to_string(),
        r#"read_text("sub/../../x")"#.to_string(),
        r#"write_text("w.txt", "x")"#.to_string(),
    ] {
        let r = run_with(&mut s, &mut d, &src);
        assert!(r.error.is_some(), "{src}");
    }
    assert!(!dir.join("w.txt").exists());
    // Writing needs its own grant; export and write_text then work inside it.
    let w = tmp("writable");
    s.set_permissions(Permissions::none().with_base(&w).allow_write(&w));
    let r = run_with(&mut s, &mut d, r#"write_text("out.txt", "data"); add_primitive("cube"); export_scene("out.glb"); read_text("out.txt")"#);
    assert_eq!(r.value.as_deref(), Some("data"), "{:?}", r.error);
    assert!(w.join("out.glb").metadata().is_ok_and(|m| m.len() > 100));
    // Importing what was exported goes through the same permission.
    let r = run_with(&mut s, &mut d, r#"cmd::import_file(#{ path: "out.glb" }).created.len()"#);
    assert!(r.ok(), "{:?}", r.error);
    assert!(d.scene.len() >= 2);
    // A file over the limit is refused.
    let mut small = Session::new(Limits { max_file_bytes: 3, ..Limits::default() }, Permissions::none().with_base(&w).allow_read(&w));
    let r = run_with(&mut small, &mut d, r#"read_text("out.txt")"#);
    assert!(r.error.is_some_and(|e| e.message.contains("larger than")));
}

// ---- errors ----

#[test]
fn errors_name_the_file_line_and_column() {
    let mut d = Document::new();
    let mut s = Session::new(Limits::default(), Permissions::none());
    let mut ctx = RunContext::default();
    let r = s.run(&mut d, &mut ctx, "scatter.rhai", "let a = 1;\nlet b = 2;\nlet c = name_of(404);\n");
    let e = r.error.unwrap();
    assert_eq!((e.file.as_str(), e.line, e.kind), ("scatter.rhai", Some(3), ErrorKind::Runtime));
    assert!(e.to_string().starts_with("scatter.rhai:3:"), "{e}");
    assert!(e.message.contains("no object 404"), "{e}");

    let r = s.run(&mut d, &mut ctx, "bad.rhai", "let a = 1;\n\nlet b = (2 + ;\n");
    let e = r.error.unwrap();
    assert_eq!((e.line, e.kind), (Some(3), ErrorKind::Syntax));
    assert!(e.to_string().starts_with("bad.rhai:3:"), "{e}");

    // A mistyped variable is caught before anything runs.
    let r = s.run(&mut d, &mut ctx, "typo.rhai", "add_group();\nlet total = 1;\nprint(totl);\n");
    assert_eq!(d.scene.len(), 0);
    assert!(r.error.is_some_and(|e| e.line == Some(3) && e.message.to_lowercase().contains("totl")));

    // Inside functions: the line in the function, and where it was called from.
    let r = s.run(&mut d, &mut ctx, "f.rhai", "fn pick(i) {\n  name_of(i)\n}\nadd_group();\npick(99);\n");
    let e = r.error.unwrap();
    assert_eq!(e.line, Some(2));
    assert!(e.message.contains("called from line 5"), "{e}");
    // A check parses without running.
    assert!(s.check("c.rhai", "let x = 1;").is_ok());
    assert!(s.check("c.rhai", "let x = ").is_err());
}

#[test]
fn garbage_never_panics() {
    use proptest::prelude::*;
    let mut runner = proptest::test_runner::TestRunner::new(proptest::test_runner::Config { cases: 300, ..Default::default() });
    let limits = Limits { max_operations: 20_000, max_time: Duration::from_millis(500), ..Limits::default() };
    let strategy = prop_oneof![
        "[ -~\n]{0,200}",
        proptest::collection::vec(
            prop_oneof![
                Just("add_primitive("),
                Just("move_to("),
                Just("set_attr("),
                Just("evaluate("),
                Just("run(#{"),
                Just("cmd::add_step(#{"),
                Just("edit_positions("),
                Just("mesh_builder()."),
                Just("1e308"),
                Just("-9223372036854775808"),
                Just("0"),
                Just("[1,2,3]"),
                Just("\"x\""),
                Just(")"),
                Just(";"),
                Just(","),
                Just("}"),
                Just("("),
                Just("#{"),
                Just("loop {"),
                Just("."),
                Just("select("),
            ],
            0..40
        )
        .prop_map(|v| v.concat()),
    ];
    runner
        .run(&strategy, |src: String| {
            let mut s = Session::new(limits.clone(), Permissions::none());
            let mut d = Document::new();
            let mut ctx = RunContext::default();
            let r = s.run(&mut d, &mut ctx, "fuzz", &src);
            // Whatever happened, the scene is valid and the document is intact.
            prop_assert!(d.scene.clone().repaired().is_ok());
            prop_assert_eq!(d.group_depth(), 0);
            let _ = r;
            Ok(())
        })
        .unwrap();
}

#[test]
fn extreme_numbers_are_sanitised_by_the_commands() {
    let (d, _) = ok(r#"let c = add_primitive("cube"); move_to(c, 1e308, -1e308, 0); scale_to(c, 0, 0, 0); rotate_to(c, 1e300, 0, 0);"#);
    let t = d.scene.get(ObjectId(1)).unwrap().transform;
    assert!(t.translation.is_finite() && t.scale.is_finite() && t.rotation.is_finite());
    let e = fails(r#"let c = add_primitive("cube"); move_to(c, 0.0 / 0.0, 0, 0)"#);
    assert!(e.contains("finite"), "{e}");
}

#[test]
fn a_script_cannot_catch_a_limit() {
    let (mut s, mut d) = limited(Limits { max_operations: 50_000, ..Limits::default() });
    let r = run_with(&mut s, &mut d, "try { loop { } } catch { } add_group(); 1");
    assert_eq!(r.error.map(|e| e.kind), Some(ErrorKind::Limit));
    assert_eq!(d.scene.len(), 0, "nothing ran after the limit");
    let (mut s, mut d) = limited(Limits { max_commands: 3, ..Limits::default() });
    let r = run_with(&mut s, &mut d, "for i in 0..10 { try { add_group(); } catch { } }");
    assert_eq!(r.error.map(|e| e.kind), Some(ErrorKind::Limit));
    assert_eq!(d.scene.len(), 3);
    // Ordinary errors can be caught.
    let (_, r) = ok(r#"let out = "none"; try { name_of(9); } catch (e) { out = "caught"; } out"#);
    assert_eq!(r.value.as_deref(), Some("caught"));
}

// ---- arguments, tools and callbacks ----

#[test]
fn arguments_read_as_the_most_specific_value() {
    let mut d = Document::new();
    let mut s = Session::new(Limits::default(), Permissions::none());
    s.set_args([("n", "7"), ("x", "2.5"), ("on", "true"), ("name", "cube"), ("odd", "1e999")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
    s.set_current_file(Some("scene.modelcraft".into()));
    let r = run_with(
        &mut s,
        &mut d,
        r#"[arg("n", 0) + 1, arg("x", 0.0) * 2.0, arg("on", false), arg("name", "x"), arg("missing", "dflt"), arg("odd", 0), args().len(), current_file()].to_string()"#,
    );
    assert_eq!(r.value.as_deref(), Some("[8, 5.0, true, \"cube\", \"dflt\", \"1e999\", 5, \"scene.modelcraft\"]"), "{:?}", r.error);
    s.set_current_file(None);
    assert_eq!(run_with(&mut s, &mut d, "current_file() == ()").value.as_deref(), Some("true"));
    assert_eq!(run_with(&mut s, &mut d, "round_to(3.14159, 2)").value.as_deref(), Some("3.14"));
    assert_eq!(run_with(&mut s, &mut d, "round_to(7, 99)").value.as_deref(), Some("7.0"));
}

#[test]
fn make_dir_needs_a_write_grant() {
    let w = tmp("mkdir");
    let mut d = Document::new();
    assert!(run(&mut d, r#"make_dir("a/b")"#).error.is_some_and(|e| e.message.contains("may not use writing")));
    let mut s = Session::new(Limits::default(), Permissions::none().with_base(&w).allow_write(&w));
    assert!(run_with(&mut s, &mut d, r#"make_dir("a/b"); write_text("a/b/f.txt", "x")"#).ok());
    assert!(w.join("a/b/f.txt").exists());
    assert!(run_with(&mut s, &mut d, r#"make_dir("../outside")"#).error.is_some());
}

#[test]
fn a_script_registers_tools_the_app_can_run() {
    let mut d = Document::new();
    let mut s = Session::new(Limits::default(), Permissions::none());
    let mut ctx = RunContext::default();
    let r = s.run(
        &mut d,
        &mut ctx,
        "tools.rhai",
        r#"
        let made = 0;
        register_tool("Add Row", "Adds a row of cubes", || {
            made += 1;
            for i in 0..3 { let c = add_primitive("cube", "Row " + made + "." + i); move_to(c, i * 3, 0, 0); }
            print("made row " + made);
        });
        register_tool("Break", || { name_of(404); });
        tool_count()
    "#,
    );
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(r.value.as_deref(), Some("2"));
    assert_eq!(d.scene.len(), 0, "registering runs nothing");
    let tools = s.tools();
    assert_eq!(
        tools.iter().map(|t| (t.label.as_str(), t.tip.as_str(), t.owner.as_str())).collect::<Vec<_>>(),
        vec![("Add Row", "Adds a row of cubes", "tools.rhai"), ("Break", "", "tools.rhai")]
    );
    // Running a tool runs its closure, which keeps its captured state between runs.
    let r = s.run_tool(&mut d, &mut ctx, "Add Row");
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(r.lines[0].text, "made row 1");
    assert_eq!(s.run_tool(&mut d, &mut ctx, "Add Row").lines[0].text, "made row 2");
    assert_eq!(d.scene.len(), 6);
    // A failing tool reports where, in the script that registered it.
    let e = s.run_tool(&mut d, &mut ctx, "Break").error.unwrap();
    assert_eq!((e.file.as_str(), e.line), ("tools.rhai", Some(8)));
    assert!(s.run_tool(&mut d, &mut ctx, "Nothing").error.is_some_and(|e| e.message.contains("no tool called")));
    // Running the script again replaces its tools; the same label is replaced, not doubled.
    s.run(&mut d, &mut ctx, "tools.rhai", r#"register_tool("Only", || {}); register_tool("Only", || { add_group("second"); });"#);
    assert_eq!(s.tools().iter().map(|t| t.label.clone()).collect::<Vec<_>>(), vec!["Only".to_string()]);
    s.run_tool(&mut d, &mut ctx, "Only");
    assert!(d.scene.objects().iter().any(|o| o.name == "second"));
    // A different script's tools stay.
    s.run(&mut d, &mut ctx, "other.rhai", r#"register_tool("Other", || {});"#);
    s.run(&mut d, &mut ctx, "tools.rhai", "1");
    assert_eq!(s.tools().iter().map(|t| t.label.clone()).collect::<Vec<_>>(), vec!["Other".to_string()]);
    // Bad labels and too many tools are refused.
    assert!(s.run(&mut d, &mut ctx, "x.rhai", r#"register_tool("  ", || {});"#).error.is_some());
    let many = s.run(&mut d, &mut ctx, "many.rhai", r#"for i in 0..200 { register_tool("T" + i, || {}); }"#);
    assert!(many.error.is_some_and(|e| e.message.contains("at most")));
}

#[test]
fn callbacks_receive_their_event_and_only_theirs() {
    let mut d = Document::new();
    let a = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    let mut s = Session::new(Limits::default(), Permissions::none());
    let mut ctx = RunContext { selection: vec![a] };
    let r = s.run(
        &mut d,
        &mut ctx,
        "watch.rhai",
        r#"
        on_selection_changed(|e| { print("selected " + e.selection.len() + " via " + e.event); });
        on_scene_loaded(|e| { print("loaded"); });
        on_frame_changed(|e| { print("frame " + e.frame); });
        on_event("scene_changed", |e| { add_group("reaction"); });
    "#,
    );
    assert!(r.ok(), "{:?}", r.error);
    assert_eq!(s.callback_count(EventKind::SelectionChanged), 1);
    assert!(s.has_callbacks());
    let texts = |reports: Vec<RunReport>| reports.into_iter().flat_map(|r| r.lines.into_iter().map(|l| l.text)).collect::<Vec<_>>();
    assert_eq!(texts(s.fire(&mut d, &mut ctx, &Event::SelectionChanged)), vec!["selected 1 via selection_changed"]);
    assert_eq!(texts(s.fire(&mut d, &mut ctx, &Event::SceneLoaded)), vec!["loaded"]);
    assert_eq!(texts(s.fire(&mut d, &mut ctx, &Event::FrameChanged { frame: 12.5, skipped: 0 })), vec!["frame 12.5"]);
    // A callback changes the scene through commands, as undoable steps.
    let reports = s.fire(&mut d, &mut ctx, &Event::SceneChanged);
    assert!(reports[0].ok() && reports[0].commands == 1);
    assert!(d.scene.objects().iter().any(|o| o.name == "reaction"));
    // No callback for an event: nothing runs.
    let mut quiet = Session::new(Limits::default(), Permissions::none());
    assert!(!quiet.has_callbacks());
    assert!(quiet.fire(&mut d, &mut ctx, &Event::SelectionChanged).is_empty());
    // Unknown event names are refused.
    assert!(s.run(&mut d, &mut ctx, "x.rhai", r#"on_event("tuesday", || {});"#).error.is_some_and(|e| e.message.contains("unknown event")));
    // Removing them.
    s.run(&mut d, &mut ctx, "watch.rhai", "clear_callbacks();");
    assert!(!s.has_callbacks());
}

#[test]
fn callbacks_can_change_the_selection_and_are_replaced_when_their_script_runs_again() {
    let mut d = Document::new();
    let a = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    let b = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    let mut s = Session::new(Limits::default(), Permissions::none());
    let mut ctx = RunContext { selection: vec![a] };
    s.run(&mut d, &mut ctx, "w.rhai", "on_selection_changed(|e| { set_selection([2]); });");
    s.run(&mut d, &mut ctx, "w.rhai", "on_selection_changed(|e| { set_selection([2]); });");
    assert_eq!(s.callback_count(EventKind::SelectionChanged), 1, "running the script again replaced it");
    let r = s.fire(&mut d, &mut ctx, &Event::SelectionChanged);
    assert_eq!(r[0].selection, Some(vec![b]));
}

#[test]
fn a_failing_callback_is_switched_off_and_a_runaway_one_is_stopped() {
    let mut d = Document::new();
    let mut s = Session::new(Limits { max_event_time: Duration::from_millis(100), ..Limits::default() }, Permissions::none());
    let mut ctx = RunContext::default();
    s.run(&mut d, &mut ctx, "bad.rhai", r#"on_scene_changed(|e| { name_of(404); }); on_selection_changed(|e| { add_group("ok"); });"#);
    for i in 1..=3 {
        let reports = s.fire(&mut d, &mut ctx, &Event::SceneChanged);
        assert!(reports[0].error.is_some(), "call {i}");
        if i == 3 {
            assert!(reports[0].lines.iter().any(|l| l.text.contains("switched off")), "{:?}", reports[0].lines);
        }
    }
    assert_eq!(s.callback_count(EventKind::SceneChanged), 0);
    assert!(s.fire(&mut d, &mut ctx, &Event::SceneChanged).is_empty());
    assert_eq!(s.callback_count(EventKind::SelectionChanged), 1, "the others stay");
    // A success resets the count.
    s.run(&mut d, &mut ctx, "flaky.rhai", r#"let n = 0; on_scene_changed(|e| { n += 1; if n % 3 != 0 { throw "no"; } });"#);
    for _ in 0..9 {
        s.fire(&mut d, &mut ctx, &Event::SceneChanged);
    }
    assert_eq!(s.callback_count(EventKind::SceneChanged), 1, "two failures then a success never reach three in a row");
    // A callback that never returns is stopped by the shorter event limit.
    s.run(&mut d, &mut ctx, "spin.rhai", "on_frame_changed(|e| { loop { } });");
    let t = std::time::Instant::now();
    let reports = s.fire(&mut d, &mut ctx, &Event::FrameChanged { frame: 1.0, skipped: 0 });
    assert_eq!(reports[0].error.as_ref().map(|e| e.kind), Some(ErrorKind::Limit));
    assert!(t.elapsed() < Duration::from_secs(3));
}
