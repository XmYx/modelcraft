use super::*;
use glam::Vec3;
use modelcraft_ops::primitives::Primitive;

fn ball(seg: u32) -> SculptMesh {
    let m = Primitive::RoundCube { radius: 1.0, segments: seg }.build().unwrap();
    SculptMesh::new(&m, None, None)
}

fn stroke(s: &mut SculptMesh, b: BrushSettings, centers: &[Vec3]) -> StrokeUndo {
    s.begin_stroke(b);
    for c in centers {
        s.dab(Dab { center: *c, view_dir: -c.normalize(), pressure: 1.0, grab_delta: Vec3::ZERO, scale: 1.0 });
    }
    s.end_stroke().unwrap().0
}

#[test]
fn raycast_hits_the_surface() {
    let s = ball(16);
    let (t, p, _) = s.raycast(Vec3::new(0.0, 0.0, 5.0), -Vec3::Z).unwrap();
    assert!((t - 4.0).abs() < 0.01 && (p.z - 1.0).abs() < 0.01);
    assert!(s.raycast(Vec3::new(5.0, 5.0, 5.0), Vec3::X).is_none());
}

#[test]
fn draw_raises_and_inverts() {
    let mut s = ball(24);
    let top = Vec3::Y;
    let u = stroke(&mut s, BrushSettings::new(BrushKind::Draw, 0.3), &[top]);
    assert!(!u.is_empty());
    let raised = s.positions.iter().map(|p| p.length()).fold(0.0f32, f32::max);
    assert!(raised > 1.005, "draw did not raise the surface ({raised})");
    // Undo puts every point back.
    s.apply_undo(&u, false);
    assert!(s.positions.iter().all(|p| (p.length() - 1.0).abs() < 1e-3));
    let mut b = BrushSettings::new(BrushKind::Draw, 0.3);
    b.invert = true;
    stroke(&mut s, b, &[top]);
    assert!(s.positions.iter().map(|p| p.length()).fold(2.0f32, f32::min) < 0.995);
}

#[test]
fn every_brush_changes_something_and_stays_finite() {
    for kind in brush::KINDS {
        let mut s = ball(16);
        if *kind == BrushKind::Smooth {
            // Roughen first so there is something to smooth.
            stroke(&mut s, BrushSettings { stamp: Stamp::Noise { scale: 8.0 }, ..BrushSettings::new(BrushKind::Draw, 0.5) }, &[Vec3::Y]);
        }
        let before = (s.positions.clone(), s.mask.clone());
        s.begin_stroke(BrushSettings::new(*kind, 0.4));
        for i in 0..5 {
            let c = Vec3::new(i as f32 * 0.05, 1.0, 0.0).normalize();
            s.dab(Dab { center: c, view_dir: -Vec3::Y, pressure: 1.0, grab_delta: Vec3::new(0.0, 0.05, 0.0), scale: 1.0 });
        }
        let (u, rec) = s.end_stroke().unwrap();
        assert_eq!(rec.dabs.len(), 5);
        assert!(!u.is_empty(), "{kind:?} changed nothing");
        assert!(s.positions.iter().all(|p| p.is_finite()));
        assert!((s.positions.clone(), s.mask.clone()) != before);
    }
}

#[test]
fn mask_protects_vertices() {
    let mut s = ball(16);
    stroke(&mut s, BrushSettings { strength: 1.0, falloff: Falloff::Constant, ..BrushSettings::new(BrushKind::Mask, 0.5) }, &[Vec3::Y]);
    let masked: Vec<u32> = (0..s.vertex_count() as u32).filter(|v| s.mask[*v as usize] >= 1.0).collect();
    assert!(!masked.is_empty());
    let before: Vec<Vec3> = masked.iter().map(|v| s.positions[*v as usize]).collect();
    stroke(&mut s, BrushSettings::new(BrushKind::Draw, 0.5), &[Vec3::Y]);
    let after: Vec<Vec3> = masked.iter().map(|v| s.positions[*v as usize]).collect();
    assert_eq!(before, after);
    let u = s.mask_invert();
    assert!(!u.is_empty());
    s.mask_all(0.0);
    assert!(s.mask.iter().all(|m| *m == 0.0));
}

