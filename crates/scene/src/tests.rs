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

#[test]
fn a_mirrored_generator_input_keeps_its_faces_outward() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let arr = s.add("Array", ObjectKind::Array { array: Array::default_of("linear").unwrap() }, None).unwrap();
    let cube = s.add("Cube", cube_kind(), Some(arr)).unwrap();
    s.edit(cube).unwrap().transform.scale = Vec3::new(-1.0, 1.0, 1.0);
    let m = ev.evaluate(&s, arr).mesh;
    assert_eq!(m.face_count(), 24);
    for f in 0..m.face_count() as u32 {
        // Each copy is a convex cube whose six faces come in a row: faces point away from its own centre.
        let centre = Vec3::new((f / 6) as f32 * 1.5, 0.0, 0.0);
        assert!(m.face_normal(f).dot(m.face_center(f) - centre) > 0.0, "face {f} points inwards");
    }
}

#[test]
fn falloff_fields_limit_where_a_deformer_acts() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let cube = s.add("Cube", cube_kind(), None).unwrap();
    let base = ev.evaluate(&s, cube).mesh.positions().to_vec();
    let twist = s.add("Twist", ObjectKind::Deformer { deformer: Deformer::Twist { angle: 90.0 } }, Some(cube)).unwrap();
    let full = ev.evaluate(&s, cube).mesh.positions().to_vec();
    assert_ne!(base, full);
    // A sphere field at the top of the cube (y = +1): the bottom stays, the top moves.
    let mut sphere = Field::default_of("sphere").unwrap();
    sphere.shape = modelcraft_field::Shape::Sphere { radius: 1.8, inner: 0.0 };
    let field = s.add("Sphere field", ObjectKind::Field { field: sphere }, Some(twist)).unwrap();
    s.edit(field).unwrap().transform.translation = Vec3::new(0.0, 1.0, 0.0);
    assert_eq!(s.field_stack(twist).len(), 1);
    let limited = ev.evaluate(&s, cube).mesh.positions().to_vec();
    for (i, p) in base.iter().enumerate() {
        if p.y < -0.5 {
            assert_eq!(limited[i], *p, "outside the field, unchanged");
        }
    }
    assert!(base.iter().zip(&limited).any(|(a, b)| a != b), "inside the field, still deformed");
    assert_ne!(limited, full);
    // Moving the field re-evaluates through the cache; hiding it removes the falloff.
    s.edit(field).unwrap().transform.translation = Vec3::new(0.0, -1.0, 0.0);
    let moved = ev.evaluate(&s, cube).mesh.positions().to_vec();
    assert_ne!(moved, limited);
    s.edit(field).unwrap().visible = false;
    assert_eq!(ev.evaluate(&s, cube).mesh.positions(), &full[..]);
    // A field outside a deformer is just data; a field alone evaluates to nothing.
    assert_eq!(ev.evaluate(&s, field).mesh.face_count(), 0);
    // Save and load keep it.
    let back: Scene = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back.get(field).unwrap().kind, s.get(field).unwrap().kind);
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
        steps.push(Step {
            id: 1,
            enabled: true,
            op: StepOp::Unwrap { seams: vec![], angle: Some(30.0), pins: vec![], auto_cut: true, margin: 0.01, method: Default::default() },
        });
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
                method: Default::default(),
            },
        });
    }
    let e = ev.evaluate(&s, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    assert!(e.mesh.has_uvs());
}

