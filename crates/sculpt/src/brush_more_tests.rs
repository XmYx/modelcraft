//! Rotate, Elastic Move and Scrape, and radial symmetry.

use glam::{Mat3, Vec3};
use modelcraft_ops::primitives::Primitive;

use crate::brush::{MAX_RADIAL, Symmetry};
use crate::{BrushKind, BrushSettings, Dab, SculptMesh, Stroke, StrokeUndo};

fn ball(seg: u32) -> SculptMesh {
    SculptMesh::new(&Primitive::RoundCube { radius: 1.0, segments: seg }.build().unwrap(), None, None)
}

fn volume(s: &SculptMesh) -> f32 {
    s.triangles().iter().map(|t| s.positions[t[0] as usize].dot(s.positions[t[1] as usize].cross(s.positions[t[2] as usize])) / 6.0).sum()
}

/// A stroke of `steps` dabs from `at`; with `travel` the centre goes along with `delta`.
fn stroke(s: &mut SculptMesh, b: BrushSettings, at: Vec3, view: Vec3, delta: Vec3, steps: usize, travel: bool) -> (StrokeUndo, Stroke) {
    s.begin_stroke(b);
    let mut c = at;
    for _ in 0..steps {
        if travel {
            c += delta;
        }
        s.dab(Dab { center: c, view_dir: view, pressure: 1.0, grab_delta: delta, scale: 1.0 });
    }
    s.end_stroke().unwrap()
}

fn from_axis(p: Vec3) -> f32 {
    Vec3::new(p.x, 0.0, p.z).length()
}

#[test]
fn rotate_turns_what_it_caught_around_the_view_direction() {
    let mut s = ball(24);
    let before = s.positions.clone();
    let v0 = volume(&s);
    // Looking down at the top; the pointer moves sideways (for this view, sideways is Z).
    let (view, delta) = (-Vec3::Y, Vec3::new(0.0, 0.0, 0.05));
    let (u, recorded) = stroke(&mut s, BrushSettings::new(BrushKind::Rotate, 0.6), Vec3::Y, view, delta, 8, false);
    let after = s.positions.clone();
    assert!(u.indices.len() > 10);
    for v in &u.indices {
        let (p, q) = (before[*v as usize], after[*v as usize]);
        // A turn around the axis through the brush centre: height and distance to the axis stay.
        assert!((p.y - q.y).abs() < 1e-5, "height changed");
        assert!((from_axis(p) - from_axis(q)).abs() < 1e-4, "distance to the axis changed");
    }
    assert!((volume(&s) - v0).abs() < 1e-3, "a turn in place keeps the volume");
    // Replay gives the same surface; undo puts it back.
    let mut again = ball(24);
    again.replay(&recorded);
    assert_eq!(again.positions, after);
    s.apply_undo(&u, false);
    assert_eq!(s.positions, before);
    // Inverting turns the other way.
    let mut inv = BrushSettings::new(BrushKind::Rotate, 0.6);
    inv.invert = true;
    let mut t = ball(24);
    stroke(&mut t, inv, Vec3::Y, view, delta, 8, false);
    let v = u.indices.iter().copied().find(|v| from_axis(before[*v as usize]) > 0.2).unwrap() as usize;
    let cross = |a: Vec3, b: Vec3| a.x * b.z - a.z * b.x;
    assert!(cross(before[v], after[v]) * cross(before[v], t.positions[v]) < 0.0);
}

#[test]
fn elastic_move_carries_the_centre_fades_with_distance_and_keeps_the_volume() {
    let mut s = ball(32);
    let before = s.positions.clone();
    let v0 = volume(&s);
    // Slide the surface sideways at the top: five steps of 0.02.
    let delta = Vec3::new(0.02, 0.0, 0.0);
    let (u, recorded) = stroke(&mut s, BrushSettings::new(BrushKind::ElasticMove, 0.25), Vec3::Y, -Vec3::Y, delta, 5, true);
    let top = (0..before.len()).min_by(|a, b| before[*a].distance(Vec3::Y).total_cmp(&before[*b].distance(Vec3::Y))).unwrap();
    let moved = s.positions[top] - before[top];
    assert!(moved.x > 0.06 && moved.x <= 0.1001, "the centre goes with the pointer: {moved}");
    // Nothing beyond three radii of the path moves, and the pull falls off on the way there.
    let far: Vec<usize> = (0..before.len()).filter(|i| before[*i].distance(Vec3::Y) > 0.25 * 3.0 + 0.1 + 1e-3).collect();
    assert!(!far.is_empty() && far.iter().all(|i| s.positions[*i] == before[*i]));
    let mid = (0..before.len()).find(|i| (before[*i].distance(Vec3::Y) - 0.4).abs() < 0.03).unwrap();
    let m = (s.positions[mid] - before[mid]).length();
    assert!(m > 0.0 && m < moved.length() * 0.7, "{m} against {}", moved.length());
    // Rubber, not clay: the volume stays.
    let dv = (volume(&s) - v0).abs() / v0;
    assert!(dv < 0.002, "{dv}");
    let mut again = ball(32);
    again.replay(&recorded);
    assert_eq!(again.positions, s.positions);
    s.apply_undo(&u, false);
    assert_eq!(s.positions, before);
    // The mask holds a vertex still.
    s.mask_edit(|i, _, _| if i as usize == top { 1.0 } else { 0.0 });
    stroke(&mut s, BrushSettings::new(BrushKind::ElasticMove, 0.25), Vec3::Y, -Vec3::Y, delta, 3, true);
    assert_eq!(s.positions[top], before[top]);
}