#[test]
fn symmetry_mirrors_the_dab() {
    let mut s = ball(20);
    let mut b = BrushSettings::new(BrushKind::Draw, 0.3);
    b.symmetry.x = true;
    let c = Vec3::new(0.6, 0.6, 0.0).normalize();
    stroke(&mut s, b, &[c]);
    let near = |q: Vec3| s.positions.iter().filter(|p| p.distance(q) < 0.15).map(|p| p.length()).fold(0.0f32, f32::max);
    let (r, l) = (near(c), near(c * Vec3::new(-1.0, 1.0, 1.0)));
    assert!(r > 1.003 && (r - l).abs() < 1e-3, "right {r} left {l}");
}

#[test]
fn replay_is_deterministic_and_dirty_chunks_cover_changes() {
    let base = Primitive::RoundCube { radius: 1.0, segments: 40 }.build().unwrap();
    let mut a = SculptMesh::new(&base, None, None);
    a.take_dirty_chunks();
    a.begin_stroke(BrushSettings::new(BrushKind::Clay, 0.3));
    for i in 0..10 {
        a.dab(Dab { center: Vec3::new(0.0, 1.0, i as f32 * 0.03).normalize(), view_dir: -Vec3::Y, pressure: 0.7, grab_delta: Vec3::ZERO, scale: 1.0 });
    }
    let (u, rec) = a.end_stroke().unwrap();
    let chunks = a.take_dirty_chunks();
    for i in &u.indices {
        assert!(chunks.iter().any(|(s, l)| (*i as usize) >= *s && (*i as usize) < s + l));
    }
    let total: usize = chunks.iter().map(|c| c.1).sum();
    assert!(total < a.vertex_count(), "a local stroke should not dirty everything");
    let mut b = SculptMesh::new(&base, None, None);
    let u2 = b.replay(&rec);
    assert_eq!(u, u2);
    assert_eq!(a.positions, b.positions);
}

#[test]
fn hostile_dabs_are_ignored() {
    let mut s = ball(8);
    s.begin_stroke(BrushSettings::new(BrushKind::Draw, f32::NAN));
    s.dab(Dab { center: Vec3::NAN, view_dir: Vec3::Y, pressure: 1.0, grab_delta: Vec3::ZERO, scale: 1.0 });
    s.dab(Dab { center: Vec3::Y, view_dir: Vec3::Y, pressure: f32::INFINITY, grab_delta: Vec3::ZERO, scale: 1.0 });
    let _ = s.end_stroke();
    assert!(s.positions.iter().all(|p| p.is_finite()));
}

mod detail_levels {
    use glam::{Quat, Vec3};
    use modelcraft_mesh::PolyMesh;
    use modelcraft_ops::primitives::Primitive;
    use modelcraft_ops::subdivide::catmull_clark;
    use proptest::prelude::*;

    use crate::detail::{Refine, shape, step_down, step_up};

    fn bases() -> Vec<PolyMesh> {
        vec![
            Primitive::Cube { size: Vec3::splat(2.0), segments: 2 }.build().unwrap(),
            Primitive::Plane { width: 2.0, depth: 2.0, segments: 3 }.build().unwrap(),
            Primitive::Cylinder { radius: 1.0, height: 2.0, sides: 7, rings: 1, caps: true }.build().unwrap(),
            Primitive::Torus { radius: 1.0, tube: 0.3, segments: 6, sides: 5 }.build().unwrap(),
        ]
    }

