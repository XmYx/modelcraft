use glam::{Quat, Vec3};
use proptest::prelude::*;

use super::*;
use crate::primitives::Primitive;

fn placed(p: Primitive, at: Vec3, rot: Quat) -> PolyMesh {
    let mut m = p.build().unwrap();
    m.map_positions(|_, v| rot * v + at);
    m
}

fn cube(size: Vec3, at: Vec3) -> PolyMesh {
    placed(Primitive::Cube { size, segments: 1 }, at, Quat::IDENTITY)
}

fn ball(r: f32, at: Vec3) -> PolyMesh {
    placed(Primitive::Sphere { radius: r, segments: 24, rings: 12 }, at, Quat::IDENTITY)
}

fn vol(m: &PolyMesh) -> f64 {
    volume(&to_tris(m))
}

fn closed(m: &PolyMesh) -> bool {
    (0..m.edge_count() as u32).all(|e| !m.is_boundary_edge(e))
}

/// The three booleans of two boxes obey inclusion and exclusion, to within the snapping grid.
fn check_inclusion_exclusion(a: &PolyMesh, b: &PolyMesh, expect_overlap: Option<f64>) {
    let (va, vb) = (vol(a), vol(b));
    let u = combine(a, b, BoolOp::Union).unwrap();
    let i = combine(a, b, BoolOp::Intersect).unwrap();
    let s = combine(a, b, BoolOp::Subtract).unwrap();
    for m in [&u, &i, &s] {
        assert!(closed(m));
        m.validate().unwrap();
    }
    let (vu, vi, vs) = (vol(&u), vol(&i), vol(&s));
    let tol = 1e-4 * (va + vb).max(1.0);
    assert!((vu + vi - va - vb).abs() < tol, "union {vu} + intersection {vi} != {va} + {vb}");
    assert!((vs - (va - vi)).abs() < tol, "subtract {vs} != {va} - {vi}");
    if let Some(o) = expect_overlap {
        assert!((vi - o).abs() < tol, "overlap {vi}, expected {o}");
    }
}

#[test]
fn two_overlapping_boxes_follow_inclusion_and_exclusion() {
    let a = cube(Vec3::splat(2.0), Vec3::ZERO);
    let b = cube(Vec3::splat(2.0), Vec3::new(1.0, 0.5, 0.25));
    // Overlap: 1 × 1.5 × 1.75.
    check_inclusion_exclusion(&a, &b, Some(1.0 * 1.5 * 1.75));
}

#[test]
fn cube_minus_sphere_is_closed_and_manifold_and_keeps_uvs_and_patches() {
    let a = cube(Vec3::splat(2.0), Vec3::ZERO);
    let b = ball(1.2, Vec3::new(0.2, 0.1, -0.1));
    let m = combine(&a, &b, BoolOp::Subtract).unwrap();
    assert!(closed(&m));
    m.validate().unwrap();
    assert!(m.has_uvs(), "both inputs had UVs");
    for uv in m.corner_uvs() {
        assert!(uv.x > -1e-3 && uv.x < 1.001 && uv.y > -1e-3 && uv.y < 1.001, "{uv:?}");
    }
    // The cube's six patches and the sphere's one (renumbered after them).
    let patches: std::collections::BTreeSet<u32> = (0..m.face_count() as u32).map(|f| m.patch(f)).collect();
    assert!(patches.contains(&6), "the cut surface comes from the sphere: {patches:?}");
    assert!((0..6).filter(|p| patches.contains(p)).count() >= 4, "the cube faces survive: {patches:?}");
    // A cube of volume 8 with most of a sphere taken out.
    let v = vol(&m);
    assert!(v > 1.0 && v < 8.0, "{v}");
    // No vertex lies inside the sphere (the cut surface is on it).
    let c = Vec3::new(0.2, 0.1, -0.1);
    assert!(m.positions().iter().all(|p| (*p - c).length() > 1.2 - 2e-3 || p.abs().max_element() > 0.999), "a vertex is inside the sphere");
}

