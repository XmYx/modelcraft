use super::*;
use glam::Vec3;
use modelcraft_ops::{edit, primitives::Primitive};
use proptest::prelude::*;

fn plane(n: u32) -> PolyMesh {
    Primitive::Plane { width: 4.0, depth: 2.0, segments: n }.build().unwrap()
}

fn welded(p: Primitive) -> PolyMesh {
    edit::weld(&p.build().unwrap(), 1e-4).unwrap()
}

fn pole(m: &PolyMesh, top: bool) -> u32 {
    let key = |v: &u32| if top { m.position(*v).y } else { -m.position(*v).y };
    (0..m.vertex_count() as u32).max_by(|a, b| key(a).total_cmp(&key(b))).unwrap()
}

fn euler(m: &PolyMesh) -> i64 {
    m.vertex_count() as i64 - m.edge_count() as i64 + m.face_count() as i64
}

/// Axis-aligned UV bounds of the faces of each island.
fn island_bounds(m: &PolyMesh, charts: &Charts) -> Vec<(glam::Vec2, glam::Vec2)> {
    charts
        .islands
        .iter()
        .map(|i| {
            let (mut lo, mut hi) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
            for f in &i.faces {
                for h in m.face_half_edges(*f) {
                    lo = lo.min(m.uv(h));
                    hi = hi.max(m.uv(h));
                }
            }
            (lo, hi)
        })
        .collect()
}

#[test]
fn plane_unwraps_to_an_isometry() {
    let m = plane(8);
    let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &UnwrapOptions::default()).unwrap();
    assert_eq!(res.islands.len(), 1);
    assert!(res.islands[0].converged && !res.islands[0].fallback);
    let (d, _) = measure(&out);
    eprintln!("plane 8x8:     {d:?}");
    assert!(d.max_angle_error_deg < 0.01, "{d:?}");
    assert!(d.area_rms_log < 1e-4, "{d:?}");
    assert_eq!(d.flipped, 0);
    // The island fills the unit square's width: the plane is twice as wide as deep.
    let (lo, hi) = island_bounds(&out, &Charts::build(&out, &SeamSet::new()).unwrap())[0];
    assert!(lo.min_element() >= 0.0 && hi.max_element() <= 1.0);
    assert!(((hi.x - lo.x) / (hi.y - lo.y) - 2.0).abs() < 1e-3);
}

#[test]
fn cube_by_angle_gives_six_undistorted_islands() {
    let m = Primitive::Cube { size: Vec3::splat(2.0), segments: 2 }.build().unwrap();
    let m = edit::weld(&m, 1e-4).unwrap();
    let mut seams = SeamSet::new();
    let n = seams.mark_by_angle(&m, 30.0);
    assert_eq!(n, 12 * 2, "12 cube edges, two segments each");
    let (out, res) = unwrap_mesh(&m, &seams, &UnwrapOptions::default()).unwrap();
    assert_eq!(res.islands.len(), 6);
    assert_eq!(res.cut.edges_added, 0, "every face is already a disc");
    let (d, _) = measure(&out);
    eprintln!("cube by angle: {d:?}");
    assert!(d.max_angle_error_deg < 0.01 && d.area_rms_log < 1e-4 && d.flipped == 0, "{d:?}");
    // Islands do not overlap and stay inside the unit square.
    let charts = Charts::build(&out, &res.seams).unwrap();
    let b = island_bounds(&out, &charts);
    for (i, (lo, hi)) in b.iter().enumerate() {
        assert!(lo.min_element() >= 0.0 && hi.max_element() <= 1.0 + 1e-6, "island {i}: {lo} {hi}");
        for (lo2, hi2) in b.iter().skip(i + 1) {
            assert!(hi.x <= lo2.x || hi2.x <= lo.x || hi.y <= lo2.y || hi2.y <= lo.y, "islands overlap");
        }
    }
}

#[test]
fn cylinder_with_a_seam_unrolls_exactly() {
    let m = welded(Primitive::Cylinder { radius: 1.0, height: 2.0, sides: 24, rings: 4, caps: false });
    assert_eq!(euler(&m), 0, "an open tube is an annulus");
    // The seam: the straight line down one side.
    let mut seams = SeamSet::new();
    let side: Vec<u32> = (0..m.vertex_count() as u32).filter(|v| (m.position(*v).z + 1.0).abs() < 1e-3 && m.position(*v).x.abs() < 1e-3).collect();
    let (top, bottom) = (
        side.iter().copied().max_by(|a, b| m.position(*a).y.total_cmp(&m.position(*b).y)).unwrap(),
        side.iter().copied().min_by(|a, b| m.position(*a).y.total_cmp(&m.position(*b).y)).unwrap(),
    );
    assert!(seams.mark_path(&m, top, bottom).unwrap() >= 4);
    let charts = Charts::build(&m, &seams).unwrap();
    assert!(charts.islands.iter().all(Island::is_disc));
    let (out, res) = unwrap_mesh(&m, &seams, &UnwrapOptions::default()).unwrap();
    assert_eq!(res.cut.edges_added, 0);
    let (d, _) = measure(&out);
    eprintln!("cylinder:      {d:?}");
    assert!(d.max_angle_error_deg < 0.05 && d.area_rms_log < 1e-3 && d.flipped == 0, "{d:?}");
}

