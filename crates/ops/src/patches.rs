//! Patch grouping and mesh checks.
//!
//! Patches are the per-face groups ModelCraft tints, masks by and exports as parts. Grouping
//! rebuilds them from the shape: by the angle between neighbouring faces (a flood fill that stops
//! at creases sharper than the threshold), by connected shells, or by merging the patches of the
//! selected faces into one. [`check`] reports what usually needs cleaning on imported meshes
//! (n-gons, triangles, open borders, faces with no area, poles); [`clean_up`] welds near-coincident
//! vertices and removes faces with no area.

use std::collections::{BTreeSet, VecDeque};

use crate::{OpError, OpResult, PolyMesh, clean_selection, edit, rebuild};

/// How [`group`] forms patches.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Grouping {
    /// Neighbouring faces whose normals differ by less than `degrees` share a patch.
    Angle { degrees: f32 },
    /// Every connected piece is one patch.
    Shells,
}

/// Faces in patch order of discovery: regions of faces connected across edges that `joins`
/// accepts. Returns the patch of every face.
fn regions(m: &PolyMesh, joins: impl Fn(u32, u32) -> bool) -> Vec<u32> {
    let n = m.face_count();
    let mut patch = vec![u32::MAX; n];
    let mut next = 0u32;
    let mut queue = VecDeque::new();
    for start in 0..n as u32 {
        if patch.get(start as usize).is_none_or(|p| *p != u32::MAX) {
            continue;
        }
        if let Some(p) = patch.get_mut(start as usize) {
            *p = next;
        }
        queue.push_back(start);
        while let Some(f) = queue.pop_front() {
            for h in m.face_half_edges(f).collect::<Vec<_>>() {
                let t = m.twin(h);
                if t == modelcraft_mesh::NONE {
                    continue;
                }
                let g = m.face_of(t);
                if g == modelcraft_mesh::NONE || g as usize >= n {
                    continue;
                }
                if patch.get(g as usize).is_some_and(|p| *p == u32::MAX) && joins(f, g) {
                    if let Some(p) = patch.get_mut(g as usize) {
                        *p = next;
                    }
                    queue.push_back(g);
                }
            }
        }
        next += 1;
    }
    patch
}

/// The patch of every face after grouping.
pub fn grouping(m: &PolyMesh, how: Grouping) -> Vec<u32> {
    match how {
        Grouping::Shells => regions(m, |_, _| true),
        Grouping::Angle { degrees } => {
            let d = if degrees.is_finite() { degrees.clamp(0.0, 180.0) } else { 30.0 };
            let cos = d.to_radians().cos();
            let normals: Vec<glam::Vec3> = (0..m.face_count() as u32).map(|f| m.face_normal(f)).collect();
            let n = |f: u32| normals.get(f as usize).copied().unwrap_or_default();
            regions(m, |a, b| n(a).dot(n(b)) >= cos - 1e-6)
        }
    }
}

/// Rebuild the patches of `m` by `how`.
pub fn group(m: &PolyMesh, how: Grouping) -> OpResult {
    let patches = grouping(m, how);
    let mut polys = m.to_polygons();
    for (p, g) in polys.iter_mut().zip(patches) {
        p.patch = g;
    }
    rebuild(m.positions().to_vec(), polys)
}

/// Give the selected faces a patch of their own (one higher than any patch in use).
pub fn new_patch(m: &PolyMesh, faces: &[u32]) -> OpResult {
    let faces = clean_selection(faces, m.face_count());
    if faces.is_empty() {
        return Err(OpError::Invalid("select faces first".into()));
    }
    let next = (0..m.face_count() as u32).map(|f| m.patch(f)).max().map_or(0, |p| p.saturating_add(1));
    edit::set_patch(m, &faces, next)
}

/// The faces of `patch`.
pub fn faces_of_patch(m: &PolyMesh, patch: u32) -> Vec<u32> {
    (0..m.face_count() as u32).filter(|f| m.patch(*f) == patch).collect()
}

/// The patches in use, ascending.
pub fn patches_in_use(m: &PolyMesh) -> Vec<u32> {
    (0..m.face_count() as u32).map(|f| m.patch(f)).collect::<BTreeSet<_>>().into_iter().collect()
}

/// What [`check`] found, as element indices.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct MeshCheck {
    /// Faces with more than four corners.
    pub ngons: Vec<u32>,
    pub triangles: Vec<u32>,
    /// Edges on an open border.
    pub border_edges: Vec<u32>,
    /// Faces whose area is (nearly) zero.
    pub degenerate_faces: Vec<u32>,
    /// Vertices where five or more edges meet (poles worth knowing about before subdividing).
    pub poles: Vec<u32>,
    /// Connected pieces.
    pub shells: usize,
}

impl MeshCheck {
    /// Nothing a cleanup would change.
    pub fn is_clean(&self) -> bool {
        self.degenerate_faces.is_empty()
    }
}

fn face_area(m: &PolyMesh, f: u32) -> f32 {
    let v: Vec<glam::Vec3> = m.face_vertices(f).map(|v| m.position(v)).collect();
    let Some(&a) = v.first() else { return 0.0 };
    let mut s = glam::Vec3::ZERO;
    for w in v.windows(2).skip(1) {
        if let [b, c] = w {
            s += (*b - a).cross(*c - a);
        }
    }
    0.5 * s.length()
}

