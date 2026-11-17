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
        s.dab(Dab { center: *c, view_dir: -c.normalize(), pressure: 1.0, grab_delta: Vec3::ZERO });
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
            s.dab(Dab { center: c, view_dir: -Vec3::Y, pressure: 1.0, grab_delta: Vec3::new(0.0, 0.05, 0.0) });
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
        a.dab(Dab { center: Vec3::new(0.0, 1.0, i as f32 * 0.03).normalize(), view_dir: -Vec3::Y, pressure: 0.7, grab_delta: Vec3::ZERO });
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
    s.dab(Dab { center: Vec3::NAN, view_dir: Vec3::Y, pressure: 1.0, grab_delta: Vec3::ZERO });
    s.dab(Dab { center: Vec3::Y, view_dir: Vec3::Y, pressure: f32::INFINITY, grab_delta: Vec3::ZERO });
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
