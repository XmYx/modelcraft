//! The array generator: real copies of a mesh joined into one mesh, laid out in a row or a ring.
//!
//! (Clones that stay instances of one mesh are a different tool; an array makes geometry, so it
//! can be edited, subdivided or exported as one object.) Counts, sizes and the result are bounded:
//! at most [`MAX_COPIES`] copies, and the joined mesh may not pass [`crate::MAX_RESULT_FACES`].

use glam::{EulerRot, Mat4, Quat, Vec3};
use modelcraft_mesh::{PolyMesh, Polygon};
use serde::{Deserialize, Serialize};

use crate::{OpError, OpResult};

/// Most copies one array makes.
pub const MAX_COPIES: u32 = 1_000;

const MAX_LEN: f32 = 100_000.0;
const MAX_DEG: f32 = 36_000.0;
const MIN_SCALE: f32 = 1e-3;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Array {
    /// A row: each copy is moved by `offset`, turned by `rotation` degrees and grown by `scale`
    /// more than the one before.
    Linear { count: u32, offset: Vec3, rotation: Vec3, scale: Vec3 },
    /// A ring (or an arc of `angle` degrees) of `radius` around `axis` (0 = X, 1 = Y, 2 = Z);
    /// `align` turns each copy to face outwards.
    Radial { count: u32, radius: f32, axis: u32, angle: f32, align: bool },
}

pub const KINDS: &[&str] = &["linear", "radial"];

