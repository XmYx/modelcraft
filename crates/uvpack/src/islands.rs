//! UV islands of a mesh that already has UVs, and packing them in place.

use glam::{Vec2, Vec3};
use modelcraft_mesh::{NONE, PolyMesh};

use crate::pack::{PackIsland, PackOptions, PackResult, pack};
use crate::{MAX_FACES, PackError};

/// How far apart two UVs may be and still count as the same point.
const SAME: f32 = 1e-6;

fn find(p: &mut [u32], mut x: u32) -> u32 {
    loop {
        let parent = p.get(x as usize).copied().unwrap_or(x);
        if parent == x {
            return x;
        }
        let grand = p.get(parent as usize).copied().unwrap_or(parent);
        if let Some(s) = p.get_mut(x as usize) {
            *s = grand;
        }
        x = grand;
    }
}

/// The islands of `mesh`: groups of faces connected through edges whose two sides carry the same
/// UVs at both ends. Empty for a mesh without UVs. Islands are in order of their first face.
pub fn uv_islands(mesh: &PolyMesh) -> Vec<Vec<u32>> {
    let nf = mesh.face_count();
    if !mesh.has_uvs() || nf > MAX_FACES {
        return Vec::new();
    }
    let mut dsu: Vec<u32> = (0..nf as u32).collect();
    let same = |a: u32, b: u32| (mesh.uv(a) - mesh.uv(b)).abs().max_element() <= SAME;
    for h in 0..mesh.corner_count() as u32 {
        let t = mesh.twin(h);
        if h >= t || mesh.face_of(t) == NONE {
            continue;
        }
        if same(h, mesh.next(t)) && same(mesh.next(h), t) {
            let (ra, rb) = (find(&mut dsu, mesh.face_of(h)), find(&mut dsu, mesh.face_of(t)));
            if ra != rb
                && let Some(s) = dsu.get_mut(ra.max(rb) as usize)
            {
                *s = ra.min(rb);
            }
        }
    }
    let mut index = vec![NONE; nf];
    let mut out: Vec<Vec<u32>> = Vec::new();
    for f in 0..nf as u32 {
        let r = find(&mut dsu, f) as usize;
        let id = match index.get(r).copied() {
            Some(i) if i != NONE => i,
            _ => {
                let i = out.len() as u32;
                if let Some(s) = index.get_mut(r) {
                    *s = i;
                }
                out.push(Vec::new());
                i
            }
        };
        if let Some(v) = out.get_mut(id as usize) {
            v.push(f);
        }
    }
    out
}

/// The triangles of a face as fans, in UV space and as surface area.
fn face_fan(mesh: &PolyMesh, f: u32, uv_tris: &mut Vec<[Vec2; 3]>) -> f32 {
    let c: Vec<u32> = mesh.face_half_edges(f).collect();
    let p = |i: usize| c.get(i).map_or(Vec3::ZERO, |h| mesh.position(mesh.origin(*h)));
    let u = |i: usize| c.get(i).map_or(Vec2::ZERO, |h| mesh.uv(*h));
    let mut area = 0.0;
    for i in 1..c.len().saturating_sub(1) {
        uv_tris.push([u(0), u(i), u(i + 1)]);
        area += 0.5 * (p(i) - p(0)).cross(p(i + 1) - p(0)).length();
    }
    area
}

/// The islands as [`PackIsland`]s, with the surface area of each.
pub fn pack_islands(mesh: &PolyMesh, faces: &[Vec<u32>]) -> Vec<PackIsland> {
    faces
        .iter()
        .map(|fs| {
            let mut island = PackIsland::default();
            for &f in fs {
                island.area_3d += face_fan(mesh, f, &mut island.triangles);
            }
            island
        })
        .collect()
}

/// Texel density of each island: `√(UV area / surface area)`, in UV units per metre.
pub fn island_densities(mesh: &PolyMesh) -> Vec<f32> {
    pack_islands(mesh, &uv_islands(mesh)).iter().map(|i| if i.area_3d > 0.0 { (i.uv_area() / i.area_3d).sqrt() } else { 0.0 }).collect()
}

/// Pack the islands of `mesh` into the unit square. Returns one UV per corner (in corner order,
/// see [`PolyMesh::corner_count`]) and what the packer did.
pub fn pack_mesh(mesh: &PolyMesh, opts: &PackOptions) -> Result<(Vec<Vec2>, PackResult), PackError> {
    if !mesh.has_uvs() {
        return Err(PackError::NoUvs);
    }
    let groups = uv_islands(mesh);
    if groups.is_empty() {
        return Err(PackError::NoFaces);
    }
    let islands = pack_islands(mesh, &groups);
    let result = pack(&islands, opts)?;
    let mut uvs: Vec<Vec2> = mesh.corner_uvs().to_vec();
    for (group, place) in groups.iter().zip(&result.placements) {
        for &f in group {
            for h in mesh.face_half_edges(f) {
                if let Some(u) = uvs.get_mut(h as usize) {
                    *u = place.apply(mesh.uv(h));
                }
            }
        }
    }
    Ok((uvs, result))
}
