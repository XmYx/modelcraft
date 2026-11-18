use glam::{Mat4, Vec2, Vec3};
use modelcraft_mesh::PolyMesh;
use modelcraft_ops::primitives::Primitive;
use proptest::prelude::*;

use crate::groups::*;
use crate::{BrushKind, BrushSettings, Dab, SculptMesh};

fn cube() -> PolyMesh {
    Primitive::default_of("cube").unwrap().build().unwrap()
}
fn ball(seg: u32) -> PolyMesh {
    Primitive::RoundCube { radius: 1.0, segments: seg }.build().unwrap()
}
/// A view from +Z looking at the origin, orthographic over −2..2.
fn front() -> Mat4 {
    Mat4::orthographic_rh(-2.0, 2.0, -2.0, 2.0, 0.1, 20.0) * Mat4::look_at_rh(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y)
}
fn closed(m: &PolyMesh) -> bool {
    (0..m.edge_count() as u32).all(|e| !m.is_boundary_edge(e))
}

#[test]
fn a_cube_has_six_patches_by_angle_and_one_by_part() {
    let m = cube();
    let (p, n) = patches_by_normal(&m, 30.0);
    assert_eq!(n, 6);
    assert_eq!(p.len(), m.face_count());
    for f in 0..m.face_count() as u32 {
        for g in 0..m.face_count() as u32 {
            assert_eq!(p[f as usize] == p[g as usize], m.face_normal(f).dot(m.face_normal(g)) > 0.9);
        }
    }
    assert_eq!(patches_by_part(&m).1, 1);
    // A wide angle joins everything; a silly one is clamped rather than refused.
    assert_eq!(patches_by_normal(&m, 170.0).1, 1);
    assert_eq!(patches_by_normal(&m, f32::NAN).1, 6);
    assert!(patches_by_normal(&ball(8), 30.0).1 == 1, "a smooth ball is one patch");
}

#[test]
fn uv_islands_follow_the_seams() {
    let m = cube();
    if m.has_uvs() {
        let (p, n) = patches_by_uv_island(&m).unwrap();
        assert!(n >= 1 && n <= m.face_count() as u32);
        assert_eq!(p.len(), m.face_count());
    }
    // Two quads sharing an edge: one island when the UVs agree across it, two when they do not.
    let pos = vec![Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y, Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 0.0)];
    let uv = |x: f32, y: f32| Vec2::new(x, y);
    let a = modelcraft_mesh::Polygon::with_uvs(vec![0, 1, 2, 3], vec![uv(0.0, 0.0), uv(0.5, 0.0), uv(0.5, 1.0), uv(0.0, 1.0)]);
    let joined = modelcraft_mesh::Polygon::with_uvs(vec![1, 4, 5, 2], vec![uv(0.5, 0.0), uv(1.0, 0.0), uv(1.0, 1.0), uv(0.5, 1.0)]);
    let apart = modelcraft_mesh::Polygon::with_uvs(vec![1, 4, 5, 2], vec![uv(0.6, 0.0), uv(1.0, 0.0), uv(1.0, 1.0), uv(0.6, 1.0)]);
    let one = PolyMesh::build(pos.clone(), vec![a.clone(), joined]).unwrap().0;
    let two = PolyMesh::build(pos, vec![a, apart]).unwrap().0;
    assert_eq!(patches_by_uv_island(&one).unwrap().1, 1);
    assert_eq!(patches_by_uv_island(&two).unwrap().1, 2);
    let bare = PolyMesh::from_faces(vec![Vec3::ZERO, Vec3::X, Vec3::Y], &[vec![0, 1, 2]]).unwrap();
    assert_eq!(patches_by_uv_island(&bare), Err(GroupError::NoUvs));
}

#[test]
fn the_mask_becomes_a_patch_and_a_shell() {
    let m = ball(12);
    let mask: Vec<f32> = m.positions().iter().map(|p| if p.y > 0.3 { 1.0 } else { 0.0 }).collect();
    let (p, moved) = patch_from_mask(&m, &mask).unwrap();
    assert!(moved > 0 && moved < m.face_count());
    assert_eq!(p.iter().filter(|x| **x == *p.iter().max().unwrap()).count(), moved);
    assert_eq!(patch_from_mask(&m, &vec![0.0; m.vertex_count()]), Err(GroupError::NothingMasked));
    for t in [0.1f32, -0.1] {
        let shell = extract_shell(&m, &mask, t).unwrap();
        shell.validate().unwrap();
        assert!(closed(&shell), "thickness {t}");
        // Two skins of the masked faces plus one quad per rim edge.
        assert!(shell.face_count() > 2 * moved);
        assert_eq!(shell.vertex_count() as i64 - shell.edge_count() as i64 + shell.face_count() as i64, 2, "one closed cap");
        let (tris, _) = shell.triangulate();
        let vol: f32 = tris.iter().map(|t| shell.position(t[0]).dot(shell.position(t[1]).cross(shell.position(t[2]))) / 6.0).sum();
        assert!(vol > 0.0, "the shell faces outwards (thickness {t}): {vol}");
    }
    assert_eq!(extract_shell(&m, &mask, 0.0), Err(GroupError::BadThickness));
    assert_eq!(extract_shell(&m, &mask, f32::NAN), Err(GroupError::BadThickness));
    assert_eq!(extract_shell(&m, &vec![0.0; m.vertex_count()], 0.1), Err(GroupError::NothingMasked));
}

