//! Connect: join several meshes into one. Each input comes with the matrix that puts it in the
//! joined mesh's space; patches are renumbered so the parts keep distinct patches, and UVs
//! are kept when every part has them. An optional weld then merges points that coincide.

use glam::Mat4;

use crate::{MAX_RESULT_FACES, OpError, OpResult, PolyMesh, Polygon, edit, rebuild};

/// Most meshes one connect joins.
pub const MAX_PARTS: usize = 10_000;

/// The joined mesh. `weld` is the distance within which points merge (0 leaves them apart).
pub fn connect(parts: &[(&PolyMesh, Mat4)], weld: f32) -> OpResult {
    if parts.is_empty() {
        return Err(OpError::Invalid("there is nothing to connect".into()));
    }
    if parts.len() > MAX_PARTS {
        return Err(OpError::Invalid(format!("at most {MAX_PARTS} meshes can be connected")));
    }
    let faces: usize = parts.iter().map(|(m, _)| m.face_count()).sum();
    let verts: usize = parts.iter().map(|(m, _)| m.vertex_count()).sum();
    if faces > MAX_RESULT_FACES || verts > modelcraft_mesh::MAX_ELEMENTS {
        return Err(OpError::TooLarge(faces));
    }
    let uvs = parts.iter().all(|(m, _)| m.has_uvs() || m.face_count() == 0);
    let mut positions = Vec::with_capacity(verts);
    let mut polygons: Vec<Polygon> = Vec::with_capacity(faces);
    let mut patch_base = 0u32;
    for (m, to_joined) in parts {
        let offset = positions.len() as u32;
        let to = if to_joined.is_finite() { *to_joined } else { Mat4::IDENTITY };
        positions.extend(m.positions().iter().map(|p| to.transform_point3(*p)));
        let mirrored = to.determinant() < 0.0;
        let mut top = 0u32;
        for mut p in m.to_polygons() {
            top = top.max(p.patch);
            p.patch = p.patch.saturating_add(patch_base);
            for v in &mut p.verts {
                *v += offset;
            }
            // A mirroring matrix turns faces inside out; wind them back.
            if mirrored {
                p.verts.reverse();
                if let Some(u) = &mut p.uvs {
                    u.reverse();
                }
            }
            if !uvs {
                p.uvs = None;
            }
            polygons.push(p);
        }
        patch_base = patch_base.saturating_add(top).saturating_add(1);
    }
    let joined = rebuild(positions, polygons)?;
    if weld.is_finite() && weld > 0.0 { edit::weld(&joined, weld.min(1_000.0)) } else { Ok(joined) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use glam::Vec3;

    fn cube() -> PolyMesh {
        Primitive::Cube { size: Vec3::ONE, segments: 1 }.build().unwrap()
    }

    #[test]
    fn parts_are_joined_in_their_places_with_distinct_patches() {
        let (a, b) = (cube(), cube());
        let m = connect(&[(&a, Mat4::IDENTITY), (&b, Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0)))], 0.0).unwrap();
        m.validate().unwrap();
        assert_eq!(m.face_count(), 12);
        assert_eq!(m.vertex_count(), 16);
        let (lo, hi) = m.bounds();
        assert!((lo.x + 0.5).abs() < 1e-5 && (hi.x - 3.5).abs() < 1e-5);
        let patches: std::collections::BTreeSet<u32> = (0..12).map(|f| m.patch(f)).collect();
        assert_eq!(patches.len(), 12, "six patches each, none shared: {patches:?}");
        assert!(m.has_uvs());
    }

    #[test]
    fn a_weld_closes_the_seam_between_touching_parts() {
        let a = Primitive::Plane { width: 2.0, depth: 2.0, segments: 2 }.build().unwrap();
        let m = connect(&[(&a, Mat4::IDENTITY), (&a, Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0)))], 1e-4).unwrap();
        assert_eq!(m.vertex_count(), 15, "the shared column of three points merged");
        let loose = connect(&[(&a, Mat4::IDENTITY), (&a, Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0)))], 0.0).unwrap();
        assert_eq!(loose.vertex_count(), 18);
    }

    #[test]
    fn a_mirroring_matrix_keeps_faces_outward() {
        let m = connect(&[(&cube(), Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)))], 0.0).unwrap();
        let (lo, hi) = m.bounds();
        let c = (lo + hi) / 2.0;
        assert!((0..m.face_count() as u32).all(|f| m.face_normal(f).dot(m.face_center(f) - c) > 0.0));
    }

    #[test]
    fn no_parts_hostile_matrices_and_a_part_without_uvs() {
        assert!(connect(&[], 0.0).is_err());
        let m = connect(&[(&cube(), Mat4::from_scale(Vec3::splat(f32::NAN)))], f32::NAN).unwrap();
        m.validate().unwrap();
        let mut bare = cube();
        bare.clear_uvs();
        let m = connect(&[(&cube(), Mat4::IDENTITY), (&bare, Mat4::IDENTITY)], 0.0).unwrap();
        assert!(!m.has_uvs(), "UVs only when every part has them");
        // An empty part is skipped without costing the others their UVs.
        let m = connect(&[(&cube(), Mat4::IDENTITY), (&PolyMesh::default(), Mat4::IDENTITY)], 0.0).unwrap();
        assert!(m.has_uvs());
    }
}