    fn close(a: &[Vec3], b: &[Vec3], eps: f32) {
        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b).enumerate() {
            assert!(x.distance(*y) <= eps, "vertex {i}: {x} vs {y}");
        }
    }

    #[test]
    fn the_linear_map_matches_the_subdivision_operation() {
        for base in bases() {
            let fine = catmull_clark(&base, 1).unwrap();
            let r = Refine::new(&base);
            assert_eq!((r.coarse_count(), r.fine_count()), (base.vertex_count(), fine.vertex_count()));
            close(&r.apply(base.positions()), fine.positions(), 1e-5);
            // Weights sum to one, so a constant mask stays constant.
            assert!(r.apply(&vec![1.0f32; base.vertex_count()]).iter().all(|m| (m - 1.0).abs() < 1e-5));
        }
    }

    fn noisy(m: &PolyMesh, seed: u32, amount: f32) -> Vec<Vec3> {
        let h = |i: u32, k: u32| ((i.wrapping_mul(2654435761).wrapping_add(k.wrapping_mul(40503)).wrapping_add(seed) >> 8) & 0xffff) as f32 / 65535.0 - 0.5;
        m.positions().iter().enumerate().map(|(i, p)| *p + Vec3::new(h(i as u32, 1), h(i as u32, 2), h(i as u32, 3)) * amount).collect()
    }

    #[test]
    fn down_then_up_restores_every_vertex() {
        for base in bases() {
            let fine = catmull_clark(&base, 1).unwrap();
            let r = Refine::new(&base);
            let sculpted = noisy(&fine, 7, 0.2);
            let (coarse, disp) = step_down(&r, &fine, &sculpted).unwrap();
            close(&step_up(&r, &fine, &coarse, Some(&disp)).unwrap(), &sculpted, 2e-5);
            // Without the stored displacement the level is the plain smooth surface.
            close(&step_up(&r, &fine, &coarse, None).unwrap(), &r.apply(&coarse), 0.0);
        }
    }

    #[test]
    fn the_fitted_coarse_surface_comes_close_to_the_old_vertices() {
        let base = Primitive::Cube { size: Vec3::splat(2.0), segments: 3 }.build().unwrap();
        let fine = catmull_clark(&base, 1).unwrap();
        let r = Refine::new(&base);
        let sculpted = noisy(&fine, 3, 0.1);
        let (coarse, _) = step_down(&r, &fine, &sculpted).unwrap();
        let smooth = r.apply(&coarse);
        close(&smooth[..base.vertex_count()], &sculpted[..base.vertex_count()], 2e-3);
        // Plain copying of the old vertices would leave the smooth surface much further away.
        let copied = r.apply(&sculpted[..base.vertex_count()]);
        assert!(copied[..base.vertex_count()].iter().zip(&sculpted).any(|(a, b)| a.distance(*b) > 2e-2));
    }

    #[test]
    fn mismatched_meshes_are_an_error() {
        let a = Primitive::Cube { size: Vec3::ONE, segments: 1 }.build().unwrap();
        let b = Primitive::Cube { size: Vec3::ONE, segments: 3 }.build().unwrap();
        let r = Refine::new(&a);
        assert!(step_up(&r, &b, a.positions(), None).is_err());
        assert!(step_down(&r, &b, b.positions()).is_err());
        assert!(step_up(&r, &catmull_clark(&a, 1).unwrap(), b.positions(), None).is_err());
    }

    #[test]
    fn shape_layer_strength_scales_what_was_recorded() {
        let anchor = vec![Vec3::ZERO, Vec3::X];
        let mut pos = vec![Vec3::Y, Vec3::X];
        let mut delta = vec![Vec3::ZERO; 2];
        assert!(shape::settle(&mut delta, &pos, &anchor, 0.5));
        assert_eq!(delta, vec![Vec3::Y * 2.0, Vec3::ZERO]);
        shape::add_scaled(&mut pos, &delta, -0.5);
        assert_eq!(pos, anchor);
        assert!(!shape::settle(&mut delta, &pos, &anchor, 0.0), "zero strength records nothing");
        assert!(!shape::settle(&mut delta, &pos, &anchor[..1], 1.0));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]
        /// Detail rides on the coarse surface: turning and moving the coarse level turns and
        /// moves the fine detail with it.
        #[test]
        fn detail_follows_a_rigid_motion_of_the_coarse_level(seed in 0u32..1000, ax in -1.0f32..1.0, ay in -1.0f32..1.0, angle in -3.0f32..3.0, tx in -2.0f32..2.0) {
            let base = Primitive::Cube { size: Vec3::splat(2.0), segments: 2 }.build().unwrap();
            let fine = catmull_clark(&base, 1).unwrap();
            let r = Refine::new(&base);
            let sculpted = noisy(&fine, seed, 0.15);
            let (coarse, disp) = step_down(&r, &fine, &sculpted).unwrap();
            let q = Quat::from_axis_angle(Vec3::new(ax, ay, 0.3).normalize(), angle);
            let mv = |p: &Vec3| q * *p + Vec3::new(tx, 0.5, -0.25);
            let moved: Vec<Vec3> = coarse.iter().map(mv).collect();
            let up = step_up(&r, &fine, &moved, Some(&disp)).unwrap();
            let want: Vec<Vec3> = sculpted.iter().map(mv).collect();
            for (a, b) in up.iter().zip(&want) {
                prop_assert!(a.distance(*b) < 2e-4, "{a} vs {b}");
            }
        }

        #[test]
        fn hostile_values_never_panic(n in 0usize..40, v in prop::num::f32::ANY) {
            let base = Primitive::Plane { width: 1.0, depth: 1.0, segments: 2 }.build().unwrap();
            let fine = catmull_clark(&base, 1).unwrap();
            let r = Refine::new(&base);
            let junk = vec![Vec3::splat(v); n];
            let _ = step_up(&r, &fine, &junk, Some(&junk));
            let _ = step_down(&r, &fine, &junk);
            let _ = r.apply(&junk);
        }
    }
}

