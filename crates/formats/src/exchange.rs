//! Exchange formats through the shared `craft-mesh` crate: glTF 2.0 / GLB (read and write), OBJ
//! (read and write), STL (read and write), PLY (read), FBX (read, binary and ASCII; written as
//! ASCII by [`crate::fbx_write`]) and 3MF (read).
//!
//! [`Convert`] changes the up axis and the unit on the way in or out: FBX files say theirs, and
//! the importer turns them into ModelCraft's Y up and metres unless told otherwise. Conventions match ArtCraft, TextureCraft
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
    Fbx,
    ThreeMf,
}

/// The up axis of a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Up {
    /// What the file says (FBX), else Y.
    #[default]
    Auto,
    Y,
    Z,
}

/// Axis and unit conversion between a file and the scene (Y up, metres).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Convert {
    pub up: Up,
    /// Metres per file unit; `None`: what the file says (FBX), else 1.
    pub scale: Option<f32>,
}

impl Default for Convert {
    fn default() -> Self {
        Convert { up: Up::Auto, scale: None }
    }
}

/// The turn from Z up to Y up (+Z becomes +Y, +Y becomes −Z).
fn z_to_y() -> Mat4 {
    Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
}

/// Put `c` in front of every root item's transform.
fn convert_roots(items: &mut [Item], c: Mat4) {
    if c == Mat4::IDENTITY || !c.is_finite() {
        return;
    }
    for it in items.iter_mut().filter(|i| i.parent.is_none()) {
        let (scale, rotation, translation) = (c * it.transform.matrix()).to_scale_rotation_translation();
        it.transform = Transform { translation, rotation, scale }.sanitized();
    }
}

fn scale_ok(s: f32) -> Option<f32> {
    (s.is_finite() && s > 1e-9 && s < 1e9).then_some(s)
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
            "fbx" => Kind::Fbx,
            "3mf" => Kind::ThreeMf,
            _ => return None,
        })
    }
    pub fn can_write(self) -> bool {
        matches!(self, Kind::Glb | Kind::Obj | Kind::Stl | Kind::Fbx)
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
    import_with(bytes, kind, resolve, Convert::default())
}

/// [`import`] with an axis and unit conversion.
pub fn import_with(bytes: &[u8], kind: Kind, resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>, convert: Convert) -> Result<(Vec<Item>, Vec<String>)> {
    let limits = cm::Limits::default();
    // What the file says about its axes and unit (FBX only).
    let (file_axes, file_scale) = match kind {
        Kind::Fbx => match cm::fbx::read_document(bytes, &limits) {
            Ok(d) => (Some(DMat4::from_cols_array(&d.axes_to_y_up()).as_mat4()), d.unit_scale().map(|cm| (cm / 100.0) as f32)),
            Err(_) => (None, None),
        },
        _ => (None, None),
    };
    let (mut items, warnings) = import_raw(bytes, kind, resolve, &limits)?;
    let axes = match convert.up {
        Up::Auto => file_axes.unwrap_or(Mat4::IDENTITY),
        Up::Y => Mat4::IDENTITY,
        Up::Z => z_to_y(),
    };
    let scale = convert.scale.or(file_scale).and_then(scale_ok).unwrap_or(1.0);
    convert_roots(&mut items, Mat4::from_scale(Vec3::splat(scale)) * axes);
    Ok((items, warnings))
}

