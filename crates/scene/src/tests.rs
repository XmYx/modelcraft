use super::*;
use glam::Vec3;
use modelcraft_ops::primitives::Primitive;

fn cube_kind() -> ObjectKind {
    ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::Cube { size: Vec3::splat(2.0), segments: 1 } }, steps: vec![] }
}

#[test]
fn history_is_cached_and_reevaluated_from_the_change() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let id = s.add("Cube", cube_kind(), None).unwrap();
    let sid = s.fresh_id();
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps.push(Step { id: sid, enabled: true, op: StepOp::Extrude { faces: vec![0], distance: 1.0 } });
        steps.push(Step { id: sid + 1, enabled: true, op: StepOp::Subdivide { levels: 1 } });
    }
    let a = ev.evaluate(&s, id);
    assert!(a.errors.is_empty());
    assert_eq!(a.mesh.face_count(), 40);
    let runs = ev.runs;
    let _ = ev.evaluate(&s, id);
    assert_eq!(ev.runs, runs, "a second evaluation must hit the cache");
    // Editing the last step re-runs only that step.
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps[1].op = StepOp::Subdivide { levels: 2 };
    }
    assert_eq!(ev.evaluate(&s, id).mesh.face_count(), 160);
    assert_eq!(ev.runs, runs + 1, "only the changed step runs again");
    // Switch the extrude off: the subdivision of a plain cube.
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps[0].enabled = false;
    }
    assert_eq!(ev.evaluate(&s, id).mesh.face_count(), 96);
    // Collapse.
    s.collapse(id, &mut ev).unwrap();
    assert!(s.get(id).unwrap().steps().is_empty());
    assert_eq!(ev.evaluate(&s, id).mesh.face_count(), 96);
}

#[test]
fn stale_steps_are_reported_not_fatal() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let id = s.add("Cube", cube_kind(), None).unwrap();
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps.push(Step { id: 1, enabled: true, op: StepOp::Bevel { edges: vec![500], width: 0.1 } });
        steps.push(Step { id: 2, enabled: true, op: StepOp::Sculpt { layer: SculptLayer::default() } });
    }
    let e = ev.evaluate(&s, id);
    assert_eq!(e.errors.len(), 2);
    assert_eq!(e.mesh.face_count(), 6);
}

#[test]
fn smooth_cage_and_deformers_follow_their_children() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let cage = s.add("Smooth cage", ObjectKind::SmoothCage { levels: 2 }, None).unwrap();
    let cube = s.add("Cube", cube_kind(), Some(cage)).unwrap();
    assert!(s.is_cage_input(cube));
    assert_eq!(ev.evaluate(&s, cage).mesh.face_count(), 96);
    s.edit(cage).unwrap().kind = ObjectKind::SmoothCage { levels: 1 };
    assert_eq!(ev.evaluate(&s, cage).mesh.face_count(), 24);
    // A twist under the cube changes the cage's output too.
    let before = ev.evaluate(&s, cage).mesh.positions().to_vec();
    s.add("Twist", ObjectKind::Deformer { deformer: Deformer::Twist { angle: 45.0 } }, Some(cube)).unwrap();
    let after = ev.evaluate(&s, cage).mesh.positions().to_vec();
    assert_ne!(before, after);
}

#[test]
fn tree_edits_refuse_cycles_and_round_trip() {
    let mut s = Scene::new();
    let a = s.add("A", ObjectKind::Group, None).unwrap();
    let b = s.add("B", ObjectKind::Group, Some(a)).unwrap();
    let c = s.add("C", cube_kind(), Some(b)).unwrap();
    assert!(s.reparent(a, Some(c)).is_err());
    assert_eq!(s.depth(c), Some(2));
    assert_eq!(s.tree_order().iter().map(|(_, d)| *d).collect::<Vec<_>>(), vec![0, 1, 2]);
    s.edit(a).unwrap().transform.translation = Vec3::X;
    assert_eq!(s.world_matrix(c).transform_point3(Vec3::ZERO), Vec3::X);
    let json = serde_json::to_string(&s).unwrap();
    let back: Scene = serde_json::from_str::<Scene>(&json).unwrap().repaired().unwrap();
    assert_eq!(back.len(), 3);
    let removed = s.remove(b);
    assert_eq!(removed.len(), 2);
    assert_eq!(s.len(), 1);
    assert_eq!(s.unique_name("A"), "A.1");
}

