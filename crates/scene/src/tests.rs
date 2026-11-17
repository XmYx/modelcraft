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

fn cloner_scene(kind: &str) -> (Scene, ObjectId, ObjectId) {
    let mut s = Scene::new();
    let gen_id = s.add("Clones", ObjectKind::Cloner { cloner: Cloner::default_of(kind).unwrap(), target: None }, None).unwrap();
    let src = s.add("Source", cube_kind(), Some(gen_id)).unwrap();
    (s, gen_id, src)
}

#[test]
fn clone_generator_gives_instances_of_its_source_not_copies() {
    let (s, id, src) = cloner_scene("linear");
    let mut ev = Evaluator::new();
    let e = ev.evaluate(&s, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    assert_eq!(e.mesh.face_count(), 0, "the generator itself has no mesh");
    let inst = e.instances.expect("instances");
    assert_eq!(inst.clones.len(), 5);
    assert_eq!(inst.source_object, Some(src));
    // Every clone shares the one evaluated source mesh.
    assert!(Arc::ptr_eq(&inst.source, &ev.evaluate(&s, src).mesh));
    assert_eq!(inst.triangle_count(), 12 * 5);
    assert!(s.is_cage_input(src), "the source is shown through the generator");
    assert_eq!(s.generator_input(id), Some(src));
}

#[test]
fn clones_follow_the_source_and_settings_and_are_cached() {
    let (mut s, id, src) = cloner_scene("linear");
    let mut ev = Evaluator::new();
    let runs0 = {
        let _ = ev.evaluate(&s, id);
        ev.runs
    };
    let _ = ev.evaluate(&s, id);
    assert_eq!(ev.runs, runs0, "a second evaluation hits the cache");
    // The source's own transform is part of every clone.
    s.edit(src).unwrap().transform.translation = Vec3::new(0.0, 3.0, 0.0);
    let inst = ev.evaluate(&s, id).instances.unwrap();
    assert!((inst.clones.instances[2].matrix.w_axis.truncate() - Vec3::new(2.0, 3.0, 0.0)).length() < 1e-5);
    // Settings re-evaluate.
    s.edit(id).unwrap().kind = ObjectKind::Cloner { cloner: Cloner::default_of("grid").unwrap(), target: None };
    assert_eq!(ev.evaluate(&s, id).instances.unwrap().clones.len(), 16);
}

#[test]
fn clone_generator_without_a_source_or_target_reports_it() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let id = s.add("Clones", ObjectKind::Cloner { cloner: Cloner::default_of("surface").unwrap(), target: None }, None).unwrap();
    let e = ev.evaluate(&s, id);
    assert!(e.instances.is_none());
    assert!(e.errors.iter().any(|e| e.message.contains("source")));
    s.add("Source", cube_kind(), Some(id)).unwrap();
    let e = ev.evaluate(&s, id);
    assert!(e.instances.is_none());
    assert!(e.errors.iter().any(|e| e.message.contains("target")), "{:?}", e.errors);
}

#[test]
fn surface_clones_follow_the_target_object_and_where_it_sits() {
    let (mut s, id, _) = cloner_scene("surface");
    let target = s.add("Ground", cube_kind(), None).unwrap();
    s.edit(id).unwrap().kind = ObjectKind::Cloner { cloner: Cloner::default_of("surface").unwrap(), target: Some(target) };
    let mut ev = Evaluator::new();
    let first = ev.evaluate(&s, id).instances.unwrap();
    assert_eq!(first.clones.len(), 60);
    // The 2-unit cube: every clone is on its surface (|coordinate| = 1).
    for i in &first.clones.instances {
        assert!((i.matrix.w_axis.truncate().abs().max_element() - 1.0).abs() < 1e-4);
    }
    // Moving the target moves the clones with it, through the cache.
    s.edit(target).unwrap().transform.translation = Vec3::new(10.0, 0.0, 0.0);
    let moved = ev.evaluate(&s, id).instances.unwrap();
    assert!(moved.clones.instances.iter().all(|i| i.matrix.w_axis.x > 8.9));
    // Pointing at itself or at a deleted object is an error report, never a loop.
    s.edit(id).unwrap().kind = ObjectKind::Cloner { cloner: Cloner::default_of("surface").unwrap(), target: Some(id) };
    assert!(ev.evaluate(&s, id).instances.is_none());
    s.remove(target);
    s.edit(id).unwrap().kind = ObjectKind::Cloner { cloner: Cloner::default_of("vertices").unwrap(), target: Some(target) };
    assert!(ev.evaluate(&s, id).instances.is_none());
}

#[test]
fn clone_generators_survive_a_save_and_load() {
    let (s, id, _) = cloner_scene("radial");
    let json = serde_json::to_string(&s).unwrap();
    let back: Scene = serde_json::from_str(&json).unwrap();
    let back = back.repaired().unwrap();
    assert_eq!(back.get(id).unwrap().kind, s.get(id).unwrap().kind);
    let mut ev = Evaluator::new();
    assert_eq!(ev.evaluate(&back, id).instances.unwrap().clones.len(), 8);
}

#[test]
fn only_the_source_is_hidden_other_children_of_a_clone_generator_draw() {
    let (mut s, id, src) = cloner_scene("linear");
    let extra = s.add("Extra", cube_kind(), Some(id)).unwrap();
    assert!(s.is_cage_input(src));
    assert!(!s.is_cage_input(extra), "a second mesh inside is an ordinary object");
    // A smooth cage still hides its mesh child.
    let cage = s.add("Cage", ObjectKind::SmoothCage { levels: 1 }, None).unwrap();
    let inner = s.add("Inner", cube_kind(), Some(cage)).unwrap();
    assert!(s.is_cage_input(inner));
}

#[test]
fn unwrap_step_writes_uvs_that_later_steps_keep() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let id = s.add("Cube", cube_kind(), None).unwrap();
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps.push(Step { id: 1, enabled: true, op: StepOp::Unwrap { seams: vec![], angle: Some(30.0), pins: vec![], auto_cut: true, margin: 0.01 } });
        steps.push(Step { id: 2, enabled: true, op: StepOp::Subdivide { levels: 1 } });
    }
    let e = ev.evaluate(&s, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    assert!(e.mesh.has_uvs());
    assert!(e.mesh.corner_uvs().iter().all(|u| u.is_finite() && u.min_element() >= -1e-6 && u.max_element() <= 1.0 + 1e-6));
    // The step round-trips through JSON with its defaults filled in.
    let json = serde_json::to_string(&s.get(id).unwrap().steps()[0]).unwrap();
    assert!(json.contains("\"op\":\"unwrap\""));
    let back: Step = serde_json::from_str(r#"{"id":1,"op":"unwrap"}"#).unwrap();
    assert!(matches!(back.op, StepOp::Unwrap { auto_cut: true, ref seams, .. } if seams.is_empty()));
}

#[test]
fn unwrap_step_with_stale_input_is_skipped() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let id = s.add("Cube", cube_kind(), None).unwrap();
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        // Seams and a pin that name things the cube does not have are ignored, not fatal.
        steps.push(Step {
            id: 1,
            enabled: true,
            op: StepOp::Unwrap {
                seams: vec![[900, 901], [0, 0]],
                angle: Some(f32::NAN),
                pins: vec![UvPin { corner: 9999, uv: [0.0, 0.0] }],
                auto_cut: true,
                margin: 5.0,
            },
        });
    }
    let e = ev.evaluate(&s, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    assert!(e.mesh.has_uvs());
}