#[test]
fn a_rectangle_and_a_lasso_pick_what_is_under_them() {
    let m = ball(12);
    let right = ScreenRegion::rect(front(), Vec2::new(0.013, -1.0), Vec2::new(1.0, 1.0));
    assert!(right.contains(Vec3::new(0.5, 0.0, 0.0), Vec3::Z));
    assert!(!right.contains(Vec3::new(-0.5, 0.0, 0.0), Vec3::Z));
    let faces = faces_in_region(&m, &right);
    for (f, inside) in faces.iter().enumerate() {
        assert_eq!(*inside, m.face_vertices(f as u32).all(|v| m.position(v).x > 0.026), "face {f}");
    }
    // Front only leaves out the far side.
    let near = ScreenRegion { front_only: true, eye: Vec3::new(0.0, 0.0, 5.0), ..right.clone() };
    let front_faces = faces_in_region(&m, &near);
    assert!(front_faces.iter().filter(|x| **x).count() < faces.iter().filter(|x| **x).count());
    assert!(front_faces.iter().enumerate().all(|(f, inside)| !inside || m.face_center(f as u32).z > -0.2));
    // A triangle lasso: the even-odd rule on a concave outline.
    let lasso = ScreenRegion {
        points: vec![Vec2::new(-0.4, -0.4), Vec2::new(0.4, -0.4), Vec2::new(0.0, 0.0), Vec2::new(0.4, 0.4), Vec2::new(-0.4, 0.4)],
        ..right.clone()
    };
    assert!(lasso.contains(Vec3::new(-0.6, 0.0, 0.0), Vec3::Z));
    assert!(!lasso.contains(Vec3::new(0.6, 0.0, 0.0), Vec3::Z), "the notch is outside");
    // Bad regions pick nothing.
    let two = ScreenRegion { points: vec![Vec2::ZERO, Vec2::ONE], ..right.clone() };
    assert!(!two.is_valid() && faces_in_region(&m, &two).iter().all(|x| !x));
    let nan = ScreenRegion { points: vec![Vec2::ZERO, Vec2::ONE, Vec2::splat(f32::NAN)], ..right };
    assert!(!nan.is_valid());
}

#[test]
fn hidden_faces_are_not_picked_or_sculpted_and_come_back_untouched() {
    let m = ball(16);
    let mut s = SculptMesh::new(&m, None, None);
    assert_eq!(s.face_count(), m.face_count());
    let top: Vec<bool> = (0..m.face_count() as u32).map(|f| m.face_center(f).y > 0.0).collect();
    assert!(s.set_face_hidden(&top));
    assert!(s.any_hidden());
    assert!(!s.set_face_hidden(&[true]), "a wrong count changes nothing");
    // A ray from above passes through the hidden half and lands on the inside of the lower one.
    let (_, p, t) = s.raycast(Vec3::new(0.0, 5.0, 0.0), -Vec3::Y).unwrap();
    assert!(p.y < 0.0);
    assert!(!s.hidden_triangles()[t as usize]);
    let before = s.positions.clone();
    s.begin_stroke(BrushSettings::new(BrushKind::Draw, 0.6));
    s.dab(Dab { center: Vec3::Y, view_dir: -Vec3::Y, pressure: 1.0, grab_delta: Vec3::ZERO, scale: 1.0 });
    s.dab(Dab { center: Vec3::new(1.0, 0.05, 0.0), view_dir: -Vec3::X, pressure: 1.0, grab_delta: Vec3::ZERO, scale: 1.0 });
    let (u, _) = s.end_stroke().unwrap();
    assert!(!u.indices.is_empty(), "the visible rim moved");
    for v in &u.indices {
        assert!(before[*v as usize].y <= 1e-6, "vertex {v} belongs to hidden faces only");
    }
    // Masking a region leaves hidden vertices alone as well.
    let all = ScreenRegion::rect(front(), Vec2::splat(-1.0), Vec2::splat(1.0));
    let u = s.mask_region(&all, 1.0);
    assert!(!u.indices.is_empty());
    assert!(u.indices.iter().all(|v| before[*v as usize].y <= 1e-6));
    assert!(s.set_face_hidden(&[]));
    assert!(!s.any_hidden());
    assert!(s.raycast(Vec3::new(0.0, 5.0, 0.0), -Vec3::Y).unwrap().1.y > 0.9);
}

