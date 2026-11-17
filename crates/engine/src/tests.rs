use super::*;
use glam::Vec3;
use modelcraft_scene::Transform;
use modelcraft_sculpt::{BrushKind, Stroke};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc-engine-{}-{name}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d
}

fn add(doc: &mut Document, kind: &str) -> ObjectId {
    doc.execute(Command::AddPrimitive { kind: kind.into(), name: None, parent: None }).unwrap().created[0]
}

#[test]
fn modeling_with_history_undo_and_redo() {
    let mut d = Document::new();
    let cube = add(&mut d, "cube");
    d.execute(Command::AddStep { id: cube, op: StepOp::Extrude { faces: vec![0], distance: 1.0 } }).unwrap();
    d.execute(Command::AddStep { id: cube, op: StepOp::Bevel { edges: vec![0, 1], width: 0.1 } }).unwrap();
    let cage = d.execute(Command::AddSmoothCage { input: Some(cube), levels: 2 }).unwrap().created[0];
    let faces = d.eval.evaluate(&d.scene, cage).mesh.face_count();
    assert!(faces > 100);
    // Edit an early step: everything downstream follows.
    d.execute(Command::SetStep { id: cube, index: 0, op: StepOp::Extrude { faces: vec![0], distance: 2.0 } }).unwrap();
    let (lo, hi) = d.eval.evaluate(&d.scene, cube).mesh.bounds();
    assert!((hi - lo).max_element() > 2.9);
    assert_eq!(d.undo.len(), 5);
    for _ in 0..5 {
        d.execute(Command::Undo).unwrap();
    }
    assert!(d.scene.is_empty());
    for _ in 0..5 {
        d.execute(Command::Redo).unwrap();
    }
    assert_eq!(d.eval.evaluate(&d.scene, cage).mesh.face_count(), faces);
    d.execute(Command::Collapse { id: cube }).unwrap();
    assert!(d.scene.get(cube).unwrap().steps().is_empty());
}

#[test]
fn failed_commands_leave_no_trace() {
    let mut d = Document::new();
    assert!(d.execute(Command::AddPrimitive { kind: "teapot".into(), name: None, parent: None }).is_err());
    assert!(d.execute(Command::Delete { ids: vec![ObjectId(42)] }).is_err());
    let c = add(&mut d, "cube");
    assert!(d.execute(Command::Rename { id: c, name: "  ".into() }).is_err());
    assert!(d.execute(Command::RemoveStep { id: c, index: 3 }).is_err());
    assert_eq!(d.undo.len(), 1);
    // A stale selection is a warning, not an error.
    let out = d.execute(Command::AddStep { id: c, op: StepOp::Extrude { faces: vec![99], distance: 1.0 } }).unwrap();
    assert_eq!(out.warnings.len(), 1);
}

fn stroke(kind: BrushKind) -> Stroke {
    Stroke {
        brush: modelcraft_sculpt::BrushSettings::new(kind, 0.4),
        dabs: (0..6)
            .map(|i| modelcraft_sculpt::Dab {
                center: Vec3::new(0.0, 1.0, i as f32 * 0.04).normalize(),
                view_dir: -Vec3::Y,
                pressure: 1.0,
                grab_delta: Vec3::ZERO,
            })
            .collect(),
    }
}

