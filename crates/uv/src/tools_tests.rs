//! Tests of relax, straighten and stitch.

use super::*;
use glam::{Vec2, Vec3};
use modelcraft_ops::{edit, primitives::Primitive};
use proptest::prelude::*;

fn plane(n: u32) -> PolyMesh {
    let m = Primitive::Plane { width: 2.0, depth: 2.0, segments: n }.build().unwrap();
    // An isometric layout to start from.
    unwrap_mesh(&m, &SeamSet::new(), &UnwrapOptions::default()).unwrap().0
}

/// The corners of the vertices that are not on the border of the mesh.
fn interior_corners(m: &PolyMesh) -> Vec<u32> {
    let mut border = vec![false; m.vertex_count()];
    for h in 0..m.corner_count() as u32 {
        if m.is_boundary_edge(m.edge_of(h)) {
            border[m.origin(h) as usize] = true;
            border[m.origin(m.next(h)) as usize] = true;
        }
    }
    (0..m.corner_count() as u32).filter(|c| !border[m.origin(*c) as usize]).collect()
}

/// Perturb the UVs of the listed corners by a deterministic pseudo-random amount, the same for
/// every corner of one vertex.
fn jitter(m: &PolyMesh, corners: &[u32], amount: f32) -> PolyMesh {
    let mut uvs = m.corner_uvs().to_vec();
    for &c in corners {
        let v = m.origin(c);
        let h = (v.wrapping_mul(2_654_435_761)) as f32 / u32::MAX as f32;
        let g = (v.wrapping_mul(40_503).wrapping_add(7)) as f32 / u32::MAX as f32;
        uvs[c as usize] += Vec2::new(h - 0.5, g - 0.5) * amount;
    }
    let mut out = m.clone();
    out.set_uvs(&uvs).unwrap();
    out
}

#[test]
fn relax_restores_a_jittered_grid_to_an_isometry() {
    let flat = plane(8);
    let inner = interior_corners(&flat);
    assert!(!inner.is_empty());
    let bent = jitter(&flat, &inner, 0.03);
    let (d0, _) = measure(&bent);
    assert!(d0.max_angle_error_deg > 5.0, "the jitter must hurt: {d0:?}");
    let (out, rep) = relax_corners(&bent, &inner, &RelaxOptions { iterations: 60, mode: RelaxMode::Rigid }).unwrap();
    let (d1, _) = measure(&out);
    eprintln!("relax: {d0:?}\n   -> {d1:?}\n   {rep:?}");
    assert_eq!(rep.islands, 1);
    assert!(rep.energy_after < rep.energy_before * 0.02, "{rep:?}");
    assert!(d1.max_angle_error_deg < 1.0 && d1.flipped == 0, "{d1:?}");
    // What was not selected did not move.
    let moved: std::collections::BTreeSet<u32> = inner.iter().map(|c| m_origin(&flat, *c)).collect();
    for c in 0..flat.corner_count() as u32 {
        if !moved.contains(&m_origin(&flat, c)) {
            assert_eq!(out.uv(c), bent.uv(c), "corner {c} is on the border and stays");
        }
    }
}

fn m_origin(m: &PolyMesh, c: u32) -> u32 {
    m.origin(c)
}

#[test]
fn relaxing_a_whole_island_neither_drifts_nor_worsens_it() {
    let flat = plane(6);
    let (out, rep) = relax_corners(&flat, &[], &RelaxOptions::default()).unwrap();
    assert_eq!(rep.islands, 1);
    assert!(rep.energy_after <= rep.energy_before + 1e-9);
    let drift = (0..flat.corner_count() as u32).map(|c| (out.uv(c) - flat.uv(c)).length()).fold(0.0f32, f32::max);
    assert!(drift < 1e-4, "an isometric island stays where it is: {drift}");
    // A jittered whole island relaxes too, keeping its place and orientation about its centre.
    let bent = jitter(&flat, &(0..flat.corner_count() as u32).collect::<Vec<_>>(), 0.02);
    let (out, rep) = relax_corners(&bent, &[], &RelaxOptions { iterations: 40, mode: RelaxMode::Rigid }).unwrap();
    assert!(rep.energy_after < rep.energy_before * 0.5, "{rep:?}");
    let (d, _) = measure(&out);
    assert!(d.flipped == 0 && d.mean_angle_error_deg < measure(&bent).0.mean_angle_error_deg, "{d:?}");
    let centre = |m: &PolyMesh| (0..m.corner_count() as u32).map(|c| m.uv(c)).sum::<Vec2>() / m.corner_count() as f32;
    assert!((centre(&out) - centre(&bent)).length() < 0.05);
}

