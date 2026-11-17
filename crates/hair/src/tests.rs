use glam::Vec3;
use modelcraft_ops::primitives::Primitive;
use proptest::prelude::*;

use super::*;

fn sphere(segments: u32) -> Surface {
    let mesh = Primitive::Sphere { radius: 1.0, segments, rings: (segments / 2).max(2) }.build().unwrap();
    Surface::from_mesh(&mesh).unwrap()
}

/// A wavy rectangle in the xz plane, `n × n` quads.
fn sheet(n: u32, wave: f32) -> Surface {
    let mut positions = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let (x, z) = (i as f32 / n as f32, j as f32 / n as f32);
            positions.push(Vec3::new(x, wave * (x * 6.0).sin() * (z * 5.0).cos(), z));
        }
    }
    let mut tris = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            tris.push([a, a + 1, a + n + 1]);
            tris.push([a + 1, a + n + 2, a + n + 1]);
        }
    }
    Surface::from_triangles(positions, tris).unwrap()
}

fn grow(s: &Surface, n: usize, seed: u64) -> Guides {
    let spec = GrowSpec { amount: Amount::Count(n), seed, length: 0.2, length_jitter: 0.25, ..GrowSpec::default() };
    grow_guides(s, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap()
}

fn all_finite(h: &HairCurves) -> bool {
    h.all_points().iter().all(|p| p.is_finite()) && (0..h.strand_count()).all(|i| h.root_width(i).is_finite() && h.tip_width(i).is_finite())
}

#[test]
fn guides_start_on_the_surface_with_the_asked_length() {
    let s = sphere(24);
    let spec = GrowSpec { amount: Amount::Count(300), length: 0.3, points_per_strand: 10, ..GrowSpec::default() };
    let g = grow_guides(&s, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
    assert_eq!(g.len(), 300);
    for i in 0..g.len() {
        assert!(s.contains(&g.roots()[i]));
        assert!(g.strand(i)[0].distance(s.position(&g.roots()[i])) < 1e-5);
        assert!((g.strand_length(i) - 0.3).abs() < 1e-4, "length {}", g.strand_length(i));
        // A sphere's normal points away from the centre: the tip is farther out than the root.
        assert!(g.strand(i)[9].length() > g.strand(i)[0].length());
    }
}

#[test]
fn bend_keeps_the_length_and_moves_the_tip() {
    let s = sheet(8, 0.0);
    let straight = GrowSpec { amount: Amount::Count(20), length: 0.5, ..GrowSpec::default() };
    let bent = GrowSpec { bend: Vec3::new(0.0, 1.0, 0.0), ..straight.clone() };
    let a = grow_guides(&s, &straight, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let b = grow_guides(&s, &bent, &SurfaceMaps::default(), &Limits::default()).unwrap();
    for i in 0..20 {
        assert!((b.strand_length(i) - 0.5).abs() < 1e-4);
        assert!(b.strand(i)[7].y > a.strand(i)[7].y + 0.05, "the sheet normal is -y, so +y bends the tip back toward the surface side");
    }
}

#[test]
fn the_same_seed_gives_the_same_hair_and_another_seed_does_not() {
    let s = sphere(16);
    let a = grow(&s, 200, 5);
    assert_eq!(a, grow(&s, 200, 5));
    assert_ne!(a, grow(&s, 200, 6));
    let spec = InterpSpec {
        amount: Amount::Count(500),
        clump: Clump { amount: 0.5, tip_bias: 1.0, jitter: 0.3 },
        frizz: Frizz { amount: 0.1, frequency: 4.0 },
        ..InterpSpec::default()
    };
    let h1 = interpolate(&s, &a, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let h2 = interpolate(&s, &a, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
    assert_eq!(h1, h2);
}

#[test]
fn a_strand_does_not_change_when_the_count_does() {
    let s = sphere(16);
    let g = grow(&s, 100, 3);
    let few = interpolate(&s, &g, &InterpSpec { amount: Amount::Count(100), ..InterpSpec::default() }, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let many = interpolate(&s, &g, &InterpSpec { amount: Amount::Count(400), ..InterpSpec::default() }, &SurfaceMaps::default(), &Limits::default()).unwrap();
    for i in 0..100 {
        assert_eq!(few.strand(i), many.strand(i));
        assert_eq!(few.roots()[i], many.roots()[i]);
    }
    assert_eq!(few.prefix_for_fraction(0.25), 25);
    assert_eq!(many.prefix_for_fraction(1.0), 400);
    assert_eq!(many.prefix_for_fraction(f32::NAN), 400);
}

#[test]
fn a_hair_rooted_at_a_guide_reproduces_it() {
    // One hair per guide cannot be forced through sampling, so give the hair's root by hand: a
    // single-guide groom must reproduce that guide's shape at any root, rotated onto the new normal.
    let s = sheet(6, 0.0);
    let spec = GrowSpec { amount: Amount::Count(1), length: 0.4, bend: Vec3::new(0.3, 0.0, 0.0), ..GrowSpec::default() };
    let g = grow_guides(&s, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let hair = interpolate(&s, &g, &InterpSpec { amount: Amount::Count(30), ..InterpSpec::default() }, &SurfaceMaps::default(), &Limits::default()).unwrap();
    // A flat sheet: every normal is the same, so each hair is the guide translated to its root.
    for i in 0..30 {
        let shift = hair.strand(i)[0] - g.strand(0)[0];
        for (a, b) in hair.strand(i).iter().zip(g.strand(0)) {
            assert!(a.distance(*b + shift) < 1e-5);
        }
    }
}

#[test]
fn many_guides_blend_between_their_neighbours() {
    let s = sheet(10, 0.0);
    let g = grow(&s, 40, 9);
    for blend in [Blend::Barycentric, Blend::Nearest { k: 4 }] {
        let spec = InterpSpec { amount: Amount::Count(600), blend, ..InterpSpec::default() };
        let h = interpolate(&s, &g, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
        let (min_l, max_l) = (0..g.len()).fold((f32::MAX, 0.0f32), |(a, b), i| (a.min(g.strand_length(i)), b.max(g.strand_length(i))));
        for i in 0..h.strand_count() {
            // Offsets are blended linearly with weights summing to one, so a hair is never longer than
            // the longest guide nor shorter than the shortest straight guide allows.
            let l = h.strand_length(i);
            assert!(l <= max_l + 1e-4 && l >= min_l - 1e-4, "{l} not in [{min_l}, {max_l}]");
        }
        assert!(all_finite(&h));
    }
}

#[test]
fn a_root_on_a_guide_root_takes_that_guide_alone() {
    use crate::interp::weights_for;
    use crate::kdtree::KdTree;
    let roots: Vec<Vec3> = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0)];
    let tree = KdTree::new(roots.clone());
    for (i, q) in roots.iter().enumerate() {
        for (blend, k) in [(Blend::Barycentric, 3), (Blend::Nearest { k: 4 }, 4)] {
            let found = tree.nearest(*q, k);
            let w = weights_for(blend, found.as_slice(), *q, &roots);
            assert_eq!(w.items[0].0 as usize, i);
            assert!((w.items[0].1 - 1.0).abs() < 1e-6, "{blend:?}: {:?}", &w.items[..w.len]);
        }
    }
    // Between two guides the weights split and sum to one.
    let q = Vec3::new(0.5, 0.0, 0.0);
    let found = tree.nearest(q, 3);
    let w = weights_for(Blend::Barycentric, found.as_slice(), q, &roots);
    let sum: f32 = w.items[..w.len].iter().map(|x| x.1).sum();
    assert!((sum - 1.0).abs() < 1e-5);
    // Three collinear guides fall back to inverse distance rather than dividing by zero.
    let line = vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0];
    let tree = KdTree::new(line.clone());
    let found = tree.nearest(Vec3::new(0.4, 0.1, 0.0), 3);
    let w = weights_for(Blend::Barycentric, found.as_slice(), Vec3::new(0.4, 0.1, 0.0), &line);
    assert!(w.items[..w.len].iter().all(|x| x.1.is_finite() && x.1 >= 0.0));
    assert!((w.items[..w.len].iter().map(|x| x.1).sum::<f32>() - 1.0).abs() < 1e-5);
}

#[test]
fn offsets_are_carried_onto_the_normal_of_the_hair_root() {
    // On a sphere guides stick out radially; every hair must too, since each guide's offsets are turned
    // from its root normal onto the hair's root normal.
    let s = sphere(20);
    let g = grow_guides(&s, &GrowSpec { amount: Amount::Count(60), length: 0.2, ..GrowSpec::default() }, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let h = interpolate(
        &s,
        &g,
        &InterpSpec { amount: Amount::Count(500), blend: Blend::Nearest { k: 3 }, ..InterpSpec::default() },
        &SurfaceMaps::default(),
        &Limits::default(),
    )
    .unwrap();
    for i in 0..h.strand_count() {
        let n = s.normal(&h.roots()[i]);
        let tip = *h.strand(i).last().unwrap() - h.strand(i)[0];
        assert!(tip.normalize().dot(n) > 0.99, "hair {i} leans {}", tip.normalize().dot(n));
        assert!((tip.length() - 0.2).abs() < 0.01);
    }
}

#[test]
fn resampling_keeps_the_ends_and_the_shape() {
    let s = sheet(6, 0.0);
    let spec = GrowSpec { amount: Amount::Count(1), length: 0.5, points_per_strand: 6, bend: Vec3::new(0.4, 0.0, 0.0), ..GrowSpec::default() };
    let g = grow_guides(&s, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let h = interpolate(
        &s,
        &g,
        &InterpSpec { amount: Amount::Count(3), points_per_strand: Some(21), ..InterpSpec::default() },
        &SurfaceMaps::default(),
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(h.points_per_strand(), 21);
    for i in 0..3 {
        let shift = h.strand(i)[0] - g.strand(0)[0];
        assert!(h.strand(i)[20].distance(g.strand(0)[5] + shift) < 1e-5);
        // The spline stays near the guide's polyline length.
        assert!((h.strand_length(i) - g.strand_length(0)).abs() < 0.02);
    }
}

#[test]
fn clumping_gathers_tips_and_zero_clump_does_not() {
    let s = sheet(10, 0.0);
    let g = grow(&s, 6, 4);
    let base = InterpSpec { amount: Amount::Count(400), blend: Blend::Nearest { k: 3 }, ..InterpSpec::default() };
    let clumped = InterpSpec { clump: Clump { amount: 1.0, tip_bias: 1.0, jitter: 0.0 }, ..base.clone() };
    let a = interpolate(&s, &g, &base, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let b = interpolate(&s, &g, &clumped, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let tip = |h: &HairCurves, i: usize| *h.strand(i).last().unwrap();
    // With full clumping every tip sits on a guide tip.
    for i in 0..b.strand_count() {
        let t = tip(&b, i);
        assert!((0..g.len()).any(|k| g.strand(k).last().unwrap().distance(t) < 1e-4));
        // roots are unmoved
        assert_eq!(a.strand(i)[0], b.strand(i)[0]);
    }
    let distinct = |h: &HairCurves| {
        let mut tips: Vec<[i32; 3]> = (0..h.strand_count()).map(|i| tip(h, i)).map(|p| [(p.x * 1e3) as i32, (p.y * 1e3) as i32, (p.z * 1e3) as i32]).collect();
        tips.sort_unstable();
        tips.dedup();
        tips.len()
    };
    assert!(distinct(&b) <= g.len());
    assert!(distinct(&a) > g.len());
}

#[test]
fn frizz_moves_points_but_never_the_root() {
    let s = sheet(8, 0.0);
    let g = grow(&s, 10, 1);
    let calm = InterpSpec { amount: Amount::Count(50), ..InterpSpec::default() };
    let wild = InterpSpec { frizz: Frizz { amount: 0.3, frequency: 5.0 }, ..calm.clone() };
    let a = interpolate(&s, &g, &calm, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let b = interpolate(&s, &g, &wild, &SurfaceMaps::default(), &Limits::default()).unwrap();
    for i in 0..50 {
        assert_eq!(a.strand(i)[0], b.strand(i)[0]);
        assert!(a.strand(i).last().unwrap().distance(*b.strand(i).last().unwrap()) > 1e-3);
    }
    assert!(all_finite(&b));
}

#[test]
fn width_tapers_from_root_to_tip() {
    let s = sheet(4, 0.0);
    let g = grow(&s, 4, 1);
    let spec = InterpSpec { amount: Amount::Count(50), width: Width { root: 0.02, tip: 0.002, exponent: 1.5, jitter: 0.3 }, ..InterpSpec::default() };
    let h = interpolate(&s, &g, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for i in 0..50 {
        let mut last = f32::MAX;
        for k in 0..=10 {
            let w = h.width_at(i, k as f32 / 10.0);
            assert!(w <= last + 1e-9 && w >= 0.0);
            last = w;
        }
        assert!((h.width_at(i, 0.0) - h.root_width(i)).abs() < 1e-9);
        assert!((h.width_at(i, 1.0) - h.tip_width(i)).abs() < 1e-9);
        assert!(h.root_width(i) >= 0.02 * 0.7 - 1e-6 && h.root_width(i) <= 0.02 * 1.3 + 1e-6);
        seen.insert((h.root_width(i) * 1e6) as i64);
    }
    assert!(seen.len() > 10, "width jitter varies strands");
    assert_eq!(h.width_at(0, f32::NAN), h.root_width(0));
}

#[test]
fn a_density_map_keeps_hair_off_zero_regions() {
    let s = sheet(8, 0.0);
    // Vertices with x < 0.5 grow nothing, the rest everything.
    let density: Vec<f32> = s.positions().iter().map(|p| if p.x < 0.5 { 0.0 } else { 1.0 }).collect();
    let maps = SurfaceMaps { density: Some(&density), length: None };
    let spec = GrowSpec { amount: Amount::Count(2000), ..GrowSpec::default() };
    let g = grow_guides(&s, &spec, &maps, &Limits::default()).unwrap();
    assert_eq!(g.len(), 2000);
    // Triangles whose three vertices are all zero never receive a root. Border triangles may, at a
    // density that fades to zero, so every root lies at x >= 0.5 - one cell.
    for r in g.roots() {
        assert!(s.position(r).x >= 0.5 - 1.0 / 8.0 - 1e-4);
    }
    // All-zero density grows nothing, without an error.
    let zero = vec![0.0; s.vertex_count()];
    let none = grow_guides(&s, &spec, &SurfaceMaps { density: Some(&zero), length: None }, &Limits::default()).unwrap();
    assert!(none.is_empty());
}

#[test]
fn per_area_density_scales_with_the_surface() {
    let s = sheet(8, 0.0);
    let spec = GrowSpec { amount: Amount::PerUnitArea(1000.0), ..GrowSpec::default() };
    let g = grow_guides(&s, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
    assert_eq!(g.len(), (s.area() * 1000.0).round() as usize);
    // Half the density map, half the strands.
    let half = vec![0.5; s.vertex_count()];
    let h = grow_guides(&s, &spec, &SurfaceMaps { density: Some(&half), length: None }, &Limits::default()).unwrap();
    assert_eq!(h.len(), (s.area() * 500.0).round() as usize);
}

#[test]
fn length_map_scales_strands() {
    let s = sheet(8, 0.0);
    let half = vec![0.5; s.vertex_count()];
    let spec = GrowSpec { amount: Amount::Count(30), length: 0.4, ..GrowSpec::default() };
    let g = grow_guides(&s, &spec, &SurfaceMaps { density: None, length: Some(&half) }, &Limits::default()).unwrap();
    for i in 0..30 {
        assert!((g.strand_length(i) - 0.2).abs() < 1e-4);
    }
}

#[test]
fn caps_are_enforced_before_anything_is_allocated() {
    let s = sphere(8);
    let small = Limits { max_guides: 50, max_strands: 1000, max_points_per_strand: 16, max_total_points: 5000 };
    let too_many = GrowSpec { amount: Amount::Count(51), ..GrowSpec::default() };
    assert!(matches!(grow_guides(&s, &too_many, &SurfaceMaps::default(), &small), Err(HairError::TooMany { what: "guides", .. })));
    let too_long = GrowSpec { amount: Amount::Count(10), points_per_strand: 17, ..GrowSpec::default() };
    assert!(matches!(grow_guides(&s, &too_long, &SurfaceMaps::default(), &small), Err(HairError::OutOfRange { .. })));
    let huge = GrowSpec { amount: Amount::Count(usize::MAX), ..GrowSpec::default() };
    assert!(matches!(grow_guides(&s, &huge, &SurfaceMaps::default(), &Limits::default()), Err(HairError::TooMany { .. })));
    let area = GrowSpec { amount: Amount::PerUnitArea(f32::MAX), ..GrowSpec::default() };
    assert!(matches!(grow_guides(&s, &area, &SurfaceMaps::default(), &Limits::default()), Err(HairError::TooMany { .. })));
    let g = grow(&s, 20, 1);
    let strands = InterpSpec { amount: Amount::Count(1001), ..InterpSpec::default() };
    assert!(matches!(interpolate(&s, &g, &strands, &SurfaceMaps::default(), &small), Err(HairError::TooMany { what: "strands", .. })));
    let points = InterpSpec { amount: Amount::Count(900), points_per_strand: Some(10), ..InterpSpec::default() };
    assert!(matches!(interpolate(&s, &g, &points, &SurfaceMaps::default(), &small), Err(HairError::TooMany { what: "points", .. })));
    // A caller cannot raise a limit past the absolute ceiling.
    let wild = Limits { max_guides: usize::MAX, max_strands: usize::MAX, max_points_per_strand: usize::MAX, max_total_points: usize::MAX }.clamped();
    assert_eq!(wild.max_strands, ABSOLUTE_MAX_STRANDS);
    assert_eq!(wild.max_total_points, ABSOLUTE_MAX_TOTAL_POINTS);
}

#[test]
fn hostile_inputs_are_errors_not_panics() {
    let s = sphere(8);
    let l = Limits::default();
    let m = SurfaceMaps::default();
    for bad in [f32::NAN, f32::INFINITY, -1.0, 1e30] {
        let spec = GrowSpec { length: bad, ..GrowSpec::default() };
        assert!(grow_guides(&s, &spec, &m, &l).is_err());
        let spec = GrowSpec { length_jitter: bad, ..GrowSpec::default() };
        assert!(grow_guides(&s, &spec, &m, &l).is_err());
    }
    assert!(grow_guides(&s, &GrowSpec { bend: Vec3::splat(f32::NAN), ..GrowSpec::default() }, &m, &l).is_err());
    assert!(grow_guides(&s, &GrowSpec { points_per_strand: 1, ..GrowSpec::default() }, &m, &l).is_err());
    assert!(grow_guides(&s, &GrowSpec { points_per_strand: 0, ..GrowSpec::default() }, &m, &l).is_err());
    let short = vec![1.0; 3];
    assert!(matches!(grow_guides(&s, &GrowSpec::default(), &SurfaceMaps { density: Some(&short), length: None }, &l), Err(HairError::LengthMismatch { .. })));
    // A mesh with no area, bad indices and NaN positions.
    assert_eq!(Surface::from_triangles(vec![Vec3::ZERO; 3], vec![[0, 1, 2]]).unwrap_err(), HairError::NoSurface);
    assert_eq!(Surface::from_triangles(vec![Vec3::ZERO; 3], vec![[0, 1, 9]]).unwrap_err(), HairError::BadIndex);
    assert!(Surface::from_triangles(vec![Vec3::NAN; 3], vec![[0, 1, 2]]).is_err());
    assert_eq!(Surface::from_triangles(vec![], vec![]).unwrap_err(), HairError::NoSurface);
    // Interpolating without guides, with a stale root, with bad widths.
    let g = grow(&s, 10, 1);
    let empty = Guides::from_parts(vec![], 4, vec![], &l).unwrap();
    assert_eq!(interpolate(&s, &empty, &InterpSpec::default(), &m, &l).unwrap_err(), HairError::NoGuides);
    let stale = Guides::from_parts(vec![SurfacePoint { tri: 99999, bary: [1.0, 0.0, 0.0] }], 2, vec![Vec3::ZERO, Vec3::Y], &l).unwrap();
    assert_eq!(interpolate(&s, &stale, &InterpSpec::default(), &m, &l).unwrap_err(), HairError::BadRoot);
    for spec in [
        InterpSpec { width: Width { root: f32::NAN, ..Width::default() }, ..InterpSpec::default() },
        InterpSpec { width: Width { exponent: 0.0, ..Width::default() }, ..InterpSpec::default() },
        InterpSpec { clump: Clump { amount: 2.0, tip_bias: 1.0, jitter: 0.0 }, ..InterpSpec::default() },
        InterpSpec { frizz: Frizz { amount: f32::INFINITY, frequency: 1.0 }, ..InterpSpec::default() },
        InterpSpec { points_per_strand: Some(1), ..InterpSpec::default() },
    ] {
        assert!(interpolate(&s, &g, &spec, &m, &l).is_err(), "{spec:?}");
    }
    // Stored data with the wrong shape.
    assert!(Guides::from_parts(vec![SurfacePoint { tri: 0, bary: [1.0, 0.0, 0.0] }], 3, vec![Vec3::ZERO; 2], &l).is_err());
    assert!(Guides::from_parts(vec![SurfacePoint { tri: 0, bary: [1.0, 0.0, 0.0] }], 2, vec![Vec3::NAN; 2], &l).is_err());
    assert!(HairCurves::from_parts(2, vec![Vec3::ZERO; 2], vec![SurfacePoint { tri: 0, bary: [1.0, 0.0, 0.0] }], vec![-1.0], vec![0.0], 1.0, &l).is_err());
}

#[test]
fn guides_follow_a_deforming_surface() {
    let s = sheet(8, 0.1);
    let mut g = grow(&s, 40, 3);
    let before = g.clone();
    let moved = s.with_positions(s.positions().iter().map(|p| *p + Vec3::new(2.0, -1.0, 0.5)).collect()).unwrap();
    g.follow(&s, &moved).unwrap();
    for i in 0..g.len() {
        for (a, b) in g.strand(i).iter().zip(before.strand(i)) {
            assert!(a.distance(*b + Vec3::new(2.0, -1.0, 0.5)) < 1e-4);
        }
        assert!(moved.position(&g.roots()[i]).distance(g.strand(i)[0]) < 1e-4);
    }
    // Different topology is refused.
    assert!(g.follow(&s, &sheet(4, 0.0)).is_err());
}

#[test]
fn serde_round_trip() {
    let s = sphere(8);
    let g = grow(&s, 12, 1);
    let h = interpolate(&s, &g, &InterpSpec { amount: Amount::Count(30), ..InterpSpec::default() }, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let g2: Guides = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
    let h2: HairCurves = serde_json::from_str(&serde_json::to_string(&h).unwrap()).unwrap();
    assert_eq!(g, g2);
    assert_eq!(h, h2);
    let spec = InterpSpec::default();
    let back: InterpSpec = serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
    assert_eq!(spec, back);
}

#[test]
fn memory_estimate_covers_what_is_held() {
    let s = sphere(12);
    let g = grow(&s, 50, 1);
    let h = interpolate(
        &s,
        &g,
        &InterpSpec { amount: Amount::Count(777), points_per_strand: Some(12), ..InterpSpec::default() },
        &SurfaceMaps::default(),
        &Limits::default(),
    )
    .unwrap();
    let held = h.all_points().len() * 12 + h.roots().len() * std::mem::size_of::<SurfacePoint>() + h.strand_count() * 8;
    assert_eq!(h.bytes(), held);
    assert_eq!(estimate_bytes(usize::MAX, usize::MAX), usize::MAX);
}

#[test]
fn nearest_k_matches_brute_force() {
    use crate::kdtree::KdTree;
    let pts: Vec<Vec3> = (0..500u64).map(|i| Vec3::new(rng_f(i, 0), rng_f(i, 1), rng_f(i, 2))).collect();
    let tree = KdTree::new(pts.clone());
    for q in 0..50u64 {
        let q = Vec3::new(rng_f(q + 1000, 0), rng_f(q + 1000, 1), rng_f(q + 1000, 2));
        for k in [1usize, 3, 8] {
            let found = tree.nearest(q, k);
            let mut brute: Vec<f32> = pts.iter().map(|p| p.distance_squared(q)).collect();
            brute.sort_by(f32::total_cmp);
            assert_eq!(found.as_slice().len(), k);
            for (f, b) in found.as_slice().iter().zip(&brute) {
                assert!((f.0 - b).abs() < 1e-7);
            }
        }
    }
    // Fewer points than k, and duplicates.
    let tiny = KdTree::new(vec![Vec3::ONE, Vec3::ONE]);
    assert_eq!(tiny.nearest(Vec3::ZERO, 8).as_slice().len(), 2);
    assert_eq!(KdTree::new(vec![]).nearest(Vec3::ZERO, 3).as_slice().len(), 0);
}

fn rng_f(i: u64, c: u64) -> f32 {
    crate::rng::rand(77, i, c)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Whatever the spec, interpolation yields finite hair whose roots lie on the surface and whose
    /// strands never exceed the caps.
    #[test]
    fn interpolation_is_always_finite_and_rooted(
        seed in any::<u64>(),
        guides in 1usize..60,
        strands in 0usize..400,
        pps in 2usize..24,
        k in 1usize..12,
        barycentric in any::<bool>(),
        clump in 0.0f32..1.0,
        frizz in 0.0f32..2.0,
        jitter in 0.0f32..1.0,
        wave in 0.0f32..0.5,
    ) {
        let s = sheet(6, wave);
        let g = grow_guides(&s, &GrowSpec { amount: Amount::Count(guides), seed, length: 0.3, length_jitter: jitter, ..GrowSpec::default() }, &SurfaceMaps::default(), &Limits::default()).unwrap();
        let spec = InterpSpec {
            amount: Amount::Count(strands),
            seed: seed ^ 0xabc,
            blend: if barycentric { Blend::Barycentric } else { Blend::Nearest { k } },
            points_per_strand: Some(pps),
            length_jitter: jitter,
            clump: Clump { amount: clump, tip_bias: 1.0, jitter },
            frizz: Frizz { amount: frizz, frequency: 3.0 },
            width: Width { root: 0.01, tip: 0.001, exponent: 1.0, jitter },
        };
        let h = interpolate(&s, &g, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
        prop_assert_eq!(h.strand_count(), strands);
        prop_assert_eq!(h.total_points(), strands * pps);
        prop_assert!(all_finite(&h));
        for i in 0..h.strand_count() {
            prop_assert!(s.contains(&h.roots()[i]));
            prop_assert!(h.strand(i)[0].distance(s.position(&h.roots()[i])) < 1e-4);
        }
    }

    /// Guides never leave their length budget and always start on the surface, for any spec.
    #[test]
    fn guides_keep_their_length(
        seed in any::<u64>(),
        n in 0usize..200,
        pps in 2usize..32,
        length in 0.0f32..5.0,
        jitter in 0.0f32..1.0,
        lean in 0.0f32..1.0,
        bx in -3.0f32..3.0,
        by in -3.0f32..3.0,
    ) {
        let s = sphere(12);
        let spec = GrowSpec { amount: Amount::Count(n), seed, length, length_jitter: jitter, points_per_strand: pps, lean, lean_toward: Vec3::new(1.0, 0.2, -0.4), bend: Vec3::new(bx, by, 0.0) };
        let g = grow_guides(&s, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
        prop_assert_eq!(g.len(), n);
        for i in 0..g.len() {
            prop_assert!(s.contains(&g.roots()[i]));
            let l = g.strand_length(i);
            prop_assert!(l.is_finite() && l <= length * (1.0 + jitter) + 1e-3 && l >= length * (1.0 - jitter) - 1e-3);
        }
    }

    /// Strand `i` does not depend on how many strands are made.
    #[test]
    fn strands_are_independent_of_the_amount(seed in any::<u64>(), a in 1usize..80, extra in 0usize..80) {
        let s = sheet(5, 0.2);
        let g = grow(&s, 12, seed);
        let make = |n| interpolate(&s, &g, &InterpSpec { amount: Amount::Count(n), seed, clump: Clump { amount: 0.4, tip_bias: 1.0, jitter: 0.2 }, frizz: Frizz { amount: 0.1, frequency: 2.0 }, ..InterpSpec::default() }, &SurfaceMaps::default(), &Limits::default()).unwrap();
        let (few, many) = (make(a), make(a + extra));
        for i in 0..a {
            prop_assert_eq!(few.strand(i), many.strand(i));
        }
    }

    /// Hostile maps and specs never panic: they give hair or an error.
    #[test]
    fn nothing_panics_on_wild_numbers(
        d in prop::collection::vec(prop::num::f32::ANY, 0..60),
        len in prop::num::f32::ANY,
        amount in prop::num::f32::ANY,
        seed in any::<u64>(),
    ) {
        let s = sheet(3, 0.1);
        let maps = SurfaceMaps { density: Some(&d), length: None };
        let spec = GrowSpec { amount: Amount::PerUnitArea(amount), length: len, seed, ..GrowSpec::default() };
        if let Ok(g) = grow_guides(&s, &spec, &maps, &Limits { max_guides: 500, ..Limits::default() }) {
            prop_assert!(g.all_points().iter().all(|p| p.is_finite()));
        }
    }
}
