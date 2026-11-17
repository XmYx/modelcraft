//! Inverse kinematics on a chain of points.
//!
//! Every solver takes the chain's joint positions (root first, effector last) and a goal, keeps
//! the root where it is and every bone at its length, and returns the new positions. Turning
//! positions back into joint rotations is the caller's job ([`aim_delta`]).
//!
//! - [`two_bone`]: the closed-form solution for two bones (law of cosines), bending towards a
//!   pole point.
//! - [`fabrik`]: forward-and-backward reaching (Aristidou and Lasenby, "FABRIK: a fast, iterative
//!   solver for the inverse kinematics problem", 2011).
//! - [`ccd`]: cyclic coordinate descent (Wang and Chen, 1991): each joint in turn swings the
//!   effector towards the goal.
//! - [`apply_pole`]: turns a solved chain about the root–effector line so that it bends towards
//!   a pole point.
//!
//! Nothing here panics: degenerate chains (zero-length bones, non-finite input) come back
//! unchanged, iteration counts and chain lengths are capped.

use glam::{Quat, Vec3};

/// Longest chain a solver accepts (joints).
pub const MAX_CHAIN: usize = 256;
/// Most iterations of an iterative solver.
pub const MAX_ITERATIONS: u32 = 200;

const EPS: f32 = 1e-6;

fn usable(points: &[Vec3], goal: Vec3) -> bool {
    points.len() >= 2 && points.len() <= MAX_CHAIN && goal.is_finite() && points.iter().all(|p| p.is_finite())
}

fn lengths(points: &[Vec3]) -> Vec<f32> {
    points.windows(2).map(|w| w.get(1).zip(w.first()).map_or(0.0, |(b, a)| (*b - *a).length())).collect()
}

/// Any unit vector at a right angle to `v`.
fn perpendicular(v: Vec3) -> Vec3 {
    let other = if v.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    v.cross(other).normalize_or(Vec3::Z)
}

/// The part of `v` at a right angle to the unit vector `axis`, normalized.
fn across(v: Vec3, axis: Vec3) -> Option<Vec3> {
    let r = v - axis * v.dot(axis);
    (r.length_squared() > EPS * EPS).then(|| r.normalize())
}

/// Two bones (`root`–`mid`–`end`) reaching for `goal`. The middle joint bends towards `pole`
/// when given, otherwise it keeps the side it is on now. A goal out of reach leaves the chain
/// straight towards it. Returns the new middle and end positions.
pub fn two_bone(root: Vec3, mid: Vec3, end: Vec3, goal: Vec3, pole: Option<Vec3>) -> (Vec3, Vec3) {
    if !usable(&[root, mid, end], goal) {
        return (mid, end);
    }
    let (a, b) = ((mid - root).length(), (end - mid).length());
    if a < EPS || b < EPS {
        return (mid, end);
    }
    let to_goal = goal - root;
    let axis = if to_goal.length() > EPS { to_goal.normalize() } else { (end - root).normalize_or((mid - root).normalize_or(Vec3::Y)) };
    // Keep the triangle possible: between fully folded and fully stretched.
    let d = to_goal.length().clamp((a - b).abs() + EPS, a + b - EPS).max(EPS);
    let along = (a * a - b * b + d * d) / (2.0 * d);
    let height = (a * a - along * along).max(0.0).sqrt();
    let bend = pole.filter(|p| p.is_finite()).and_then(|p| across(p - root, axis)).or_else(|| across(mid - root, axis)).unwrap_or_else(|| perpendicular(axis));
    (root + axis * along + bend * height, root + axis * d)
}