#[test]
fn closed_tube_without_a_seam_is_cut_automatically() {
    let m = welded(Primitive::Cylinder { radius: 1.0, height: 2.0, sides: 16, rings: 3, caps: false });
    let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &UnwrapOptions::default()).unwrap();
    assert!(res.cut.edges_added >= 3, "a path joins the two borders: {:?}", res.cut);
    assert_eq!(res.islands.len(), 1);
    let (d, _) = measure(&out);
    eprintln!("tube auto cut: {d:?}");
    assert!(d.max_angle_error_deg < 0.05 && d.flipped == 0, "{d:?}");
}

#[test]
fn sphere_cut_along_a_meridian_is_conformal() {
    let m = welded(Primitive::Sphere { radius: 1.0, segments: 32, rings: 16 });
    assert_eq!(euler(&m), 2);
    let mut seams = SeamSet::new();
    seams.mark_path(&m, pole(&m, true), pole(&m, false)).unwrap();
    let (out, res) = unwrap_mesh(&m, &seams, &UnwrapOptions::default()).unwrap();
    assert_eq!(res.islands.len(), 1);
    assert!(res.islands[0].disc && res.islands[0].converged, "{:?}", res.islands[0]);
    let (d, _) = measure(&out);
    eprintln!("sphere meridian: {d:?}");
    // LSCM is conformal, not isometric: angles stay close, area cannot.
    assert!(d.mean_angle_error_deg < 6.0, "{d:?}");
    assert!(d.flipped == 0, "{d:?}");
    assert!(d.area_rms_log > 0.05, "a sphere cannot be flattened without area distortion: {d:?}");
}

#[test]
fn sphere_without_seams_gets_a_cut_path() {
    let m = welded(Primitive::Sphere { radius: 1.0, segments: 24, rings: 12 });
    let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &UnwrapOptions::default()).unwrap();
    assert!(res.cut.edges_added > 0);
    assert_eq!(res.islands.len(), 1);
    assert!(res.islands[0].disc);
    let (d, _) = measure(&out);
    eprintln!("sphere auto:     {d:?}");
    assert!(d.mean_angle_error_deg < 6.0 && d.flipped == 0, "{d:?}");
}

#[test]
fn torus_is_cut_into_a_disc() {
    let m = welded(Primitive::Torus { radius: 1.0, tube: 0.35, segments: 24, sides: 12 });
    assert_eq!(euler(&m), 0);
    let charts = Charts::build(&m, &SeamSet::new()).unwrap();
    assert_eq!(charts.islands.len(), 1);
    assert!(!charts.islands[0].is_disc());
    let (cut, rep) = cut_to_disks(&m, &SeamSet::new()).unwrap();
    assert_eq!(rep.islands_cut, 1);
    let after = Charts::build(&m, &cut).unwrap();
    assert!(after.islands.iter().all(Island::is_disc), "{:?}", after.islands.iter().map(|i| (i.euler, i.boundary_loops)).collect::<Vec<_>>());
    let (out, _) = unwrap_mesh(&m, &SeamSet::new(), &UnwrapOptions::default()).unwrap();
    let (d, _) = measure(&out);
    eprintln!("torus auto cut:  {d:?}");
    assert!(d.mean_angle_error_deg.is_finite());
}

#[test]
fn closed_cube_is_cut_open_without_seams() {
    let m = welded(Primitive::Cube { size: Vec3::splat(2.0), segments: 1 });
    let (_, res) = unwrap_mesh(&m, &SeamSet::new(), &UnwrapOptions::default()).unwrap();
    assert_eq!(res.islands.len(), 1);
    assert!(res.islands[0].disc);
    assert!(res.cut.edges_added >= 1);
}

#[test]
fn pinned_vertices_stay_put() {
    let m = plane(4);
    let corner_at = |x: f32, z: f32| (0..m.corner_count() as u32).find(|c| (m.position(m.origin(*c)) - Vec3::new(x, 0.0, z)).length() < 1e-4).unwrap();
    let pins =
        vec![Pin { corner: corner_at(-2.0, -1.0), uv: glam::Vec2::new(0.25, 0.5) }, Pin { corner: corner_at(2.0, -1.0), uv: glam::Vec2::new(0.25, 1.5) }];
    let opts = UnwrapOptions { pins: pins.clone(), ..UnwrapOptions::default() };
    let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &opts).unwrap();
    assert!(res.islands[0].pinned);
    for p in &pins {
        assert!((out.uv(p.corner) - p.uv).length() < 1e-5, "pin moved to {}", out.uv(p.corner));
    }
    // Pinned across the 4-unit width: the map is a rotation by 90 degrees and a scale of 0.25.
    let (d, _) = measure(&out);
    assert!(d.max_angle_error_deg < 0.01 && d.flipped == 0, "{d:?}");
}

