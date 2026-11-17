//! Joint maths: aiming a joint along its bone and mirroring joints across a plane.

use glam::{Mat3, Mat4, Quat, Vec3};

/// A rotation whose `aim` axis (0 X, 1 Y, 2 Z) points along `direction` and whose `up` axis
/// points as near to `up_hint` as it can. `None` when `direction` is too short, not finite, or
/// `aim == up`. When `up_hint` is parallel to the direction another axis is used.
pub fn aim_rotation(direction: Vec3, up_hint: Vec3, aim: usize, up: usize) -> Option<Quat> {
    if aim > 2 || up > 2 || aim == up || !direction.is_finite() || direction.length_squared() < 1e-12 {
        return None;
    }
    let a = direction.normalize();
    let hint = [up_hint, Vec3::Y, Vec3::Z, Vec3::X].into_iter().filter(|h| h.is_finite()).find(|h| h.cross(a).length_squared() > 1e-6)?;
    let u = (hint - a * hint.dot(a)).normalize();
    let third = 3 - aim - up;
    // Right-handed: axis[i] × axis[i+1] = axis[i+2] (indices mod 3).
    let t = if (aim + 1) % 3 == up { a.cross(u) } else { u.cross(a) };
    let mut cols = [Vec3::ZERO; 3];
    cols[aim] = a;
    cols[up] = u;
    cols[third] = t;
    let q = Quat::from_mat3(&Mat3::from_cols(cols[0], cols[1], cols[2]));
    q.is_finite().then(|| q.normalize())
}

/// The mirror image of a rigid world matrix across the plane through the origin with normal
/// `axis` (0 X, 1 Y, 2 Z): the position is reflected and the orientation is the reflection of
/// the original orientation (`S·R·S`, a proper rotation), so a mirrored limb bends the mirrored
/// way when its joints get the mirrored rotations. Scale is kept.
pub fn mirror_matrix(world: Mat4, axis: usize) -> Mat4 {
    let mut s = Vec3::ONE;
    s[axis.min(2)] = -1.0;
    let flip = Mat4::from_scale(s);
    flip * world * flip
}

/// Line segments (pairs of points) drawing a bone from `head` to `tail` as a stretched
/// octahedron, the usual way to show which end is the parent. `width` is its half-width as a
/// share of the bone's length.
pub fn bone_shape(head: Vec3, tail: Vec3, width: f32) -> Vec<(Vec3, Vec3)> {
    let d = tail - head;
    let len = d.length();
    if !len.is_finite() || len < 1e-6 {
        return Vec::new();
    }
    let a = d / len;
    let side = if a.cross(Vec3::Y).length_squared() > 1e-4 { a.cross(Vec3::Y).normalize() } else { a.cross(Vec3::X).normalize() };
    let other = a.cross(side);
    let w = len * width.clamp(0.01, 0.5);
    let waist = head + d * 0.15;
    let ring = [waist + side * w, waist + other * w, waist - side * w, waist - other * w];
    let mut out = Vec::with_capacity(12);
    for i in 0..4 {
        out.push((head, ring[i]));
        out.push((ring[i], tail));
        out.push((ring[i], ring[(i + 1) % 4]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn aiming_points_the_axis_down_the_bone() {
        let q = aim_rotation(Vec3::new(0.0, 0.0, 3.0), Vec3::Y, 0, 1).unwrap();
        assert!((q * Vec3::X - Vec3::Z).length() < 1e-5);
        assert!((q * Vec3::Y - Vec3::Y).length() < 1e-5);
        // Up parallel to the bone: still a valid rotation.
        let q = aim_rotation(Vec3::Y * 2.0, Vec3::Y, 1, 2).unwrap();
        assert!((q * Vec3::Y - Vec3::Y).length() < 1e-5);
        assert!(aim_rotation(Vec3::ZERO, Vec3::Y, 0, 1).is_none());
        assert!(aim_rotation(Vec3::X, Vec3::Y, 1, 1).is_none());
        assert!(aim_rotation(Vec3::new(f32::NAN, 0.0, 0.0), Vec3::Y, 0, 1).is_none());
        assert!(aim_rotation(Vec3::X, Vec3::Y, 7, 1).is_none());
    }

    #[test]
    fn mirroring_reflects_position_and_orientation() {
        let w = Mat4::from_rotation_translation(Quat::from_rotation_z(0.5), Vec3::new(2.0, 1.0, 3.0));
        let m = mirror_matrix(w, 0);
        assert!((m.transform_point3(Vec3::ZERO) - Vec3::new(-2.0, 1.0, 3.0)).length() < 1e-6);
        assert!(m.determinant() > 0.0, "still a proper rotation");
        let (_, r, _) = m.to_scale_rotation_translation();
        assert!(r.dot(Quat::from_rotation_z(-0.5)).abs() > 0.9999, "a turn about Z mirrors to the opposite turn");
        assert!((mirror_matrix(m, 0).to_cols_array().iter().zip(w.to_cols_array()).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max)) < 1e-6);
        assert_eq!(bone_shape(Vec3::ZERO, Vec3::Y, 0.1).len(), 12);
        assert!(bone_shape(Vec3::ZERO, Vec3::ZERO, 0.1).is_empty());
    }

    proptest! {
        #[test]
        fn aim_is_a_rotation_for_every_axis_pair(d in (-5.0f32..5.0, -5.0f32..5.0, -5.0f32..5.0), h in (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0), aim in 0usize..3, up in 0usize..3) {
            let dir = Vec3::new(d.0, d.1, d.2);
            prop_assume!(aim != up && dir.length() > 1e-2);
            let q = aim_rotation(dir, Vec3::new(h.0, h.1, h.2), aim, up).unwrap();
            let mut axis = Vec3::ZERO;
            axis[aim] = 1.0;
            prop_assert!((q * axis - dir.normalize()).length() < 1e-3);
            prop_assert!((q.length() - 1.0).abs() < 1e-4);
        }
    }
}