mod live_detail {
    use super::*;
    use modelcraft_mesh::{PolyMesh, Polygon};
    use proptest::prelude::*;

    fn live(kind: BrushKind, radius: f32, edge: f32) -> BrushSettings {
        let mut b = BrushSettings::new(kind, radius);
        b.live_detail = Some(edge);
        b
    }

    /// Build the triangles as a mesh and insist nothing had to be repaired.
    fn as_clean_mesh(s: &SculptMesh) -> PolyMesh {
        let polys = s.triangles().iter().map(|t| Polygon::new(t.to_vec())).collect();
        let (m, report) = PolyMesh::build(s.positions.clone(), polys).unwrap();
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(m.vertex_count(), s.vertex_count());
        m
    }

    fn closed(m: &PolyMesh) -> bool {
        (0..m.edge_count() as u32).all(|e| !m.is_boundary_edge(e))
    }

    #[test]
    fn a_stroke_adds_triangles_under_the_brush_only() {
        let mut s = ball(6);
        let (v0, t0) = (s.vertex_count(), s.triangle_count());
        let u = stroke(&mut s, live(BrushKind::Draw, 0.5, 0.08), &[Vec3::Y, Vec3::new(0.2, 1.0, 0.0).normalize()]);
        assert!(s.triangle_count() > t0 * 3, "{} -> {}", t0, s.triangle_count());
        assert!(u.topo.is_some() && s.topology_changed());
        let m = as_clean_mesh(&s);
        assert!(closed(&m), "no cracks");
        assert_eq!(m.vertex_count() as i64 - m.edge_count() as i64 + m.face_count() as i64, 2);
        // Near the brush centre the edges are down to the detail size; the far side is untouched.
        let near: Vec<f32> = (0..m.edge_count() as u32)
            .map(|e| m.edge_vertices(e))
            .filter(|(a, b)| ((m.position(*a) + m.position(*b)) * 0.5).distance(Vec3::Y) < 0.25)
            .map(|(a, b)| m.position(a).distance(m.position(b)))
            .collect();
        assert!(!near.is_empty() && near.iter().all(|l| *l <= 0.08 * 1.6), "{:?}", near.iter().cloned().fold(0.0f32, f32::max));
        assert!(s.positions.iter().take(v0).filter(|p| p.y < -0.5).all(|p| (p.length() - 1.0).abs() < 1e-6));
        assert!(s.triangles().iter().filter(|t| t.iter().all(|v| s.positions[*v as usize].y < -0.5)).count() > 20);
        // Picking still works on the new triangles, and normals are unit length.
        let (_, p, _) = s.raycast(Vec3::new(0.0, 5.0, 0.0), -Vec3::Y).unwrap();
        assert!(p.y > 1.0);
        assert!(s.normals.iter().all(|n| (n.length() - 1.0).abs() < 1e-3));
        assert_eq!(s.normals.len(), s.vertex_count());
    }

    #[test]
    fn undo_and_redo_restore_the_triangles_exactly() {
        let mut s = ball(6);
        let (p0, t0) = (s.positions.clone(), s.triangles().to_vec());
        let u1 = stroke(&mut s, live(BrushKind::Clay, 0.4, 0.1), &[Vec3::X]);
        let u2 = stroke(&mut s, live(BrushKind::Inflate, 0.4, 0.05), &[Vec3::new(1.0, 0.2, 0.0).normalize(), Vec3::X]);
        let (p2, t2, m2) = (s.positions.clone(), s.triangles().to_vec(), s.mask.clone());
        s.apply_undo(&u2, false);
        s.apply_undo(&u1, false);
        assert_eq!(s.positions, p0);
        assert_eq!(s.triangles(), t0.as_slice());
        assert!(!s.topology_changed());
        assert!(s.raycast(Vec3::new(5.0, 0.0, 0.0), -Vec3::X).is_some());
        s.apply_undo(&u1, true);
        s.apply_undo(&u2, true);
        assert_eq!((s.positions.clone(), s.triangles().to_vec(), s.mask.clone()), (p2, t2, m2));
        as_clean_mesh(&s);
        // A record that does not fit the mesh is refused whole.
        let before = s.positions.clone();
        s.apply_undo(&u1, true);
        assert_eq!(s.positions, before);
        // Fixed-topology strokes still work after live ones, and undo normally.
        let u3 = stroke(&mut s, BrushSettings::new(BrushKind::Draw, 0.3), &[Vec3::X]);
        assert!(u3.topo.is_none() && !u3.is_empty());
        s.apply_undo(&u3, false);
        assert_eq!(s.positions, before);
    }