#[test]
fn union_and_intersection_of_a_cube_and_a_sphere_add_up() {
    let a = cube(Vec3::splat(2.0), Vec3::ZERO);
    let b = ball(1.1, Vec3::new(0.3, 0.0, 0.0));
    let (u, i, s) = (combine(&a, &b, BoolOp::Union).unwrap(), combine(&a, &b, BoolOp::Intersect).unwrap(), combine(&a, &b, BoolOp::Subtract).unwrap());
    for m in [&u, &i, &s] {
        assert!(closed(m));
        m.validate().unwrap();
    }
    let (va, vb) = (vol(&a), vol(&b));
    assert!((vol(&u) + vol(&i) - va - vb).abs() < 1e-3 * va, "{} {} {va} {vb}", vol(&u), vol(&i));
    assert!((vol(&s) - (va - vol(&i))).abs() < 1e-3 * va);
}

#[test]
fn boxes_that_share_a_face_plane_or_touch_resolve_without_gaps() {
    // Side by side, sharing a whole face: the union is a 4 × 2 × 2 slab.
    let a = cube(Vec3::splat(2.0), Vec3::ZERO);
    let b = cube(Vec3::splat(2.0), Vec3::new(2.0, 0.0, 0.0));
    let u = combine(&a, &b, BoolOp::Union);
    // Either a closed result with the right volume, or a reported error: never an open mesh.
    if let Ok(u) = u {
        assert!(closed(&u));
        assert!((vol(&u) - 16.0).abs() < 1e-3, "{}", vol(&u));
    }
    // Sharing the top plane but overlapping: a stepped union.
    let c = cube(Vec3::new(2.0, 1.0, 2.0), Vec3::new(1.0, 0.5, 0.0));
    let u = combine(&a, &c, BoolOp::Union).expect("overlapping boxes that share a plane");
    assert!(closed(&u));
    u.validate().unwrap();
    assert!((vol(&u) - (8.0 + 4.0 - 1.0 * 1.0 * 2.0)).abs() < 1e-3, "{}", vol(&u));
    // Sharing a plane exactly (top faces level) and overlapping: the common volume is right.
    let d = cube(Vec3::new(2.0, 2.0, 2.0), Vec3::new(1.0, 0.0, 1.0));
    check_inclusion_exclusion(&a, &d, Some(1.0 * 2.0 * 1.0));
}

#[test]
fn disjoint_and_nested_meshes_use_the_winding_number() {
    let a = cube(Vec3::splat(2.0), Vec3::ZERO);
    let far = cube(Vec3::splat(1.0), Vec3::new(10.0, 0.0, 0.0));
    let u = combine(&a, &far, BoolOp::Union).unwrap();
    assert!((vol(&u) - 9.0).abs() < 1e-4);
    assert_eq!(combine(&a, &far, BoolOp::Intersect).unwrap().face_count(), 0);
    assert!((vol(&combine(&a, &far, BoolOp::Subtract).unwrap()) - 8.0).abs() < 1e-4);
    // A box fully inside another: subtracting it leaves a hollow shell.
    let inner = cube(Vec3::splat(1.0), Vec3::ZERO);
    let shell = combine(&a, &inner, BoolOp::Subtract).unwrap();
    assert!(closed(&shell));
    assert!((vol(&shell) - 7.0).abs() < 1e-4, "{}", vol(&shell));
    assert!((vol(&combine(&a, &inner, BoolOp::Union).unwrap()) - 8.0).abs() < 1e-4);
    assert!((vol(&combine(&a, &inner, BoolOp::Intersect).unwrap()) - 1.0).abs() < 1e-4);
}

#[test]
fn the_result_can_be_combined_again() {
    let a = cube(Vec3::splat(2.0), Vec3::ZERO);
    let b = ball(1.2, Vec3::ZERO);
    let c = placed(Primitive::Cylinder { radius: 0.9, height: 4.0, sides: 24, rings: 1, caps: true }, Vec3::ZERO, Quat::IDENTITY);
    let once = combine(&a, &b, BoolOp::Subtract).unwrap();
    let twice = combine(&once, &c, BoolOp::Subtract).unwrap();
    assert!(closed(&twice));
    twice.validate().unwrap();
    assert!(vol(&twice) < vol(&once));
}