#[test]
fn single_pin_keeps_that_vertex() {
    let m = plane(4);
    let c = 7;
    let pin = Pin { corner: c, uv: glam::Vec2::new(3.0, -2.0) };
    let opts = UnwrapOptions { pins: vec![pin], ..UnwrapOptions::default() };
    let (out, _) = unwrap_mesh(&m, &SeamSet::new(), &opts).unwrap();
    assert!((out.uv(c) - pin.uv).length() < 1e-5);
    let (d, _) = measure(&out);
    assert!(d.max_angle_error_deg < 0.01, "{d:?}");
}

#[test]
fn seam_set_marks_clears_and_round_trips() {
    let m = plane(2);
    let mut s = SeamSet::new();
    assert!(s.mark(&m, 0));
    assert!(!s.mark(&m, 0), "already marked");
    assert!(s.contains(&m, 0) && !s.contains(&m, 1));
    assert!(!s.mark(&m, 999), "no such edge");
    assert!(s.toggle_edges(&m, &[0, 1]), "one of two is unmarked: mark both");
    assert_eq!(s.len(), 2);
    assert!(!s.toggle_edges(&m, &[0, 1]), "both marked: clear both");
    assert!(s.is_empty());
    s.mark_edges(&m, &[2, 3, 3]);
    let pairs = s.to_vertex_pairs();
    assert_eq!(SeamSet::from_vertex_pairs(&pairs), s);
    assert_eq!(s.edge_ids(&m), vec![2, 3]);
    let json = serde_json::to_string(&s).unwrap();
    assert_eq!(serde_json::from_str::<SeamSet>(&json).unwrap(), s);
    // Pairs that are not edges of the mesh are dropped.
    let mut bad = SeamSet::from_vertex_pairs(&[[0, 8], [0, 1], [0, 0], [500, 1]]);
    assert_eq!(bad.len(), 3);
    assert_eq!(bad.retain_existing(&m), 2);
    assert!(s.clear(&m, 2) && !s.clear(&m, 2));
    s.clear_all();
    assert!(s.is_empty());
}

#[test]
fn seams_from_existing_uvs() {
    // A cube primitive has a UV island per face.
    let m = Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap();
    let s = SeamSet::from_uv_discontinuities(&m);
    let welded = edit::weld(&m, 1e-4).unwrap();
    assert_eq!(welded.edge_count(), 12);
    let charts = Charts::build(&m, &s).unwrap();
    assert_eq!(charts.islands.len(), 6);
    assert!(SeamSet::from_uv_discontinuities(&PolyMesh::from_faces(vec![Vec3::ZERO, Vec3::X, Vec3::Y], &[vec![0, 1, 2]]).unwrap()).is_empty());
}

#[test]
fn bow_tie_vertex_gives_two_uv_vertices() {
    // Two triangles meeting only at the origin.
    let pos = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::NEG_X, Vec3::NEG_Y];
    let m = PolyMesh::from_faces(pos, &[vec![0, 1, 2], vec![0, 3, 4]]).unwrap();
    let c = Charts::build(&m, &SeamSet::new()).unwrap();
    assert_eq!(c.islands.len(), 2);
    assert_eq!(c.vertex_source.iter().filter(|v| **v == 0).count(), 2);
    assert!(c.islands.iter().all(Island::is_disc));
}

#[test]
fn shelf_layout_fits_without_overlap() {
    let sizes: Vec<glam::DVec2> =
        vec![glam::DVec2::new(3.0, 1.0), glam::DVec2::new(1.0, 1.0), glam::DVec2::new(2.0, 2.0), glam::DVec2::new(0.5, 4.0), glam::DVec2::new(1.0, 0.2)];
    let (places, s) = shelf(&sizes, 0.01);
    assert!(s > 0.0);
    for (i, (p, z)) in places.iter().zip(&sizes).enumerate() {
        assert!(p.x >= 0.01 - 1e-9 && p.y >= 0.01 - 1e-9 && p.x + z.x * s <= 0.99 + 1e-9 && p.y + z.y * s <= 0.99 + 1e-9, "{i}");
        for (q, w) in places.iter().zip(&sizes).skip(i + 1) {
            assert!(p.x + z.x * s <= q.x || q.x + w.x * s <= p.x || p.y + z.y * s <= q.y || q.y + w.y * s <= p.y, "overlap");
        }
    }
    assert_eq!(shelf(&[], 0.01).0.len(), 0);
}