#[test]
fn relaxing_a_curved_island_trades_what_each_mode_promises() {
    let sphere = edit::weld(&Primitive::Sphere { radius: 1.0, segments: 24, rings: 12 }.build().unwrap(), 1e-4).unwrap();
    let mut seams = SeamSet::new();
    let top = (0..sphere.vertex_count() as u32).max_by(|a, b| sphere.position(*a).y.total_cmp(&sphere.position(*b).y)).unwrap();
    let bottom = (0..sphere.vertex_count() as u32).min_by(|a, b| sphere.position(*a).y.total_cmp(&sphere.position(*b).y)).unwrap();
    seams.mark_path(&sphere, top, bottom).unwrap();
    let (laid, _) = unwrap_mesh(&sphere, &seams, &UnwrapOptions::default()).unwrap();
    for mode in [RelaxMode::Rigid, RelaxMode::Conformal] {
        let (out, rep) = relax_corners(&laid, &[], &RelaxOptions { iterations: 15, mode }).unwrap();
        let (d0, d1) = (measure(&laid).0, measure(&out).0);
        eprintln!("{mode:?}: {rep:?}\n  before {d0:?}\n  after  {d1:?}");
        assert!(rep.energy_after <= rep.energy_before + 1e-12, "{rep:?}");
        assert_eq!(d1.flipped, 0);
        assert!(out.corner_uvs().iter().all(|u| u.is_finite()));
        match mode {
            // As rigid as possible spreads the area out, at the price of some angles.
            RelaxMode::Rigid => assert!(d1.area_rms_log < d0.area_rms_log * 0.7, "{d0:?} -> {d1:?}"),
            // As similar as possible keeps the angles where they were.
            RelaxMode::Conformal => assert!(d1.mean_angle_error_deg < d0.mean_angle_error_deg + 0.2, "{d0:?} -> {d1:?}"),
        }
    }
}

#[test]
fn relax_needs_uvs_and_handles_odd_input() {
    let bare = Primitive::Plane { width: 1.0, depth: 1.0, segments: 2 }.build().unwrap();
    let mut bare = bare;
    bare.clear_uvs();
    assert_eq!(relax_corners(&bare, &[], &RelaxOptions::default()).unwrap_err(), UvError::NoUvs);
    let flat = plane(3);
    // Corners that do not exist select nothing; zero iterations count as one.
    let (out, rep) = relax_corners(&flat, &[9_999_999], &RelaxOptions { iterations: 0, mode: RelaxMode::Rigid }).unwrap();
    assert_eq!(rep.islands, 0);
    assert_eq!(out.corner_uvs(), flat.corner_uvs());
}

