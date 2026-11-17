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
        Command::AddArray { kind: "radial".into(), input: Some(ObjectId(1)) },
        Command::AddSymmetry { input: None, axis: 1 },
        Command::SetArray { id: ObjectId(2), array: modelcraft_scene::Array::default_of("linear").unwrap() },
        Command::SetSymmetry { id: ObjectId(3), axis: 2, weld: 0.01 },
        Command::Undo,
    ];
    for c in cmds {
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&j).unwrap(), c);
    }
    let c: Command = serde_json::from_str(r#"{"cmd":"add_primitive","kind":"cube"}"#).unwrap();
    assert!(matches!(c, Command::AddPrimitive { .. }));
    assert_eq!(crate::COMMANDS.len(), 32);
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
fn array_and_symmetry_commands_chain_undo_and_sanitize() {
    use modelcraft_scene::Array;
    let mut d = Document::new();
    let out = d.execute(Command::AddArray { kind: "linear".into(), input: None }).unwrap();
    let arr = out.created[0];
    assert_eq!(d.scene.len(), 2, "a cube is made inside it");
    assert_eq!(d.eval.evaluate(&d.scene, arr).mesh.face_count(), 24);
    // A symmetry around the array: the generators nest.
    let sym = d.execute(Command::AddSymmetry { input: Some(arr), axis: 0 }).unwrap().created[0];
    assert_eq!(d.scene.get(arr).unwrap().parent, Some(sym));
    assert_eq!(d.triangle_count(), d.eval.evaluate(&d.scene, sym).mesh.triangle_count());
    // Settings: sanitized, one undo step each.
    let before = d.undo.len();
    d.execute(Command::SetArray {
        id: arr,
        array: Array::Linear { count: u32::MAX, offset: glam::Vec3::NAN, rotation: glam::Vec3::ZERO, scale: glam::Vec3::ZERO },
    })
    .unwrap();
    assert_eq!(d.undo.len(), before + 1);
    if let ObjectKind::Array { array: Array::Linear { count, offset, .. } } = d.scene.get(arr).unwrap().kind {
        assert_eq!((count, offset), (modelcraft_ops::array::MAX_COPIES, glam::Vec3::ZERO));
    } else {
        panic!("not a linear array");
    }
    d.execute(Command::SetSymmetry { id: sym, axis: 9, weld: f32::NAN }).unwrap();
    assert!(matches!(d.scene.get(sym).unwrap().kind, ObjectKind::Symmetry { axis: 2, .. }));
    d.execute(Command::Undo).unwrap();
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.eval.evaluate(&d.scene, arr).mesh.face_count(), 24);
    // Wrong kinds fail without a trace.
    let n = d.undo.len();
    let group = d.execute(Command::AddGroup { name: None }).unwrap().created[0];
    assert!(d.execute(Command::AddArray { kind: "linear".into(), input: Some(group) }).is_err());
    assert!(d.execute(Command::AddArray { kind: "spiral".into(), input: None }).is_err());
    assert!(d.execute(Command::SetArray { id: sym, array: Array::default_of("radial").unwrap() }).is_err());
    assert!(d.execute(Command::SetSymmetry { id: arr, axis: 0, weld: 0.0 }).is_err());
    assert_eq!(d.undo.len(), n + 1, "only the group was added");
}