#[test]
fn bad_input_is_reported_not_crashed_on() {
    let a = cube(Vec3::splat(2.0), Vec3::ZERO);
    let open = Primitive::Plane { width: 2.0, depth: 2.0, segments: 2 }.build().unwrap();
    assert!(matches!(boolean(&to_tris(&a), &to_tris(&open), BoolOp::Union, &Limits::default()), Err(BoolError::Open(_))));
    let none = TriMesh::default();
    assert!(matches!(boolean(&to_tris(&a), &none, BoolOp::Union, &Limits::default()), Err(BoolError::Empty(_))));
    let mut nan = to_tris(&a);
    nan.positions[0] = Vec3::NAN;
    assert!(matches!(boolean(&to_tris(&a), &nan, BoolOp::Union, &Limits::default()), Err(BoolError::Invalid(_))));
    let mut flat = to_tris(&a);
    let p = flat.positions[flat.tris[0][0] as usize];
    flat.positions[flat.tris[0][1] as usize] = p;
    flat.positions[flat.tris[0][2] as usize] = p;
    assert!(boolean(&to_tris(&a), &flat, BoolOp::Union, &Limits::default()).is_err());
    let mut bad_index = to_tris(&a);
    bad_index.tris[0][0] = 9999;
    assert!(matches!(boolean(&to_tris(&a), &bad_index, BoolOp::Union, &Limits::default()), Err(BoolError::Invalid(_))));
    // Limits are honoured.
    let tight = Limits { max_triangles: 4, ..Limits::default() };
    assert!(matches!(boolean(&to_tris(&a), &to_tris(&a), BoolOp::Union, &tight), Err(BoolError::TooLarge(_))));
    let few_pairs = Limits { max_pairs: 0, ..Limits::default() };
    let b = ball(1.2, Vec3::ZERO);
    assert!(matches!(boolean(&to_tris(&a), &to_tris(&b), BoolOp::Union, &few_pairs), Err(BoolError::TooLarge(_))));
    let no_winding = Limits { max_winding_work: 0, ..Limits::default() };
    let far = cube(Vec3::splat(1.0), Vec3::new(10.0, 0.0, 0.0));
    assert!(matches!(boolean(&to_tris(&a), &to_tris(&far), BoolOp::Union, &no_winding), Err(BoolError::TooLarge(_))));
}

#[test]
fn an_inside_out_mesh_is_turned_around() {
    let a = cube(Vec3::splat(2.0), Vec3::ZERO);
    let mut b = to_tris(&cube(Vec3::splat(2.0), Vec3::new(1.0, 0.0, 0.0)));
    for t in &mut b.tris {
        t.swap(1, 2);
    }
    if let Some(u) = &mut b.uvs {
        for c in u {
            c.swap(1, 2);
        }
    }
    let out = boolean(&to_tris(&a), &b, BoolOp::Union, &Limits::default()).unwrap();
    assert!((volume(&out.mesh) - 12.0).abs() < 1e-3);
}

#[test]
fn work_is_counted_and_bounded_by_the_overlap() {
    let a = placed(Primitive::Sphere { radius: 1.0, segments: 48, rings: 24 }, Vec3::ZERO, Quat::IDENTITY);
    let b = placed(Primitive::Sphere { radius: 1.0, segments: 48, rings: 24 }, Vec3::new(1.2, 0.0, 0.0), Quat::IDENTITY);
    let (m, stats) = combine_with(&a, &b, BoolOp::Union, &Limits::default()).unwrap();
    assert!(closed(&m));
    // Only triangles near the lens of overlap are examined: far fewer pairs than all of them.
    let all = (to_tris(&a).tris.len() * to_tris(&b).tris.len()) as usize;
    assert!(stats.triangle_pairs * 10 < all, "{} of {all}", stats.triangle_pairs);
    assert!(stats.crossings > 20);
}