/// FABRIK. Stops when the effector is within `tolerance` of the goal or after `iterations`
/// passes. Returns the new positions (the same length as `points`).
pub fn fabrik(points: &[Vec3], goal: Vec3, iterations: u32, tolerance: f32) -> Vec<Vec3> {
    let mut p = points.to_vec();
    if !usable(points, goal) {
        return p;
    }
    let len = lengths(points);
    let total: f32 = len.iter().sum();
    let Some(root) = p.first().copied() else { return p };
    if total < EPS {
        return p;
    }
    let n = p.len();
    if (goal - root).length() >= total {
        // Out of reach: a straight line towards the goal.
        let dir = (goal - root).normalize_or(Vec3::Y);
        let mut at = root;
        for (slot, l) in p.iter_mut().skip(1).zip(&len) {
            at += dir * *l;
            *slot = at;
        }
        return p;
    }
    let tolerance = if tolerance.is_finite() { tolerance.max(1e-6) } else { 1e-4 };
    for _ in 0..iterations.clamp(1, MAX_ITERATIONS) {
        if p.last().is_some_and(|e| (*e - goal).length() <= tolerance) {
            break;
        }
        // Backward: put the effector on the goal and pull the chain after it.
        if let Some(last) = p.last_mut() {
            *last = goal;
        }
        for i in (0..n - 1).rev() {
            let (Some(next), Some(here), Some(l)) = (p.get(i + 1).copied(), p.get(i).copied(), len.get(i)) else { continue };
            let dir = (here - next).normalize_or(perpendicular(Vec3::Y));
            if let Some(slot) = p.get_mut(i) {
                *slot = next + dir * *l;
            }
        }
        // Forward: put the root back and push the chain out again.
        if let Some(first) = p.first_mut() {
            *first = root;
        }
        for i in 0..n - 1 {
            let (Some(here), Some(next), Some(l)) = (p.get(i).copied(), p.get(i + 1).copied(), len.get(i)) else { continue };
            let dir = (next - here).normalize_or(perpendicular(Vec3::Y));
            if let Some(slot) = p.get_mut(i + 1) {
                *slot = here + dir * *l;
            }
        }
    }
    p
}

/// Cyclic coordinate descent: from the last joint to the root, turn each joint so the effector
/// swings towards the goal. Bone lengths are kept exactly (points are only rotated).
pub fn ccd(points: &[Vec3], goal: Vec3, iterations: u32, tolerance: f32) -> Vec<Vec3> {
    let mut p = points.to_vec();
    if !usable(points, goal) {
        return p;
    }
    let n = p.len();
    let tolerance = if tolerance.is_finite() { tolerance.max(1e-6) } else { 1e-4 };
    for _ in 0..iterations.clamp(1, MAX_ITERATIONS) {
        if p.last().is_some_and(|e| (*e - goal).length() <= tolerance) {
            break;
        }
        for j in (0..n - 1).rev() {
            let (Some(pivot), Some(effector)) = (p.get(j).copied(), p.last().copied()) else { continue };
            let (from, to) = (effector - pivot, goal - pivot);
            if from.length_squared() < EPS * EPS || to.length_squared() < EPS * EPS {
                continue;
            }
            let turn = Quat::from_rotation_arc(from.normalize(), to.normalize());
            if !turn.is_finite() {
                continue;
            }
            for slot in p.iter_mut().skip(j + 1) {
                *slot = pivot + turn * (*slot - pivot);
            }
        }
    }
    p
}

/// Turn the inner joints of a solved chain about the line from its root to its effector so that
/// the joint farthest from that line points towards `pole`. Root and effector stay.
pub fn apply_pole(points: &mut [Vec3], pole: Vec3) {
    let (Some(root), Some(end)) = (points.first().copied(), points.last().copied()) else { return };
    if points.len() < 3 || !pole.is_finite() || !points.iter().all(|p| p.is_finite()) {
        return;
    }
    let Some(axis) = (end - root).try_normalize() else { return };
    let mut far: Option<Vec3> = None;
    let mut best = EPS;
    for p in points.iter() {
        let off = (*p - root) - axis * (*p - root).dot(axis);
        if off.length() > best {
            best = off.length();
            far = Some(off.normalize());
        }
    }
    let (Some(from), Some(to)) = (far, across(pole - root, axis)) else { return };
    let angle = from.cross(to).dot(axis).atan2(from.dot(to));
    let turn = Quat::from_axis_angle(axis, angle);
    if !turn.is_finite() {
        return;
    }
    let last = points.len() - 1;
    for p in points.iter_mut().take(last).skip(1) {
        *p = root + turn * (*p - root);
    }
}

