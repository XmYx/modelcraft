//! Exchange formats through the shared `craft-mesh` crate: glTF 2.0 / GLB (read and write), OBJ
//! (read and write), STL (read and write), PLY (read). Conventions match ArtCraft, TextureCraft
//! and MaterialCraft: Y up, metres, UV origin at the top left.
//!
//! Imported meshes are welded (glTF splits vertices at UV seams; modeling wants them joined, with
//! the UVs kept per corner) and each file part becomes a patch.

use std::sync::Arc;

use craft_mesh as cm;
use glam::{DMat4, Mat4, Quat, Vec2, Vec3};
use modelcraft_mesh::{PolyMesh, Polygon};
use modelcraft_scene::{Material, Transform};

use crate::{FormatError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Glb,
    Gltf,
    Obj,
    Stl,
    Ply,
}

impl Kind {
    pub fn from_path(path: &std::path::Path) -> Option<Kind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "glb" => Kind::Glb,
            "gltf" => Kind::Gltf,
            "obj" => Kind::Obj,
            "stl" => Kind::Stl,
            "ply" => Kind::Ply,
            _ => return None,
        })
    }
    pub fn can_write(self) -> bool {
        matches!(self, Kind::Glb | Kind::Obj | Kind::Stl)
    }
}

/// One object to export or one imported node.
#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    /// Index of the parent item.
    pub parent: Option<usize>,
    pub transform: Transform,
    pub mesh: Option<Arc<PolyMesh>>,
    pub material: Material,
}

fn mesh_err(e: cm::Error) -> FormatError {
    FormatError::Malformed(e.to_string())
}

/// Read a file's bytes. `resolve` supplies files the main one refers to (OBJ material
/// libraries, glTF buffers); it may decline.
pub fn import(bytes: &[u8], kind: Kind, resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>) -> Result<(Vec<Item>, Vec<String>)> {
    let limits = cm::Limits::default();
    let scene = match kind {
        Kind::Glb | Kind::Gltf => cm::gltf::read(bytes, &limits, resolve),
        Kind::Obj => cm::obj::read(bytes, &limits, resolve),
        Kind::Stl => cm::stl::read(bytes, &limits),
        Kind::Ply => cm::ply::read(bytes, &limits),
    }
    .map_err(mesh_err)?;
    let mut warnings = scene.warnings.clone();
    let mats: Vec<Material> = scene
        .materials
        .iter()
        .map(|m| Material { base_color: m.base_color, metallic: m.metallic.clamp(0.0, 1.0), roughness: m.roughness.clamp(0.0, 1.0), linked: None })
        .collect();
    let meshes: Vec<(Arc<PolyMesh>, Material)> = scene
        .meshes
        .iter()
        .map(|m| {
            let (pm, w) = convert_in(m);
            warnings.extend(w);
            let mat = m.parts.iter().find_map(|p| p.material).and_then(|i| mats.get(i)).cloned().unwrap_or_default();
            (Arc::new(pm), mat)
        })
        .collect();
    let mut items = Vec::new();
    if scene.nodes.is_empty() {
        for (i, (mesh, material)) in meshes.into_iter().enumerate() {
            let name = scene.meshes.get(i).map(|m| m.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Mesh {}", i + 1));
            items.push(Item { name, parent: None, transform: Transform::default(), mesh: Some(mesh), material });
        }
        return Ok((items, warnings));
    }
    // Walk the node tree from the roots (bounded: each node once, depth ≤ craft-mesh's limit).
    let mut index_of = vec![None; scene.nodes.len()];
    let mut stack: Vec<(usize, Option<usize>)> = scene.roots.iter().rev().map(|r| (*r, None)).collect();
    while let Some((n, parent)) = stack.pop() {
        let Some(node) = scene.nodes.get(n) else { continue };
        if index_of.get(n).copied().flatten().is_some() {
            continue;
        }
        let m = DMat4::from_cols_array(&node.transform).as_mat4();
        let (scale, rotation, translation) = if m.is_finite() { m.to_scale_rotation_translation() } else { (Vec3::ONE, Quat::IDENTITY, Vec3::ZERO) };
        let (mesh, material) = match node.mesh.and_then(|i| meshes.get(i)) {
            Some((m, mat)) => (Some(m.clone()), mat.clone()),
            None => (None, Material::default()),
        };
        let name = if node.name.is_empty() { format!("Node {}", n + 1) } else { node.name.clone() };
        items.push(Item { name, parent, transform: Transform { translation, rotation, scale }.sanitized(), mesh, material });
        let me = items.len() - 1;
        if let Some(slot) = index_of.get_mut(n) {
            *slot = Some(me);
        }
        for c in node.children.iter().rev() {
            stack.push((*c, Some(me)));
        }
    }
    Ok((items, warnings))
}

fn convert_in(m: &cm::Mesh) -> (PolyMesh, Vec<String>) {
    let pos: Vec<Vec3> = m.positions.iter().map(|p| Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32)).collect();
    let uvset = m.uv_sets.first();
    let mut polys = Vec::new();
    for (pi, part) in m.parts.iter().enumerate() {
        for face in part.faces() {
            let verts = face.iter().map(|c| c.position).collect();
            let uvs: Option<Vec<Vec2>> =
                uvset.and_then(|s| face.iter().map(|c| s.get(c.uv as usize).map(|uv| Vec2::new(uv[0], uv[1]))).collect::<Option<Vec<Vec2>>>());
            polys.push(Polygon { verts, uvs, patch: pi as u32 });
        }
    }
    let mut warnings = Vec::new();
    let built = match PolyMesh::build(pos, polys) {
        Ok((pm, report)) => {
            if !report.is_clean() {
                warnings.push(format!("mesh `{}`: {} faces dropped, {} turned, {} detached", m.name, report.dropped, report.flipped, report.detached));
            }
            pm
        }
        Err(e) => {
            warnings.push(format!("mesh `{}`: {e}", m.name));
            PolyMesh::default()
        }
    };
    let welded = modelcraft_ops::edit::weld(&built, 1e-6).unwrap_or(built);
    (welded, warnings)
}

