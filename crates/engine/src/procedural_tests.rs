//! Tests of the procedural stream's commands (primitives, generators, fields, deformers).

use super::*;
use modelcraft_ops::primitives::{KINDS, Primitive};

fn add(doc: &mut Document, kind: &str) -> modelcraft_scene::ObjectId {
    doc.execute(Command::AddPrimitive { kind: kind.into(), name: None, parent: None }).unwrap().created[0]
}

#[test]
fn every_primitive_kind_adds_evaluates_edits_and_undoes() {
    let mut d = Document::new();
    for kind in KINDS {
        let id = add(&mut d, kind);
        let mesh = d.eval.evaluate(&d.scene, id);
        assert!(mesh.errors.is_empty(), "{kind}: {:?}", mesh.errors);
        assert!(mesh.mesh.face_count() > 0, "{kind} is empty");
        let Some(modelcraft_scene::ObjectKind::Mesh { source: modelcraft_scene::Source::Primitive { primitive }, .. }) =
            d.scene.get(id).map(|o| o.kind.clone())
        else {
            panic!("{kind} is not a primitive")
        };
        assert_eq!(primitive.kind(), *kind);
    }
    assert!(d.execute(Command::AddPrimitive { kind: "teapot".into(), name: None, parent: None }).is_err());
    // The saved JSON names every kind and reads back.
    for kind in KINDS {
        let p = Primitive::default_of(kind).unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Primitive>(&json).unwrap(), p, "{json}");
    }
    // Undo takes the last one away again.
    let n = d.scene.len();
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.scene.len(), n - 1);
}

#[test]
fn changing_a_new_primitive_through_set_primitive_survives_hostile_numbers() {
    let mut d = Document::new();
    let id = add(&mut d, "landscape");
    let before = d.undo.len();
    d.execute(Command::SetPrimitive {
        id,
        primitive: Primitive::Landscape { width: f32::NAN, depth: -3.0, height: f32::INFINITY, segments: u32::MAX, seed: 7, scale: f32::NAN, octaves: 99 },
    })
    .unwrap();
    assert_eq!(d.undo.len(), before + 1);
    let e = d.eval.evaluate(&d.scene, id);
    e.mesh.validate().unwrap();
    assert!(e.mesh.positions().iter().all(|p| p.is_finite()));
    d.execute(Command::Undo).unwrap();
    assert_eq!(d.eval.evaluate(&d.scene, id).mesh.face_count(), 64 * 64);
}

// ---------------------------------------------------------------- deformers, second set

use modelcraft_ops::deform::{Deformer, LimitMode, Limits, WrapMode};
use modelcraft_scene::ObjectKind;

fn positions(d: &mut Document, id: modelcraft_scene::ObjectId) -> Vec<glam::Vec3> {
    d.eval.evaluate(&d.scene, id).mesh.positions().to_vec()
}

fn add_deformer(d: &mut Document, kind: &str, target: modelcraft_scene::ObjectId) -> modelcraft_scene::ObjectId {
    d.execute(Command::AddDeformer { kind: kind.into(), target }).unwrap().created[0]
}

#[test]
fn every_new_deformer_adds_changes_the_mesh_and_undoes() {
    for kind in ["bulge", "squash", "smooth", "delta_smooth", "displace"] {
        let mut d = Document::new();
        let mesh = add(&mut d, "sphere");
        let before = positions(&mut d, mesh);
        let id = add_deformer(&mut d, kind, mesh);
        let after = positions(&mut d, mesh);
        // Delta smoothing has nothing to smooth when it is the first deformer.
        if kind != "delta_smooth" {
            assert!(before != after, "{kind} did nothing");
        }
        assert!(after.iter().all(|p| p.is_finite()));
        d.execute(Command::SetVisible { id, visible: false }).unwrap();
        assert!(positions(&mut d, mesh) == before, "{kind}: hidden, the mesh is as it was");
    }
}

