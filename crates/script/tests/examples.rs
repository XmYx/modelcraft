//! Every script in `examples/scripts` runs here, against a real document, and its result is
//! checked. A test at the end fails when an example has no test.

use std::collections::BTreeMap;
use std::path::PathBuf;

use modelcraft_engine::scene::ObjectId;
use modelcraft_engine::{Command, Document};
use modelcraft_script::{Limits, Permissions, RunContext, RunReport, Session};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/scripts")
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-examples-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let _ = std::fs::create_dir_all(&d);
    std::fs::canonicalize(&d).unwrap_or(d)
}

fn run(example: &str, args: &[(&str, &str)], doc: &mut Document, ctx: &mut RunContext, perms: Permissions) -> RunReport {
    let text = std::fs::read_to_string(dir().join(example)).unwrap_or_else(|e| panic!("{example}: {e}"));
    let mut s = Session::new(Limits::default(), perms);
    s.set_args(args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>());
    s.run(doc, ctx, example, &text)
}

fn simple(example: &str, args: &[(&str, &str)], doc: &mut Document) -> RunReport {
    run(example, args, doc, &mut RunContext::default(), Permissions::none())
}

fn add(doc: &mut Document, kind: &str, name: &str) -> ObjectId {
    doc.execute(Command::AddPrimitive { kind: kind.into(), name: Some(name.into()), parent: None }).unwrap().created[0]
}

fn assert_ok(r: &RunReport) {
    assert!(r.ok(), "{}", r.error.as_ref().map(ToString::to_string).unwrap_or_default());
}

fn positions(doc: &Document) -> Vec<[f32; 3]> {
    doc.scene.objects().iter().map(|o| o.transform.translation.to_array()).collect()
}

#[test]
fn scatter() {
    let args = [("count", "25"), ("seed", "3"), ("area", "8"), ("kind", "cube")];
    let mut a = Document::new();
    let r = simple("scatter.rhai", &args, &mut a);
    assert_ok(&r);
    assert_eq!(a.scene.len(), 26, "the group and 25 objects");
    assert_eq!(a.undo.len(), 1, "one undo step");
    let group = a.scene.objects().iter().find(|o| o.name == "Scatter 3").map(|o| o.id).expect("the group");
    assert_eq!(a.scene.children(group).len(), 25);
    for o in a.scene.objects().iter().filter(|o| o.parent.is_some()) {
        let t = o.transform.translation;
        assert!(t.x.abs() <= 4.0 && t.z.abs() <= 4.0 && t.y > 0.0, "{t:?}");
        assert!(o.transform.scale.x >= 0.05 && o.transform.scale.x <= 0.4);
    }
    // The same seed gives the same scatter, another seed another one.
    let mut b = Document::new();
    assert_ok(&simple("scatter.rhai", &args, &mut b));
    assert_eq!(positions(&a), positions(&b));
    let mut c = Document::new();
    assert_ok(&simple("scatter.rhai", &[("count", "25"), ("seed", "4"), ("area", "8"), ("kind", "cube")], &mut c));
    assert_ne!(positions(&a), positions(&c));
    // Defaults work, and one undo removes everything.
    let mut d = Document::new();
    assert_ok(&simple("scatter.rhai", &[], &mut d));
    assert_eq!(d.scene.len(), 41);
    d.execute(Command::Undo).unwrap();
    assert!(d.scene.is_empty());
    assert!(!simple("scatter.rhai", &[("kind", "teapot")], &mut Document::new()).ok());
}