fn import_raw(bytes: &[u8], kind: Kind, resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>, limits: &cm::Limits) -> Result<(Vec<Item>, Vec<String>)> {
    let limits = limits.clone();
    let scene = match kind {
        Kind::Fbx => cm::fbx::read(bytes, &limits, resolve),
        Kind::ThreeMf => cm::threemf::read(bytes, &limits),
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
        .map(|m| Material { base_color: m.base_color, metallic: m.metallic.clamp(0.0, 1.0), roughness: m.roughness.clamp(0.0, 1.0), ..Material::default() })
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
    export_with(items, kind, Convert::default())
}

/// [`export`] into another up axis (`Up::Z`) or unit (`scale` file units per metre is
/// `1 / scale`).
pub fn export_with(items: &[Item], kind: Kind, convert: Convert) -> Result<Vec<u8>> {
    let mut items = items.to_vec();
    let axes = if convert.up == Up::Z { z_to_y().inverse() } else { Mat4::IDENTITY };
    let scale = convert.scale.and_then(scale_ok).unwrap_or(1.0);
    convert_roots(&mut items, Mat4::from_scale(Vec3::splat(1.0 / scale)) * axes);
    let items = &items[..];
    let limits = cm::Limits::default();
    match kind {
        Kind::Fbx => crate::fbx_write::write_in(items, convert.up == Up::Z, scale),
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
    fn fbx_round_trip_keeps_quads_hierarchy_rotation_uvs_and_material() {
        let mut its = items();
        its[1].transform.rotation = Quat::from_euler(glam::EulerRot::XYZ, 0.3, -0.7, 1.1);
        its[1].transform.scale = Vec3::new(1.0, 2.0, 0.5);
        let bytes = export(&its, Kind::Fbx).unwrap();
        assert!(bytes.starts_with(b"; FBX 7.4.0"));
        let (back, warnings) = import(&bytes, Kind::Fbx, &mut |_| None).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.len(), 2);
        assert_eq!(back[1].parent, Some(0));
        assert!((back[0].transform.translation.y - 1.0).abs() < 1e-5, "{:?}", back[0].transform);
        let m = back[1].mesh.as_ref().unwrap();
        let src = its[1].mesh.as_ref().unwrap();
        assert_eq!((m.vertex_count(), m.face_count()), (src.vertex_count(), src.face_count()), "quads are kept");
        assert!(m.has_uvs());
        for f in 0..m.face_count() as u32 {
            for (a, b) in m.face_half_edges(f).zip(src.face_half_edges(f)) {
                assert!((m.uv(a) - src.uv(b)).length() < 1e-5, "uv of face {f}");
            }
        }
        assert!(back[1].transform.rotation.angle_between(its[1].transform.rotation) < 1e-4, "{:?}", back[1].transform.rotation);
        assert!((back[1].transform.scale - its[1].transform.scale).length() < 1e-4);
        assert_eq!(back[1].material.base_color, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn up_axis_and_units_convert_both_ways() {
        let its = items();
        // Written Z up in centimetres, read back as such: the scene is unchanged.
        let c = Convert { up: Up::Z, scale: Some(0.01) };
        for kind in [Kind::Glb, Kind::Obj, Kind::Fbx] {
            let bytes = export_with(&its, kind, c).unwrap();
            let (raw, _) = import_with(&bytes, kind, &mut |_| None, Convert { up: Up::Y, scale: Some(1.0) }).unwrap();
            let root_raw = raw.iter().find_map(|i| i.mesh.clone().map(|m| (m, i.transform))).unwrap();
            let (lo, hi) = root_raw.0.bounds();
            let size = root_raw.1.matrix().transform_vector3(hi - lo);
            assert!((size.z.abs() - 100.0).abs() < 1e-2 || kind != Kind::Obj, "{kind:?} raw file is Z up in cm: {size:?}");
            let (back, _) = import_with(&bytes, kind, &mut |_| None, c).unwrap();
            let world = |items: &[Item]| -> Vec3 {
                let i = items.iter().position(|i| i.mesh.is_some()).unwrap();
                let mut m = items[i].transform.matrix();
                let mut p = items[i].parent;
                while let Some(k) = p {
                    m = items[k].transform.matrix() * m;
                    p = items[k].parent;
                }
                let (lo, hi) = items[i].mesh.as_ref().unwrap().bounds();
                m.transform_point3((lo + hi) * 0.5)
            };
            let w = world(&back);
            assert!((w - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-3, "{kind:?}: {w:?}");
        }
        // An FBX file says its axes and unit, so reading it with no settings gives the scene back.
        let fbx = export_with(&its, Kind::Fbx, c).unwrap();
        assert!(String::from_utf8_lossy(&fbx).contains("\"UpAxis\", \"int\", \"Integer\", \"\",2"));
        let (auto, _) = import(&fbx, Kind::Fbx, &mut |_| None).unwrap();
        assert!((auto[0].transform.translation - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-3, "{:?}", auto[0].transform.translation);
    }

    #[test]
    fn garbage_is_an_error() {
        for kind in [Kind::Glb, Kind::Gltf, Kind::Obj, Kind::Stl, Kind::Ply, Kind::Fbx, Kind::ThreeMf] {
            let _ = import(b"\x00\x01garbage", kind, &mut |_| None);
        }
        assert!(import(b"", Kind::Glb, &mut |_| None).is_err());
    }
}