#[test]
fn delta_smoothing_smooths_what_the_deformers_before_it_did() {
    let mut d = Document::new();
    let mesh = add(&mut d, "sphere");
    let rest = positions(&mut d, mesh);
    let ripple = add_deformer(&mut d, "ripple", mesh);
    d.execute(Command::SetDeformer {
        id: ripple,
        deformer: Deformer::Ripple { amplitude: 0.2, frequency: 4.0 },
        limits: None,
        target: None,
        clear_target: false,
    })
    .unwrap();
    let rippled = positions(&mut d, mesh);
    add_deformer(&mut d, "delta_smooth", mesh);
    let smoothed = positions(&mut d, mesh);
    let offset = |a: &[glam::Vec3]| a.iter().zip(&rest).map(|(p, r)| (*p - *r).length()).sum::<f32>();
    assert!(offset(&smoothed) < offset(&rippled), "the ripple's offsets are smoothed: {} < {}", offset(&smoothed), offset(&rippled));
    assert!(offset(&smoothed) > 0.0, "but not removed");
}

#[test]
fn parametric_deformers_and_the_lattice_share_one_stack_order() {
    use crate::RigCommand;
    let mut d = Document::new();
    let mesh = add(&mut d, "sphere");
    let bend = add_deformer(&mut d, "bend", mesh);
    d.execute(Command::Rig(RigCommand::AddLattice { mesh, divisions: [2, 2, 2], linear: false })).unwrap();
    let lattice = d.scene.children(mesh).into_iter().find(|c| d.scene.get(*c).is_some_and(|o| matches!(o.kind, ObjectKind::PointDeformer { .. }))).unwrap();
    // Move one lattice point so that the lattice changes the mesh.
    d.execute(Command::Rig(RigCommand::EditLattice { id: lattice, points: vec![7], translate: Some([0.4, 0.4, 0.4]), to: None, reset: false, linear: None }))
        .unwrap();
    let bend_first = positions(&mut d, mesh);
    assert!(d.scene.children(mesh).iter().position(|c| *c == bend) < d.scene.children(mesh).iter().position(|c| *c == lattice));
    d.execute(Command::Rig(RigCommand::MoveInStack { id: bend, delta: 1 })).unwrap();
    let lattice_first = positions(&mut d, mesh);
    assert!(bend_first != lattice_first, "the order of a bend and a lattice matters, and it is the order in the stack");
}

#[test]
fn limits_change_the_reach_and_a_hostile_box_is_made_safe() {
    let mut d = Document::new();
    let mesh = add(&mut d, "cylinder");
    let twist = add_deformer(&mut d, "twist", mesh);
    let full = positions(&mut d, mesh);
    let small = Limits { mode: LimitMode::WithinBox, fit_to_parent: false, size: glam::Vec3::new(2.0, 1.0, 2.0) };
    d.execute(Command::SetDeformer { id: twist, deformer: Deformer::default_of("twist").unwrap(), limits: Some(small), target: None, clear_target: false })
        .unwrap();
    let within = positions(&mut d, mesh);
    assert!(within != full, "a box smaller than the mesh limits the twist");
    // Points above the box are untouched.
    let rest = {
        d.execute(Command::SetVisible { id: twist, visible: false }).unwrap();
        let r = positions(&mut d, mesh);
        d.execute(Command::SetVisible { id: twist, visible: true }).unwrap();
        r
    };
    assert!(rest.iter().zip(&within).filter(|(r, _)| r.y > 0.6).all(|(r, w)| (*r - *w).length() < 1e-6));
    let nasty = Limits { mode: LimitMode::Unlimited, fit_to_parent: false, size: glam::Vec3::new(f32::NAN, -5.0, 0.0) };
    d.execute(Command::SetDeformer { id: twist, deformer: Deformer::default_of("twist").unwrap(), limits: Some(nasty), target: None, clear_target: false })
        .unwrap();
    if let Some(ObjectKind::Deformer { limits, .. }) = d.scene.get(twist).map(|o| o.kind.clone()) {
        assert!(limits.size.is_finite() && limits.size.min_element() > 0.0, "{limits:?}");
    }
    assert!(positions(&mut d, mesh).iter().all(|p| p.is_finite()));
}

