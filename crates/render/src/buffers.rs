//! CPU-side vertex data for the GPU path.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use modelcraft_mesh::PolyMesh;

/// A shaded vertex: position, normal, mask (sculpt), patch index, the UV of its corner (zero
/// for meshes without UVs and for shared-vertex data) and its vertex colour (white when none;
/// it tints the object colour).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub mask: f32,
    pub patch: f32,
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// The colour of a vertex nothing has painted.
pub const WHITE: [f32; 4] = [1.0; 4];

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

/// Meshes with at least this many faces are turned into drawing data on several cores.
pub const PARALLEL_FACES: usize = 16_384;
/// Most runs of faces built at once.
const MAX_RUNS: usize = 8;

/// Drawing data with auto-smoothed normals: each face corner gets the average normal of the faces
/// around its vertex that bend less than [`SMOOTH_ANGLE_DEG`] from its own face.
pub fn mesh_data(m: &PolyMesh) -> MeshData {
    mesh_data_in(m, MAX_RUNS)
}

/// [`mesh_data`] built in at most `max_runs` runs of faces at once (1: on the calling thread).
pub fn mesh_data_in(m: &PolyMesh, max_runs: usize) -> MeshData {
    let corners: usize = (0..m.face_count() as u32).map(|f| m.face_len(f)).sum();
    if corners > CORNER_LIMIT {
        return shared_data(m.positions(), &m.vertex_normals(), None, &m.triangulate().0);
    }
    use rayon::prelude::*;
    let faces = m.face_count() as u32;
    // Large meshes (a skinned character re-uploaded every frame of playback) are built in a few
    // runs of faces at once; each run fills its own piece and the pieces are joined in order,
    // so the result is the same as building it in one go.
    let runs = if (faces as usize) < PARALLEL_FACES { 1 } else { max_runs.min(faces as usize / (PARALLEL_FACES / 4)).max(1) as u32 };
    let fnorm: Vec<Vec3> = if runs == 1 {
        (0..faces).map(|f| m.face_normal(f)).collect()
    } else {
        (0..faces).into_par_iter().with_min_len(4096).map(|f| m.face_normal(f)).collect()
    };
    let cos = SMOOTH_ANGLE_DEG.to_radians().cos();
    let build = |from: u32, to: u32| -> MeshData {
        let mut out = MeshData { vertices: Vec::new(), indices: Vec::new() };
        for f in from..to {
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
                let uv = if m.has_uvs() { m.uv(h).to_array() } else { [0.0; 2] };
                let color = if m.has_colors() { m.color(h) } else { WHITE };
                out.vertices.push(MeshVertex { pos: m.position(v).to_array(), normal: normal.to_array(), mask: 0.0, patch: m.patch(f) as f32, uv, color });
                n += 1;
            }
            for i in 1..n.saturating_sub(1) {
                out.indices.extend_from_slice(&[first, first + i, first + i + 1]);
            }
        }
        out
    };
    if runs == 1 {
        return build(0, faces);
    }
    let per = faces.div_ceil(runs);
    let pieces: Vec<MeshData> = (0..runs).into_par_iter().map(|r| build(r * per, ((r + 1) * per).min(faces))).collect();
    let mut out = MeshData { vertices: Vec::with_capacity(corners), indices: Vec::with_capacity(corners * 2) };
    for piece in pieces {
        let base = out.vertices.len() as u32;
        out.vertices.extend_from_slice(&piece.vertices);
        out.indices.extend(piece.indices.iter().map(|i| i + base));
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
            let uv = if m.has_uvs() { m.uv(h).to_array() } else { [0.0; 2] };
            let color = if m.has_colors() { m.color(h) } else { WHITE };
            out.vertices.push(MeshVertex { pos: m.position(m.origin(h)).to_array(), normal: n, mask: 0.0, patch: m.patch(f) as f32, uv, color });
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
        uv: [0.0; 2],
        color: WHITE,
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

    #[test]
    fn large_meshes_built_on_several_cores_equal_the_one_core_result() {
        use modelcraft_scene::Primitive;
        let m = Primitive::Cylinder { radius: 1.0, height: 8.0, sides: 128, rings: 200, caps: true }.build().unwrap();
        assert!(m.face_count() >= PARALLEL_FACES);
        let (one, many) = (mesh_data_in(&m, 1), mesh_data_in(&m, MAX_RUNS));
        assert_eq!(one.indices, many.indices);
        assert!(one.vertices == many.vertices, "the same vertices, bit for bit");
        assert_eq!(mesh_data(&m).vertices.len(), one.vertices.len());
        assert!(one.indices.iter().all(|i| (*i as usize) < one.vertices.len()));
    }
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
        // Position, normal, mask, patch, UV and the vertex colour: 56 bytes (it was 40 before colours).
        assert_eq!(std::mem::size_of::<MeshVertex>(), 56);
    }

    #[test]
    fn corners_carry_their_uvs() {
        // A cube with UVs: every drawn corner has the UV of its face corner; without UVs they are zero.
        let mut m = cube();
        let corner_uvs: Vec<glam::Vec2> = (0..m.corner_count()).map(|c| glam::Vec2::new(c as f32 * 0.01, 0.5)).collect();
        m.set_uvs(&corner_uvs).unwrap();
        let d = mesh_data(&m);
        let uvs: Vec<[f32; 2]> = m.face_half_edges(0).map(|h| m.uv(h).to_array()).collect();
        assert!(m.has_uvs());
        assert_eq!(d.vertices.iter().take(uvs.len()).map(|v| v.uv).collect::<Vec<_>>(), uvs);
        let mut bare = m.clone();
        bare.clear_uvs();
        assert!(mesh_data(&bare).vertices.iter().all(|v| v.uv == [0.0; 2]));
    }
}