#[test]
fn patches_per_face_round_trip_through_the_triangles() {
    let m = ball(6);
    let mut s = SculptMesh::new(&m, None, None);
    let want: Vec<u32> = (0..m.face_count() as u32).map(|f| f % 5).collect();
    let rev = s.view_rev();
    assert!(s.set_face_patches(&want));
    assert_ne!(s.view_rev(), rev);
    assert_eq!(s.face_patches(), want);
    assert_eq!(s.triangle_patches().len(), s.triangle_count());
    assert_eq!(s.patch_of_triangle(0), Some(want[s.face_of_triangle(0) as usize]));
    assert!(!s.set_face_patches(&want[1..]));
}

#[test]
fn clip_moves_one_side_onto_the_plane_and_respects_the_mask() {
    let m = ball(12);
    let mut s = SculptMesh::new(&m, None, None);
    let u = s.mask_all(0.0);
    assert!(u.is_empty());
    // Mask the +X half, then clip the top off.
    s.mask_edit(|_, p, _| if p.x > 0.0 { 1.0 } else { 0.0 });
    let before = s.positions.clone();
    let u = s.clip_plane(Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 2.0, 0.0));
    assert!(!u.indices.is_empty());
    for (v, p) in s.positions.iter().enumerate() {
        if before[v].x > 0.0 {
            assert_eq!(*p, before[v], "masked");
        } else {
            assert!(p.y <= 0.5 + 1e-5);
        }
    }
    s.apply_undo(&u, false);
    assert_eq!(s.positions, before);
    assert!(s.clip_plane(Vec3::ZERO, Vec3::ZERO).is_empty());
    assert!(s.clip_plane(Vec3::splat(f32::INFINITY), Vec3::Y).is_empty());
}

#[test]
fn occlusion_masks_the_inside_of_a_dent_and_not_the_open_surface() {
    // A ball with a deep pit pushed into its top.
    let mut m = ball(24);
    m.map_positions(|_, p| {
        let r = Vec2::new(p.x, p.z).length();
        if p.y > 0.0 && r < 0.35 { Vec3::new(p.x, p.y - 1.2 * (1.0 - r / 0.35), p.z) } else { p }
    });
    let mut s = SculptMesh::new(&m, None, None);
    let u = s.mask_by_occlusion();
    assert!(!u.indices.is_empty());
    let (mut pit, mut open) = (Vec::new(), Vec::new());
    for (v, p) in s.positions.iter().enumerate() {
        if p.y > 0.0 && Vec2::new(p.x, p.z).length() < 0.15 {
            pit.push(s.mask[v]);
        } else if p.y < -0.5 {
            open.push(s.mask[v]);
        }
    }
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    assert!(!pit.is_empty() && !open.is_empty());
    assert!(mean(&pit) > 0.5, "the pit is in shadow: {}", mean(&pit));
    assert!(mean(&open) < 0.05, "the underside sees the sky: {}", mean(&open));
    assert!(s.mask.iter().all(|m| (0.0..=1.0).contains(m)));
    // The same mesh gives the same mask.
    let mut again = SculptMesh::new(&m, None, None);
    again.mask_by_occlusion();
    assert_eq!(again.mask, s.mask);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn any_mask_gives_a_valid_shell_or_a_clear_refusal(seed in any::<u64>(), share in 0.05f32..0.95, thickness in -0.3f32..0.3) {
        let m = ball(5);
        let mut x = seed | 1;
        let mask: Vec<f32> = (0..m.vertex_count()).map(|_| {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            if (x % 1000) as f32 / 1000.0 < share { 1.0 } else { 0.0 }
        }).collect();
        match extract_shell(&m, &mask, thickness) {
            Ok(shell) => {
                prop_assert!(shell.validate().is_ok());
                prop_assert!(shell.positions().iter().all(|p| p.is_finite()));
            }
            Err(e) => prop_assert!(matches!(e, GroupError::NothingMasked | GroupError::BadThickness)),
        }
    }

    #[test]
    fn groups_cover_every_face_once(angle in 1.0f32..179.0, seg in 1u32..7) {
        let m = ball(seg);
        let (p, n) = patches_by_normal(&m, angle);
        prop_assert_eq!(p.len(), m.face_count());
        prop_assert!(p.iter().all(|x| (1..=n).contains(x)));
        let mut seen = vec![false; n as usize];
        for x in &p { seen[(*x - 1) as usize] = true; }
        prop_assert!(seen.iter().all(|s| *s), "no empty patch number");
    }
}