/// The world rotation that takes a bone pointing along `from` to point along `to` by the
/// shortest turn; identity when either is too short.
pub fn aim_delta(from: Vec3, to: Vec3) -> Quat {
    match (from.try_normalize(), to.try_normalize()) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() => {
            let q = Quat::from_rotation_arc(a, b);
            if q.is_finite() { q } else { Quat::IDENTITY }
        }
        _ => Quat::IDENTITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn close(a: Vec3, b: Vec3, eps: f32) -> bool {
        (a - b).length() < eps
    }

    #[test]
    fn two_bones_reach_the_goal_and_bend_to_the_pole() {
        let (root, mid, end) = (Vec3::ZERO, Vec3::Y, Vec3::Y * 2.0);
        let goal = Vec3::new(1.0, 1.0, 0.0);
        let (m, e) = two_bone(root, mid, end, goal, Some(Vec3::new(0.0, 0.0, 5.0)));
        assert!(close(e, goal, 1e-4), "{e:?}");
        assert!(((m - root).length() - 1.0).abs() < 1e-4 && ((e - m).length() - 1.0).abs() < 1e-4, "bone lengths are kept");
        assert!(m.z > 0.5, "the elbow points at the pole: {m:?}");
        let (m2, _) = two_bone(root, mid, end, goal, Some(Vec3::new(0.0, 0.0, -5.0)));
        assert!(m2.z < -0.5, "and follows it to the other side");
        // Out of reach: straight towards the goal, at full length.
        let (m, e) = two_bone(root, mid, end, Vec3::X * 10.0, None);
        assert!(close(e, Vec3::X * 2.0, 1e-3) && close(m, Vec3::X, 1e-2), "{m:?} {e:?}");
        // Too close: folded as far as the bones allow, never NaN.
        let (m, e) = two_bone(root, mid, Vec3::Y * 1.5, Vec3::ZERO, None);
        assert!(m.is_finite() && e.is_finite());
        // Degenerate input comes back unchanged.
        assert_eq!(two_bone(root, root, end, goal, None), (root, end));
        assert_eq!(two_bone(root, mid, end, Vec3::NAN, None), (mid, end));
    }

    #[test]
    fn without_a_pole_the_bend_keeps_its_side() {
        let (root, mid, end) = (Vec3::ZERO, Vec3::new(0.2, 1.0, 0.0), Vec3::Y * 2.0);
        let (m, _) = two_bone(root, mid, end, Vec3::Y * 1.5, None);
        assert!(m.x > 0.3, "{m:?}");
        let (m, _) = two_bone(root, Vec3::new(-0.2, 1.0, 0.0), end, Vec3::Y * 1.5, None);
        assert!(m.x < -0.3, "{m:?}");
    }

    #[test]
    fn iterative_solvers_reach_and_stop_at_full_stretch() {
        let chain: Vec<Vec3> = (0..6).map(|i| Vec3::Y * i as f32).collect();
        let goal = Vec3::new(2.0, 2.0, 1.0);
        for (name, solved) in [("fabrik", fabrik(&chain, goal, 50, 1e-4)), ("ccd", ccd(&chain, goal, 100, 1e-4))] {
            assert!(close(solved[5], goal, 1e-2), "{name}: {:?}", solved[5]);
            assert_eq!(solved[0], chain[0], "{name}: the root stays");
            for w in solved.windows(2) {
                assert!(((w[1] - w[0]).length() - 1.0).abs() < 1e-3, "{name}: bone lengths are kept");
            }
        }
        let far = fabrik(&chain, Vec3::X * 100.0, 10, 1e-4);
        assert!(close(far[5], Vec3::X * 5.0, 1e-3));
        // Hostile input: unchanged.
        assert_eq!(fabrik(&chain, Vec3::splat(f32::INFINITY), 10, 1e-4), chain);
        assert_eq!(ccd(&chain[..1], goal, 10, 1e-4), chain[..1].to_vec());
        assert_eq!(fabrik(&[Vec3::ZERO; 4], goal, 10, 1e-4), vec![Vec3::ZERO; 4], "zero-length bones");
    }

    #[test]
    fn the_pole_turns_the_chain_without_moving_its_ends() {
        let mut p = vec![Vec3::ZERO, Vec3::new(0.5, 1.0, 0.0), Vec3::new(0.5, 2.0, 0.0), Vec3::Y * 3.0];
        let before = p.clone();
        apply_pole(&mut p, Vec3::new(0.0, 1.0, 9.0));
        assert_eq!((p[0], p[3]), (before[0], before[3]));
        assert!(p[1].z > 0.49 && p[1].x.abs() < 1e-4, "{:?}", p[1]);
        for (a, b) in p.windows(2).zip(before.windows(2)) {
            assert!(((a[1] - a[0]).length() - (b[1] - b[0]).length()).abs() < 1e-4);
        }
        // A straight chain has no bend to turn.
        let mut straight = vec![Vec3::ZERO, Vec3::Y, Vec3::Y * 2.0];
        apply_pole(&mut straight, Vec3::X);
        assert_eq!(straight, vec![Vec3::ZERO, Vec3::Y, Vec3::Y * 2.0]);
    }

    #[test]
    fn aim_delta_turns_one_direction_into_another() {
        let q = aim_delta(Vec3::Y, Vec3::X);
        assert!(close(q * Vec3::Y, Vec3::X, 1e-5));
        assert_eq!(aim_delta(Vec3::ZERO, Vec3::X), Quat::IDENTITY);
    }

    fn point() -> impl Strategy<Value = Vec3> {
        (-4.0f32..4.0, -4.0f32..4.0, -4.0f32..4.0).prop_map(|(x, y, z)| Vec3::new(x, y, z))
    }

    proptest! {
        #[test]
        fn two_bone_keeps_lengths_and_reaches_when_it_can(mid in point(), end in point(), goal in point(), pole in point()) {
            let root = Vec3::ZERO;
            let (a, b) = (mid.length(), (end - mid).length());
            prop_assume!(a > 0.05 && b > 0.05);
            let (m, e) = two_bone(root, mid, end, goal, Some(pole));
            prop_assert!(m.is_finite() && e.is_finite());
            prop_assert!(((m - root).length() - a).abs() < 1e-3 * (1.0 + a));
            prop_assert!(((e - m).length() - b).abs() < 1e-3 * (1.0 + b));
            let d = goal.length();
            if d < a + b - 1e-2 && d > (a - b).abs() + 1e-2 {
                prop_assert!((e - goal).length() < 1e-2, "reachable goal missed by {}", (e - goal).length());
            }
        }

        #[test]
        fn chains_stay_finite_and_keep_their_bones(pts in proptest::collection::vec(point(), 2..12), goal in point(), iters in 0u32..40) {
            let len = lengths(&pts);
            for solved in [fabrik(&pts, goal, iters, 1e-4), ccd(&pts, goal, iters, 1e-4)] {
                prop_assert_eq!(solved.len(), pts.len());
                prop_assert!(solved.iter().all(|p| p.is_finite()));
                prop_assert_eq!(solved[0], pts[0]);
                if len.iter().all(|l| *l > 1e-3) {
                    for (w, l) in solved.windows(2).zip(&len) {
                        prop_assert!(((w[1] - w[0]).length() - l).abs() < 1e-2 * (1.0 + l));
                    }
                }
            }
        }
    }
}
