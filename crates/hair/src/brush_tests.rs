use glam::Vec3;
use proptest::prelude::*;

use super::*;

/// A flat square, `n × n` quads, spanning -1..1 in x and z (normal -y by the winding below).
fn plate(n: u32) -> Surface {
    let mut positions = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            positions.push(Vec3::new(i as f32 / n as f32 * 2.0 - 1.0, 0.0, j as f32 / n as f32 * 2.0 - 1.0));
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

fn groom(s: &Surface, n: usize, seed: u64) -> Guides {
    let spec = GrowSpec { amount: Amount::Count(n), length: 0.4, length_jitter: 0.3, points_per_strand: 8, seed, ..GrowSpec::default() };
    grow_guides(s, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap()
}

fn everywhere(kind: BrushKind) -> Brush {
    Brush { radius: 100.0, falloff: 0.0, strength: 1.0, ..Brush::new(kind) }
}

fn one_dab(brush: Brush, delta: Vec3) -> Stroke {
    Stroke { brush, symmetry: Symmetry::default(), dabs: vec![Dab { center: Vec3::ZERO, normal: Vec3::NEG_Y, delta, pressure: 1.0 }] }
}

fn seg_lengths(g: &Guides, i: usize) -> Vec<f32> {
    g.strand(i).windows(2).map(|w| w[0].distance(w[1])).collect()
}

fn same_roots_and_lengths(a: &Guides, b: &Guides) {
    for i in 0..a.len() {
        assert_eq!(a.strand(i)[0], b.strand(i)[0], "roots never move");
        for (x, y) in seg_lengths(a, i).iter().zip(seg_lengths(b, i)) {
            assert!((x - y).abs() < 1e-4 * (1.0 + x.abs()), "segment {x} became {y}");
        }
    }
}

#[test]
fn comb_bends_without_stretching_and_tips_follow_most() {
    let s = plate(8);
    let g0 = groom(&s, 40, 1);
    let mut g = g0.clone();
    let delta = Vec3::new(0.2, 0.0, 0.0);
    let report = apply_stroke(&mut g, &s, None, &one_dab(everywhere(BrushKind::Comb), delta)).unwrap();
    assert_eq!(report.guides_touched, 40);
    same_roots_and_lengths(&g0, &g);
    for i in 0..g.len() {
        let moved = |j: usize| (g.strand(i)[j] - g0.strand(i)[j]).x;
        assert!(moved(7) > moved(3) && moved(3) > 0.0, "tip {} mid {}", moved(7), moved(3));
    }
}

#[test]
fn cut_shortens_only_the_longer_strands_to_the_cut_length() {
    let s = plate(8);
    let g0 = groom(&s, 60, 2);
    let mut g = g0.clone();
    let brush = Brush { cut_length: 0.3, ..everywhere(BrushKind::Cut) };
    apply_stroke(&mut g, &s, None, &one_dab(brush, Vec3::ZERO)).unwrap();
    let (mut cut, mut kept) = (0, 0);
    for i in 0..g.len() {
        let (before, after) = (g0.strand_length(i), g.strand_length(i));
        if before > 0.3 {
            assert!((after - 0.3).abs() < 1e-3, "{before} cut to {after}");
            cut += 1;
        } else {
            assert!((after - before).abs() < 1e-5, "a shorter strand is left alone");
            kept += 1;
        }
        assert_eq!(g.strand(i)[0], g0.strand(i)[0]);
    }
    assert!(cut > 0 && kept > 0, "the fixture has both kinds of strand");
}

#[test]
fn length_grows_and_inverted_shrinks() {
    let s = plate(6);
    let g0 = groom(&s, 20, 3);
    let (mut up, mut down) = (g0.clone(), g0.clone());
    apply_stroke(&mut up, &s, None, &one_dab(everywhere(BrushKind::Length), Vec3::ZERO)).unwrap();
    apply_stroke(&mut down, &s, None, &one_dab(Brush { invert: true, ..everywhere(BrushKind::Length) }, Vec3::ZERO)).unwrap();
    for i in 0..g0.len() {
        assert!(up.strand_length(i) > g0.strand_length(i) * 1.2);
        assert!(down.strand_length(i) < g0.strand_length(i) * 0.8);
        assert_eq!(up.strand(i)[0], g0.strand(i)[0]);
    }
}

#[test]
fn puff_lifts_toward_the_normal_and_inverted_lays_down() {
    let s = plate(6);
    // Strands leaning away from the normal (-y): grow with a lean toward +x.
    let spec = GrowSpec { amount: Amount::Count(20), length: 0.4, lean: 0.9, lean_toward: Vec3::X, ..GrowSpec::default() };
    let g0 = grow_guides(&s, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let tilt = |g: &Guides, i: usize| (*g.strand(i).last().unwrap() - g.strand(i)[0]).normalize().dot(Vec3::NEG_Y);
    let (mut up, mut flat) = (g0.clone(), g0.clone());
    apply_stroke(&mut up, &s, None, &one_dab(everywhere(BrushKind::Puff), Vec3::ZERO)).unwrap();
    apply_stroke(&mut flat, &s, None, &one_dab(Brush { invert: true, ..everywhere(BrushKind::Puff) }, Vec3::ZERO)).unwrap();
    same_roots_and_lengths(&g0, &up);
    same_roots_and_lengths(&g0, &flat);
    for i in 0..g0.len() {
        assert!(tilt(&up, i) > tilt(&g0, i), "puffed strands stand up");
        assert!(tilt(&flat, i) < tilt(&g0, i), "inverted puff lays them down");
    }
}

#[test]
fn smooth_removes_kinks_and_evens_neighbours() {
    let s = plate(8);
    let mut g0 = groom(&s, 30, 4);
    // Put a kink in every strand.
    for i in 0..g0.len() {
        let st = g0.strand_mut(i);
        st[4] += Vec3::new(0.05, 0.0, 0.05);
    }
    let kink = |g: &Guides| -> f32 { (0..g.len()).map(|i| g.strand(i).windows(3).map(|w| (w[0] - 2.0 * w[1] + w[2]).length()).sum::<f32>()).sum() };
    let mut g = g0.clone();
    for _ in 0..6 {
        apply_stroke(&mut g, &s, None, &one_dab(everywhere(BrushKind::Smooth), Vec3::ZERO)).unwrap();
    }
    assert!(kink(&g) < kink(&g0) * 0.7, "{} then {}", kink(&g0), kink(&g));
    for i in 0..g.len() {
        assert_eq!(g.strand(i)[0], g0.strand(i)[0]);
    }
}

#[test]
fn clump_gathers_toward_the_nearest_strand_and_inverted_spreads() {
    let s = plate(8);
    let g0 = groom(&s, 30, 5);
    let spread = |g: &Guides| -> f32 {
        let tips: Vec<Vec3> = (0..g.len()).map(|i| *g.strand(i).last().unwrap()).collect();
        let mean = tips.iter().sum::<Vec3>() / tips.len() as f32;
        tips.iter().map(|t| t.distance(mean)).sum::<f32>() / tips.len() as f32
    };
    // A small brush at the centre: the strand nearest it leads.
    let brush = Brush { radius: 0.8, falloff: 0.0, strength: 1.0, ..Brush::new(BrushKind::Clump) };
    let mut g = g0.clone();
    for _ in 0..3 {
        apply_stroke(&mut g, &s, None, &one_dab(brush, Vec3::ZERO)).unwrap();
    }
    assert!(spread(&g) < spread(&g0), "{} then {}", spread(&g0), spread(&g));
    same_roots_and_lengths(&g0, &g);
    let mut wide = g0.clone();
    apply_stroke(&mut wide, &s, None, &one_dab(Brush { invert: true, ..brush }, Vec3::ZERO)).unwrap();
    assert!(spread(&wide) > spread(&g0));
}

#[test]
fn frizz_is_seeded_and_keeps_lengths() {
    let s = plate(6);
    let g0 = groom(&s, 20, 6);
    let stroke = one_dab(Brush { seed: 9, ..everywhere(BrushKind::Frizz) }, Vec3::ZERO);
    let (mut a, mut b) = (g0.clone(), g0.clone());
    apply_stroke(&mut a, &s, None, &stroke).unwrap();
    apply_stroke(&mut b, &s, None, &stroke).unwrap();
    assert_eq!(a, b, "same stroke, same result");
    assert_ne!(a, g0);
    same_roots_and_lengths(&g0, &a);
    let mut c = g0.clone();
    apply_stroke(&mut c, &s, None, &one_dab(Brush { seed: 10, ..everywhere(BrushKind::Frizz) }, Vec3::ZERO)).unwrap();
    assert_ne!(a, c, "another seed, another wobble");
}

#[test]
fn only_guides_in_reach_move_and_falloff_weakens_the_edge() {
    let s = plate(8);
    let g0 = groom(&s, 200, 7);
    let mut g = g0.clone();
    let brush = Brush { radius: 0.5, falloff: 1.0, strength: 1.0, ..Brush::new(BrushKind::Comb) };
    let stroke = one_dab(brush, Vec3::new(0.2, 0.0, 0.0));
    apply_stroke(&mut g, &s, None, &stroke).unwrap();
    let mut near_moved = 0.0f32;
    let mut edge_moved = 0.0f32;
    for i in 0..g.len() {
        let d = g0.strand(i)[0].distance(Vec3::ZERO);
        let moved = (*g.strand(i).last().unwrap() - *g0.strand(i).last().unwrap()).length();
        if d >= 0.5 {
            assert_eq!(g.strand(i), g0.strand(i), "outside the radius nothing moves");
        } else if d < 0.25 {
            near_moved = near_moved.max(moved);
        } else if d > 0.4 {
            edge_moved = edge_moved.max(moved);
        }
    }
    assert!(near_moved > 0.05 && edge_moved < near_moved * 0.5, "centre {near_moved} edge {edge_moved}");
}

#[test]
fn a_mask_protects_and_scales_the_effect() {
    let s = plate(8);
    let g0 = groom(&s, 100, 8);
    let stroke = one_dab(everywhere(BrushKind::Comb), Vec3::new(0.2, 0.0, 0.0));
    let zero = vec![0.0; s.vertex_count()];
    let mut g = g0.clone();
    let r = apply_stroke(&mut g, &s, Some(&zero), &stroke).unwrap();
    assert_eq!((r.guides_touched, g == g0), (0, true));
    // Protect the left half: only guides on the right move.
    let mask: Vec<f32> = s.positions().iter().map(|p| if p.x < 0.0 { 0.0 } else { 1.0 }).collect();
    let mut g = g0.clone();
    apply_stroke(&mut g, &s, Some(&mask), &stroke).unwrap();
    for i in 0..g.len() {
        let x = g0.strand(i)[0].x;
        if x < -0.3 {
            assert_eq!(g.strand(i), g0.strand(i));
        }
        if x > 0.3 {
            assert_ne!(g.strand(i), g0.strand(i));
        }
    }
    // A half mask halves the movement.
    let half = vec![0.5; s.vertex_count()];
    let (mut full, mut part) = (g0.clone(), g0.clone());
    apply_stroke(&mut full, &s, None, &stroke).unwrap();
    apply_stroke(&mut part, &s, Some(&half), &stroke).unwrap();
    let moved = |g: &Guides| (*g.strand(0).last().unwrap() - *g0.strand(0).last().unwrap()).x;
    assert!((moved(&part) / moved(&full) - 0.5).abs() < 0.05, "{} vs {}", moved(&part), moved(&full));
}

#[test]
fn symmetry_acts_on_the_mirror_side_too() {
    let s = plate(8);
    let g0 = groom(&s, 300, 9);
    let dab = Dab { center: Vec3::new(0.5, 0.0, 0.0), normal: Vec3::NEG_Y, delta: Vec3::new(0.0, 0.0, 0.2), pressure: 1.0 };
    let brush = Brush { radius: 0.4, falloff: 0.0, strength: 1.0, ..Brush::new(BrushKind::Comb) };
    let touched_sides = |sym: Symmetry| {
        let mut g = g0.clone();
        apply_stroke(&mut g, &s, None, &Stroke { brush, symmetry: sym, dabs: vec![dab] }).unwrap();
        let (mut left, mut right) = (0, 0);
        for i in 0..g.len() {
            if g.strand(i) != g0.strand(i) {
                if g0.strand(i)[0].x < 0.0 { left += 1 } else { right += 1 }
            }
        }
        (left, right)
    };
    let (l, r) = touched_sides(Symmetry::default());
    assert_eq!(l, 0);
    assert!(r > 0);
    let (l, r2) = touched_sides(Symmetry { x: true, ..Symmetry::default() });
    assert!(l > 0 && r2 == r, "the mirror dab moves the other side, the original side is unchanged");
    // The images: a dab on a plane is not doubled; three planes give up to eight.
    let on_plane = Dab::new(Vec3::new(0.0, 0.3, 0.3));
    assert_eq!(Symmetry { x: true, ..Symmetry::default() }.images(&on_plane).len(), 1);
    assert_eq!(Symmetry { x: true, y: true, z: true }.images(&Dab::new(Vec3::new(0.1, 0.2, 0.3))).len(), 8);
    // The mirror of a comb moves mirrored.
    let m = Dab { center: Vec3::new(1.0, 2.0, 3.0), normal: Vec3::Y, delta: Vec3::new(0.1, 0.2, 0.3), pressure: 0.5 };
    let img = Symmetry { x: true, ..Symmetry::default() }.images(&m);
    assert_eq!(img[1].center, Vec3::new(-1.0, 2.0, 3.0));
    assert_eq!(img[1].delta, Vec3::new(-0.1, 0.2, 0.3));
}

#[test]
fn a_stroke_applied_in_pieces_equals_the_whole() {
    let s = plate(8);
    let g0 = groom(&s, 80, 10);
    let dabs: Vec<Dab> = (0..12)
        .map(|i| Dab { center: Vec3::new(-0.6 + i as f32 * 0.1, 0.0, 0.1), normal: Vec3::NEG_Y, delta: Vec3::new(0.05, 0.0, 0.02), pressure: 1.0 })
        .collect();
    for kind in BrushKind::ALL {
        let brush = Brush { radius: 0.35, ..Brush::new(kind) };
        let sym = Symmetry { x: true, ..Symmetry::default() };
        let mut whole = g0.clone();
        apply_stroke(&mut whole, &s, None, &Stroke { brush, symmetry: sym, dabs: dabs.clone() }).unwrap();
        let mut pieces = g0.clone();
        for (n, d) in dabs.iter().enumerate() {
            apply_dabs(&mut pieces, &s, None, &brush, sym, std::slice::from_ref(d), n).unwrap();
        }
        assert_eq!(whole, pieces, "{kind:?}");
    }
}

#[test]
fn bad_strokes_are_errors_and_leave_the_guides_alone() {
    let s = plate(4);
    let g0 = groom(&s, 10, 11);
    let ok = everywhere(BrushKind::Comb);
    let dab = Dab::new(Vec3::ZERO);
    let cases: Vec<Stroke> = vec![
        Stroke { brush: Brush { radius: f32::NAN, ..ok }, symmetry: Symmetry::default(), dabs: vec![dab] },
        Stroke { brush: Brush { strength: 2.0, ..ok }, symmetry: Symmetry::default(), dabs: vec![dab] },
        Stroke { brush: Brush { falloff: -1.0, ..ok }, symmetry: Symmetry::default(), dabs: vec![dab] },
        Stroke { brush: ok, symmetry: Symmetry::default(), dabs: vec![Dab { center: Vec3::NAN, ..dab }] },
        Stroke { brush: ok, symmetry: Symmetry::default(), dabs: vec![Dab { delta: Vec3::INFINITY, ..dab }] },
        Stroke { brush: ok, symmetry: Symmetry::default(), dabs: vec![dab; MAX_DABS + 1] },
    ];
    for c in cases {
        let mut g = g0.clone();
        assert!(apply_stroke(&mut g, &s, None, &c).is_err());
        assert_eq!(g, g0);
    }
    let mut g = g0.clone();
    assert!(matches!(apply_stroke(&mut g, &s, Some(&[1.0, 1.0]), &one_dab(ok, Vec3::ZERO)), Err(HairError::LengthMismatch { .. })));
    let stale = Guides::from_parts(vec![SurfacePoint { tri: 9999, bary: [1.0, 0.0, 0.0] }], 2, vec![Vec3::ZERO, Vec3::Y], &Limits::default()).unwrap();
    let mut stale = stale;
    assert_eq!(apply_stroke(&mut stale, &s, None, &one_dab(ok, Vec3::ZERO)).unwrap_err(), HairError::BadRoot);
    // An empty stroke does nothing.
    let mut g = g0.clone();
    assert_eq!(apply_stroke(&mut g, &s, None, &Stroke { brush: ok, symmetry: Symmetry::default(), dabs: vec![] }).unwrap(), StrokeReport::default());
}

#[test]
fn strokes_round_trip_through_json() {
    let stroke =
        Stroke { brush: Brush::new(BrushKind::Frizz), symmetry: Symmetry { x: true, ..Symmetry::default() }, dabs: vec![Dab::new(Vec3::new(0.1, 0.2, 0.3))] };
    let text = serde_json::to_string(&stroke).unwrap();
    assert!(text.contains("\"frizz\""));
    assert_eq!(serde_json::from_str::<Stroke>(&text).unwrap(), stroke);
    assert_eq!(BrushKind::from_id("comb"), Some(BrushKind::Comb));
    assert_eq!(BrushKind::from_id("nope"), None);
    for k in BrushKind::ALL {
        assert_eq!(BrushKind::from_id(k.id()), Some(k));
    }
}

#[test]
fn painting_a_map_moves_values_toward_the_target() {
    let s = plate(8);
    let mut map = vec![1.0; s.vertex_count()];
    let n = paint_map(&mut map, &s, Vec3::ZERO, 0.5, 0.0, 1.0, 0.0).unwrap();
    assert!(n > 0);
    for (v, p) in map.iter().zip(s.positions()) {
        if p.length() < 0.5 { assert!(*v < 1e-6, "inside is painted to 0") } else { assert_eq!(*v, 1.0) }
    }
    let mut soft = vec![0.0; s.vertex_count()];
    paint_map(&mut soft, &s, Vec3::ZERO, 0.6, 1.0, 1.0, 1.0).unwrap();
    let centre = s.positions().iter().position(|p| p.length() < 1e-6).unwrap();
    let edge = s.positions().iter().position(|p| (p.length() - 0.5).abs() < 0.07).unwrap();
    assert!(soft[centre] > soft[edge] && soft[edge] > 0.0);
    assert!(paint_map(&mut map, &s, Vec3::ZERO, f32::NAN, 0.0, 1.0, 0.0).is_err());
    assert!(paint_map(&mut map, &s, Vec3::ZERO, 1.0, 2.0, 1.0, 0.0).is_err());
    assert!(paint_map(&mut map[..3], &s, Vec3::ZERO, 1.0, 0.0, 1.0, 0.0).is_err());
    assert!(paint_map(&mut map, &s, Vec3::NAN, 1.0, 0.0, 1.0, 0.0).is_err());
}

fn any_kind() -> impl Strategy<Value = BrushKind> {
    prop::sample::select(BrushKind::ALL.to_vec())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the brush, the dabs and the symmetry, guides stay finite, roots stay put and every
    /// brush but Cut and Length keeps every segment length.
    #[test]
    fn strokes_keep_guides_valid(
        kind in any_kind(),
        seed in any::<u64>(),
        radius in 0.001f32..3.0,
        strength in 0.0f32..1.0,
        falloff in 0.0f32..1.0,
        invert in any::<bool>(),
        sym in (any::<bool>(), any::<bool>(), any::<bool>()),
        dabs in prop::collection::vec(((-1.5f32..1.5, -0.5f32..0.5, -1.5f32..1.5), (-0.5f32..0.5, -0.5f32..0.5, -0.5f32..0.5), 0.0f32..1.0), 0..12),
    ) {
        let s = plate(5);
        let g0 = groom(&s, 40, seed);
        let brush = Brush { radius, strength, falloff, invert, seed, cut_length: 0.2, ..Brush::new(kind) };
        let stroke = Stroke {
            brush,
            symmetry: Symmetry { x: sym.0, y: sym.1, z: sym.2 },
            dabs: dabs.iter().map(|(c, d, p)| Dab { center: Vec3::new(c.0, c.1, c.2), normal: Vec3::NEG_Y, delta: Vec3::new(d.0, d.1, d.2), pressure: *p }).collect(),
        };
        let mut g = g0.clone();
        apply_stroke(&mut g, &s, None, &stroke).unwrap();
        prop_assert!(g.all_points().iter().all(|p| p.is_finite()));
        for i in 0..g.len() {
            prop_assert_eq!(g.strand(i)[0], g0.strand(i)[0]);
            if !matches!(kind, BrushKind::Cut | BrushKind::Length) {
                for (a, b) in seg_lengths(&g0, i).iter().zip(seg_lengths(&g, i)) {
                    prop_assert!((a - b).abs() < 2e-4 * (1.0 + a), "{kind:?}: segment {a} became {b}");
                }
            } else if kind == BrushKind::Cut {
                prop_assert!(g.strand_length(i) <= g0.strand_length(i) + 1e-4);
            }
        }
        // Deterministic.
        let mut again = g0.clone();
        apply_stroke(&mut again, &s, None, &stroke).unwrap();
        prop_assert_eq!(g, again);
    }

    /// Hostile numbers never panic: they give an error or valid guides.
    #[test]
    fn wild_numbers_never_panic(
        kind in any_kind(),
        radius in prop::num::f32::ANY,
        strength in prop::num::f32::ANY,
        cx in prop::num::f32::ANY,
        dx in prop::num::f32::ANY,
    ) {
        let s = plate(3);
        let mut g = groom(&s, 10, 1);
        let brush = Brush { radius, strength, ..Brush::new(kind) };
        let stroke = Stroke { brush, symmetry: Symmetry::default(), dabs: vec![Dab { center: Vec3::new(cx, 0.0, 0.0), normal: Vec3::Y, delta: Vec3::new(dx, 0.0, 0.0), pressure: 1.0 }] };
        if apply_stroke(&mut g, &s, None, &stroke).is_ok() {
            prop_assert!(g.all_points().iter().all(|p| p.is_finite()));
        }
    }
}