/// Look the mesh over.
pub fn check(m: &PolyMesh) -> MeshCheck {
    let (lo, hi) = m.bounds();
    let scale = (hi - lo).max_element().max(1e-6);
    let tiny = (scale * 1e-6) * (scale * 1e-6);
    let mut c = MeshCheck::default();
    for f in 0..m.face_count() as u32 {
        match m.face_len(f) {
            3 => c.triangles.push(f),
            4 => {}
            _ => c.ngons.push(f),
        }
        if face_area(m, f) <= tiny {
            c.degenerate_faces.push(f);
        }
    }
    c.border_edges = (0..m.edge_count() as u32).filter(|e| m.is_boundary_edge(*e)).collect();
    c.poles = (0..m.vertex_count() as u32).filter(|v| m.vertex_neighbors(*v).count() >= 5).collect();
    c.shells = grouping(m, Grouping::Shells).into_iter().max().map_or(0, |p| p as usize + 1);
    c
}

/// Weld vertices closer than `distance` and remove faces with no area.
pub fn clean_up(m: &PolyMesh, distance: f32) -> OpResult {
    let welded = edit::weld(m, distance)?;
    let bad = check(&welded).degenerate_faces;
    if bad.is_empty() { Ok(welded) } else { edit::delete_faces(&welded, &bad) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use glam::Vec3;
    use modelcraft_mesh::Polygon;
    use proptest::prelude::*;

    fn cube() -> PolyMesh {
        Primitive::default_of("cube").and_then(|p| p.build().ok()).unwrap()
    }

    #[test]
    fn angle_grouping_finds_the_six_sides_of_a_subdivided_cube() {
        let m = crate::subdivide::catmull_clark(&cube(), 0).unwrap();
        // Flatten every face's patch first: grouping must rebuild them from the shape.
        let one = edit::set_patch(&m, &(0..m.face_count() as u32).collect::<Vec<_>>(), 0).unwrap();
        let g = group(&one, Grouping::Angle { degrees: 30.0 }).unwrap();
        assert_eq!(patches_in_use(&g).len(), 6);
        let g = group(&one, Grouping::Angle { degrees: 120.0 }).unwrap();
        assert_eq!(patches_in_use(&g).len(), 1, "a wide threshold joins across the corners");
        let sphere = Primitive::default_of("sphere").and_then(|p| p.build().ok()).unwrap();
        let s = group(&sphere, Grouping::Angle { degrees: 30.0 }).unwrap();
        assert_eq!(patches_in_use(&s).len(), 1, "a smooth surface is one patch");
        assert_eq!(s.face_count(), sphere.face_count());
    }

    #[test]
    fn shells_new_patches_and_patch_selection() {
        let pos = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(5.0, 0.0, 0.0),
            Vec3::new(6.0, 0.0, 0.0),
            Vec3::new(6.0, 1.0, 0.0),
        ];
        let (m, _) = PolyMesh::build(pos, vec![Polygon::new(vec![0, 1, 2, 3]), Polygon::new(vec![4, 5, 6])]).unwrap();
        let g = group(&m, Grouping::Shells).unwrap();
        assert_eq!(patches_in_use(&g), vec![0, 1]);
        let c = cube();
        let n = new_patch(&c, &[2, 3]).unwrap();
        let top = patches_in_use(&n).into_iter().max().unwrap();
        assert_eq!(faces_of_patch(&n, top), vec![2, 3]);
        assert!(new_patch(&c, &[99]).is_err(), "no valid face");
    }

    #[test]
    fn check_reports_and_clean_up_repairs() {
        let c = check(&cube());
        assert!(c.ngons.is_empty() && c.triangles.is_empty() && c.border_edges.is_empty() && c.degenerate_faces.is_empty());
        assert_eq!(c.shells, 1);
        // A quad, a pentagon, a triangle and a sliver triangle with no area, all open.
        let pos = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.5, 0.5, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::new(4.0, 0.0, 0.0),
            Vec3::new(5.0, 0.0, 0.0),
        ];
        let polys = vec![Polygon::new(vec![0, 1, 2, 3]), Polygon::new(vec![1, 4, 5, 6, 2]), Polygon::new(vec![4, 7, 5]), Polygon::new(vec![7, 8, 9])];
        let (m, _) = PolyMesh::build(pos, polys).unwrap();
        let c = check(&m);
        assert_eq!(c.ngons, vec![1]);
        assert_eq!(c.triangles, vec![2, 3]);
        assert_eq!(c.degenerate_faces, vec![3]);
        assert!(!c.border_edges.is_empty());
        assert!(!c.is_clean());
        let fixed = clean_up(&m, 1e-5).unwrap();
        assert_eq!(fixed.face_count(), 3);
        assert!(check(&fixed).is_clean());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn grouping_keeps_the_mesh_and_covers_every_face(deg in -10.0f32..400.0, seg in 1u32..6) {
            let m = Primitive::Torus { radius: 1.0, tube: 0.4, segments: 4 + seg * 2, sides: 3 + seg }.build().unwrap();
            for how in [Grouping::Angle { degrees: deg }, Grouping::Angle { degrees: f32::NAN }, Grouping::Shells] {
                let g = group(&m, how).unwrap();
                prop_assert_eq!(g.face_count(), m.face_count());
                prop_assert_eq!(g.positions(), m.positions());
                let used = patches_in_use(&g);
                prop_assert!(used.len() <= m.face_count());
                prop_assert_eq!(used.first().copied(), Some(0));
            }
            let _ = clean_up(&m, deg);
            let _ = check(&m);
        }
    }
}