    #[test]
    fn replaying_a_stroke_gives_the_same_mesh() {
        let b = live(BrushKind::Draw, 0.45, 0.06);
        let dabs: Vec<Dab> = (0..12)
            .map(|i| {
                let c = Vec3::new((i as f32 * 0.1).sin(), 1.0, (i as f32 * 0.1).cos() * 0.3).normalize();
                Dab { center: c, view_dir: -c, pressure: 0.8, grab_delta: Vec3::ZERO, scale: 1.0 }
            })
            .collect();
        let mut sym = b.clone();
        sym.symmetry.x = true;
        for brush in [b, sym] {
            let (mut a, mut c) = (ball(8), ball(8));
            let st = Stroke { brush, dabs: dabs.clone() };
            a.replay(&st);
            c.replay(&st);
            assert_eq!(a.positions, c.positions);
            assert_eq!(a.triangles(), c.triangles());
            as_clean_mesh(&a);
        }
    }

    #[test]
    fn masked_surfaces_and_the_mask_brush_are_left_alone() {
        let mut s = ball(6);
        s.mask_all(1.0);
        let t0 = s.triangle_count();
        let u = stroke(&mut s, live(BrushKind::Draw, 0.5, 0.05), &[Vec3::Y]);
        assert_eq!(s.triangle_count(), t0);
        assert!(u.is_empty());
        s.mask_all(0.0);
        stroke(&mut s, live(BrushKind::Mask, 0.5, 0.05), &[Vec3::Y]);
        assert_eq!(s.triangle_count(), t0);
        // The detail size is bounded by the brush radius, so a tiny size cannot explode.
        stroke(&mut s, live(BrushKind::Draw, 0.5, 1e-9), &[Vec3::Y]);
        assert!(s.triangle_count() < 200_000, "{}", s.triangle_count());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]
        #[test]
        fn random_live_strokes_stay_valid_and_undo_cleanly(
            dabs in prop::collection::vec((-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0, 0.0f32..1.0), 1..6),
            radius in 0.05f32..0.9, edge in prop::num::f32::ANY, kind in 0usize..8, sym in any::<bool>()
        ) {
            let mut s = ball(4);
            let (p0, t0) = (s.positions.clone(), s.triangles().to_vec());
            let mut b = live(crate::brush::KINDS[kind], radius, edge);
            b.symmetry.x = sym;
            s.begin_stroke(b);
            for (x, y, z, pr) in dabs {
                let c = Vec3::new(x, y, z).normalize_or(Vec3::Y);
                s.dab(Dab { center: c, view_dir: -c, pressure: pr, grab_delta: Vec3::new(0.01, 0.0, 0.0), scale: 1.0 });
            }
            let u = s.end_stroke().unwrap().0;
            let m = as_clean_mesh(&s);
            prop_assert!(closed(&m));
            prop_assert!(s.positions.iter().all(|p| p.is_finite()));
            s.apply_undo(&u, false);
            prop_assert_eq!(&s.positions, &p0);
            prop_assert_eq!(s.triangles(), t0.as_slice());
        }
    }
}

mod brush_depth {
    use super::*;
    use crate::alpha::{ALPHAS, SIDE};
    use crate::preset::{self, BrushPreset};
    use proptest::prelude::*;

    fn radii(s: &SculptMesh) -> (f32, f32) {
        s.positions.iter().map(|p| p.length()).fold((f32::MAX, 0.0f32), |(lo, hi), r| (lo.min(r), hi.max(r)))
    }