#[test]
fn straightening_lays_a_chain_on_its_end_to_end_line() {
    let flat = plane(6);
    // The bottom border chain: vertices with the largest z (UV v is down, but any border will do).
    let zmax = (0..flat.vertex_count() as u32).map(|v| flat.position(v).z).fold(f32::MIN, f32::max);
    let chain: Vec<u32> = (0..flat.corner_count() as u32).filter(|c| (flat.position(flat.origin(*c)).z - zmax).abs() < 1e-4).collect();
    assert!(chain.len() >= 7);
    let wavy = jitter(&flat, &chain, 0.04);
    let (end_a, end_b) = {
        let mut sorted = chain.clone();
        sorted.sort_by(|a, b| flat.position(flat.origin(*a)).x.total_cmp(&flat.position(flat.origin(*b)).x));
        (sorted[0], *sorted.last().unwrap())
    };
    let (out, n) = straighten_corners(&wavy, &chain, 1.0).unwrap();
    assert_eq!(n, 1);
    // Ends stay, everything lies on the line through them.
    assert_eq!(out.uv(end_a), wavy.uv(end_a));
    assert_eq!(out.uv(end_b), wavy.uv(end_b));
    let (p, q) = (wavy.uv(end_a), wavy.uv(end_b));
    for &c in &chain {
        let w = out.uv(c) - p;
        assert!(w.perp_dot(q - p).abs() / (q - p).length() < 1e-4, "{c} is off the line: {w:?}");
    }
    // Half strength sits between.
    let (half, _) = straighten_corners(&wavy, &chain, 0.5).unwrap();
    let mid = chain.iter().copied().find(|c| *c != end_a && *c != end_b).unwrap();
    assert!(((half.uv(mid) - 0.5 * (wavy.uv(mid) + out.uv(mid))).length()) < 1e-4);
    // Nothing selected, or a closed loop, changes nothing.
    assert_eq!(straighten_corners(&wavy, &[], 1.0).unwrap().1, 0);
    let all: Vec<u32> = (0..wavy.corner_count() as u32).collect();
    // Every vertex of a grid is a branch point or an end of several chains: not a single simple path.
    assert_eq!(straighten_corners(&wavy, &all, 1.0).unwrap().0.corner_count(), wavy.corner_count());
    assert!(straighten_corners(&wavy, &chain, f32::NAN).is_err());
}

fn cube_net() -> PolyMesh {
    Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap()
}

/// A plane cut along its middle into two isometric islands laid out side by side, and the seam.
fn cut_plane() -> (PolyMesh, Vec<u32>) {
    let m = Primitive::Plane { width: 2.0, depth: 2.0, segments: 4 }.build().unwrap();
    let key = |v: u32| (m.position(v).x.abs() < 1e-4, m.position(v).z);
    let top = (0..m.vertex_count() as u32).filter(|v| key(*v).0).min_by(|a, b| key(*a).1.total_cmp(&key(*b).1)).unwrap();
    let bottom = (0..m.vertex_count() as u32).filter(|v| key(*v).0).max_by(|a, b| key(*a).1.total_cmp(&key(*b).1)).unwrap();
    let mut seams = SeamSet::new();
    seams.mark_path(&m, top, bottom).unwrap();
    let edges = seams.edge_ids(&m);
    assert_eq!(edges.len(), 4);
    (unwrap_mesh(&m, &seams, &UnwrapOptions::default()).unwrap().0, edges)
}

#[test]
fn stitching_with_alignment_moves_the_island_and_keeps_it_rigid() {
    let (m, edges) = cut_plane();
    assert_eq!(Charts::build(&m, &SeamSet::from_uv_discontinuities(&m)).unwrap().islands.len(), 2);
    // Knock the right-hand island sideways and turn it, so the seam sides are far apart.
    let charts = Charts::build(&m, &SeamSet::from_uv_discontinuities(&m)).unwrap();
    let corners = island_corners(&m, &charts, m.edge_half_edge(edges[0]));
    let moved = transform_corners(
        &m,
        &corners,
        &UvTransform { rotate_deg: 33.0, translate: Vec2::new(0.1, 0.2), pivot: Vec2::new(0.5, 0.5), ..UvTransform::default() },
    )
    .unwrap();
    let h = m.edge_half_edge(edges[0]);
    let t = m.twin(h);
    assert!((moved.uv(h) - moved.uv(m.next(t))).length() > 0.05, "the sides start apart");
    let aligned = stitch_islands(&moved, &edges, &StitchOptions { align: true, scale: false }).unwrap();
    for &e in &edges {
        let (h, t) = (m.edge_half_edge(e), m.twin(m.edge_half_edge(e)));
        assert!((aligned.uv(h) - aligned.uv(m.next(t))).length() < 1e-4 && (aligned.uv(m.next(h)) - aligned.uv(t)).length() < 1e-4, "edge {e}: the sides meet");
    }
    let (d, _) = measure(&aligned);
    eprintln!("aligned {d:?}");
    assert!(d.max_angle_error_deg < 0.01 && d.area_rms_log < 1e-4 && d.flipped == 0, "{d:?}");
    assert_eq!(Charts::build(&aligned, &SeamSet::from_uv_discontinuities(&aligned)).unwrap().islands.len(), 1);
    // Plain welding of the same edges crushes both islands to their midpoints.
    let welded = stitch_islands(&moved, &edges, &StitchOptions::default()).unwrap();
    assert_eq!(welded.corner_uvs(), stitch_edges(&moved, &edges).unwrap().corner_uvs());
    assert!(measure(&welded).0.max_angle_error_deg > 1.0, "{:?}", measure(&welded).0);
}