#[test]
fn batch_export() {
    let out = tmp("export");
    let mut doc = Document::new();
    add(&mut doc, "cube", "Box");
    let s = add(&mut doc, "sphere", "Ball");
    doc.execute(Command::SetVisible { id: s, visible: false }).unwrap();
    add(&mut doc, "cone", "Spike/One");
    let undo = doc.undo.len();
    let perms = Permissions::none().with_base(&out).allow_write(&out);
    let r = run("batch_export.rhai", &[("dir", "exports"), ("format", "obj")], &mut doc, &mut RunContext::default(), perms);
    assert_ok(&r);
    let files: Vec<String> = std::fs::read_dir(out.join("exports")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    let mut files = files;
    files.sort();
    assert_eq!(files, vec!["Ball.obj", "Box.obj", "Spike_One.obj"]);
    for f in &files {
        assert!(std::fs::metadata(out.join("exports").join(f)).unwrap().len() > 50, "{f}");
    }
    assert!(std::fs::read_to_string(out.join("exports/Box.obj")).unwrap().contains("v "));
    // Visibility is back as it was, and the whole thing is one undo step.
    assert_eq!(doc.scene.objects().iter().map(|o| o.visible).collect::<Vec<_>>(), vec![true, false, true]);
    assert_eq!(doc.undo.len(), undo + 1);
    // Without a grant it refuses before touching anything.
    let mut other = Document::new();
    add(&mut other, "cube", "Box");
    let r = simple("batch_export.rhai", &[], &mut other);
    assert!(r.error.is_some_and(|e| e.message.contains("allow writing files")));
}

#[test]
fn rename_by_pattern() {
    let mut doc = Document::new();
    for n in ["Cube", "Cube", "Cube", "Sphere"] {
        add(&mut doc, if n == "Cube" { "cube" } else { "sphere" }, n);
    }
    let r = simple("rename_by_pattern.rhai", &[("pattern", "Cube*"), ("name", "Part_#")], &mut doc);
    assert_ok(&r);
    assert_eq!(r.value.as_deref(), Some("3"));
    let names: Vec<String> = doc.scene.objects().iter().map(|o| o.name.clone()).collect();
    assert_eq!(names, vec!["Part_01", "Part_02", "Part_03", "Sphere"]);
    assert_eq!(doc.undo.len(), 5, "four objects and one rename step");
    // More than 99 matches use three digits.
    let mut big = Document::new();
    for _ in 0..101 {
        add(&mut big, "cube", "Tile");
    }
    assert_ok(&simple("rename_by_pattern.rhai", &[("pattern", "Tile*"), ("name", "T#")], &mut big));
    assert_eq!(big.scene.objects().first().map(|o| o.name.as_str()), Some("T001"));
    assert_eq!(big.scene.objects().last().map(|o| o.name.as_str()), Some("T101"));
}

#[test]
fn mesh_stats() {
    let mut doc = Document::new();
    add(&mut doc, "cube", "Box");
    add(&mut doc, "plane", "Floor");
    doc.execute(Command::AddGroup { name: Some("Not a mesh".into()) }).unwrap();
    let r = simple("mesh_stats.rhai", &[], &mut doc);
    assert_ok(&r);
    let out: Vec<&str> = r.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(out[0], "Box: 8 vertices, 6 faces, 12 triangles, area 24.0, volume 8.0");
    assert!(out[1].starts_with("Floor: 25 vertices, 16 faces, 32 triangles, area 16.0") && out[1].ends_with("volume open"), "{}", out[1]);
    assert_eq!(out[2], "2 meshes, 44 triangles, area 40.0");
    assert_eq!(out.len(), 3);
    assert_eq!(r.value.as_deref(), Some("44"));
}

#[test]
fn staircase() {
    let mut doc = Document::new();
    let r = simple("staircase.rhai", &[], &mut doc);
    assert_ok(&r);
    let id = doc.scene.objects().iter().find(|o| o.name == "Staircase").map(|o| o.id).expect("the staircase");
    let m = doc.eval.evaluate(&doc.scene, id).mesh;
    assert_eq!((m.vertex_count(), m.face_count()), (12 * 8, 12 * 6));
    let (lo, hi) = m.bounds();
    assert!((hi.y - 12.0 * 0.18).abs() < 1e-4 && (hi.z - 12.0 * 0.28).abs() < 1e-4 && (hi.x - 1.2).abs() < 1e-5);
    assert_eq!((lo.x, lo.y, lo.z), (0.0, 0.0, 0.0));
    assert!(r.lines[0].text.contains("12 steps"));
    // Parameters change the shape; nonsense is refused with a message.
    let mut d2 = Document::new();
    assert_ok(&simple("staircase.rhai", &[("steps", "3"), ("rise", "0.5"), ("run", "1"), ("width", "2")], &mut d2));
    let id = d2.scene.objects()[0].id;
    let (_, hi) = d2.eval.evaluate(&d2.scene, id).mesh.bounds();
    assert_eq!((hi.x, hi.y, hi.z), (2.0, 1.5, 3.0));
    for bad in ["0", "201", "-4"] {
        let r = simple("staircase.rhai", &[("steps", bad)], &mut Document::new());
        assert!(r.error.is_some_and(|e| e.message.contains("steps must be")), "{bad}");
    }
}

#[test]
fn distribute_selection() {
    let mut doc = Document::new();
    let mut ids = Vec::new();
    for (i, x) in [0.0f32, 10.0, 1.0, 2.0].into_iter().enumerate() {
        let id = add(&mut doc, "cube", &format!("C{i}"));
        doc.execute(Command::SetTransform {
            id,
            transform: modelcraft_engine::scene::Transform { translation: glam::Vec3::new(x, 0.5, 0.0), ..Default::default() },
        })
        .unwrap();
        ids.push(id);
    }
    let mut ctx = RunContext { selection: ids.clone() };
    let r = run("distribute_selection.rhai", &[], &mut doc, &mut ctx, Permissions::none());
    assert_ok(&r);
    let mut xs: Vec<f32> = ids.iter().map(|i| doc.scene.get(*i).unwrap().transform.translation.x).collect();
    xs.sort_by(f32::total_cmp);
    for (got, want) in xs.iter().zip([0.0, 10.0 / 3.0, 20.0 / 3.0, 10.0]) {
        assert!((got - want).abs() < 1e-4, "{xs:?}");
    }
    assert!(doc.scene.objects().iter().all(|o| o.transform.translation.y == 0.5), "other axes stay");
    assert_eq!(r.value.as_deref(), Some("4"));
    // Needs three objects and a valid axis.
    let r = run("distribute_selection.rhai", &[], &mut doc, &mut RunContext { selection: vec![ids[0]] }, Permissions::none());
    assert!(r.error.is_some_and(|e| e.message.contains("at least three")));
    let r = run("distribute_selection.rhai", &[("axis", "w")], &mut doc, &mut RunContext { selection: ids.clone() }, Permissions::none());
    assert!(r.error.is_some_and(|e| e.message.contains("axis must be")));
}

#[test]
fn dependency_report() {
    let mut doc = Document::new();
    let r = simple("dependency_report.rhai", &[], &mut doc);
    assert_ok(&r);
    assert_eq!(r.lines[0].text, "no object reads another object's result");
    let c = add(&mut doc, "cube", "Cube");
    doc.execute(Command::AddDeformer { kind: "bend".into(), target: c }).unwrap();
    doc.execute(Command::AddSmoothCage { input: Some(c), levels: 1 }).unwrap();
    let r = simple("dependency_report.rhai", &[], &mut doc);
    assert_ok(&r);
    let lines: Vec<&str> = r.lines.iter().map(|l| l.text.as_str()).collect();
    assert!(lines.contains(&"Cube is bent by Bend"), "{lines:?}");
    assert!(lines.contains(&"Smooth cage is built from Cube"), "{lines:?}");
    assert_eq!(r.value.as_deref(), Some("2"));
}

/// Every example is listed here and has a test above.
#[test]
fn every_example_has_a_test() {
    let tested = ["scatter", "batch_export", "rename_by_pattern", "mesh_stats", "staircase", "distribute_selection", "dependency_report"];
    let mut found: Vec<String> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "rhai"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    found.sort();
    let mut want: Vec<String> = tested.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(found, want, "an example without a test (or a test without an example)");
    assert!(found.len() >= 5);
    // The example library also parses with the default session (a cheap guard against typos).
    let s = Session::new(Limits::default(), Permissions::none());
    for n in &found {
        let text = std::fs::read_to_string(dir().join(format!("{n}.rhai"))).unwrap();
        assert!(s.check(n, &text).is_ok(), "{n}");
    }
}