    #[test]
    fn trim_only_lowers_and_fill_only_raises() {
        let mut bumpy = ball(24);
        stroke(&mut bumpy, BrushSettings::new(BrushKind::Draw, 0.15), &[Vec3::Y; 6]);
        let before = bumpy.positions.clone();
        let mut b = BrushSettings::new(BrushKind::Trim, 0.4);
        b.strength = 1.0;
        stroke(&mut bumpy, b.clone(), &[Vec3::Y * 1.02; 3]);
        let moved: Vec<f32> = bumpy.positions.iter().zip(&before).map(|(a, o)| (*a - *o).dot(Vec3::Y)).collect();
        assert!(moved.iter().all(|d| *d <= 1e-4), "trim raised something");
        assert!(moved.iter().any(|d| *d < -1e-3), "trim lowered nothing");
        let before = bumpy.positions.clone();
        b.kind = BrushKind::Fill;
        stroke(&mut bumpy, b, &[Vec3::Y * 1.02; 3]);
        let moved: Vec<f32> = bumpy.positions.iter().zip(&before).map(|(a, o)| (*a - *o).dot(Vec3::Y)).collect();
        assert!(moved.iter().all(|d| *d >= -1e-4), "fill lowered something");
        assert!(moved.iter().any(|d| *d > 1e-3), "fill raised nothing");
    }

    #[test]
    fn plateau_stops_at_one_step_while_draw_keeps_piling() {
        let top = [Vec3::Y; 8];
        let (mut once, mut often, mut draw) = (ball(24), ball(24), ball(24));
        stroke(&mut once, BrushSettings::new(BrushKind::Plateau, 0.3), &top[..1]);
        stroke(&mut often, BrushSettings::new(BrushKind::Plateau, 0.3), &top);
        stroke(&mut draw, BrushSettings::new(BrushKind::Draw, 0.3), &top);
        assert!(radii(&once).1 > 1.01);
        assert!((radii(&once).1 - radii(&often).1).abs() < 2e-3, "{} vs {}", radii(&once).1, radii(&often).1);
        assert!(radii(&draw).1 > radii(&often).1 + 0.02);
    }

    #[test]
    fn surface_grab_leaves_the_other_side_of_a_thin_wall_alone() {
        let slab = Primitive::Cube { size: Vec3::new(2.0, 0.05, 2.0), segments: 10 }.build().unwrap();
        let pull = |kind| {
            let mut s = SculptMesh::new(&slab, None, None);
            let before = s.positions.clone();
            s.begin_stroke(BrushSettings::new(kind, 0.5));
            s.dab(Dab { center: Vec3::new(0.0, 0.025, 0.0), view_dir: -Vec3::Y, pressure: 1.0, grab_delta: Vec3::Y * 0.3, scale: 1.0 });
            s.end_stroke();
            let moved = |top: bool| {
                s.positions
                    .iter()
                    .zip(&before)
                    .filter(|(_, o)| (o.y > 0.0) == top && o.x.abs() < 0.3 && o.z.abs() < 0.3)
                    .map(|(a, o)| a.y - o.y)
                    .fold(0.0f32, f32::max)
            };
            (moved(true), moved(false))
        };
        let (top, bottom) = pull(BrushKind::Grab);
        assert!(top > 0.2 && bottom > 0.2, "a plain grab takes both sides: {top} {bottom}");
        let (top, bottom) = pull(BrushKind::SurfaceGrab);
        assert!(top > 0.2 && bottom == 0.0, "a surface grab takes one side: {top} {bottom}");
    }

    #[test]
    fn pull_and_slide_follow_the_stroke_direction() {
        let dir = Vec3::new(0.2, 0.0, 0.0);
        for kind in [BrushKind::Pull, BrushKind::Slide] {
            let mut s = ball(24);
            let before = s.positions.clone();
            s.begin_stroke(BrushSettings::new(kind, 0.3));
            s.dab(Dab { center: Vec3::Y, view_dir: -Vec3::Y, pressure: 1.0, grab_delta: dir, scale: 1.0 });
            s.end_stroke();
            let shift: Vec3 = s.positions.iter().zip(&before).map(|(a, o)| *a - *o).sum();
            assert!(shift.x > 0.1 && shift.x.abs() > shift.y.abs() * 3.0, "{kind:?} {shift}");
        }
    }

