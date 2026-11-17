//! Cleaning up keyed Euler rotations.
//!
//! The same rotation has many Euler triples: any angle plus a whole turn, and, for every
//! three-axis order, the "flipped" triple `(x + 180°, 180° − y, z + 180°)`. Keys captured from a
//! pose (auto-key, baking, motion capture) can therefore jump between equal rotations, and the
//! curve between two such keys spins the long way round. [`euler_filter`] rewrites each key as
//! the equal triple nearest to the key before it, so the curves are as continuous as the motion.

use crate::curve::{Curve, TIME_EPSILON};

/// The angle equal to `degrees` (mod 360) nearest to `near`.
pub fn nearest_turn(degrees: f64, near: f64) -> f64 {
    degrees + ((near - degrees) / 360.0).round() * 360.0
}

/// Remove whole-turn jumps between neighbouring keys of one angle curve. Returns how many keys
/// changed.
pub fn unwrap_curve(c: &mut Curve) -> usize {
    let mut changed = 0;
    let mut prev: Option<f64> = None;
    for k in &mut c.keys {
        if let Some(p) = prev {
            let v = nearest_turn(k.v, p);
            if (v - k.v).abs() > 1e-9 {
                k.v = v;
                changed += 1;
            }
        }
        prev = Some(k.v);
    }
    if changed > 0 {
        c.retangent();
    }
    changed
}

/// Filter the three angle curves (degrees) of one rotation. When the curves have keys at the
/// same frames, each key is replaced by the nearest of its equal triples (whole turns and the
/// flip); otherwise each curve only loses its whole-turn jumps. Returns how many keys changed.
pub fn euler_filter(x: &mut Curve, y: &mut Curve, z: &mut Curve) -> usize {
    let aligned = x.len() == y.len()
        && y.len() == z.len()
        && x.keys.iter().zip(&y.keys).zip(&z.keys).all(|((a, b), c)| (a.t - b.t).abs() < TIME_EPSILON && (b.t - c.t).abs() < TIME_EPSILON);
    if !aligned {
        return unwrap_curve(x) + unwrap_curve(y) + unwrap_curve(z);
    }
    let mut changed = 0;
    let mut prev: Option<[f64; 3]> = None;
    for i in 0..x.len() {
        let (Some(kx), Some(ky), Some(kz)) = (x.keys.get(i).map(|k| k.v), y.keys.get(i).map(|k| k.v), z.keys.get(i).map(|k| k.v)) else { break };
        let here = [kx, ky, kz];
        let best = match prev {
            None => here,
            Some(p) => {
                let near = |c: [f64; 3]| [nearest_turn(c[0], p[0]), nearest_turn(c[1], p[1]), nearest_turn(c[2], p[2])];
                let dist = |c: [f64; 3]| (c[0] - p[0]).abs() + (c[1] - p[1]).abs() + (c[2] - p[2]).abs();
                let (a, b) = (near(here), near([kx + 180.0, 180.0 - ky, kz + 180.0]));
                if dist(b) + 1e-9 < dist(a) { b } else { a }
            }
        };
        if best.iter().zip(here).any(|(a, b)| (a - b).abs() > 1e-9) {
            changed += 1;
            for (c, v) in [(&mut *x, best[0]), (&mut *y, best[1]), (&mut *z, best[2])] {
                if let Some(k) = c.keys.get_mut(i) {
                    k.v = v;
                }
            }
        }
        prev = Some(best);
    }
    if changed > 0 {
        x.retangent();
        y.retangent();
        z.retangent();
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{EulerRot, Quat};
    use proptest::prelude::*;

    fn quat(x: f64, y: f64, z: f64) -> Quat {
        Quat::from_euler(EulerRot::XYZ, (x as f32).to_radians(), (y as f32).to_radians(), (z as f32).to_radians())
    }

    #[test]
    fn whole_turns_and_flips_are_removed() {
        let mut c = Curve::from_points(&[(0.0, 170.0), (1.0, -175.0), (2.0, -160.0), (3.0, 190.0)]);
        assert_eq!(unwrap_curve(&mut c), 2);
        assert_eq!(c.keys.iter().map(|k| k.v).collect::<Vec<_>>(), vec![170.0, 185.0, 200.0, 190.0]);
        // A flipped triple in the middle of a smooth turn about Y.
        let mut x = Curve::from_points(&[(0.0, 0.0), (1.0, 180.0), (2.0, 0.0)]);
        let mut y = Curve::from_points(&[(0.0, 80.0), (1.0, 85.0), (2.0, 100.0)]);
        let mut z = Curve::from_points(&[(0.0, 0.0), (1.0, -180.0), (2.0, 0.0)]);
        assert_eq!(euler_filter(&mut x, &mut y, &mut z), 1);
        assert_eq!((x.keys[1].v, y.keys[1].v, z.keys[1].v), (0.0, 95.0, 0.0));
        // Curves with different key times: only whole turns go.
        let mut x = Curve::from_points(&[(0.0, 0.0), (5.0, 350.0)]);
        let mut y = Curve::from_points(&[(0.0, 0.0)]);
        let mut z = Curve::new();
        assert_eq!(euler_filter(&mut x, &mut y, &mut z), 1);
        assert_eq!(x.keys[1].v, -10.0);
    }

    proptest! {
        #[test]
        fn the_filter_keeps_every_rotation_and_never_lengthens_a_step(keys in proptest::collection::vec((-720.0f64..720.0, -720.0f64..720.0, -720.0f64..720.0), 1..12)) {
            let pts = |f: fn(&(f64, f64, f64)) -> f64| keys.iter().enumerate().map(|(i, k)| (i as f64, f(k))).collect::<Vec<_>>();
            let (mut x, mut y, mut z) = (Curve::from_points(&pts(|k| k.0)), Curve::from_points(&pts(|k| k.1)), Curve::from_points(&pts(|k| k.2)));
            let before: Vec<Quat> = keys.iter().map(|k| quat(k.0, k.1, k.2)).collect();
            let step = |x: &Curve, y: &Curve, z: &Curve, i: usize| (x.keys[i].v - x.keys[i - 1].v).abs() + (y.keys[i].v - y.keys[i - 1].v).abs() + (z.keys[i].v - z.keys[i - 1].v).abs();
            euler_filter(&mut x, &mut y, &mut z);
            for (i, q) in before.iter().enumerate() {
                let after = quat(x.keys[i].v, y.keys[i].v, z.keys[i].v);
                prop_assert!(q.dot(after).abs() > 0.999, "key {i} changed its rotation");
                if i > 0 {
                    prop_assert!(step(&x, &y, &z, i) <= 540.0 + 1e-6, "no step is longer than half a turn per angle");
                }
            }
        }
    }
}