#[test]
fn unwrap_rejects_an_empty_mesh_and_a_mesh_without_uv_measures_zero() {
    assert_eq!(unwrap(&PolyMesh::default(), &SeamSet::new(), &UnwrapOptions::default()).unwrap_err(), UvError::NoFaces);
    let m = PolyMesh::from_faces(vec![Vec3::ZERO, Vec3::X, Vec3::Y], &[vec![0, 1, 2]]).unwrap();
    assert_eq!(measure(&m).0, Distortion::default());
}

#[test]
fn cg_solves_a_small_system() {
    use crate::sparse::{BlockCsr, solve_cg};
    // Two coupled vertices: [[4,1],[1,3]] blocks of the identity pattern.
    let rows = vec![vec![0, 1], vec![0, 1]];
    let mut a = BlockCsr::from_pattern(&rows).unwrap();
    a.add(0, 0, [4.0, 0.0, 0.0, 4.0]);
    a.add(0, 1, [1.0, 0.0, 0.0, 1.0]);
    a.add(1, 0, [1.0, 0.0, 0.0, 1.0]);
    a.add(1, 1, [3.0, 0.0, 0.0, 3.0]);
    let b = [1.0, 2.0, 3.0, 4.0];
    let mut x = [0.0; 4];
    let r = solve_cg(&a, &b, &mut x, 1e-12, 50);
    assert!(r.converged, "{r:?}");
    let mut y = [0.0; 4];
    a.mul(&x, &mut y);
    for (p, q) in y.iter().zip(b) {
        assert!((p - q).abs() < 1e-9);
    }
}

proptest! {
    #[test]
    fn perturbed_grids_unwrap_to_finite_unflipped_uvs(n in 2u32..7, seed in 0u32..1000, amp in 0.0f32..0.15) {
        // A grid whose vertices are pushed around a little in all three axes.
        let base = Primitive::Plane { width: 3.0, depth: 3.0, segments: n }.build().unwrap();
        let mut m = base.clone();
        m.map_positions(|i, p| {
            let h = |k: u32| ((i.wrapping_mul(2654435761).wrapping_add(seed.wrapping_mul(40503)).wrapping_add(k.wrapping_mul(97))) % 1000) as f32 / 1000.0 - 0.5;
            p + Vec3::new(h(1), h(2), h(3)) * amp
        });
        let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &UnwrapOptions::default()).unwrap();
        prop_assert!(out.corner_uvs().iter().all(|u| u.is_finite() && u.min_element() >= -1e-6 && u.max_element() <= 1.0 + 1e-6));
        prop_assert!(res.islands.iter().all(|i| !i.fallback));
        let (d, _) = measure(&out);
        prop_assert_eq!(d.flipped, 0);
        prop_assert!(d.mean_angle_error_deg < 12.0, "{:?}", d);
    }

    #[test]
    fn random_seams_never_break_the_unwrap(marks in proptest::collection::vec(0u32..400, 0..40)) {
        let m = welded(Primitive::Sphere { radius: 1.0, segments: 8, rings: 6 });
        let mut s = SeamSet::new();
        s.mark_edges(&m, &marks);
        let (out, res) = unwrap_mesh(&m, &s, &UnwrapOptions::default()).unwrap();
        prop_assert!(out.corner_uvs().iter().all(|u| u.is_finite()));
        let charts = Charts::build(&m, &res.seams).unwrap();
        prop_assert!(charts.islands.iter().all(Island::is_disc), "{:?}", charts.islands.iter().map(|i| (i.euler, i.boundary_loops)).collect::<Vec<_>>());
        let faces: usize = charts.islands.iter().map(|i| i.faces.len()).sum();
        prop_assert_eq!(faces, m.face_count());
    }
}