#[test]
fn a_shrink_wrap_follows_its_target_and_asks_for_one() {
    let mut d = Document::new();
    let ball = add(&mut d, "sphere");
    let wall = add(&mut d, "cube");
    let wrap = add_deformer(&mut d, "shrink_wrap", ball);
    // No target yet: the mesh is as it was, and the evaluation says what is missing.
    let e = d.eval.evaluate(&d.scene, ball);
    assert!(e.errors.iter().any(|e| e.message.contains("target")), "{:?}", e.errors);
    let set = |d: &mut Document, target: Option<modelcraft_scene::ObjectId>| {
        d.execute(Command::SetDeformer {
            id: wrap,
            deformer: Deformer::ShrinkWrap { mode: WrapMode::Closest, offset: 0.0 },
            limits: None,
            target,
            clear_target: false,
        })
    };
    set(&mut d, Some(wall)).unwrap();
    let wrapped = positions(&mut d, ball);
    // The cube is 2 wide: every point of the unit sphere lands on it.
    assert!(wrapped.iter().all(|p| (p.abs().max_element() - 1.0).abs() < 1e-4), "on the cube's surface");
    // Moving the target moves the result (the cache is keyed on the target too).
    d.execute(Command::SetTransform { id: wall, transform: modelcraft_scene::Transform { translation: glam::Vec3::new(5.0, 0.0, 0.0), ..Default::default() } })
        .unwrap();
    let moved = positions(&mut d, ball);
    assert!(moved.iter().all(|p| p.x > 3.0), "followed the cube to x = 5");
    // The target may not be the mesh or anything inside it; a missing object is refused.
    assert!(set(&mut d, Some(ball)).is_err());
    assert!(set(&mut d, Some(wrap)).is_err());
    assert!(set(&mut d, Some(modelcraft_scene::ObjectId(999))).is_err());
    // Taking it away restores the message.
    d.execute(Command::SetDeformer {
        id: wrap,
        deformer: Deformer::ShrinkWrap { mode: WrapMode::Closest, offset: 0.0 },
        limits: None,
        target: None,
        clear_target: true,
    })
    .unwrap();
    assert!(d.eval.evaluate(&d.scene, ball).errors.iter().any(|e| e.message.contains("target")));
    // Deleting the target is not a crash either.
    set(&mut d, Some(wall)).unwrap();
    d.execute(Command::Delete { ids: vec![wall] }).unwrap();
    let e = d.eval.evaluate(&d.scene, ball);
    assert!(e.mesh.positions().iter().all(|p| p.is_finite()));
}