fn arb_vec(lo: f32, hi: f32) -> impl Strategy<Value = Vec3> {
    (lo..hi, lo..hi, lo..hi).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(40))]

    /// Overlapping boxes at random places and sizes: a closed manifold result with the right
    /// volumes, or a reported error, never a panic.
    #[test]
    fn random_boxes_give_closed_meshes_or_a_reported_error(
        size_a in arb_vec(0.5, 3.0), size_b in arb_vec(0.5, 3.0), at in arb_vec(-1.5, 1.5), op in 0usize..3,
    ) {
        let a = cube(size_a, Vec3::ZERO);
        let b = cube(size_b, at);
        let op = [BoolOp::Union, BoolOp::Subtract, BoolOp::Intersect][op];
        match combine(&a, &b, op) {
            Ok(m) => {
                prop_assert!(closed(&m));
                prop_assert!(m.validate().is_ok());
                // The exact overlap of two axis-aligned boxes.
                let overlap = (0..3).map(|k| {
                    let (a0, a1) = (-size_a[k] as f64 / 2.0, size_a[k] as f64 / 2.0);
                    let (b0, b1) = (at[k] as f64 - size_b[k] as f64 / 2.0, at[k] as f64 + size_b[k] as f64 / 2.0);
                    (a1.min(b1) - a0.max(b0)).max(0.0)
                }).product::<f64>();
                let (va, vb) = ((size_a.x * size_a.y * size_a.z) as f64, (size_b.x * size_b.y * size_b.z) as f64);
                let want = match op { BoolOp::Union => va + vb - overlap, BoolOp::Subtract => va - overlap, BoolOp::Intersect => overlap };
                prop_assert!((vol(&m) - want).abs() < 1e-3 * (va + vb), "{op:?}: {} vs {want}", vol(&m));
            }
            Err(e) => prop_assert!(!e.to_string().is_empty()),
        }
    }

    /// A rotated box against a sphere: the same, with inclusion and exclusion.
    #[test]
    fn rotated_boxes_and_spheres_obey_inclusion_and_exclusion(
        size in arb_vec(0.8, 2.5), axis in arb_vec(-1.0, 1.0), angle in 0.0f32..3.0, at in arb_vec(-1.0, 1.0), r in 0.4f32..1.6,
    ) {
        let rot = Quat::from_axis_angle(axis.normalize_or(Vec3::Y), angle);
        let a = placed(Primitive::Cube { size, segments: 1 }, Vec3::ZERO, rot);
        let b = ball(r, at);
        let (Ok(u), Ok(i), Ok(s)) = (combine(&a, &b, BoolOp::Union), combine(&a, &b, BoolOp::Intersect), combine(&a, &b, BoolOp::Subtract)) else { return Ok(()) };
        for m in [&u, &i, &s] {
            prop_assert!(closed(m));
        }
        let tol = 1e-3 * (vol(&a) + vol(&b));
        prop_assert!((vol(&u) + vol(&i) - vol(&a) - vol(&b)).abs() < tol);
        prop_assert!((vol(&s) - (vol(&a) - vol(&i))).abs() < tol);
    }

    /// Random triangle soup never panics: it is accepted as a closed mesh or turned down.
    #[test]
    fn hostile_soup_never_panics(
        pts in proptest::collection::vec(arb_vec(-2.0, 2.0), 3..12),
        tris in proptest::collection::vec((0u32..14, 0u32..14, 0u32..14), 1..10),
    ) {
        let soup = TriMesh {
            positions: pts,
            tris: tris.into_iter().map(|(a, b, c)| [a, b, c]).collect(),
            uvs: None,
            patches: Vec::new(),
        };
        let mut soup = soup;
        soup.patches = vec![0; soup.tris.len()];
        let a = to_tris(&cube(Vec3::splat(2.0), Vec3::ZERO));
        for op in [BoolOp::Union, BoolOp::Subtract, BoolOp::Intersect] {
            let _ = boolean(&a, &soup, op, &Limits::default());
            let _ = boolean(&soup, &a, op, &Limits::default());
        }
    }
}