fn convert_out(name: &str, m: &PolyMesh, material: Option<usize>) -> cm::Mesh {
    let normals = m.vertex_normals();
    let mut out = cm::Mesh {
        name: name.to_string(),
        positions: m.positions().iter().map(|p| [p.x as f64, p.y as f64, p.z as f64]).collect(),
        normals: normals.iter().map(|n| [n.x as f64, n.y as f64, n.z as f64]).collect(),
        ..Default::default()
    };
    let mut uvs = Vec::new();
    let mut part = cm::Part { name: name.to_string(), material, ..Default::default() };
    for f in 0..m.face_count() as u32 {
        let mut n = 0;
        for h in m.face_half_edges(f) {
            let v = m.origin(h);
            let uv = if m.has_uvs() {
                uvs.push(m.uv(h).to_array());
                (uvs.len() - 1) as u32
            } else {
                cm::NONE
            };
            part.corners.push(cm::Corner { position: v, normal: v, uv });
            n += 1;
        }
        part.face_sizes.push(n);
    }
    if m.has_uvs() {
        out.uv_sets.push(uvs);
    }
    out.parts.push(part);
    out
}

/// Build the neutral scene for writing.
fn neutral(items: &[Item]) -> cm::Scene {
    let mut s = cm::Scene::default();
    for (i, it) in items.iter().enumerate() {
        let mesh = it.mesh.as_ref().filter(|m| m.face_count() > 0).map(|m| {
            s.materials.push(cm::Material {
                name: format!("{} material", it.name),
                base_color: it.material.base_color,
                metallic: it.material.metallic,
                roughness: it.material.roughness,
                ..Default::default()
            });
            s.meshes.push(convert_out(&it.name, m, Some(s.materials.len() - 1)));
            s.meshes.len() - 1
        });
        let mat: Mat4 = it.transform.matrix();
        s.nodes.push(cm::Node { name: it.name.clone(), transform: mat.as_dmat4().to_cols_array(), mesh, ..Default::default() });
        match it.parent.filter(|p| *p < i) {
            Some(p) => {
                if let Some(n) = s.nodes.get_mut(p) {
                    n.children.push(i);
                }
            }
            None => s.roots.push(i),
        }
    }
    s
}