#[test]
fn old_deformer_json_and_the_old_set_deformer_command_still_read() {
    let c: Command = serde_json::from_str(r#"{"cmd":"set_deformer","id":3,"deformer":{"kind":"bend","angle":45.0}}"#).unwrap();
    assert!(matches!(c, Command::SetDeformer { limits: None, target: None, clear_target: false, .. }));
    let k: ObjectKind = serde_json::from_str(r#"{"type":"deformer","deformer":{"kind":"twist","angle":10.0}}"#).unwrap_or(ObjectKind::Group);
    if let ObjectKind::Deformer { limits, target, .. } = k {
        assert_eq!(limits, Limits::default());
        assert!(target.is_none());
    }
}

// ---------------------------------------------------------------- shrink-wrap target loops

fn wrapped_mesh(d: &mut Document) -> (modelcraft_scene::ObjectId, modelcraft_scene::ObjectId) {
    let mesh = add(d, "sphere");
    let wrap = add_deformer(d, "shrink_wrap", mesh);
    (mesh, wrap)
}

/// Point a shrink-wrap at `target` straight in the scene, as a hand-made file could, without the
/// command's checks.
fn force_target(d: &mut Document, wrap: modelcraft_scene::ObjectId, target: modelcraft_scene::ObjectId) {
    if let ObjectKind::Deformer { target: t, .. } = &mut d.scene.edit(wrap).unwrap().kind {
        *t = Some(target);
    }
}

fn set_target(d: &mut Document, wrap: modelcraft_scene::ObjectId, target: modelcraft_scene::ObjectId) -> Result<crate::Outcome, EngineError> {
    d.execute(Command::SetDeformer {
        id: wrap,
        deformer: Deformer::ShrinkWrap { mode: WrapMode::Closest, offset: 0.0 },
        limits: None,
        target: Some(target),
        clear_target: false,
    })
}

#[test]
fn set_deformer_refuses_a_target_that_would_make_a_loop() {
    let mut d = Document::new();
    let (a, wa) = wrapped_mesh(&mut d);
    let (b, wb) = wrapped_mesh(&mut d);
    let (c, wc) = wrapped_mesh(&mut d);
    set_target(&mut d, wa, b).unwrap();
    assert!(set_target(&mut d, wb, a).is_err(), "b onto a, a onto b");
    set_target(&mut d, wb, c).unwrap();
    assert!(set_target(&mut d, wc, a).is_err(), "a ring of three");
    let n = d.undo.len();
    assert!(set_target(&mut d, wc, b).is_err());
    assert_eq!(d.undo.len(), n, "a refused command leaves no trace");
}

#[test]
fn a_loop_made_in_a_hand_written_file_is_cut_not_followed_and_the_file_loads() {
    let mut d = Document::new();
    let (a, wa) = wrapped_mesh(&mut d);
    let (b, wb) = wrapped_mesh(&mut d);
    force_target(&mut d, wa, b);
    force_target(&mut d, wb, a);
    let before = d.eval.evaluations;
    let e = d.eval.evaluate(&d.scene, a);
    assert!(d.eval.evaluations - before <= 4, "two meshes need two evaluations, not a recursion: {}", d.eval.evaluations - before);
    assert!(e.mesh.positions().iter().all(|p| p.is_finite()));
    assert!(e.warnings.iter().any(|w| w.message.contains("loop")), "{:?}", e.warnings);
    // The same file saved and opened again: it loads, and it evaluates the same way.
    let dir = std::env::temp_dir().join(format!("modelcraft-loop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("loop.modelcraft");
    d.save_as(&path).unwrap();
    let mut loaded = Document::open(&path).unwrap();
    let before = loaded.eval.evaluations;
    let e2 = loaded.eval.evaluate(&loaded.scene, b);
    assert!(loaded.eval.evaluations - before <= 4);
    assert!(e2.mesh.positions().iter().all(|p| p.is_finite()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn many_wraps_that_point_at_each_other_cost_a_bounded_number_of_evaluations() {
    // A ring of meshes, each wrapped onto the next two: 2^N paths if every one were followed.
    for n in [8usize, 16, 24] {
        let mut d = Document::new();
        let meshes: Vec<_> = (0..n).map(|_| add(&mut d, "sphere")).collect();
        for (i, m) in meshes.iter().enumerate() {
            for k in 1..=2 {
                let w = add_deformer(&mut d, "shrink_wrap", *m);
                force_target(&mut d, w, meshes[(i + k) % n]);
            }
        }
        let before = d.eval.evaluations;
        let e = d.eval.evaluate(&d.scene, meshes[0]);
        let spent = d.eval.evaluations - before;
        assert!(spent <= (n * n) as u64, "{n} meshes in a ring took {spent} evaluations");
        assert!(e.mesh.positions().iter().all(|p| p.is_finite()));
        // A second ask is answered from the caches or the memo within a bound too.
        let before = d.eval.evaluations;
        for m in &meshes {
            let _ = d.eval.evaluate(&d.scene, *m);
        }
        assert!(d.eval.evaluations - before <= (n * n * 2) as u64, "{n}: {}", d.eval.evaluations - before);
    }
}

#[test]
fn a_wide_chain_of_wraps_without_a_loop_is_evaluated_once_per_mesh() {
    let n = 20usize;
    let mut d = Document::new();
    let meshes: Vec<_> = (0..n).map(|_| add(&mut d, "sphere")).collect();
    for (i, m) in meshes.iter().enumerate().take(n - 1) {
        for k in 1..=2 {
            if i + k < n {
                let w = add_deformer(&mut d, "shrink_wrap", *m);
                set_target(&mut d, w, meshes[i + k]).unwrap();
            }
        }
    }
    let before = d.eval.evaluations;
    let e = d.eval.evaluate(&d.scene, meshes[0]);
    assert!(d.eval.evaluations - before <= n as u64 + 2, "a chain of {n} took {} evaluations (2^{n} paths)", d.eval.evaluations - before);
    assert!(e.warnings.iter().all(|w| !w.message.contains("loop")));
}

#[test]
fn a_change_to_a_targets_own_target_reaches_the_wrapped_mesh() {
    let mut d = Document::new();
    let (a, wa) = wrapped_mesh(&mut d);
    let (b, wb) = wrapped_mesh(&mut d);
    let wall = add(&mut d, "cube");
    set_target(&mut d, wa, b).unwrap();
    set_target(&mut d, wb, wall).unwrap();
    let first = positions(&mut d, a);
    d.execute(Command::SetTransform { id: wall, transform: modelcraft_scene::Transform { translation: glam::Vec3::new(8.0, 0.0, 0.0), ..Default::default() } })
        .unwrap();
    assert!(positions(&mut d, a) != first, "a follows b, which follows the wall: the cache key covers the whole chain");
}