#[test]
fn sculpting_undo_and_save() {
    let mut d = Document::new();
    let ball = add(&mut d, "round_cube");
    d.execute(Command::BeginSculpt { id: ball }).unwrap();
    let before = d.sculpt.as_ref().unwrap().mesh.positions.clone();
    d.execute(Command::SculptStroke { id: ball, stroke: stroke(BrushKind::Draw) }).unwrap();
    d.execute(Command::SculptStroke { id: ball, stroke: stroke(BrushKind::Inflate) }).unwrap();
    let after = d.sculpt.as_ref().unwrap().mesh.positions.clone();
    assert_ne!(before, after);
    d.execute(Command::Undo).unwrap();
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.sculpt.as_ref().unwrap().mesh.positions, before);
    d.execute(Command::Redo).unwrap();
    d.execute(Command::Redo).unwrap();
    assert_eq!(d.sculpt.as_ref().unwrap().mesh.positions, after);
    // Undo past the start of sculpting removes the layer.
    let dir = tmp("sculpt");
    let p = dir.join("s.modelcraft");
    d.save_as(&p).unwrap();
    let reopened = Document::open(&p).unwrap();
    let mut d2 = reopened;
    let m = d2.eval.evaluate(&d2.scene, ball).mesh;
    assert_eq!(m.positions(), after.as_slice());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn journal_recovers_unsaved_work_after_a_crash() {
    let dir = tmp("journal");
    let p = dir.join("crash.modelcraft");
    let mut d = Document::new();
    let cube = add(&mut d, "cube");
    d.save_as(&p).unwrap();
    // Unsaved work: a step, a transform, a sculpt stroke, an undo.
    d.execute(Command::AddStep { id: cube, op: StepOp::Subdivide { levels: 2 } }).unwrap();
    let t = Transform { translation: Vec3::new(1.0, 2.0, 3.0), ..Default::default() };
    d.execute(Command::SetTransform { id: cube, transform: t }).unwrap();
    let sphere = add(&mut d, "sphere");
    d.execute(Command::Undo).unwrap();
    d.execute(Command::BeginSculpt { id: cube }).unwrap();
    d.stroke_begin(modelcraft_sculpt::BrushSettings::new(BrushKind::Clay, 0.5));
    for dab in stroke(BrushKind::Clay).dabs {
        d.stroke_dab(dab);
    }
    d.stroke_end();
    d.sync_sculpt();
    let expected = d.eval.evaluate(&d.scene, cube).mesh;
    drop(d); // crash: no save
    let mut r = Document::open(&p).unwrap();
    assert!(r.pending_recovery >= 6);
    r.recover().unwrap();
    r.sync_sculpt();
    assert!(r.scene.get(sphere).is_none(), "the undo was replayed too");
    assert_eq!(r.scene.get(cube).unwrap().transform.translation, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(*r.eval.evaluate(&r.scene, cube).mesh, *expected);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn undo_memory_is_bounded() {
    let mut d = Document::new();
    d.undo.budget = 20_000;
    for _ in 0..100 {
        add(&mut d, "cube");
    }
    assert!(d.undo.bytes() <= 20_000 + 512 + 100 * 300);
    assert!(d.undo.dropped > 0);
    assert!(d.undo.len() < 100);
}

#[test]
fn import_export_round_trip_and_hostile_paths() {
    let dir = tmp("io");
    let mut d = Document::new();
    let c = add(&mut d, "torus");
    d.execute(Command::AddDeformer { kind: "twist".into(), target: c }).unwrap();
    for ext in ["glb", "obj", "stl"] {
        let p = dir.join(format!("out.{ext}"));
        d.export(&p).unwrap();
        let mut e = Document::new();
        let out = e.execute(Command::Import { path: p.display().to_string() }).unwrap();
        assert!(!out.created.is_empty(), "{ext}");
    }
    assert!(d.execute(Command::Import { path: dir.join("nope.glb").display().to_string() }).is_err());
    assert!(d.execute(Command::Import { path: "/etc/passwd".into() }).is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn commands_round_trip_as_json() {
    let cmds = vec![
        Command::AddPrimitive { kind: "cube".into(), name: Some("A".into()), parent: None },
        Command::AddStep { id: ObjectId(1), op: StepOp::LoopCut { edge: 3, cuts: 2 } },
        Command::SculptStroke { id: ObjectId(1), stroke: stroke(BrushKind::Grab) },
        Command::Undo,
    ];
    for c in cmds {
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&j).unwrap(), c);
    }
    let c: Command = serde_json::from_str(r#"{"cmd":"add_primitive","kind":"cube"}"#).unwrap();
    assert!(matches!(c, Command::AddPrimitive { .. }));
    assert_eq!(crate::COMMANDS.len(), 26);
}