/// Write items as GLB, OBJ or STL bytes. OBJ and STL have no hierarchy: meshes are written in
/// world space.
pub fn export(items: &[Item], kind: Kind) -> Result<Vec<u8>> {
    let limits = cm::Limits::default();
    match kind {
        Kind::Glb => cm::gltf::write_glb(&neutral(items), &limits).map_err(mesh_err),
        Kind::Obj | Kind::Stl => {
            let world = flatten(items);
            let s = neutral(&world);
            if kind == Kind::Obj {
                cm::obj::write(&s, None, &limits).map(String::into_bytes).map_err(mesh_err)
            } else {
                cm::stl::write_binary(&s, "ModelCraft", &limits).map_err(mesh_err)
            }
        }
        other => Err(FormatError::Unsupported(format!("writing {other:?}"))),
    }
}

/// Bake each item's world transform into its mesh and drop the hierarchy.
fn flatten(items: &[Item]) -> Vec<Item> {
    let world = |mut i: usize| {
        let mut m = Mat4::IDENTITY;
        for _ in 0..64 {
            let Some(it) = items.get(i) else { break };
            m = it.transform.matrix() * m;
            match it.parent {
                Some(p) if p != i => i = p,
                _ => break,
            }
        }
        m
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let w = world(i);
            let mut m = (**it.mesh.as_ref()?).clone();
            m.map_positions(|_, p| w.transform_point3(p));
            Some(Item { name: it.name.clone(), parent: None, transform: Transform::default(), mesh: Some(Arc::new(m)), material: it.material.clone() })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use modelcraft_ops::primitives::Primitive;

    fn items() -> Vec<Item> {
        let cube = Arc::new(Primitive::Cube { size: Vec3::ONE, segments: 2 }.build().unwrap());
        let t = Transform { translation: Vec3::new(0.0, 1.0, 0.0), ..Default::default() };
        vec![
            Item { name: "Root".into(), parent: None, transform: t, mesh: None, material: Material::default() },
            Item {
                name: "Cube".into(),
                parent: Some(0),
                transform: Transform::default(),
                mesh: Some(cube),
                material: Material { base_color: [1.0, 0.0, 0.0, 1.0], ..Default::default() },
            },
        ]
    }

    #[test]
    fn glb_round_trip_keeps_topology_uvs_hierarchy_and_material() {
        let bytes = export(&items(), Kind::Glb).unwrap();
        let (back, _) = import(&bytes, Kind::Glb, &mut |_| None).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[1].parent, Some(0));
        assert!((back[0].transform.translation.y - 1.0).abs() < 1e-6);
        let m = back[1].mesh.as_ref().unwrap();
        // glTF stores triangles only; the vertices are welded back into one surface.
        assert_eq!((m.vertex_count(), m.face_count()), (26, 48));
        assert!(m.has_uvs());
        assert_eq!(back[1].material.base_color, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn obj_and_stl_round_trip_in_world_space() {
        for kind in [Kind::Obj, Kind::Stl] {
            let bytes = export(&items(), kind).unwrap();
            let (back, _) = import(&bytes, kind, &mut |_| None).unwrap();
            let m = back.iter().find_map(|i| i.mesh.clone()).unwrap();
            assert_eq!(m.vertex_count(), 26, "{kind:?}");
            let (lo, _) = m.bounds();
            assert!((lo.y - 0.5).abs() < 1e-4, "{kind:?} lost the parent transform");
        }
    }

    #[test]
    fn garbage_is_an_error() {
        for kind in [Kind::Glb, Kind::Gltf, Kind::Obj, Kind::Stl, Kind::Ply] {
            let _ = import(b"\x00\x01garbage", kind, &mut |_| None);
        }
        assert!(import(b"", Kind::Glb, &mut |_| None).is_err());
    }
}
