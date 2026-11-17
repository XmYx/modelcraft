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