#[test]
fn uv_edit_steps_move_weld_and_record_seams() {
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let id = s.add("Cube", cube_kind(), None).unwrap();
    let base = ev.evaluate(&s, id).mesh;
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps.push(Step { id: 1, enabled: true, op: StepOp::Seams { edges: vec![[0, 1], [1, 2]] } });
        steps.push(Step {
            id: 2,
            enabled: true,
            op: StepOp::TransformUvs { corners: vec![0, 1, 2, 3], translate: [0.5, 0.0], rotate_deg: 0.0, scale: [1.0, 1.0], pivot: [0.0, 0.0] },
        });
    }
    let e = ev.evaluate(&s, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    // The marker shares the mesh; the transform moved exactly four corners.
    for c in 0..base.corner_count() as u32 {
        let moved = c < 4;
        assert_eq!((e.mesh.uv(c) - base.uv(c)).length() > 0.4, moved, "corner {c}");
    }
    let steps_only_seams = ev.steps_output(&s, id, 1);
    assert!(std::sync::Arc::ptr_eq(&steps_only_seams.mesh, &base) || steps_only_seams.mesh.corner_uvs() == base.corner_uvs());
    // Weld an edge; JSON defaults for the new steps.
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps.clear();
        steps.push(Step { id: 3, enabled: true, op: StepOp::StitchUvs { edges: vec![0], align: false, scale: false } });
        steps.push(Step { id: 4, enabled: true, op: StepOp::StitchUvs { edges: vec![999], align: false, scale: false } });
    }
    let e = ev.evaluate(&s, id);
    assert_eq!(e.errors.len(), 1, "an edge that does not exist is reported, the step skipped");
    let back: Step = serde_json::from_str(r#"{"id":1,"op":"transform_uvs","corners":[1,2]}"#).unwrap();
    assert!(matches!(back.op, StepOp::TransformUvs { scale: [1.0, 1.0], rotate_deg, .. } if rotate_deg == 0.0));
    let back: Step = serde_json::from_str(r#"{"id":1,"op":"seams"}"#).unwrap();
    assert!(back.op.is_marker());
}

#[test]
fn pack_and_project_steps_write_uvs() {
    use modelcraft_uvpack::Axis;
    let mut s = Scene::new();
    let mut ev = Evaluator::new();
    let id = s.add("Cube", cube_kind(), None).unwrap();
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps.push(Step { id: 1, enabled: true, op: StepOp::ProjectUvs { projection: Projection::Box, fit: Fit::Uniform, faces: vec![] } });
        steps.push(Step { id: 2, enabled: true, op: StepOp::PackUvs { margin: 0.01, rotation: Rotation::Fit, density: Density::Uniform } });
    }
    let e = ev.evaluate(&s, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    assert!(e.mesh.has_uvs());
    assert!(e.mesh.corner_uvs().iter().all(|u| u.is_finite() && u.min_element() >= -1e-5 && u.max_element() <= 1.0 + 1e-5));
    // JSON defaults.
    let back: Step = serde_json::from_str(r#"{"id":1,"op":"pack_uvs"}"#).unwrap();
    assert!(matches!(back.op, StepOp::PackUvs { rotation: Rotation::Fit, density: Density::Uniform, .. }));
    let back: Step = serde_json::from_str(r#"{"id":1,"op":"project_uvs","projection":{"kind":"planar","from":"pos_y"}}"#).unwrap();
    assert!(matches!(back.op, StepOp::ProjectUvs { projection: Projection::Planar { from: Axis::PosY }, fit: Fit::Uniform, .. }));
    // A selection that no longer fits is reported, not fatal; packing a mesh without UVs too.
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps.clear();
        steps.push(Step { id: 3, enabled: true, op: StepOp::ProjectUvs { projection: Projection::Box, fit: Fit::None, faces: vec![999] } });
    }
    assert_eq!(ev.evaluate(&s, id).errors.len(), 1);
}

#[test]
fn objects_are_found_by_id_through_every_kind_of_change() {
    // The index must agree with a plain walk of the list after adds, removes, undo-style
    // re-inserts, reparenting, reordering, and for scenes straight from a file.
    fn check(s: &Scene, ids: &[ObjectId]) {
        for id in ids.iter().copied().chain([ObjectId(0), ObjectId(9_999)]) {
            let walked = s.objects().iter().find(|o| o.id == id).map(|o| o.id);
            assert_eq!(s.get(id).map(|o| o.id), walked, "get {id:?}");
            assert_eq!(s.index_of(id), s.objects().iter().position(|o| o.id == id), "index_of {id:?}");
        }
    }
    let mut s = Scene::new();
    let mut ids = Vec::new();
    for i in 0..40 {
        let parent = if i % 3 == 0 { None } else { ids.get(i / 2).copied() };
        ids.push(s.add(format!("O{i}"), ObjectKind::Group, parent).unwrap());
        check(&s, &ids);
    }
    let removed = s.remove(ids[4]);
    assert!(removed.len() > 1, "a subtree went");
    check(&s, &ids);
    for (i, o) in removed.into_iter().rev() {
        s.insert_at(i, o);
    }
    check(&s, &ids);
    s.reparent(ids[7], Some(ids[1])).unwrap();
    check(&s, &ids);
    s.move_in_stack(ids[9], -1).unwrap();
    s.move_in_stack(ids[2], 5).unwrap();
    check(&s, &ids);
    s.edit(ids[20]).unwrap().name = "renamed".into();
    assert_eq!(s.get(ids[20]).unwrap().name, "renamed");
    // From a file: no index until it is repaired, and lookups still work in between.
    let json = serde_json::to_string(&s).unwrap();
    let raw: Scene = serde_json::from_str(&json).unwrap();
    check(&raw, &ids);
    let mut bare = s.clone();
    *bare.index = crate::Index::default();
    assert!(bare == s, "the index is not part of what a scene is");
    check(&bare, &ids);
    let fixed = raw.repaired().unwrap();
    check(&fixed, &ids);
    // A clone keeps working after the original changes, and the other way round.
    let mut copy = fixed.clone();
    copy.remove(ids[0]);
    check(&copy, &ids);
    check(&fixed, &ids);
    // A thousand objects: every lookup lands, and a missing id is known to be missing.
    let mut big = Scene::new();
    let many: Vec<ObjectId> = (0..1000).map(|i| big.add(format!("B{i}"), ObjectKind::Group, None).unwrap()).collect();
    assert!(many.iter().all(|id| big.get(*id).is_some_and(|o| o.id == *id)));
    assert!(big.get(ObjectId(123_456)).is_none());
}
