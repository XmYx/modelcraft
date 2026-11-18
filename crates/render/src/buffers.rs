//! CPU-side vertex data for the GPU path.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use modelcraft_mesh::PolyMesh;

/// A shaded vertex: position, normal, mask (sculpt) and patch index.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub mask: f32,
    pub patch: f32,
}

/// A coloured line or overlay vertex.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

/// Vertices and triangle indices of a mesh.
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
}

/// Corners whose faces bend less than this share a normal (auto smoothing).
pub const SMOOTH_ANGLE_DEG: f32 = 40.0;

/// Above this many corners, use shared vertex normals (meshes that dense look smooth anyway).
pub const CORNER_LIMIT: usize = 2_000_000;

/// Drawing data with auto-smoothed normals: each face corner gets the average normal of the faces
/// around its vertex that bend less than [`SMOOTH_ANGLE_DEG`] from its own face.
pub fn mesh_data(m: &PolyMesh) -> MeshData {
    let corners: usize = (0..m.face_count() as u32).map(|f| m.face_len(f)).sum();
    if corners > CORNER_LIMIT {
        return shared_data(m.positions(), &m.vertex_normals(), None, &m.triangulate().0);
    }
    let fnorm: Vec<Vec3> = (0..m.face_count() as u32).map(|f| m.face_normal(f)).collect();
    let cos = SMOOTH_ANGLE_DEG.to_radians().cos();
    let mut out = MeshData { vertices: Vec::with_capacity(corners), indices: Vec::with_capacity(corners * 2) };
    for f in 0..m.face_count() as u32 {
        let nf = fnorm.get(f as usize).copied().unwrap_or(Vec3::Y);
        let first = out.vertices.len() as u32;
        let mut n = 0u32;
        for h in m.face_half_edges(f) {
            let v = m.origin(h);
            let mut sum = Vec3::ZERO;
            for g in m.vertex_faces(v) {
                let ng = fnorm.get(g as usize).copied().unwrap_or(Vec3::ZERO);
                if ng.dot(nf) >= cos {
                    sum += ng;
                }
            }
            let normal = sum.normalize_or(nf);
            out.vertices.push(MeshVertex { pos: m.position(v).to_array(), normal: normal.to_array(), mask: 0.0, patch: m.patch(f) as f32 });
            n += 1;
        }
        for i in 1..n.saturating_sub(1) {
            out.indices.extend_from_slice(&[first, first + i, first + i + 1]);
        }
    }
    out
}

/// Drawing data with one normal per face (flat shading).
pub fn flat_data(m: &PolyMesh) -> MeshData {
    let corners: usize = (0..m.face_count() as u32).map(|f| m.face_len(f)).sum();
    let mut out = MeshData { vertices: Vec::with_capacity(corners), indices: Vec::with_capacity(corners * 2) };
    for f in 0..m.face_count() as u32 {
        let n = m.face_normal(f).to_array();
        let first = out.vertices.len() as u32;
        let mut count = 0u32;
        for h in m.face_half_edges(f) {
            out.vertices.push(MeshVertex { pos: m.position(m.origin(h)).to_array(), normal: n, mask: 0.0, patch: m.patch(f) as f32 });
            count += 1;
        }
        for i in 1..count.saturating_sub(1) {
            out.indices.extend_from_slice(&[first, first + i, first + i + 1]);
        }
    }
    out
}

/// Shared-vertex data (sculpt meshes): one vertex per mesh vertex.
pub fn shared_data(positions: &[Vec3], normals: &[Vec3], mask: Option<&[f32]>, tris: &[[u32; 3]]) -> MeshData {
    let vertices = (0..positions.len()).map(|i| vertex_at(positions, normals, mask, i)).collect();
    MeshData { vertices, indices: tris.iter().flatten().copied().collect() }
}

pub fn vertex_at(positions: &[Vec3], normals: &[Vec3], mask: Option<&[f32]>, i: usize) -> MeshVertex {
    MeshVertex {
        pos: positions.get(i).copied().unwrap_or_default().to_array(),
        normal: normals.get(i).copied().unwrap_or(Vec3::Y).to_array(),
        mask: mask.and_then(|m| m.get(i)).copied().unwrap_or(0.0),
        patch: 0.0,
    }
}

/// All edges as line vertices; `None` when the mesh has more than `max_edges`.
pub fn wireframe(m: &PolyMesh, color: [f32; 4], max_edges: usize) -> Option<Vec<LineVertex>> {
    if m.edge_count() > max_edges {
        return None;
    }
    let mut out = Vec::with_capacity(m.edge_count() * 2);
    for e in 0..m.edge_count() as u32 {
        let (a, b) = m.edge_vertices(e);
        out.push(LineVertex { pos: m.position(a).to_array(), color });
        out.push(LineVertex { pos: m.position(b).to_array(), color });
    }
    Some(out)
}

/// A ground grid in XZ: `lines` per side, spacing `step`, with the X (red) and Z (blue) axes.
pub fn grid(lines: i32, step: f32, minor: [f32; 4], major: [f32; 4]) -> Vec<LineVertex> {
    let n = lines.clamp(1, 1000);
    let ext = n as f32 * step;
    let mut out = Vec::with_capacity((4 * n as usize + 2) * 2 + 4);
    for i in -n..=n {
        if i == 0 {
            continue;
        }
        let t = i as f32 * step;
        let c = if i % 10 == 0 { major } else { minor };
        out.push(LineVertex { pos: [t, 0.0, -ext], color: c });
        out.push(LineVertex { pos: [t, 0.0, ext], color: c });
        out.push(LineVertex { pos: [-ext, 0.0, t], color: c });
        out.push(LineVertex { pos: [ext, 0.0, t], color: c });
    }
    let red = [0.86, 0.30, 0.30, 1.0];
    let blue = [0.30, 0.50, 0.92, 1.0];
    out.extend([
        LineVertex { pos: [-ext, 0.0, 0.0], color: red },
        LineVertex { pos: [ext, 0.0, 0.0], color: red },
        LineVertex { pos: [0.0, 0.0, -ext], color: blue },
        LineVertex { pos: [0.0, 0.0, ext], color: blue },
    ]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use modelcraft_mesh::Polygon;

    fn cube() -> PolyMesh {
        let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let pos = vec![p(-1., -1., -1.), p(1., -1., -1.), p(1., 1., -1.), p(-1., 1., -1.), p(-1., -1., 1.), p(1., -1., 1.), p(1., 1., 1.), p(-1., 1., 1.)];
        let faces = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [2, 3, 7, 6], [1, 2, 6, 5], [0, 4, 7, 3]];
        PolyMesh::build(pos, faces.iter().map(|f| Polygon::new(f.to_vec())).collect()).unwrap().0
    }

    #[test]
    fn cube_corners_stay_flat() {
        let d = mesh_data(&cube());
        assert_eq!(d.vertices.len(), 24);
        assert_eq!(d.indices.len(), 36);
        for v in &d.vertices {
            let n = Vec3::from(v.normal);
            assert!((n.abs().max_element() - 1.0).abs() < 1e-5, "cube normal {n} was smoothed");
        }
        assert_eq!(wireframe(&cube(), [1.0; 4], 100).map(|w| w.len()), Some(24));
        assert!(wireframe(&cube(), [1.0; 4], 5).is_none());
    }

    #[test]
    fn grid_has_axes() {
        let g = grid(10, 1.0, [0.5; 4], [0.7; 4]);
        assert_eq!(g.len(), 20 * 4 + 4);
        assert_eq!(std::mem::size_of::<MeshVertex>(), 32);
    }
}