fn box_overlap(size_a: Vec3, size_b: Vec3, at: Vec3) -> f64 {
    (0..3)
        .map(|k| {
            let (a0, a1) = (-size_a[k] as f64 / 2.0, size_a[k] as f64 / 2.0);
            let (b0, b1) = (at[k] as f64 - size_b[k] as f64 / 2.0, at[k] as f64 + size_b[k] as f64 / 2.0);
            (a1.min(b1) - a0.max(b0)).max(0.0)
        })
        .product()
}

/// Boxes on a coarse grid share planes, edges and corners all the time: the hard cases for any
/// boolean. Every one that is accepted must have exactly the right volume, and nearly all must
/// be accepted.
#[test]
fn boxes_on_a_coarse_grid_with_shared_planes_edges_and_corners() {
    let mut accepted = 0;
    let mut total = 0;
    let mut failures = Vec::new();
    for sa in [1, 2, 3] {
        for sb in [1, 2, 3] {
            for dx in -3..=3 {
                for dy in -3..=3 {
                    for dz in [-2, 0, 1, 2] {
                        let size_a = Vec3::new(sa as f32, 2.0, 2.0);
                        let size_b = Vec3::new(sb as f32, 2.0, 2.0);
                        let at = Vec3::new(dx as f32 * 0.5, dy as f32 * 0.5, dz as f32 * 0.5);
                        let (a, b) = (cube(size_a, Vec3::ZERO), cube(size_b, at));
                        let overlap = box_overlap(size_a, size_b, at);
                        let (va, vb) = (vol(&a), vol(&b));
                        for (op, want) in [(BoolOp::Union, va + vb - overlap), (BoolOp::Subtract, va - overlap), (BoolOp::Intersect, overlap)] {
                            total += 1;
                            match combine(&a, &b, op) {
                                Ok(m) => {
                                    accepted += 1;
                                    assert!(closed(&m), "{op:?} {size_a} {size_b} {at}");
                                    assert!((vol(&m) - want).abs() < 1e-3, "{op:?} {size_a} {size_b} {at}: {} vs {want}", vol(&m));
                                }
                                Err(e) => failures.push(format!("{op:?} {size_a} {size_b} {at}: {e}")),
                            }
                        }
                    }
                }
            }
        }
    }
    eprintln!("{accepted} of {total} accepted; first failures: {:?}", &failures[..failures.len().min(5)]);
    assert!(accepted * 100 >= total * 97, "{accepted} of {total} accepted: {:?}", &failures[..failures.len().min(8)]);
}

/// The same mesh against itself, and against a copy of itself moved by whole grid steps: every
/// face is coplanar with another. Closed with the right volume, or a reported error.
#[test]
fn a_mesh_against_itself_or_a_copy_never_leaves_an_open_surface() {
    let shapes = [
        cube(Vec3::splat(2.0), Vec3::ZERO),
        ball(1.0, Vec3::ZERO),
        placed(Primitive::Torus { radius: 1.0, tube: 0.35, segments: 24, sides: 12 }, Vec3::ZERO, Quat::IDENTITY),
        placed(Primitive::Cube { size: Vec3::splat(2.0), segments: 3 }, Vec3::ZERO, Quat::IDENTITY),
    ];
    for a in &shapes {
        for at in [Vec3::ZERO, Vec3::new(0.5, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.5, 0.5, 0.5)] {
            let mut b = a.clone();
            b.map_positions(|_, p| p + at);
            let (va, vb) = (vol(a), vol(&b));
            for op in [BoolOp::Union, BoolOp::Subtract, BoolOp::Intersect] {
                match combine(a, &b, op) {
                    Ok(m) => {
                        assert!(closed(&m), "{op:?} at {at}");
                        m.validate().unwrap();
                        let v = vol(&m);
                        // Between nothing and both together, whatever the overlap is.
                        assert!(v >= -1e-3 && v <= va + vb + 1e-3, "{op:?} at {at}: {v}");
                    }
                    Err(e) => assert!(!e.to_string().is_empty()),
                }
            }
            if at != Vec3::ZERO {
                let u = combine(a, &b, BoolOp::Union).unwrap();
                let i = combine(a, &b, BoolOp::Intersect).unwrap();
                assert!((vol(&u) + vol(&i) - va - vb).abs() < 1e-3 * (va + vb), "at {at}: {} + {} vs {va} + {vb}", vol(&u), vol(&i));
            }
        }
    }
}