#[test]
fn measure_agrees_with_hand_computed_distortion() {
    // A square plane, whose primitive UVs are isometric up to scale.
    let m = Primitive::Plane { width: 2.0, depth: 2.0, segments: 4 }.build().unwrap();
    let base = measure(&m).0;
    assert!(base.max_stretch < 1.0001 && base.flipped == 0, "{base:?}");
    // Stretch u by 2: every triangle has stretch exactly 2, the same area scale everywhere, and
    // the right angle of each quad corner becomes 2·atan(1/2)-type angles, so the angle error is not zero.
    let mut stretched = m.clone();
    let uvs: Vec<glam::Vec2> = (0..m.corner_count() as u32).map(|c| m.uv(c) * glam::Vec2::new(2.0, 1.0)).collect();
    stretched.set_uvs(&uvs).unwrap();
    let (d, per_tri) = measure(&stretched);
    assert_eq!(d.triangles, per_tri.len());
    assert!((d.mean_stretch - 2.0).abs() < 1e-5 && (d.max_stretch - 2.0).abs() < 1e-5, "{d:?}");
    assert!(d.area_rms_log < 1e-6 && d.flipped == 0, "{d:?}");
    // A 45-45-90 triangle with legs scaled 2:1 has its 45 degree corners at atan(1/2) = 26.565 and
    // atan(2) = 63.435 degrees, an error of 18.435 degrees.
    assert!((d.max_angle_error_deg - 18.4349).abs() < 0.01, "{d:?}");
    // Mirrored UVs are all flipped.
    let mirrored: Vec<glam::Vec2> = (0..m.corner_count() as u32).map(|c| m.uv(c) * glam::Vec2::new(-1.0, 1.0)).collect();
    let mut mir = m.clone();
    mir.set_uvs(&mirrored).unwrap();
    assert_eq!(measure(&mir).0.flipped, measure(&mir).0.triangles);
    // Uneven areas: scale one corner of UV space up.
    let uneven: Vec<glam::Vec2> = (0..m.corner_count() as u32).map(|c| if m.uv(c).x > 0.5 { m.uv(c) * 3.0 } else { m.uv(c) }).collect();
    let mut un = m.clone();
    un.set_uvs(&uneven).unwrap();
    assert!(measure(&un).0.area_rms_log > 0.1);
    // Face averages.
    let pf = per_face(&stretched, &per_tri);
    assert_eq!(pf.len(), stretched.face_count());
    assert!(pf.iter().all(|f| (f.stretch - 2.0).abs() < 1e-4));
}

#[test]
fn mesh_uv_accessors_validate_their_input() {
    let mut m = plane(2);
    assert_eq!(m.corner_count(), 16);
    assert_eq!(m.corner_uvs().len(), 16);
    assert!(m.set_uvs(&[glam::Vec2::ZERO; 3]).is_err());
    let mut bad = vec![glam::Vec2::ZERO; 16];
    bad[3] = glam::Vec2::new(f32::NAN, 0.0);
    assert!(m.set_uvs(&bad).is_err());
    assert_eq!(m.corner_uvs().len(), 16, "unchanged after a rejected write");
    m.clear_uvs();
    assert!(!m.has_uvs() && m.corner_uvs().is_empty());
    m.set_uvs(&[glam::Vec2::ONE; 16]).unwrap();
    m.validate().unwrap();
    // UVs survive a polygon round trip.
    let back = PolyMesh::build(m.positions().to_vec(), m.to_polygons()).unwrap().0;
    assert_eq!(back.corner_uvs(), m.corner_uvs());
}

/// A random symmetric positive-definite block matrix on a ring-with-chords graph.
fn random_spd(n: usize, seed: u64) -> (crate::sparse::BlockCsr, Vec<Vec<u32>>) {
    use crate::sparse::BlockCsr;
    let mut s = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let mut rnd = move || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((s >> 33) as f64 / (1u64 << 31) as f64) - 0.5
    };
    let mut rows: Vec<Vec<u32>> = (0..n).map(|i| vec![i as u32, ((i + 1) % n) as u32, ((i + n - 1) % n) as u32, ((i * 7 + 3) % n) as u32]).collect();
    // Symmetrise the pattern.
    let snapshot = rows.clone();
    for (i, r) in snapshot.iter().enumerate() {
        for j in r {
            rows[*j as usize].push(i as u32);
        }
    }
    for r in &mut rows {
        r.sort_unstable();
        r.dedup();
    }
    let mut a = BlockCsr::from_pattern(&rows).unwrap();
    // A = B Bᵀ-like: random off-diagonal blocks, strictly dominant diagonal blocks.
    for i in 0..n {
        let mut off = 0.0;
        for &j in &rows[i] {
            let j = j as usize;
            if j > i {
                let b = [rnd(), rnd(), rnd(), rnd()];
                a.add(i, j as u32, b);
                a.add(j, i as u32, [b[0], b[2], b[1], b[3]]);
                off += 4.0;
            }
        }
        a.add(i, i as u32, [off + 8.0, 0.1, 0.1, off + 8.0]);
    }
    (a, rows)
}