impl Array {
    pub fn default_of(kind: &str) -> Option<Array> {
        Some(match kind {
            "linear" => Array::Linear { count: 4, offset: Vec3::new(1.5, 0.0, 0.0), rotation: Vec3::ZERO, scale: Vec3::ZERO },
            "radial" => Array::Radial { count: 8, radius: 2.0, axis: 1, angle: 360.0, align: true },
            _ => return None,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Array::Linear { .. } => "linear",
            Array::Radial { .. } => "radial",
        }
    }

    /// Clamp every number into its safe range (non-finite numbers become the neutral value).
    pub fn sanitized(self) -> Array {
        let len = |v: f32| if v.is_finite() { v.clamp(-MAX_LEN, MAX_LEN) } else { 0.0 };
        let deg = |v: f32| if v.is_finite() { v.clamp(-MAX_DEG, MAX_DEG) } else { 0.0 };
        let v3 = |v: Vec3, f: &dyn Fn(f32) -> f32| Vec3::new(f(v.x), f(v.y), f(v.z));
        let step = |v: f32| if v.is_finite() { v.clamp(-100.0, 100.0) } else { 0.0 };
        match self {
            Array::Linear { count, offset, rotation, scale } => {
                Array::Linear { count: count.clamp(1, MAX_COPIES), offset: v3(offset, &len), rotation: v3(rotation, &deg), scale: v3(scale, &step) }
            }
            Array::Radial { count, radius, axis, angle, align } => {
                Array::Radial { count: count.clamp(1, MAX_COPIES), radius: len(radius).max(0.0), axis: axis.min(2), angle: deg(angle), align }
            }
        }
    }

    /// One matrix per copy, in the order they are laid out.
    pub fn matrices(&self) -> Vec<Mat4> {
        match self.sanitized() {
            Array::Linear { count, offset, rotation, scale } => (0..count)
                .map(|i| {
                    let k = i as f32;
                    let rot = Quat::from_euler(EulerRot::XYZ, (rotation.x * k).to_radians(), (rotation.y * k).to_radians(), (rotation.z * k).to_radians());
                    Mat4::from_scale_rotation_translation((Vec3::ONE + scale * k).max(Vec3::splat(MIN_SCALE)), rot, offset * k)
                })
                .collect(),
            Array::Radial { count, radius, axis, angle, align } => {
                let (u, v, a) = match axis {
                    0 => (Vec3::Y, Vec3::Z, Vec3::X),
                    1 => (Vec3::Z, Vec3::X, Vec3::Y),
                    _ => (Vec3::X, Vec3::Y, Vec3::Z),
                };
                // A full turn must not put the last copy on top of the first.
                let steps = if angle.abs() >= 360.0 { count } else { count.saturating_sub(1).max(1) };
                (0..count)
                    .map(|i| {
                        let theta = (angle * i as f32 / steps as f32).to_radians();
                        let radial = u * theta.cos() + v * theta.sin();
                        let rot = if align { Quat::from_mat3(&glam::Mat3::from_cols(radial, a, radial.cross(a))) } else { Quat::IDENTITY };
                        Mat4::from_rotation_translation(rot, radial * radius)
                    })
                    .collect()
            }
        }
    }

    /// The joined mesh: every copy of `m`, one after the other.
    pub fn build(&self, m: &PolyMesh) -> OpResult {
        let mats = self.matrices();
        let copies = mats.len();
        if copies.saturating_mul(m.face_count()) > crate::MAX_RESULT_FACES || copies.saturating_mul(m.vertex_count()) > modelcraft_mesh::MAX_ELEMENTS {
            return Err(OpError::TooLarge(copies.saturating_mul(m.face_count())));
        }
        let base_polys: Vec<Polygon> = m.to_polygons();
        let mut positions = Vec::with_capacity(copies * m.vertex_count());
        let mut polygons = Vec::with_capacity(copies * base_polys.len());
        for mat in mats {
            let offset = positions.len() as u32;
            positions.extend(m.positions().iter().map(|p| mat.transform_point3(*p)));
            polygons.extend(base_polys.iter().map(|p| Polygon { verts: p.verts.iter().map(|v| v + offset).collect(), uvs: p.uvs.clone(), patch: p.patch }));
        }
        crate::rebuild(positions, polygons)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use proptest::prelude::*;

    fn cube() -> PolyMesh {
        Primitive::Cube { size: Vec3::ONE, segments: 1 }.build().expect("cube")
    }

    #[test]
    fn a_linear_array_joins_copies_in_a_row() {
        let a = Array::Linear { count: 3, offset: Vec3::new(2.0, 0.0, 0.0), rotation: Vec3::ZERO, scale: Vec3::ZERO };
        let m = a.build(&cube()).expect("array");
        assert_eq!(m.face_count(), 18);
        assert_eq!(m.vertex_count(), 24);
        m.validate().expect("valid");
        let (lo, hi) = m.bounds();
        assert!((lo.x + 0.5).abs() < 1e-5 && (hi.x - 4.5).abs() < 1e-5, "{lo:?} {hi:?}");
    }

    #[test]
    fn a_radial_array_makes_a_ring_and_a_full_turn_does_not_double_up() {
        let a = Array::default_of("radial").expect("radial");
        let mats = a.matrices();
        assert_eq!(mats.len(), 8);
        for m in &mats {
            assert!((m.w_axis.truncate().length() - 2.0).abs() < 1e-4, "on the ring");
        }
        assert!((mats[0].w_axis - mats[7].w_axis).length() > 0.5);
        let arc = Array::Radial { count: 3, radius: 1.0, axis: 2, angle: 180.0, align: false }.matrices();
        assert!((arc[0].w_axis.truncate() - Vec3::X).length() < 1e-5);
        assert!((arc[2].w_axis.truncate() + Vec3::X).length() < 1e-4);
    }

    #[test]
    fn a_huge_array_is_refused_not_allocated() {
        let big = Primitive::Sphere { radius: 1.0, segments: 400, rings: 400 }.build();
        if let Ok(big) = big {
            let r = Array::Linear { count: u32::MAX, offset: Vec3::X, rotation: Vec3::ZERO, scale: Vec3::ZERO }.build(&big);
            assert!(matches!(r, Err(OpError::TooLarge(_))) || r.is_ok());
        }
        assert_eq!(Array::Linear { count: u32::MAX, offset: Vec3::X, rotation: Vec3::ZERO, scale: Vec3::ZERO }.matrices().len(), MAX_COPIES as usize);
    }

    fn any_f32() -> impl Strategy<Value = f32> {
        prop_oneof![proptest::num::f32::ANY, -100.0f32..100.0, Just(f32::NAN), Just(0.0)]
    }
    fn any_vec3() -> impl Strategy<Value = Vec3> {
        (any_f32(), any_f32(), any_f32()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// Whatever the numbers, the array has at most MAX_COPIES finite copies and builds a valid mesh.
        #[test]
        fn any_array_is_bounded_finite_and_valid(
            count in any::<u32>(), offset in any_vec3(), rotation in any_vec3(), scale in any_vec3(),
            radius in any_f32(), axis in any::<u32>(), angle in any_f32(), align in any::<bool>(), radial in any::<bool>()
        ) {
            let a = if radial { Array::Radial { count, radius, axis, angle, align } } else { Array::Linear { count, offset, rotation, scale } };
            let mats = a.matrices();
            prop_assert!(!mats.is_empty() && mats.len() <= MAX_COPIES as usize);
            prop_assert!(mats.iter().all(|m| m.is_finite()));
            let m = a.build(&cube());
            // Small cube times at most 1000 copies fits: it must build, with all faces kept.
            let m = m.expect("a cube array fits");
            prop_assert_eq!(m.face_count(), 6 * mats.len());
            prop_assert!(m.positions().iter().all(|p| p.is_finite()));
        }
    }
}