#[test]
fn hostile_scene_is_repaired() {
    let json = r#"{"objects":[
        {"id":1,"name":"x","parent":2,"kind":{"type":"group"}},
        {"id":2,"name":"y","parent":1,"kind":{"type":"group"}},
        {"id":2,"name":"dup","kind":{"type":"group"}},
        {"id":3,"name":"z","parent":99,"transform":{"translation":[0,0,0],"rotation":[0,0,0,0],"scale":[0,1,1]},"kind":{"type":"group"}}
    ]}"#;
    let s: Scene = serde_json::from_str::<Scene>(json).unwrap().repaired().unwrap();
    assert_eq!(s.len(), 3);
    assert!(s.objects().iter().all(|o| s.depth(o.id).is_some()));
    let z = s.get(ObjectId(3)).unwrap();
    assert!(z.parent.is_none());
    assert!(z.transform.rotation.is_normalized());
    assert!(s.fresh_id_peek() > 3);
}

#[test]
fn picking_hits_the_front_face() {
    let m = Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap();
    let r = pick::Ray { origin: Vec3::new(0.0, 0.0, 5.0), dir: -Vec3::Z };
    let h = pick::pick_face(&m, &r).unwrap();
    assert!((h.t - 4.0).abs() < 1e-5);
    assert!(m.face_normal(h.face).z > 0.9);
    assert!(pick::pick_vertex(&m, &r).is_some());
    assert!(pick::pick_edge(&m, &r).is_some());
}

#[test]
fn array_and_symmetry_generators_join_and_mirror_their_input() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let sym = s.add("Symmetry", ObjectKind::Symmetry { axis: 0, weld: 1e-4 }, None).unwrap();
    assert!(ev.evaluate(&s, sym).errors.iter().any(|e| e.message.contains("needs a mesh")));
    let cube = s.add("Cube", cube_kind(), Some(sym)).unwrap();
    s.edit(cube).unwrap().transform.translation = Vec3::new(3.0, 0.0, 0.0);
    // The input's own transform counts: two separate cubes, one each side of the plane.
    let e = ev.evaluate(&s, sym);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    assert_eq!(e.mesh.face_count(), 12);
    let (lo, hi) = e.mesh.bounds();
    assert!((lo.x + 4.0).abs() < 1e-4 && (hi.x - 4.0).abs() < 1e-4, "{lo:?} {hi:?}");
    assert!(s.is_cage_input(cube));
    // An array around the symmetry: the generators chain, each hidden behind the next.
    let arr = s.add("Array", ObjectKind::Array { array: Array::default_of("linear").unwrap() }, None).unwrap();
    s.reparent(sym, Some(arr)).unwrap();
    assert!(s.is_cage_input(sym));
    assert_eq!(ev.evaluate(&s, arr).mesh.face_count(), 48);
    // Editing the cube re-evaluates through both generators; the cache serves an unchanged scene.
    let runs = ev.runs;
    let _ = ev.evaluate(&s, arr);
    assert_eq!(ev.runs, runs);
    s.edit(cube).unwrap().transform.translation = Vec3::new(0.0, 0.0, 0.0);
    let after = ev.evaluate(&s, arr);
    assert!(after.mesh.face_count() > 0 && after.errors.iter().all(|e| !e.message.is_empty()));
    // A hostile count is refused or capped, never an allocation: the array stays valid.
    s.edit(arr).unwrap().kind = ObjectKind::Array { array: Array::Linear { count: u32::MAX, offset: Vec3::X, rotation: Vec3::ZERO, scale: Vec3::ZERO } };
    let big = ev.evaluate(&s, arr);
    assert!(big.mesh.face_count() <= modelcraft_ops::MAX_RESULT_FACES);
    big.mesh.validate().unwrap();
    // A second child that makes geometry is an ordinary object, not hidden.
    let extra = s.add("Extra", cube_kind(), Some(arr)).unwrap();
    assert!(!s.is_cage_input(extra));
    // Save and load.
    let back: Scene = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back.get(arr).unwrap().kind, s.get(arr).unwrap().kind);
    assert_eq!(back.get(sym).unwrap().kind, s.get(sym).unwrap().kind);
}
