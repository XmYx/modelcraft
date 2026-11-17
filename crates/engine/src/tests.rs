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
                scale: 1.0,
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
        Command::SculptDetail { id: ObjectId(1), op: crate::DetailOp::Up },
        Command::SculptShape { id: ObjectId(1), op: crate::ShapeOp::Strength { index: 0, strength: 0.25 } },
        Command::SculptShape { id: ObjectId(1), op: crate::ShapeOp::Select { index: None } },
        Command::Undo,
    ];
    for c in cmds {
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&j).unwrap(), c);
    }
    let c: Command = serde_json::from_str(r#"{"cmd":"add_primitive","kind":"cube"}"#).unwrap();
    assert!(matches!(c, Command::AddPrimitive { .. }));
    assert_eq!(crate::COMMANDS.len(), 28);
}

fn sculpt_positions(d: &Document) -> Vec<glam::Vec3> {
    d.sculpt.as_ref().unwrap().mesh.positions.clone()
}

fn far(a: &[glam::Vec3], b: &[glam::Vec3]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| x.distance(*y)).fold(0.0, f32::max)
}

#[test]
fn detail_levels_keep_the_sculpt_and_carry_it_through_coarse_edits() {
    use crate::DetailOp::{Down, Up};
    let mut d = Document::new();
    let ball = d.execute(Command::AddPrimitive { kind: "round_cube".into(), name: None, parent: None }).unwrap().created[0];
    d.execute(Command::BeginSculpt { id: ball }).unwrap();
    let base_n = d.sculpt.as_ref().unwrap().mesh.vertex_count();
    assert!(matches!(d.execute(Command::SculptDetail { id: ball, op: Down }), Err(EngineError::Invalid(_))), "already at the base");
    d.execute(Command::SculptDetail { id: ball, op: Up }).unwrap();
    assert_eq!(d.sculpt_layer(ball).unwrap().level, 1);
    let fine_n = d.sculpt.as_ref().unwrap().mesh.vertex_count();
    assert!(fine_n > base_n * 3);
    // Sculpt fine detail, then go down and up again: nothing is lost.
    d.execute(Command::SculptStroke { id: ball, stroke: stroke(BrushKind::Draw) }).unwrap();
    d.execute(Command::SculptStroke { id: ball, stroke: stroke(BrushKind::Crease) }).unwrap();
    let detailed = sculpt_positions(&d);
    d.execute(Command::SculptDetail { id: ball, op: Down }).unwrap();
    assert_eq!(d.sculpt.as_ref().unwrap().mesh.vertex_count(), base_n);
    assert_eq!(d.sculpt_layer(ball).unwrap().finer.len(), 1);
    d.execute(Command::SculptDetail { id: ball, op: Up }).unwrap();
    assert!(far(&sculpt_positions(&d), &detailed) < 1e-4);
    // The evaluated object is the level being shown.
    assert_eq!(d.eval.evaluate(&d.scene, ball).mesh.vertex_count(), fine_n);
    // A coarse edit moves the fine detail with it, and undo walks back through the levels.
    d.execute(Command::SculptDetail { id: ball, op: Down }).unwrap();
    d.execute(Command::SculptStroke { id: ball, stroke: stroke(BrushKind::Inflate) }).unwrap();
    d.execute(Command::SculptDetail { id: ball, op: Up }).unwrap();
    assert!(far(&sculpt_positions(&d), &detailed) > 1e-3);
    for _ in 0..3 {
        d.execute(Command::Undo).unwrap();
    }
    d.ensure_session(ball).unwrap();
    assert!(far(&sculpt_positions(&d), &detailed) < 1e-4);
    // Save and load keep the level and the stored displacement.
    d.execute(Command::SculptDetail { id: ball, op: Down }).unwrap();
    let dir = tmp("detail");
    let path = dir.join("d.modelcraft");
    d.save_as(&path).unwrap();
    let mut r = Document::open(&path).unwrap();
    r.execute(Command::SculptDetail { id: ball, op: Up }).unwrap();
    assert!(far(&sculpt_positions(&r), &detailed) < 1e-4);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn shape_layers_blend_with_their_strength_and_survive_undo_and_reload() {
    use crate::ShapeOp;
    let mut d = Document::new();
    let ball = d.execute(Command::AddPrimitive { kind: "round_cube".into(), name: None, parent: None }).unwrap().created[0];
    d.execute(Command::BeginSculpt { id: ball }).unwrap();
    d.execute(Command::SculptStroke { id: ball, stroke: stroke(BrushKind::Draw) }).unwrap();
    let base = sculpt_positions(&d);
    let shape = |d: &mut Document, op: ShapeOp| d.execute(Command::SculptShape { id: ball, op });
    shape(&mut d, ShapeOp::Add { name: Some("  Brow\n ".into()) }).unwrap();
    assert_eq!(d.sculpt_layer(ball).unwrap().shapes[0].name, "Brow");
    d.execute(Command::SculptStroke { id: ball, stroke: stroke(BrushKind::Inflate) }).unwrap();
    let full = sculpt_positions(&d);
    assert!(far(&full, &base) > 1e-3);
    // Half strength is half way; zero is the base; back to one is the full sculpt.
    shape(&mut d, ShapeOp::Strength { index: 0, strength: 0.5 }).unwrap();
    let half: Vec<glam::Vec3> = base.iter().zip(&full).map(|(a, b)| a.lerp(*b, 0.5)).collect();
    assert!(far(&sculpt_positions(&d), &half) < 1e-5);
    shape(&mut d, ShapeOp::Strength { index: 0, strength: 0.0 }).unwrap();
    assert!(far(&sculpt_positions(&d), &base) < 1e-5);
    d.execute(Command::Undo).unwrap();
    d.ensure_session(ball).unwrap();
    assert!(far(&sculpt_positions(&d), &half) < 1e-5);
    // Sculpting at half strength records twice the offset, so full strength doubles it.
    shape(&mut d, ShapeOp::Add { name: None }).unwrap();
    assert_eq!(d.sculpt_layer(ball).unwrap().active_shape, Some(1));
    shape(&mut d, ShapeOp::Strength { index: 1, strength: 0.5 }).unwrap();
    let before = sculpt_positions(&d);
    d.execute(Command::SculptStroke { id: ball, stroke: stroke(BrushKind::Draw) }).unwrap();
    let after = sculpt_positions(&d);
    shape(&mut d, ShapeOp::Strength { index: 1, strength: 1.0 }).unwrap();
    let doubled: Vec<glam::Vec3> = before.iter().zip(&after).map(|(a, b)| *a + (*b - *a) * 2.0).collect();
    assert!(far(&sculpt_positions(&d), &doubled) < 1e-4);
    // Detail level up carries the shape layers; down is refused until they are merged.
    d.execute(Command::SculptDetail { id: ball, op: crate::DetailOp::Up }).unwrap();
    let up = sculpt_positions(&d);
    assert!(d.execute(Command::SculptDetail { id: ball, op: crate::DetailOp::Down }).is_err());
    assert!(far(&sculpt_positions(&d), &up) == 0.0, "a refused command changes nothing");
    // Save, reload, then remove both layers: only the base stroke (subdivided) is left.
    let dir = tmp("shapes");
    let path = dir.join("s.modelcraft");
    d.save_as(&path).unwrap();
    let mut r = Document::open(&path).unwrap();
    r.ensure_session(ball).unwrap();
    assert!(far(&sculpt_positions(&r), &up) < 1e-6);
    assert_eq!(r.sculpt_layer(ball).unwrap().shapes.len(), 2);
    shape(&mut r, ShapeOp::Merge { index: 1 }).unwrap();
    assert!(far(&sculpt_positions(&r), &up) < 1e-6, "merging keeps the shape");
    shape(&mut r, ShapeOp::Remove { index: 0 }).unwrap();
    assert!(far(&sculpt_positions(&r), &up) > 1e-3);
    assert!(r.sculpt_layer(ball).unwrap().shapes.is_empty());
    // Bad indices and values are errors, not panics.
    assert!(shape(&mut r, ShapeOp::Remove { index: 9 }).is_err());
    assert!(shape(&mut r, ShapeOp::Select { index: Some(3) }).is_err());
    shape(&mut r, ShapeOp::Add { name: None }).unwrap();
    assert!(shape(&mut r, ShapeOp::Strength { index: 0, strength: f32::NAN }).is_err());
    shape(&mut r, ShapeOp::Strength { index: 0, strength: 99.0 }).unwrap();
    assert_eq!(r.sculpt_layer(ball).unwrap().shapes[0].strength, 1.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn live_detail_strokes_are_undoable_saved_and_replayed_by_the_journal() {
    let dir = tmp("live");
    let p = dir.join("live.modelcraft");
    let mut d = Document::new();
    let ball = d.execute(Command::AddPrimitive { kind: "round_cube".into(), name: None, parent: None }).unwrap().created[0];
    d.save_as(&p).unwrap();
    d.execute(Command::BeginSculpt { id: ball }).unwrap();
    let (v0, t0) = (d.sculpt.as_ref().unwrap().mesh.vertex_count(), d.sculpt.as_ref().unwrap().mesh.triangle_count());
    let mut st = stroke(BrushKind::Draw);
    st.brush.live_detail = Some(0.02);
    d.execute(Command::SculptStroke { id: ball, stroke: st.clone() }).unwrap();
    let (v1, t1) = (d.sculpt.as_ref().unwrap().mesh.vertex_count(), d.sculpt.as_ref().unwrap().mesh.triangle_count());
    assert!(v1 > v0 && t1 > t0);
    // The object now evaluates to the refined triangles.
    d.sync_sculpt();
    let m = d.eval.evaluate(&d.scene, ball);
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert_eq!((m.mesh.vertex_count(), m.mesh.triangle_count()), (v1, t1));
    assert!(m.mesh.validate().is_ok());
    // A crash now: the journal replays the stroke to the same mesh.
    let after = sculpt_positions(&d);
    let mut r = Document::open(&p).unwrap();
    assert!(r.recover().unwrap() > 0);
    r.ensure_session(ball).unwrap();
    assert_eq!(sculpt_positions(&r), after);
    // Undo gives the quads back; redo, save and reload keep the triangles.
    d.execute(Command::Undo).unwrap();
    d.sync_sculpt();
    assert!(d.sculpt_layer(ball).unwrap().topology.is_none());
    assert_eq!(d.eval.evaluate(&d.scene, ball).mesh.vertex_count(), v0);
    d.execute(Command::Redo).unwrap();
    d.save_as(&p).unwrap();
    let mut r = Document::open(&p).unwrap();
    assert_eq!(r.eval.evaluate(&r.scene, ball).mesh.triangle_count(), t1);
    r.ensure_session(ball).unwrap();
    assert_eq!(sculpt_positions(&r), after);
    // Detail levels work on top of the live-detail triangles; live detail is then refused.
    r.execute(Command::SculptDetail { id: ball, op: crate::DetailOp::Up }).unwrap();
    assert!(r.sculpt.as_ref().unwrap().mesh.triangle_count() > t1 * 3);
    assert!(matches!(r.execute(Command::SculptStroke { id: ball, stroke: st.clone() }), Err(EngineError::Invalid(_))));
    assert!(r.live_detail_blocked(ball).is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn undo_history_survives_save_and_open() {
    let dir = tmp("history");
    let p = dir.join("h.modelcraft");
    let mut d = Document::new();
    let cube = add(&mut d, "cube");
    d.execute(Command::AddStep { id: cube, op: StepOp::Subdivide { levels: 1 } }).unwrap();
    d.execute(Command::BeginSculpt { id: cube }).unwrap();
    d.stroke_begin(modelcraft_sculpt::BrushSettings::new(BrushKind::Clay, 0.5));
    for dab in stroke(BrushKind::Clay).dabs {
        d.stroke_dab(dab);
    }
    d.stroke_end();
    d.execute(Command::SculptDetail { id: cube, op: crate::DetailOp::Up }).unwrap();
    d.execute(Command::EndSculpt).unwrap();
    let sphere = add(&mut d, "sphere");
    d.execute(Command::Undo).unwrap(); // the sphere waits on the redo side
    let labels = d.undo.labels();
    let sculpted = d.eval.evaluate(&d.scene, cube).mesh;
    d.save_as(&p).unwrap();
    drop(d);

    let mut r = Document::open(&p).unwrap();
    assert_eq!(r.undo.labels(), labels);
    assert_eq!(r.undo.redo_labels(), vec!["Add sphere".to_string()]);
    assert!(r.history_note.as_deref().unwrap_or("").contains("Restored 5"), "{:?}", r.history_note);
    assert!(!r.dirty);
    assert!(*r.eval.evaluate(&r.scene, cube).mesh == *sculpted, "the sculpted mesh opens as saved");
    // Undo reaches back past the open: the detail level, the stroke, the sculpt layer, the
    // step, the cube.
    r.execute(Command::Undo).unwrap();
    r.execute(Command::Undo).unwrap();
    r.sync_sculpt();
    assert!(*r.eval.evaluate(&r.scene, cube).mesh != *sculpted, "the stroke was undone");
    for _ in 0..3 {
        r.execute(Command::Undo).unwrap();
    }
    assert!(r.scene.is_empty());
    for _ in 0..6 {
        r.execute(Command::Redo).unwrap();
    }
    assert!(r.scene.get(sphere).is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn even_remesh_is_a_history_step() {
    let mut d = Document::new();
    let ball = d.execute(Command::AddPrimitive { kind: "torus".into(), name: None, parent: None }).unwrap().created[0];
    d.execute(Command::AddStep { id: ball, op: StepOp::EvenRemesh { cell: 0.08 } }).unwrap();
    let m = d.eval.evaluate(&d.scene, ball);
    assert!(m.errors.is_empty(), "{:?}", m.errors);
    assert!((0..m.mesh.face_count() as u32).all(|f| m.mesh.face_len(f) == 4));
    // A plane is open: the step reports why and the object keeps its input.
    let plane = d.execute(Command::AddPrimitive { kind: "plane".into(), name: None, parent: None }).unwrap().created[0];
    d.execute(Command::AddStep { id: plane, op: StepOp::EvenRemesh { cell: 0.1 } }).unwrap();
    let m = d.eval.evaluate(&d.scene, plane);
    assert!(m.errors.iter().any(|e| e.message.contains("closed mesh")), "{:?}", m.errors);
    // Sculpting continues on the remeshed surface.
    d.execute(Command::BeginSculpt { id: ball }).unwrap();
    d.execute(Command::SculptStroke { id: ball, stroke: stroke(BrushKind::Draw) }).unwrap();
}

#[test]
fn strokes_and_presets_keep_their_json_form() {
    use modelcraft_sculpt::preset;
    // A stroke journaled before hardness, stroke modes, image stamps and dab scale existed.
    let old = r#"{"cmd":"sculpt_stroke","id":1,"stroke":{"brush":{"kind":"draw","radius":0.3,"strength":0.4},"dabs":[{"center":[0,1,0],"view_dir":[0,-1,0],"pressure":1.0}]}}"#;
    let Command::SculptStroke { stroke: s, .. } = serde_json::from_str::<Command>(old).unwrap() else { panic!("not a stroke") };
    assert_eq!((s.brush.hardness, s.brush.spacing, s.dabs[0].scale), (0.0, 0.2, 1.0));
    assert_eq!(s.brush.stroke, modelcraft_sculpt::StrokeMode::Spaced);
    // A default brush serialises without the new fields, so old readers still parse it.
    let j = serde_json::to_string(&stroke(BrushKind::Draw)).unwrap();
    assert!(!j.contains("hardness") && !j.contains("scale") && !j.contains("\"stroke\"") && !j.contains("live_detail"), "{j}");
    for p in preset::builtin() {
        let j = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<preset::BrushPreset>(&j).unwrap(), p);
        // Every preset sculpts through the command path.
        let mut d = Document::new();
        let ball = d.execute(Command::AddPrimitive { kind: "round_cube".into(), name: None, parent: None }).unwrap().created[0];
        d.execute(Command::BeginSculpt { id: ball }).unwrap();
        let mut st = stroke(p.brush.kind);
        st.brush = p.apply(&st.brush);
        for dab in &mut st.dabs {
            dab.grab_delta = Vec3::new(0.02, 0.0, 0.0);
        }
        d.execute(Command::SculptStroke { id: ball, stroke: st }).unwrap();
        assert!(d.undo.len() >= 3, "{} changed nothing", p.name);
    }
}

#[test]
fn uv_layout_exports_as_svg_and_png() {
    let dir = tmp("uvlayout");
    let mut d = Document::new();
    assert!(d.export_uv_layout(&dir.join("a.svg"), None, 256).is_err(), "nothing to draw yet");
    let cube = add(&mut d, "cube");
    let svg = dir.join("cube.svg");
    d.export_uv_layout(&svg, None, 256).unwrap();
    let text = std::fs::read_to_string(&svg).unwrap();
    assert_eq!(text.lines().filter(|l| l.starts_with("<path")).count(), 6);
    let png = dir.join("cube.png");
    d.export_uv_layout(&png, Some(cube), 128).unwrap();
    assert_eq!(&std::fs::read(&png).unwrap()[..4], b"\x89PNG");
    // Wrong extension, missing object.
    assert!(d.export_uv_layout(&dir.join("cube.jpg"), None, 128).is_err());
    assert!(d.export_uv_layout(&svg, Some(ObjectId(99)), 128).is_err());
    // A mesh without UVs (a plain triangle imported from OBJ) has no layout.
    let obj = dir.join("tri.obj");
    std::fs::write(&obj, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
    let mut e = Document::new();
    e.execute(Command::Import { path: obj.display().to_string() }).unwrap();
    assert!(e.export_uv_layout(&dir.join("t.svg"), None, 64).is_err());
}

#[test]
fn a_history_that_no_longer_replays_is_dropped_not_fatal() {
    let dir = tmp("history-bad");
    let mesh_file = dir.join("part.obj");
    let mut src = Document::new();
    add(&mut src, "cube");
    src.export(&mesh_file).unwrap();
    let p = dir.join("h.modelcraft");
    let mut d = Document::new();
    d.execute(Command::Import { path: mesh_file.display().to_string() }).unwrap();
    d.save_as(&p).unwrap();
    // The imported file moved away: replaying the history cannot rebuild the scene.
    std::fs::remove_file(&mesh_file).unwrap();
    let r = Document::open(&p).unwrap();
    assert_eq!(r.scene.len(), 1, "the saved scene still opens");
    assert!(r.undo.is_empty());
    assert!(r.history_note.as_deref().unwrap_or("").contains("left out"), "{:?}", r.history_note);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn history_storage_is_optional_and_bounded() {
    let dir = tmp("history-opt");
    let p = dir.join("h.modelcraft");
    let mut d = Document::new();
    for _ in 0..3 {
        add(&mut d, "cube");
    }
    d.keep_history = false;
    d.save_as(&p).unwrap();
    assert!(Document::open(&p).unwrap().undo.is_empty());
    // A tight bound keeps only the newest steps; the base moves forward with the cut.
    d.keep_history = true;
    d.history_max_bytes = 130;
    d.save_as(&p).unwrap();
    let r = Document::open(&p).unwrap();
    assert_eq!(r.undo.len(), 2);
    assert_eq!(r.scene.len(), 3);
    // After the budget drops old entries, the base follows the oldest kept one.
    let mut e = Document::new();
    e.undo.budget = 2_000;
    for _ in 0..10 {
        add(&mut e, "cube");
    }
    assert!(e.undo.dropped > 0);
    e.save_as(&p).unwrap();
    let r = Document::open(&p).unwrap();
    assert_eq!(r.undo.len(), e.undo.len());
    assert_eq!(r.scene.len(), 10);
    let _ = std::fs::remove_dir_all(dir);
}
