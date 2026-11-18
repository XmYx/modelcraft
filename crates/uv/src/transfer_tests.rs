//! Tests of UV transfer between meshes.

use super::*;
use glam::Vec3;
use modelcraft_ops::{edit, primitives::Primitive};
use proptest::prelude::*;

fn laid_out_plane(n: u32) -> PolyMesh {
    let m = Primitive::Plane { width: 4.0, depth: 2.0, segments: n }.build().unwrap();
    unwrap_mesh(&m, &SeamSet::new(), &UnwrapOptions::default()).unwrap().0
}

#[test]
fn a_mesh_gets_its_own_uvs_back() {
    let sphere = edit::weld(&Primitive::Sphere { radius: 1.0, segments: 16, rings: 8 }.build().unwrap(), 1e-4).unwrap();
    let (laid, _) = unwrap_mesh(&sphere, &SeamSet::new(), &UnwrapOptions::default()).unwrap();
    let mut blank = laid.clone();
    blank.clear_uvs();
    let (uvs, rep) = transfer_uvs(&laid, &blank, &TransferOptions::default()).unwrap();
    assert_eq!((rep.corners, rep.mapped, rep.too_far), (laid.corner_count(), laid.corner_count(), 0));
    assert!(rep.max_distance < 1e-5, "{rep:?}");
    // Every corner lands at the same place in the layout: the same vertex on the same island.
    let worst = (0..laid.corner_count()).map(|c| (uvs[c] - laid.corner_uvs()[c]).length()).fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "{worst}");
}

#[test]
fn a_denser_flat_mesh_gets_an_affine_map_of_its_positions() {
    let source = laid_out_plane(4);
    let target = Primitive::Plane { width: 4.0, depth: 2.0, segments: 11 }.build().unwrap();
    let (uvs, rep) = transfer_uvs(&source, &target, &TransferOptions::default()).unwrap();
    assert_eq!(rep.too_far, 0);
    // Per-island and plain lookups agree on a single island.
    let (back, _) = transfer_uvs(&source, &target, &TransferOptions { per_island: false, ..TransferOptions::default() }).unwrap();
    let worst = uvs.iter().zip(&back).map(|(x, y)| (*x - *y).length()).fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "{worst}");
    // Opposite corners of the target plane reach opposite ends of the source layout.
    let at = |m: &PolyMesh, x: f32, z: f32| (0..m.corner_count() as u32).find(|c| (m.position(m.origin(*c)) - Vec3::new(x, 0.0, z)).length() < 1e-4).unwrap();
    let (c0, c1) = (at(&target, -2.0, -1.0), at(&target, 2.0, 1.0));
    let (s0, s1) = (at(&source, -2.0, -1.0), at(&source, 2.0, 1.0));
    assert!((uvs[c0 as usize] - source.uv(s0)).length() < 1e-4 && (uvs[c1 as usize] - source.uv(s1)).length() < 1e-4);
}

#[test]
fn per_island_lookup_keeps_a_face_on_one_island() {
    // A cube whose six faces are islands, and a finer cube: the finer one gets six islands too.
    let source = Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap();
    let target = Primitive::Cube { size: Vec3::splat(2.0), segments: 3 }.build().unwrap();
    let (uvs, rep) = transfer_uvs(&source, &target, &TransferOptions::default()).unwrap();
    assert_eq!(rep.too_far, 0);
    let mut out = target.clone();
    out.set_uvs(&uvs).unwrap();
    let islands = Charts::build(&out, &SeamSet::from_uv_discontinuities(&out)).unwrap().islands.len();
    assert_eq!(islands, 6, "one island per cube face");
    // Without the island rule the vertices on a cube edge take one side's UV and a face reaching over it is torn.
    let (plain, _) = transfer_uvs(&source, &target, &TransferOptions { per_island: false, ..TransferOptions::default() }).unwrap();
    let mut flat = target.clone();
    flat.set_uvs(&plain).unwrap();
    let span = |m: &PolyMesh| {
        (0..m.face_count() as u32)
            .map(|f| {
                let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
                for h in m.face_half_edges(f) {
                    lo = lo.min(m.uv(h));
                    hi = hi.max(m.uv(h));
                }
                (hi - lo).max_element()
            })
            .fold(0.0f32, f32::max)
    };
    assert!(span(&flat) > 2.0 * span(&out), "{} against {}", span(&flat), span(&out));
}

#[test]
fn a_distance_limit_leaves_far_corners_alone() {
    let source = laid_out_plane(3);
    let mut target = Primitive::Plane { width: 4.0, depth: 2.0, segments: 3 }.build().unwrap();
    let moved: Vec<Vec3> =
        (0..target.vertex_count() as u32).map(|v| target.position(v) + Vec3::new(0.0, if target.position(v).x > 0.0 { 5.0 } else { 0.01 }, 0.0)).collect();
    target.set_positions(&moved).unwrap();
    let before = target.corner_uvs().to_vec();
    let (uvs, rep) = transfer_uvs(&source, &target, &TransferOptions { max_distance: Some(0.1), per_island: true }).unwrap();
    assert!(rep.too_far > 0 && rep.mapped > 0 && rep.mapped + rep.too_far == rep.corners, "{rep:?}");
    for c in 0..target.corner_count() as u32 {
        if target.position(target.origin(c)).x > 0.5 {
            assert_eq!(uvs[c as usize], before[c as usize], "a far corner keeps its UV");
        }
    }
    assert!(transfer_uvs(&source, &target, &TransferOptions { max_distance: Some(f32::NAN), per_island: true }).is_err());
}

#[test]
fn transfer_needs_uvs_and_faces() {
    let bare = Primitive::Plane { width: 1.0, depth: 1.0, segments: 1 }.build().unwrap();
    let mut no_uv = bare.clone();
    no_uv.clear_uvs();
    assert_eq!(transfer_uvs(&no_uv, &bare, &TransferOptions::default()).unwrap_err(), UvError::NoUvs);
    assert_eq!(transfer_uvs(&bare, &PolyMesh::default(), &TransferOptions::default()).unwrap_err(), UvError::NoFaces);
}

proptest! {
    #[test]
    fn transfer_between_random_meshes_never_panics(
        pts in proptest::collection::vec((-3.0f32..3.0, -3.0f32..3.0, -3.0f32..3.0), 4..14),
        faces in proptest::collection::vec(proptest::collection::vec(0u32..16, 3..6), 1..24),
        shift in -5.0f32..5.0,
        per_island in any::<bool>(),
        limit in proptest::option::of(0.0f32..4.0),
    ) {
        let source = laid_out_plane(3);
        let pos: Vec<Vec3> = pts.iter().map(|p| Vec3::new(p.0 + shift, p.1, p.2)).collect();
        let (target, _) = PolyMesh::build(pos, faces.into_iter().map(modelcraft_mesh::Polygon::new).collect()).unwrap_or_default();
        match transfer_uvs(&source, &target, &TransferOptions { max_distance: limit, per_island }) {
            Ok((uvs, rep)) => {
                prop_assert_eq!(uvs.len(), target.corner_count());
                prop_assert!(uvs.iter().all(|u| u.is_finite()));
                prop_assert_eq!(rep.mapped + rep.too_far, rep.corners);
            }
            Err(e) => prop_assert_eq!(e, UvError::NoFaces),
        }
    }
}