#[test]
fn stitching_can_match_the_scale_of_the_islands() {
    let (m, edges) = cut_plane();
    let charts = Charts::build(&m, &SeamSet::from_uv_discontinuities(&m)).unwrap();
    // Make the second island half the size of the first, about its own centre.
    let corners = island_corners(&m, &charts, m.edge_half_edge(edges[0]));
    let c = corners.iter().map(|c| m.uv(*c)).sum::<Vec2>() / corners.len() as f32;
    let small = transform_corners(&m, &corners, &UvTransform { scale: Vec2::splat(0.5), pivot: c, ..UvTransform::default() }).unwrap();
    let no_scale = stitch_islands(&small, &edges, &StitchOptions { align: true, scale: false }).unwrap();
    let with_scale = stitch_islands(&small, &edges, &StitchOptions { align: true, scale: true }).unwrap();
    let (a, b) = (measure(&no_scale).0, measure(&with_scale).0);
    eprintln!("without scale {a:?}\nwith scale {b:?}");
    assert!(b.max_angle_error_deg < 0.01 && b.area_rms_log < 1e-4, "{b:?}");
    assert!(a.area_rms_log > b.area_rms_log, "{a:?}");
    for &e in &edges {
        let (h, t) = (m.edge_half_edge(e), m.twin(m.edge_half_edge(e)));
        assert!((with_scale.uv(h) - with_scale.uv(m.next(t))).length() < 1e-4);
    }
}

#[test]
fn stitching_a_chain_of_islands_places_each_against_the_last() {
    let m = cube_net();
    let seams = SeamSet::from_uv_discontinuities(&m);
    // Every seam: a spanning set of islands is aligned, the rest of the edges are welded.
    let all = seams.edge_ids(&m);
    let out = stitch_islands(&m, &all, &StitchOptions { align: true, scale: true }).unwrap();
    assert!(out.corner_uvs().iter().all(|u| u.is_finite()));
    let charts = Charts::build(&out, &SeamSet::from_uv_discontinuities(&out)).unwrap();
    assert!(charts.islands.len() < 6, "{} islands left", charts.islands.len());
    let mut bare = m.clone();
    bare.clear_uvs();
    assert_eq!(stitch_islands(&bare, &[0], &StitchOptions { align: true, scale: false }).unwrap_err(), UvError::NoUvs);
    assert_eq!(stitch_islands(&m, &[], &StitchOptions { align: true, scale: false }).unwrap().corner_uvs(), m.corner_uvs());
}

proptest! {
    #[test]
    fn the_tools_never_break_a_mesh(
        corners in proptest::collection::vec(0u32..60, 0..40),
        edges in proptest::collection::vec(0u32..40, 0..20),
        iterations in 0u32..8,
        strength in -1.0f32..2.0,
        amount in 0.0f32..0.2,
        conformal in any::<bool>(),
        scale in any::<bool>(),
    ) {
        let flat = plane(4);
        let all: Vec<u32> = (0..flat.corner_count() as u32).collect();
        let bent = jitter(&flat, &all, amount);
        let mode = if conformal { RelaxMode::Conformal } else { RelaxMode::Rigid };
        let (a, _) = relax_corners(&bent, &corners, &RelaxOptions { iterations, mode }).unwrap();
        prop_assert!(a.corner_uvs().iter().all(|u| u.is_finite()));
        let (b, _) = straighten_corners(&a, &corners, strength).unwrap();
        prop_assert!(b.corner_uvs().iter().all(|u| u.is_finite()));
        let cube = cube_net();
        let c = stitch_islands(&cube, &edges, &StitchOptions { align: true, scale }).unwrap();
        prop_assert!(c.corner_uvs().iter().all(|u| u.is_finite()));
        prop_assert_eq!(c.corner_count(), cube.corner_count());
    }
}