#[test]
fn scrape_lowers_what_stands_proud_and_never_raises() {
    let mut s = ball(32);
    // Raise a bump at the top first.
    let mut build = BrushSettings::new(BrushKind::Draw, 0.2);
    build.strength = 1.0;
    stroke(&mut s, build, Vec3::Y, -Vec3::Y, Vec3::ZERO, 12, false);
    let height = |s: &SculptMesh| s.positions.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    let bumped = s.positions.clone();
    let h0 = height(&s);
    assert!(h0 > 1.05, "{h0}");
    // Each pass starts where the pointer now touches the surface and planes a little deeper.
    let mut touched = Vec::new();
    for _ in 0..4 {
        let at = Vec3::new(0.0, height(&s), 0.0);
        let (u, _) = stroke(&mut s, BrushSettings::new(BrushKind::Scrape, 0.35), at, -Vec3::Y, Vec3::ZERO, 6, false);
        touched.extend(u.indices);
    }
    assert!(height(&s) < h0 - 0.04, "{} from {h0}", height(&s));
    for v in &touched {
        assert!(s.positions[*v as usize].y <= bumped[*v as usize].y + 1e-5, "scrape raised a point");
    }
    // Inverted it fills a hollow instead and never lowers.
    let mut t = ball(32);
    let mut hollow = BrushSettings::new(BrushKind::Draw, 0.2);
    hollow.invert = true;
    hollow.strength = 1.0;
    stroke(&mut t, hollow, Vec3::Y, -Vec3::Y, Vec3::ZERO, 12, false);
    let dented = t.positions.clone();
    let low = t.positions.iter().filter(|p| p.y > 0.0 && from_axis(**p) < 0.05).map(|p| p.y).fold(f32::MAX, f32::min);
    let mut fill = BrushSettings::new(BrushKind::Scrape, 0.35);
    fill.invert = true;
    let (u, _) = stroke(&mut t, fill, Vec3::new(0.0, low, 0.0), -Vec3::Y, Vec3::ZERO, 20, false);
    assert!(!u.indices.is_empty());
    for v in &u.indices {
        assert!(t.positions[*v as usize].y >= dented[*v as usize].y - 1e-5, "the fill lowered a point");
    }
}

#[test]
fn radial_symmetry_repeats_a_dab_around_the_axis() {
    let sym = Symmetry { radial: 6, ..Symmetry::default() };
    let passes = sym.passes();
    assert_eq!(passes.len(), 6);
    assert_eq!(passes[0], Mat3::IDENTITY);
    let p = Vec3::new(1.0, 0.3, 0.0);
    for (k, m) in passes.iter().enumerate() {
        let q = *m * p;
        assert!((q.y - 0.3).abs() < 1e-6 && (q.length() - p.length()).abs() < 1e-6);
        // Turned by k sixths of a circle from the first.
        let cos = Vec3::new(q.x, 0.0, q.z).normalize().dot(Vec3::X);
        assert!((cos - (k as f32 * std::f32::consts::TAU / 6.0).cos()).abs() < 1e-5, "copy {k}");
    }
    // With a mirror too: every combination. The count is capped, and one copy is no copy.
    assert_eq!(Symmetry { x: true, radial: 5, ..Symmetry::default() }.passes().len(), 10);
    assert_eq!(Symmetry { radial: 255, ..Symmetry::default() }.passes().len(), MAX_RADIAL as usize);
    assert_eq!(Symmetry { radial: 1, ..Symmetry::default() }.passes().len(), 1);
    assert_eq!(Symmetry::default().passes(), vec![Mat3::IDENTITY]);
    let z = Symmetry { radial: 4, radial_axis: 2, ..Symmetry::default() }.passes();
    assert!((z[1] * Vec3::Z).distance(Vec3::Z) < 1e-6);
    let unknown = Symmetry { radial: 4, radial_axis: 9, ..Symmetry::default() }.passes();
    assert!((unknown[1] * Vec3::Y).distance(Vec3::Y) < 1e-6, "an unknown axis is Y");
    // A stroke: six bumps around the equator, each as high as a single one.
    let dab = |s: &mut SculptMesh, sym: Symmetry| {
        let mut b = BrushSettings::new(BrushKind::Draw, 0.2);
        b.symmetry = sym;
        b.strength = 1.0;
        stroke(s, b, Vec3::X, -Vec3::X, Vec3::ZERO, 6, false).0
    };
    let (mut one, mut six) = (ball(24), ball(24));
    let (a, b) = (dab(&mut one, Symmetry::default()), dab(&mut six, sym));
    assert!(b.indices.len() > a.indices.len() * 5, "{} against {}", b.indices.len(), a.indices.len());
    let peak = |s: &SculptMesh, dir: Vec3| s.positions.iter().filter(|p| p.normalize().dot(dir) > 0.995).map(|p| p.length()).fold(0.0f32, f32::max);
    let want = peak(&one, Vec3::X);
    assert!(want > 1.02, "{want}");
    for k in 0..6 {
        let angle = k as f32 * std::f32::consts::TAU / 6.0;
        assert!((peak(&six, Vec3::new(angle.cos(), 0.0, angle.sin())) - want).abs() < 0.01, "bump {k}");
    }
    // Old strokes without the new fields read as before, and plain symmetry writes none of them.
    let old: Symmetry = serde_json::from_str(r#"{"x":true,"y":false,"z":false}"#).unwrap();
    assert_eq!(old, Symmetry { x: true, ..Symmetry::default() });
    assert_eq!(serde_json::to_string(&old).unwrap(), r#"{"x":true,"y":false,"z":false}"#);
}