    #[test]
    fn hardness_keeps_a_full_core_and_new_curves_end_at_zero() {
        let mut b = BrushSettings::new(BrushKind::Draw, 1.0);
        b.hardness = 0.5;
        assert_eq!(b.weight(0.3), 1.0);
        assert!(b.weight(0.75) < 1.0 && b.weight(0.75) > 0.0);
        assert_eq!(b.weight(1.0), 0.0);
        b.hardness = f32::NAN;
        assert!(b.weight(0.5).is_finite());
        for f in [Falloff::Root, Falloff::Needle] {
            assert_eq!(f.weight(0.0), 1.0);
            assert_eq!(f.weight(1.0), 0.0);
            assert!(f.weight(0.3) > f.weight(0.6));
        }
        assert!(Falloff::Root.weight(0.5) > Falloff::Linear.weight(0.5) && Falloff::Needle.weight(0.5) < Falloff::Sharp.weight(0.5));
    }

    #[test]
    fn built_in_pictures_are_in_range_and_shape_a_dab() {
        for a in ALPHAS {
            let px = a.pixels();
            assert_eq!(px.len(), SIDE * SIDE, "{a:?}");
            assert!(px.iter().all(|v| (0.0..=1.0).contains(v)), "{a:?}");
            let (lo, hi) = px.iter().fold((1.0f32, 0.0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
            assert!(hi - lo > 0.6, "{a:?} is nearly flat: {lo}..{hi}");
            assert_eq!(a.sample(1.5, 0.0), 0.0);
            assert_eq!(a.sample(f32::NAN, 0.0), 0.0);
        }
        assert!(Alpha::Star.sample(0.0, 0.0) > 0.99 && Alpha::Star.sample(0.95, 0.95) < 0.01);
        assert_eq!(crate::alpha::sample(&[1.0; 3], 2, 0.0, 0.0), 0.0, "a picture of the wrong size samples as empty");
        // A stamped dab leaves a pattern: parts of the brush area stay put, others rise.
        let mut s = ball(48);
        let mut b = BrushSettings::new(BrushKind::Draw, 0.5);
        b.stamp = Stamp::Image { alpha: Alpha::Dots, turn: 0.3 };
        b.falloff = Falloff::Constant;
        stroke(&mut s, b, &[Vec3::Y; 4]);
        let under: Vec<f32> = s.positions.iter().filter(|p| p.distance(Vec3::Y) < 0.3).map(|p| p.length()).collect();
        let (lo, hi) = under.iter().fold((f32::MAX, 0.0f32), |(lo, hi), r| (lo.min(*r), hi.max(*r)));
        assert!(hi - lo > 0.01, "{lo}..{hi}");
    }

    fn maker(mode: StrokeMode) -> DabMaker {
        let mut b = BrushSettings::new(BrushKind::Draw, 0.1);
        b.stroke = mode;
        DabMaker::new(&b)
    }

    fn feed(m: &mut DabMaker, n: usize) -> Vec<DabOut> {
        (0..n).map(|i| m.push(Vec3::new(i as f32 * 0.013, 0.0, 0.0), -Vec3::Y, 1.0)).collect()
    }

    #[test]
    fn stroke_modes_place_dabs_as_described() {
        // Spaced: even steps of spacing × radius, each carrying its movement.
        let dabs: Vec<Dab> = feed(&mut maker(StrokeMode::Spaced), 40).into_iter().flat_map(|o| o.dabs).collect();
        assert!(dabs.len() >= 19, "{}", dabs.len());
        for w in dabs.windows(2).skip(1) {
            let d = w[1].center.distance(w[0].center);
            assert!((0.019..0.027).contains(&d), "{d}");
            assert!((w[1].grab_delta - (w[1].center - w[0].center)).length() < 1e-6);
        }
        // Dots: at least a radius apart.
        let dots: Vec<Dab> = feed(&mut maker(StrokeMode::Dots), 40).into_iter().flat_map(|o| o.dabs).collect();
        assert!(dots.len() >= 4 && dots.windows(2).all(|w| w[1].center.distance(w[0].center) >= 0.1 - 1e-6));
        // Spray: off the path but within the scatter, sizes vary, and the same every time.
        let spray = |_: ()| feed(&mut maker(StrokeMode::Spray { scatter: 2.0 }), 40).into_iter().flat_map(|o| o.dabs).collect::<Vec<Dab>>();
        let (a, b) = (spray(()), spray(()));
        assert_eq!(a, b);
        assert!(a.iter().skip(1).all(|d| d.center.z.abs() <= 0.2 + 1e-5 && d.center.y.abs() < 1e-5 && (0.4..=1.0).contains(&d.scale)));
        assert!(a.iter().any(|d| d.center.z.abs() > 0.02) && a.iter().any(|d| d.scale < 0.7));
        // Trail: the brush stays a leash behind the pointer.
        let mut m = maker(StrokeMode::Trail { leash: 2.0 });
        let last = feed(&mut m, 60).into_iter().flat_map(|o| o.dabs).last().unwrap();
        let pointer = 59.0 * 0.013;
        assert!((pointer - last.center.x - 0.2).abs() < 0.03, "{} behind", pointer - last.center.x);
        // Drag: every sample replaces the one dab, which stays put and grows.
        let outs = feed(&mut maker(StrokeMode::Drag), 10);
        assert!(!outs[0].rewind && outs[1..].iter().all(|o| o.rewind && o.dabs.len() == 1 && o.dabs[0].center == Vec3::ZERO));
        assert!(outs[9].dabs[0].scale > outs[2].dabs[0].scale);
    }

    #[test]
    fn a_drag_stroke_ends_as_its_last_dab_alone() {
        let mut b = BrushSettings::new(BrushKind::Draw, 0.2);
        b.stroke = StrokeMode::Drag;
        let mut live = ball(24);
        live.begin_stroke(b.clone());
        let mut m = DabMaker::new(&b);
        let mut last = None;
        for i in 0..12 {
            let out = m.push(Vec3::new(i as f32 * 0.03, 1.0, 0.0).normalize(), -Vec3::Y, 1.0);
            if out.rewind {
                assert!(live.rewind_stroke());
            }
            for d in out.dabs {
                live.dab(d);
                last = Some(d);
            }
        }
        let (_, recorded) = live.end_stroke().unwrap();
        assert_eq!(recorded.dabs, vec![last.unwrap()], "the journal holds one dab");
        let mut replayed = ball(24);
        replayed.replay(&recorded);
        assert_eq!(replayed.positions, live.positions);
        assert!(!ball(8).rewind_stroke(), "nothing to rewind outside a stroke");
    }

    #[test]
    fn presets_are_distinct_and_valid() {
        let all = preset::builtin();
        let mut names: Vec<&str> = all.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len());
        for p in &all {
            assert_eq!(p.brush.clone().sanitized(), p.brush, "{}", p.name);
        }
        let mut current = BrushSettings::new(BrushKind::Draw, 0.77);
        current.symmetry.x = true;
        current.live_detail = Some(0.1);
        let applied = all[2].apply(&current);
        assert_eq!((applied.kind, applied.radius, applied.symmetry.x, applied.live_detail), (all[2].brush.kind, 0.77, true, Some(0.1)));
        let saved = BrushPreset::new("  My\tbrush  ", &current);
        assert_eq!(saved.name, "Mybrush");
        assert!(saved.brush.live_detail.is_none() && !saved.brush.symmetry.x);
        let mut lib = all.clone();
        assert!(preset::upsert(&mut lib, saved.clone()) && preset::upsert(&mut lib, saved));
        assert_eq!(lib.len(), all.len() + 1);
        let mut bad = current.clone();
        (bad.radius, bad.strength, bad.hardness, bad.spacing) = (f32::NAN, 9.0, -3.0, f32::INFINITY);
        bad.stroke = StrokeMode::Spray { scatter: f32::NAN };
        let ok = bad.sanitized();
        assert!(ok.radius.is_finite() && ok.strength == 1.0 && ok.hardness == 0.0 && ok.spacing.is_finite());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]
        #[test]
        fn hostile_samples_never_panic(pts in prop::collection::vec((prop::num::f32::ANY, prop::num::f32::ANY, prop::num::f32::ANY), 0..30), mode in 0usize..5, radius in prop::num::f32::ANY, spacing in prop::num::f32::ANY) {
            let mut b = BrushSettings::new(BrushKind::Draw, radius);
            b.spacing = spacing;
            b.stroke = [StrokeMode::Spaced, StrokeMode::Dots, StrokeMode::Spray { scatter: spacing }, StrokeMode::Trail { leash: spacing }, StrokeMode::Drag][mode];
            let mut m = DabMaker::new(&b);
            let mut s = ball(4);
            s.begin_stroke(b);
            for (x, y, z) in pts {
                let out = m.push(Vec3::new(x, y, z), Vec3::new(z, x, y), x);
                prop_assert!(out.dabs.len() <= 64);
                if out.rewind { s.rewind_stroke(); }
                for d in out.dabs { s.dab(d); }
            }
            s.end_stroke();
            prop_assert!(s.positions.iter().all(|p| p.is_finite()));
        }
    }
}