#[test]
fn cholesky_matches_conjugate_gradients_on_random_spd_systems() {
    use crate::cholesky::{Factor, nested_dissection};
    use crate::sparse::solve_cg;
    for (n, seed) in [(3usize, 1u64), (40, 2), (300, 3)] {
        let (a, rows) = random_spd(n, seed);
        let coords: Vec<[f64; 3]> = (0..n).map(|i| [(i as f64).sin() * 10.0, (i as f64).cos() * 10.0, i as f64 * 0.1]).collect();
        let order = nested_dissection(&rows, &coords);
        let mut sorted = order.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..n).collect::<Vec<_>>(), "the ordering is a permutation");
        let f = Factor::new(&a, &order, 1_000_000, 0.0).unwrap();
        let b: Vec<f64> = (0..2 * n).map(|i| (i as f64 * 0.37).sin()).collect();
        let mut x = vec![0.0; 2 * n];
        f.solve(&b, &mut x);
        let mut r = vec![0.0; 2 * n];
        a.mul(&x, &mut r);
        let err = r.iter().zip(&b).map(|(p, q)| (p - q).abs()).fold(0.0, f64::max);
        assert!(err < 1e-9, "n={n}: residual {err}");
        let mut y = vec![0.0; 2 * n];
        assert!(solve_cg(&a, &b, &mut y, 1e-13, 5000).converged);
        assert!(x.iter().zip(&y).all(|(p, q)| (p - q).abs() < 1e-7));
        // The size limit is honoured, and an indefinite matrix is refused rather than trusted.
        assert!(Factor::new(&a, &order, 1, 0.0).is_none());
    }
    let (mut a, rows) = random_spd(10, 9);
    a.add(0, 0, [-1e6, 0.0, 0.0, -1e6]);
    let order = nested_dissection(&rows, &vec![[0.0; 3]; 10]);
    assert!(Factor::new(&a, &order, 1_000_000, 0.0).is_none());
}

#[test]
fn a_large_sphere_unwraps_without_flips() {
    // 64x32 stalled conjugate gradients (residual 1e-5 after 8000 iterations); the direct solver
    // does not care how badly conditioned the system is.
    let m = welded(Primitive::Sphere { radius: 1.0, segments: 64, rings: 32 });
    let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &UnwrapOptions::default()).unwrap();
    let isl = &res.islands[0];
    assert!(isl.direct && isl.converged && isl.residual < 1e-9, "{isl:?}");
    let (d, _) = measure(&out);
    assert_eq!(d.flipped, 0, "{d:?}");
    assert!(d.mean_angle_error_deg < 2.0 && d.max_angle_error_deg < 10.0, "{d:?}");
}

proptest! {
    #[test]
    fn random_polygon_soup_never_panics(
        pts in proptest::collection::vec((-2.0f32..2.0, -2.0f32..2.0, -2.0f32..2.0), 4..14),
        faces in proptest::collection::vec(proptest::collection::vec(0u32..16, 3..6), 1..24),
        angle in proptest::option::of(0.0f32..180.0),
    ) {
        let pos: Vec<Vec3> = pts.iter().map(|p| Vec3::new(p.0, p.1, p.2)).collect();
        let (m, _) = PolyMesh::build(pos, faces.into_iter().map(modelcraft_mesh::Polygon::new).collect()).unwrap_or_default();
        let mut seams = SeamSet::new();
        if let Some(a) = angle {
            seams.mark_by_angle(&m, a);
        }
        match unwrap_mesh(&m, &seams, &UnwrapOptions::default()) {
            Ok((out, _)) => {
                prop_assert!(out.corner_uvs().iter().all(|u| u.is_finite()));
                let _ = measure(&out);
            }
            Err(e) => prop_assert_eq!(e, UvError::NoFaces),
        }
    }
}

#[test]
fn concave_polygons_are_triangulated_without_folds() {
    // A "C": the fan from its first corner folds over the notch.
    let pts = [(0.0, 0.0), (3.0, 0.0), (3.0, 1.0), (1.0, 1.0), (1.0, 2.0), (3.0, 2.0), (3.0, 3.0), (0.0, 3.0)];
    let pos: Vec<Vec3> = pts.iter().map(|(x, z)| Vec3::new(*x, 0.0, *z)).collect();
    let m = PolyMesh::from_faces(pos, &[(0..8).collect()]).unwrap();
    let mut tris = Vec::new();
    face_triangles(&m, 0, &mut tris);
    assert_eq!(tris.len(), 6);
    let normal = m.face_normal(0);
    let mut area = 0.0;
    for t in &tris {
        let p = t.map(|c| m.position(m.origin(c)));
        let cross = (p[1] - p[0]).cross(p[2] - p[0]);
        assert!(cross.dot(normal) > 0.0, "a triangle folds: {t:?}");
        area += 0.5 * cross.length();
    }
    assert!((area - 7.0).abs() < 1e-4, "{area}");
    // Quads: the shorter diagonal, or the only one inside a concave (dart) quad.
    let dart =
        PolyMesh::from_faces(vec![Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 4.0)], &[vec![0, 1, 2, 3]]).unwrap();
    let mut t = Vec::new();
    face_triangles(&dart, 0, &mut t);
    let n = dart.face_normal(0);
    assert_eq!(t.len(), 2);
    assert!(t.iter().all(|t| {
        let p = t.map(|c| dart.position(dart.origin(c)));
        (p[1] - p[0]).cross(p[2] - p[0]).dot(n) > 0.0
    }));
}

