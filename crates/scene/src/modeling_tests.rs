//! The polygon-tool steps: files saved before their options existed still read, and the options
//! change the result.

use super::*;
use glam::Vec3;
use modelcraft_ops::bevel::BevelProfile;
use modelcraft_ops::primitives::Primitive;

fn cube_scene(op: StepOp) -> (Scene, Evaluator, ObjectId) {
    let mut s = Scene::new();
    let ev = Evaluator::new();
    let id = s
        .add("Cube", ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::Cube { size: Vec3::splat(2.0), segments: 1 } }, steps: vec![] }, None)
        .unwrap();
    let sid = s.fresh_id();
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps.push(Step { id: sid, enabled: true, op });
    }
    (s, ev, id)
}

#[test]
fn steps_saved_before_the_options_existed_read_unchanged() {
    let old = [
        (r#"{"id":1,"op":"extrude","faces":[0],"distance":1.0}"#, StepOp::extrude(vec![0], 1.0)),
        (r#"{"id":2,"enabled":false,"op":"bevel","edges":[3,4],"width":0.25}"#, StepOp::bevel(vec![3, 4], 0.25)),
        (r#"{"id":3,"op":"loop_cut","edge":7,"cuts":2}"#, StepOp::loop_cut(7, 2)),
    ];
    for (json, want) in old {
        let step: Step = serde_json::from_str(json).unwrap();
        assert_eq!(step.op, want, "{json}");
        // Written again, the defaults stay out of the file: an old reader and an old diff see the same text.
        let back = serde_json::to_string(&step).unwrap();
        for key in ["segments", "profile", "tension", "offset", "subdivisions", "variation", "seed", "caps", "individual", "symmetric"] {
            assert!(!back.contains(key), "{key} written for a default: {back}");
        }
    }
}

#[test]
fn options_round_trip_and_change_the_mesh() {
    let ops = [
        StepOp::Extrude { faces: vec![0], distance: 0.5, offset: 0.1, subdivisions: 3, variation: 0.2, seed: 9, caps: false, individual: true },
        StepOp::Bevel { edges: vec![0, 1], width: 0.2, segments: 4, profile: BevelProfile::Custom, tension: 0.8 },
        StepOp::BevelVertices { verts: vec![0], width: 0.3 },
        StepOp::LoopCut { edge: 0, cuts: 3, offset: 0.4, symmetric: true, profile: -0.5 },
    ];
    for op in ops {
        let json = serde_json::to_string(&Step { id: 1, enabled: true, op: op.clone() }).unwrap();
        let back: Step = serde_json::from_str(&json).unwrap();
        assert_eq!(back.op, op);
    }
    let plain = |op: StepOp| {
        let (s, mut ev, id) = cube_scene(op);
        let e = ev.evaluate(&s, id);
        assert!(e.errors.is_empty(), "{:?}", e.errors);
        e.mesh.face_count()
    };
    assert_eq!(plain(StepOp::bevel(vec![0], 0.2)), 7);
    assert_eq!(plain(StepOp::Bevel { edges: vec![0], width: 0.2, segments: 4, profile: BevelProfile::Round, tension: 0.5 }), 10);
    assert_eq!(plain(StepOp::extrude(vec![0], 0.5)), 10);
    assert_eq!(
        plain(StepOp::Extrude { faces: vec![0], distance: 0.5, offset: 0.0, subdivisions: 3, variation: 0.0, seed: 0, caps: true, individual: false }),
        6 + 12
    );
    assert_eq!(plain(StepOp::BevelVertices { verts: vec![0], width: 0.3 }), 7);
    assert_eq!(plain(StepOp::loop_cut(0, 2)), 6 + 8);
}

#[test]
fn a_stale_selection_in_the_new_steps_is_reported_not_fatal() {
    for op in [
        StepOp::BevelVertices { verts: vec![500], width: 0.1 },
        StepOp::ExtrudeEdges { edges: vec![500], distance: 0.1, lift: 0.0 },
        StepOp::Bevel { edges: vec![500], width: 0.1, segments: 3, profile: BevelProfile::Round, tension: 0.5 },
    ] {
        let (s, mut ev, id) = cube_scene(op);
        let e = ev.evaluate(&s, id);
        assert_eq!(e.errors.len(), 1);
        assert_eq!(e.mesh.face_count(), 6);
    }
    // Edges inside a closed surface cannot be extruded: an error on the step, the mesh kept.
    let (s, mut ev, id) = cube_scene(StepOp::ExtrudeEdges { edges: vec![0], distance: 0.1, lift: 0.0 });
    let e = ev.evaluate(&s, id);
    assert_eq!(e.errors.len(), 1);
    assert_eq!(e.mesh.face_count(), 6);
}

#[test]
fn every_new_step_has_a_name() {
    for op in [StepOp::ExtrudeEdges { edges: vec![], distance: 0.0, lift: 0.0 }, StepOp::BevelVertices { verts: vec![], width: 0.0 }] {
        assert!(!op.name().is_empty());
        assert!(op.heap_bytes() == 0);
    }
}

#[test]
fn the_cutting_steps_round_trip_and_run() {
    use modelcraft_ops::cut::PenPoint;
    let ops = [
        StepOp::SlicePlane { point: Vec3::ZERO, normal: Vec3::Y, faces: vec![] },
        StepOp::SlicePlane { point: Vec3::new(0.0, 0.2, 0.0), normal: Vec3::Y, faces: vec![0, 1] },
        StepOp::CutLine { a: Vec3::new(-2.0, 0.0, 2.0), b: Vec3::new(2.0, 0.0, 2.0), view: -Vec3::Z, front_only: true },
        StepOp::Connect { verts: vec![0, 2] },
        StepOp::AddPolygon {
            points: vec![PenPoint::Position { p: Vec3::X * 3.0 }, PenPoint::Position { p: Vec3::Y * 3.0 }, PenPoint::Vertex { v: 0 }],
            toward: Some(Vec3::Z),
        },
    ];
    for op in &ops {
        let json = serde_json::to_string(&Step { id: 1, enabled: true, op: op.clone() }).unwrap();
        assert_eq!(serde_json::from_str::<Step>(&json).unwrap().op, *op);
        assert!(!op.name().is_empty());
    }
    // A plain slice keeps its optional fields out of the file.
    let json = serde_json::to_string(&Step { id: 1, enabled: true, op: ops[0].clone() }).unwrap();
    assert!(!json.contains("faces"), "{json}");
    let faces = |op: &StepOp| {
        let (s, mut ev, id) = cube_scene(op.clone());
        let e = ev.evaluate(&s, id);
        (e.mesh.face_count(), e.errors.len())
    };
    assert_eq!(faces(&ops[0]), (10, 0));
    assert_eq!(faces(&ops[2]), (9, 0));
    // The cube's face 0 has corners 0 and 2 apart.
    let (s, mut ev, id) = cube_scene(StepOp::Connect { verts: vec![] });
    let m = ev.evaluate(&s, id).mesh;
    let vs: Vec<u32> = m.face_vertices(0).collect();
    assert_eq!(faces(&StepOp::Connect { verts: vec![vs[0], vs[2]] }), (7, 0));
    assert_eq!(
        faces(&StepOp::AddPolygon {
            points: vec![PenPoint::Position { p: Vec3::X * 3.0 }, PenPoint::Position { p: Vec3::Y * 3.0 }, PenPoint::Vertex { v: 0 }],
            toward: None
        }),
        (7, 0)
    );
    // Mistakes are step errors, not crashes.
    assert_eq!(faces(&StepOp::Connect { verts: vec![500, 501] }).1, 1);
    assert_eq!(faces(&StepOp::SlicePlane { point: Vec3::ZERO, normal: Vec3::Y, faces: vec![999] }).1, 1);
    assert_eq!(faces(&StepOp::AddPolygon { points: vec![], toward: None }).1, 1);
    assert_eq!(faces(&StepOp::CutLine { a: Vec3::ZERO, b: Vec3::ZERO, view: Vec3::Z, front_only: false }).1, 1);
}
