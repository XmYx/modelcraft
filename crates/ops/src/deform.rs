//! Deformers: functions of position that bend, twist, taper, ripple or round a mesh. A deformer
//! works in its own frame (`to_local` / `to_parent` matrices supplied by the scene) along its Y
//! axis, over the mesh's extent on that axis.

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::PolyMesh;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Deformer {
    /// Bend around Z by `angle` degrees over the height.
    Bend { angle: f32 },
    /// Rotate around Y by up to `angle` degrees at the top.
    Twist { angle: f32 },
    /// Scale X and Z by `1 + amount` at the top.
    Taper { amount: f32 },
    /// Sine ripple along Y: `amplitude` in scene units, `frequency` in waves per unit.
    Ripple { amplitude: f32, frequency: f32 },
    /// Pull every point towards the mean radius (`amount` 0..1).
    Round { amount: f32 },
    /// Slide points sideways in proportion to their height above the bottom: `x` and `z` are how
    /// far the top moves per unit of height (the bottom stays put).
    Shear { x: f32, z: f32 },
}

pub const KINDS: &[&str] = &["bend", "twist", "taper", "ripple", "round", "shear"];

impl Deformer {
    pub fn default_of(kind: &str) -> Option<Deformer> {
        Some(match kind {
            "bend" => Deformer::Bend { angle: 90.0 },
            "twist" => Deformer::Twist { angle: 90.0 },
            "taper" => Deformer::Taper { amount: -0.5 },
            "ripple" => Deformer::Ripple { amplitude: 0.1, frequency: 2.0 },
            "round" => Deformer::Round { amount: 1.0 },
            "shear" => Deformer::Shear { x: 0.5, z: 0.0 },
            _ => return None,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Deformer::Bend { .. } => "bend",
            Deformer::Twist { .. } => "twist",
            Deformer::Taper { .. } => "taper",
            Deformer::Ripple { .. } => "ripple",
            Deformer::Round { .. } => "round",
            Deformer::Shear { .. } => "shear",
        }
    }

    /// Deform `m` in place. `to_local` maps mesh space into the deformer's frame.
    pub fn apply(&self, m: &mut PolyMesh, to_local: Mat4) {
        if !to_local.is_finite() || to_local.determinant().abs() < 1e-12 {
            return;
        }
        let back = to_local.inverse();
        let local: Vec<Vec3> = m.positions().iter().map(|p| to_local.transform_point3(*p)).collect();
        let (lo, hi) = local.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
        let h = (hi - lo).max(1e-6);
        let mean_r = if local.is_empty() { 1.0 } else { local.iter().map(|p| p.length()).sum::<f32>() / local.len() as f32 };
        let f = |p: Vec3| -> Vec3 {
            let t = ((p.y - lo) / h).clamp(0.0, 1.0);
            match *self {
                Deformer::Bend { angle } => {
                    let a = angle.to_radians();
                    if a.abs() < 1e-5 {
                        return p;
                    }
                    let r = h / a;
                    let th = a * t;
                    let over = p.y - lo - h * t; // beyond the top continues straight
                    let x = r - (r - p.x) * th.cos() - over * th.sin();
                    let y = lo + (r - p.x) * th.sin() + over * th.cos();
                    Vec3::new(x, y, p.z)
                }
                Deformer::Twist { angle } => {
                    let th = angle.to_radians() * t;
                    let (s, c) = th.sin_cos();
                    Vec3::new(p.x * c + p.z * s, p.y, -p.x * s + p.z * c)
                }
                Deformer::Taper { amount } => {
                    let k = 1.0 + amount * t;
                    Vec3::new(p.x * k, p.y, p.z * k)
                }
                Deformer::Ripple { amplitude, frequency } => {
                    let w = (p.y * frequency * std::f32::consts::TAU).sin() * amplitude;
                    let radial = Vec3::new(p.x, 0.0, p.z).normalize_or_zero();
                    p + radial * w
                }
                Deformer::Round { amount } => p.lerp(p.normalize_or_zero() * mean_r, amount.clamp(0.0, 1.0)),
                Deformer::Shear { x, z } => {
                    let up = p.y - lo;
                    Vec3::new(p.x + finite(x) * up, p.y, p.z + finite(z) * up)
                }
            }
        };
        let moved: Vec<Vec3> = local.into_iter().map(|p| back.transform_point3(f(p))).collect();
        m.map_positions(|i, p| moved.get(i as usize).copied().unwrap_or(p));
    }
}

fn finite(v: f32) -> f32 {
    if v.is_finite() { v.clamp(-1000.0, 1000.0) } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;

    #[test]
    fn deformers_move_points_and_keep_topology() {
        let base = Primitive::Cylinder { radius: 0.5, height: 4.0, sides: 12, rings: 8, caps: true }.build().unwrap();
        for k in KINDS {
            let mut m = base.clone();
            Deformer::default_of(k).unwrap().apply(&mut m, Mat4::IDENTITY);
            m.validate().unwrap();
            assert_eq!(m.face_count(), base.face_count());
            assert_ne!(m.positions(), base.positions(), "{k} did nothing");
        }
    }

    #[test]
    fn singular_frames_are_ignored() {
        let base = Primitive::Cube { size: Vec3::ONE, segments: 1 }.build().unwrap();
        let mut m = base.clone();
        Deformer::Twist { angle: 45.0 }.apply(&mut m, Mat4::ZERO);
        assert_eq!(m, base);
    }
}