#[test]
fn a_pin_the_solver_drops_does_not_fix_the_island() {
    // A grid with a zero-area sliver on its bottom edge: the sliver's third corner touches no
    // usable triangle, so a pin on it is dropped and the island must still be scaled and laid out.
    let n = 3u32;
    let mut pos = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            pos.push(Vec3::new(i as f32 * 5.0, 0.0, j as f32 * 5.0));
        }
    }
    let mut faces = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            faces.push(vec![a, a + n + 1, a + n + 2, a + 1]);
        }
    }
    let c = pos.len() as u32;
    pos.push(Vec3::new(2.5, 0.0, 0.0));
    faces.push(vec![0, 1, c]);
    let m = PolyMesh::from_faces(pos, &faces).unwrap();
    let corner = (0..m.corner_count() as u32).find(|h| m.origin(*h) == c).unwrap();
    let opts = UnwrapOptions { pins: vec![Pin { corner, uv: glam::Vec2::new(40.0, 40.0) }], ..UnwrapOptions::default() };
    let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &opts).unwrap();
    assert!(!res.islands[0].pinned, "{:?}", res.islands[0]);
    assert!(out.corner_uvs().iter().all(|u| u.min_element() >= -1e-6 && u.max_element() <= 1.0 + 1e-6), "the island was laid out in the unit square");
    // Two pins at the same UV keep only one: the island is fixed by it and keeps that position.
    let a = (0..m.corner_count() as u32).find(|h| m.origin(*h) == 5).unwrap();
    let b = (0..m.corner_count() as u32).find(|h| m.origin(*h) == 6).unwrap();
    let opts = UnwrapOptions {
        pins: vec![Pin { corner: a, uv: glam::Vec2::new(7.0, 7.0) }, Pin { corner: b, uv: glam::Vec2::new(7.0, 7.0) }],
        ..UnwrapOptions::default()
    };
    let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &opts).unwrap();
    assert!(res.islands[0].pinned);
    assert!((out.uv(a) - glam::Vec2::new(7.0, 7.0)).length() < 1e-4 || (out.uv(b) - glam::Vec2::new(7.0, 7.0)).length() < 1e-4);
}

fn cube_islands() -> (PolyMesh, Charts) {
    let m = Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap();
    let c = Charts::build(&m, &SeamSet::from_uv_discontinuities(&m)).unwrap();
    (m, c)
}

#[test]
fn transform_moves_exactly_the_listed_corners() {
    let (m, charts) = cube_islands();
    let corners = island_corners(&m, &charts, 0);
    assert_eq!(corners.len(), 4, "one cube face");
    let t = UvTransform { translate: glam::Vec2::new(0.25, -0.5), ..UvTransform::default() };
    let out = transform_corners(&m, &corners, &t).unwrap();
    for c in 0..m.corner_count() as u32 {
        let want = if corners.contains(&c) { m.uv(c) + glam::Vec2::new(0.25, -0.5) } else { m.uv(c) };
        assert!((out.uv(c) - want).length() < 1e-6, "corner {c}");
    }
    // Rotating a quarter turn about the island's centre keeps its shape; scaling by 2 doubles its size.
    let (lo, hi) = bounds(&m, &corners.iter().copied().collect()).unwrap();
    let pivot = 0.5 * (lo + hi);
    let rot = transform_corners(&m, &corners, &UvTransform { rotate_deg: 90.0, pivot, ..UvTransform::default() }).unwrap();
    let (rlo, rhi) = bounds(&rot, &corners.iter().copied().collect()).unwrap();
    // A quarter turn swaps the width and the height and keeps the centre.
    assert!(((rhi - rlo) - glam::Vec2::new((hi - lo).y, (hi - lo).x)).length() < 1e-5 && ((0.5 * (rlo + rhi)) - pivot).length() < 1e-5);
    assert!(corners.iter().any(|c| (rot.uv(*c) - m.uv(*c)).length() > 0.01));
    // A half turn keeps the bounds and moves every corner to the opposite one.
    let half = transform_corners(&m, &corners, &UvTransform { rotate_deg: 180.0, pivot, ..UvTransform::default() }).unwrap();
    assert!(corners.iter().all(|c| (half.uv(*c) - (2.0 * pivot - m.uv(*c))).length() < 1e-5));
    let big = transform_corners(&m, &corners, &UvTransform { scale: glam::Vec2::new(2.0, 2.0), pivot, ..UvTransform::default() }).unwrap();
    let (blo, bhi) = bounds(&big, &corners.iter().copied().collect()).unwrap();
    assert!(((bhi - blo) - (hi - lo) * 2.0).length() < 1e-5);
    // Mirroring (negative scale) flips the island in UV space.
    let flip = transform_corners(&m, &corners, &UvTransform { scale: glam::Vec2::new(-1.0, 1.0), pivot, ..UvTransform::default() }).unwrap();
    assert!(measure(&flip).0.flipped >= 2);
    // Bad input: no UVs, a non-finite transform, corners that do not exist.
    let mut bare = m.clone();
    bare.clear_uvs();
    assert_eq!(transform_corners(&bare, &[0], &t).unwrap_err(), UvError::NoUvs);
    assert!(matches!(transform_corners(&m, &[0], &UvTransform { rotate_deg: f32::NAN, ..UvTransform::default() }), Err(UvError::Invalid(_))));
    assert_eq!(transform_corners(&m, &[9999, 4_000_000_000], &t).unwrap().corner_uvs(), m.corner_uvs());
}

