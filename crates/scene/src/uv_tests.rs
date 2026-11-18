//! UV steps that spread a layout over UDIM tiles and cut along polygon groups.

use super::*;
use glam::Vec3;
use modelcraft_ops::primitives::Primitive;

fn cube() -> (Scene, Evaluator, ObjectId) {
    let mut s = Scene::new();
    let id = s
        .add("Cube", ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::Cube { size: Vec3::splat(2.0), segments: 2 } }, steps: vec![] }, None)
        .unwrap();
    (s, Evaluator::new(), id)
}

fn push(s: &mut Scene, id: ObjectId, ops: Vec<StepOp>) {
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        for (i, op) in ops.into_iter().enumerate() {
            steps.push(Step { id: i as u64 + 1, enabled: true, op });
        }
    }
}

#[test]
fn packing_into_tiles_numbers_them_and_defaults_to_one() {
    let (mut s, mut ev, id) = cube();
    push(
        &mut s,
        id,
        vec![
            StepOp::ProjectUvs { projection: Projection::Box, fit: Fit::Uniform, faces: vec![] },
            StepOp::PackUvs { margin: 0.01, rotation: Rotation::Fit, density: Density::Uniform, tiles: 3, texel_density: None },
        ],
    );
    let e = ev.evaluate(&s, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    assert_eq!(modelcraft_uvpack::udim_tiles(&e.mesh), vec![1001, 1002, 1003]);
    // A texel density asks for as many tiles as it needs, and one tile stays in the unit square.
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps[1].op = StepOp::PackUvs { margin: 0.01, rotation: Rotation::Fit, density: Density::Uniform, tiles: 1, texel_density: Some(0.05) };
    }
    let e = ev.evaluate(&s, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    assert_eq!(modelcraft_uvpack::udim_tiles(&e.mesh), vec![1001], "a low density fits one tile");
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
        steps[1].op = StepOp::PackUvs { margin: 0.01, rotation: Rotation::Fit, density: Density::Uniform, tiles: 1, texel_density: None };
    }
    let e = ev.evaluate(&s, id);
    assert!(e.mesh.corner_uvs().iter().all(|u| u.min_element() >= -1e-4 && u.max_element() <= 1.0 + 1e-4));
    // Old files have neither field.
    let back: Step = serde_json::from_str(r#"{"id":1,"op":"pack_uvs"}"#).unwrap();
    assert!(matches!(back.op, StepOp::PackUvs { tiles: 1, texel_density: None, .. }));
    let json = serde_json::to_string(&Step {
        id: 1,
        enabled: true,
        op: StepOp::PackUvs { margin: 0.01, rotation: Rotation::Fit, density: Density::Keep, tiles: 4, texel_density: Some(2.0) },
    })
    .unwrap();
    let again: Step = serde_json::from_str(&json).unwrap();
    assert!(matches!(again.op, StepOp::PackUvs { tiles: 4, texel_density: Some(d), .. } if d == 2.0));
}

#[test]
fn unwrapping_by_polygon_groups_makes_each_group_an_island() {
    let (mut s, mut ev, id) = cube();
    // Two groups: the faces of the top half of the cube, and the rest.
    let m = ev.evaluate(&s, id).mesh;
    let top: Vec<u32> = (0..m.face_count() as u32).filter(|f| m.face_half_edges(*f).all(|h| m.position(m.origin(h)).y > -1e-4)).collect();
    assert!(!top.is_empty() && top.len() < m.face_count());
    push(
        &mut s,
        id,
        vec![
            StepOp::SetPatch { faces: top.clone(), patch: 5 },
            StepOp::Unwrap { seams: vec![], angle: None, pins: vec![], auto_cut: true, margin: 0.01, method: Default::default(), from_patches: true },
        ],
    );
    let e = ev.evaluate(&s, id);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    // Each patch is an island: no island holds faces of both.
    let islands = modelcraft_uvpack::uv_islands(&e.mesh);
    assert!(islands.len() >= 2, "{} islands", islands.len());
    for isl in &islands {
        let first = e.mesh.patch(isl[0]);
        assert!(isl.iter().all(|f| e.mesh.patch(*f) == first), "an island mixes patches");
    }
    // Without the option the cube is cut by the automatic cuts only and a patch is not an island.
    if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind
        && let StepOp::Unwrap { from_patches, .. } = &mut steps[1].op
    {
        *from_patches = false;
    }
    let plain = modelcraft_uvpack::uv_islands(&ev.evaluate(&s, id).mesh);
    assert!(plain.len() != islands.len() || plain.iter().any(|i| i.iter().any(|f| e.mesh.patch(*f) != e.mesh.patch(i[0]))));
    let back: Step = serde_json::from_str(r#"{"id":1,"op":"unwrap"}"#).unwrap();
    assert!(matches!(back.op, StepOp::Unwrap { from_patches: false, .. }));
}