#[test]
fn a_torus_through_a_sphere_makes_a_genus_one_cut() {
    let torus = placed(Primitive::Torus { radius: 1.0, tube: 0.4, segments: 32, sides: 16 }, Vec3::ZERO, Quat::IDENTITY);
    let b = ball(1.0, Vec3::ZERO);
    let (i, u, s) =
        (combine(&torus, &b, BoolOp::Intersect).unwrap(), combine(&torus, &b, BoolOp::Union).unwrap(), combine(&torus, &b, BoolOp::Subtract).unwrap());
    for m in [&i, &u, &s] {
        assert!(closed(m));
        m.validate().unwrap();
    }
    let (vt, vb) = (vol(&torus), vol(&b));
    assert!((vol(&u) + vol(&i) - vt - vb).abs() < 2e-3 * (vt + vb));
    assert!((vol(&s) - (vt - vol(&i))).abs() < 2e-3 * vt);
}

#[test]
fn big_meshes_finish_quickly_and_report_their_work() {
    let a = placed(Primitive::Sphere { radius: 1.0, segments: 128, rings: 64 }, Vec3::ZERO, Quat::IDENTITY);
    let b = placed(Primitive::Sphere { radius: 1.0, segments: 128, rings: 64 }, Vec3::new(0.7, 0.2, 0.1), Quat::from_rotation_x(0.3));
    let start = std::time::Instant::now();
    let (m, stats) = combine_with(&a, &b, BoolOp::Subtract, &Limits::default()).unwrap();
    let took = start.elapsed();
    eprintln!("boolean of two {}-triangle spheres: {took:?}, {stats:?}, {} result faces", to_tris(&a).tris.len(), m.face_count());
    assert!(closed(&m));
    m.validate().unwrap();
    assert!(took < std::time::Duration::from_secs(20), "{took:?}");
}

/// The timings in `docs/performance.md`: two spheres of 4 thousand triangles are checked on every
/// run; set `MODELCRAFT_BOOLEAN_BENCH=1` to also run the 16, 65 and 261 thousand triangle sizes and
/// print every row (`cargo test -p modelcraft-ops boolean_timings -- --nocapture`).
#[test]
fn boolean_timings() {
    let sizes: &[(u32, u32)] =
        if std::env::var_os("MODELCRAFT_BOOLEAN_BENCH").is_some() { &[(64, 32), (128, 64), (256, 128), (512, 256)] } else { &[(64, 32)] };
    for &(segs, rings) in sizes {
        let a = placed(Primitive::Sphere { radius: 1.0, segments: segs, rings }, Vec3::ZERO, Quat::IDENTITY);
        let b = placed(Primitive::Sphere { radius: 1.0, segments: segs, rings }, Vec3::new(0.7, 0.2, 0.1), Quat::from_rotation_x(0.3));
        let tris = to_tris(&a).tris.len();
        let start = std::time::Instant::now();
        let (m, stats) = combine_with(&a, &b, BoolOp::Union, &Limits::default()).unwrap();
        eprintln!(
            "{tris} triangles each: {:?} total ({} crossings, {} pairs tested, {} result faces)",
            start.elapsed(),
            stats.crossings,
            stats.triangle_pairs,
            m.face_count()
        );
        assert!(closed(&m));
        assert!(stats.triangle_pairs < tris, "only the part that touches is examined");
    }
}