#[test]
fn stitching_welds_the_two_sides_of_an_edge() {
    let (m, _) = cube_islands();
    let (edge, _) = (0..m.edge_count() as u32).map(|e| (e, SeamSet::from_uv_discontinuities(&m))).find(|(e, s)| s.contains(&m, *e)).unwrap();
    let before = SeamSet::from_uv_discontinuities(&m).len();
    let out = stitch_edges(&m, &[edge]).unwrap();
    let after = SeamSet::from_uv_discontinuities(&out);
    assert!(!after.contains(&out, edge), "the edge is continuous now");
    assert_eq!(after.len(), before - 1, "no other seam was touched");
    // The welded corners sit at the centre of where they were.
    let h = m.edge_half_edge(edge);
    let t = m.twin(h);
    let want = 0.5 * (m.uv(h) + m.uv(m.next(t)));
    assert!((out.uv(h) - want).length() < 1e-5 && (out.uv(m.next(t)) - want).length() < 1e-5);
    // An edge that is already continuous, a missing one and a boundary one change nothing.
    let plane = plane(2);
    assert_eq!(stitch_edges(&plane, &[0, 1, 999]).unwrap().corner_uvs(), plane.corner_uvs());
    assert_eq!(stitch_edges(&m, &[]).unwrap().corner_uvs(), m.corner_uvs());
    // Welding every seam of a cube gives one island again (its UVs collapse onto each other).
    let all: Vec<u32> = SeamSet::from_uv_discontinuities(&m).edge_ids(&m);
    let welded = stitch_edges(&m, &all).unwrap();
    assert!(SeamSet::from_uv_discontinuities(&welded).is_empty());
    assert_eq!(Charts::build(&welded, &SeamSet::from_uv_discontinuities(&welded)).unwrap().islands.len(), 1);
    let mut bare = m.clone();
    bare.clear_uvs();
    assert_eq!(stitch_edges(&bare, &[0]).unwrap_err(), UvError::NoUvs);
}

#[test]
fn uv_vertex_and_island_selection_helpers() {
    let (m, charts) = cube_islands();
    // On a cube with split faces every UV vertex is one corner; on a welded plane an inner vertex has four.
    assert_eq!(uv_vertex_corners(&m, &charts, 0), vec![0]);
    let p = plane(2);
    let pc = Charts::build(&p, &SeamSet::from_uv_discontinuities(&p)).unwrap();
    let inner = (0..p.corner_count() as u32).find(|c| p.origin(*c) == 4).unwrap();
    assert_eq!(uv_vertex_corners(&p, &pc, inner).len(), 4);
    assert_eq!(island_corners(&p, &pc, inner).len(), p.corner_count());
    let set: std::collections::BTreeSet<u32> = [inner].into();
    assert_eq!(expand_to_uv_vertices(&p, &pc, &set).len(), 4);
    assert!(uv_vertex_corners(&p, &pc, 9999).is_empty() && island_corners(&p, &pc, 9999).is_empty());
    assert!(bounds(&p, &std::collections::BTreeSet::new()).is_none());
}

proptest! {
    #[test]
    fn random_transforms_and_stitches_never_break_the_mesh(
        corners in proptest::collection::vec(0u32..40, 0..30),
        edges in proptest::collection::vec(0u32..40, 0..20),
        rot in -720.0f32..720.0,
        sx in -3.0f32..3.0,
    ) {
        let m = Primitive::Cube { size: Vec3::ONE, segments: 1 }.build().unwrap();
        let t = UvTransform { translate: glam::Vec2::new(0.1, -0.2), rotate_deg: rot, scale: glam::Vec2::new(sx, 1.0), pivot: glam::Vec2::splat(0.5) };
        let a = transform_corners(&m, &corners, &t).unwrap();
        prop_assert!(a.corner_uvs().iter().all(|u| u.is_finite()));
        a.validate().unwrap();
        let b = stitch_edges(&a, &edges).unwrap();
        prop_assert!(b.corner_uvs().iter().all(|u| u.is_finite()));
        prop_assert_eq!(b.corner_count(), m.corner_count());
    }
}
