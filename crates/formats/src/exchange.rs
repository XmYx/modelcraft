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
    /// USD in its text form.
    Usda,
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
            "usda" | "usd" => Kind::Usda,
            _ => return None,
        })
    }
    pub fn can_write(self) -> bool {
        matches!(self, Kind::Glb | Kind::Obj | Kind::Stl | Kind::Fbx | Kind::ThreeMf | Kind::Usda)
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
    // What the file says about its axes and unit: FBX and USD say so in a header, 3MF is Z up in
    // the unit its model names.
    let (scene, file_axes, file_scale) = match kind {
        Kind::Fbx => {
            let (axes, scale) = match cm::fbx::read_document(bytes, &limits) {
                Ok(d) => (Some(DMat4::from_cols_array(&d.axes_to_y_up()).as_mat4()), d.unit_scale().map(|cm| (cm / 100.0) as f32)),
                Err(_) => (None, None),
            };
            (cm::fbx::read(bytes, &limits, resolve).map_err(mesh_err)?, axes, scale)
        }
        Kind::ThreeMf => {
            let m = cm::threemf::read_model(bytes, &limits).map_err(mesh_err)?;
            (m.scene, Some(z_to_y()), Some(m.unit.metres() as f32))
        }
        Kind::Usda => {
            let (scene, stage) = crate::usd::read(bytes, &limits)?;
            (scene, stage.z_up.then(z_to_y), stage.metres_per_unit.map(|m| m as f32))
        }
        other => (read_scene(bytes, other, resolve, &limits)?, None, None),
    };
    let (mut items, warnings) = items_of(scene);
    let axes = match convert.up {
        Up::Auto => file_axes.unwrap_or(Mat4::IDENTITY),
        Up::Y => Mat4::IDENTITY,
        Up::Z => z_to_y(),
    };
    let scale = convert.scale.or(file_scale).and_then(scale_ok).unwrap_or(1.0);
    convert_roots(&mut items, Mat4::from_scale(Vec3::splat(scale)) * axes);
    Ok((items, warnings))
}

fn read_scene(bytes: &[u8], kind: Kind, resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>, limits: &cm::Limits) -> Result<cm::Scene> {
    let limits = limits.clone();
    match kind {
        Kind::Fbx => cm::fbx::read(bytes, &limits, resolve),
        Kind::ThreeMf => cm::threemf::read(bytes, &limits),
        Kind::Glb | Kind::Gltf => cm::gltf::read(bytes, &limits, resolve),
        Kind::Obj => cm::obj::read(bytes, &limits, resolve),
        Kind::Stl => cm::stl::read(bytes, &limits),
        Kind::Ply => cm::ply::read(bytes, &limits),
        Kind::Usda => return crate::usd::read(bytes, &limits).map(|(s, _)| s),
    }
    .map_err(mesh_err)
}

/// The scene's meshes and nodes as items (bounded: each node once).
fn items_of(scene: cm::Scene) -> (Vec<Item>, Vec<String>) {
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
        return (items, warnings);
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
    (items, warnings)
}

fn convert_in(m: &cm::Mesh) -> (PolyMesh, Vec<String>) {
    let pos: Vec<Vec3> = m.positions.iter().map(|p| Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32)).collect();
    let uvset = m.uv_sets.first();
    // glTF COLOR_0 and the like: one colour per point.
    let point_colors = (!m.colors.is_empty() && m.colors.len() == m.positions.len()).then_some(&m.colors);
    let mut polys = Vec::new();
    for (pi, part) in m.parts.iter().enumerate() {
        for face in part.faces() {
            let verts = face.iter().map(|c| c.position).collect();
            let uvs: Option<Vec<Vec2>> =
                uvset.and_then(|s| face.iter().map(|c| s.get(c.uv as usize).map(|uv| Vec2::new(uv[0], uv[1]))).collect::<Option<Vec<Vec2>>>());
            let colors = point_colors.map(|cs| face.iter().map(|c| cs.get(c.position as usize).copied().unwrap_or([1.0; 4])).collect());
            polys.push(Polygon { verts, uvs, patch: pi as u32, colors });
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
    if m.has_colors() {
        out.colors = modelcraft_ops::paint::vertex_colors(m);
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
    // 3MF is Z up unless the settings say otherwise; USD says its axis in the file.
    let z = convert.up == Up::Z || (kind == Kind::ThreeMf && convert.up == Up::Auto);
    let axes = if z { z_to_y().inverse() } else { Mat4::IDENTITY };
    let mut scale = convert.scale.and_then(scale_ok).unwrap_or(1.0);
    // 3MF names its unit from a short list; any other scale is written in metres.
    if kind == Kind::ThreeMf && crate::threemf_write::unit_name(scale).is_none() {
        scale = 1.0;
    }
    convert_roots(&mut items, Mat4::from_scale(Vec3::splat(1.0 / scale)) * axes);
    let items = &items[..];
    let limits = cm::Limits::default();
    match kind {
        Kind::Fbx => crate::fbx_write::write_in(items, convert.up == Up::Z, scale),
        Kind::ThreeMf => crate::threemf_write::write_in(items, crate::threemf_write::unit_name(scale).unwrap_or("meter")),
        Kind::Usda => crate::usd::write_in(items, z, scale),
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
        for kind in [Kind::Glb, Kind::Gltf, Kind::Obj, Kind::Stl, Kind::Ply, Kind::Fbx, Kind::ThreeMf, Kind::Usda] {
            let _ = import(b"\x00\x01garbage", kind, &mut |_| None);
        }
        assert!(import(b"", Kind::Glb, &mut |_| None).is_err());
    }

    // ---- UV round trip (the TextureCraft hand-off goes through GLB) ----

    use modelcraft_ops::edit;

    /// Every distinct (position, UV) pair of a mesh, quantised so float noise does not matter.
    fn corner_set(m: &PolyMesh) -> std::collections::BTreeSet<[i64; 5]> {
        let q = |v: f32| (f64::from(v) * 1e4).round() as i64;
        let mut s = std::collections::BTreeSet::new();
        for f in 0..m.face_count() as u32 {
            for h in m.face_half_edges(f) {
                let (p, uv) = (m.position(m.origin(h)), m.uv(h));
                s.insert([q(p.x), q(p.y), q(p.z), (f64::from(uv.x) * 1e4).round() as i64, (f64::from(uv.y) * 1e4).round() as i64]);
            }
        }
        s
    }

    /// Interior edges whose two sides carry different UVs.
    fn seam_edges(m: &PolyMesh) -> usize {
        (0..m.edge_count() as u32)
            .filter(|e| {
                let h = m.edge_half_edge(*e);
                let t = m.twin(h);
                if m.face_of(h) == modelcraft_mesh::NONE || m.face_of(t) == modelcraft_mesh::NONE {
                    return false;
                }
                let same = |a: u32, b: u32| (m.uv(a) - m.uv(b)).abs().max_element() <= 1e-5;
                !(same(h, m.next(t)) && same(m.next(h), t))
            })
            .count()
    }

    fn one(mesh: PolyMesh) -> Vec<Item> {
        vec![Item { name: "Mesh".into(), parent: None, transform: Transform::default(), mesh: Some(Arc::new(mesh)), material: Material::default() }]
    }

    fn assert_uvs_survive(m: &PolyMesh, kind: Kind) {
        let bytes = export(&one(m.clone()), kind).unwrap();
        let (back, _) = import(&bytes, kind, &mut |_| None).unwrap();
        let b = back.iter().find_map(|i| i.mesh.clone()).unwrap();
        assert!(b.has_uvs(), "{kind:?}: UVs lost");
        // The positions and UVs of every corner come back, and nothing else appears.
        let (want, got) = (corner_set(m), corner_set(&b));
        let missing: Vec<_> = want.difference(&got).take(3).collect();
        let extra: Vec<_> = got.difference(&want).take(3).collect();
        assert!(missing.is_empty() && extra.is_empty(), "{kind:?}: missing {missing:?}, extra {extra:?}");
        // Seams (UV discontinuities) are neither created nor healed.
        assert_eq!(seam_edges(&b), seam_edges(m), "{kind:?}: seam count changed");
    }

    #[test]
    fn uvs_round_trip_exactly_through_glb_and_obj_for_every_primitive() {
        for kind in modelcraft_ops::primitives::KINDS {
            let mesh = Primitive::default_of(kind).unwrap().build().unwrap();
            for format in [Kind::Glb, Kind::Obj] {
                assert_uvs_survive(&mesh, format);
            }
        }
    }

    #[test]
    fn uvs_outside_the_unit_square_and_a_flat_layout_survive() {
        // Tiled UVs (a texture repeated 3 times, and a negative offset) are legal in glTF and OBJ.
        let mut m = Primitive::Plane { width: 2.0, depth: 2.0, segments: 3 }.build().unwrap();
        let uvs: Vec<Vec2> = m.corner_uvs().iter().map(|u| *u * 3.0 + Vec2::new(-1.5, 0.25)).collect();
        m.set_uvs(&uvs).unwrap();
        for format in [Kind::Glb, Kind::Obj] {
            assert_uvs_survive(&m, format);
        }
        // A cube with every face split in UV space (six islands) keeps its twelve seam edges.
        let cube = Primitive::Cube { size: Vec3::ONE, segments: 1 }.build().unwrap();
        assert_eq!(seam_edges(&cube), 12);
        assert_uvs_survive(&cube, Kind::Glb);
        // Welded geometry with a single seam (a tube closed with one seam line) stays a tube.
        let tube = edit::weld(&Primitive::Cylinder { radius: 1.0, height: 2.0, sides: 12, rings: 2, caps: false }.build().unwrap(), 1e-5).unwrap();
        assert!(seam_edges(&tube) > 0);
        for format in [Kind::Glb, Kind::Obj] {
            assert_uvs_survive(&tube, format);
        }
    }

    #[test]
    fn obj_stores_v_upwards_and_glb_downwards() {
        // The plane's corner at the top left of the image has uv (0, 0).
        let m = Primitive::Plane { width: 2.0, depth: 2.0, segments: 1 }.build().unwrap();
        let obj = String::from_utf8(export(&one(m.clone()), Kind::Obj).unwrap()).unwrap();
        let vts: Vec<(f32, f32)> = obj
            .lines()
            .filter_map(|l| l.strip_prefix("vt "))
            .map(|l| {
                let mut it = l.split_whitespace().filter_map(|x| x.parse::<f32>().ok());
                (it.next().unwrap(), it.next().unwrap())
            })
            .collect();
        assert_eq!(vts.len(), 4);
        // Our (u, v) = (0, 0) is the image's top left: OBJ has v up, so it is written as (0, 1).
        assert!(vts.contains(&(0.0, 1.0)) && vts.contains(&(1.0, 0.0)), "{vts:?}");
        // GLB keeps (u, v) as they are: read the TEXCOORD_0 floats back from the binary chunk.
        let glb = export(&one(m.clone()), Kind::Glb).unwrap();
        let uvs = glb_texcoords(&glb);
        assert_eq!(uvs.len(), 4);
        assert!(uvs.contains(&[0.0, 0.0]) && uvs.contains(&[1.0, 1.0]) && uvs.contains(&[1.0, 0.0]) && uvs.contains(&[0.0, 1.0]), "{uvs:?}");
        let want: Vec<[f32; 2]> = m.corner_uvs().iter().map(|u| u.to_array()).collect();
        assert!(want.iter().all(|u| uvs.contains(u)), "{want:?} vs {uvs:?}");
    }

    /// TEXCOORD_0 of the first primitive of a GLB, read from its binary chunk without craft-mesh.
    fn glb_texcoords(bytes: &[u8]) -> Vec<[f32; 2]> {
        let json = glb_json(bytes);
        let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let bin = 20 + json_len;
        let bin_len = u32::from_le_bytes(bytes[bin..bin + 4].try_into().unwrap()) as usize;
        assert_eq!(&bytes[bin + 4..bin + 8], b"BIN\0");
        let data = &bytes[bin + 8..bin + 8 + bin_len];
        let acc = &json["accessors"][json["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"].as_u64().unwrap() as usize];
        let view = &json["bufferViews"][acc["bufferView"].as_u64().unwrap() as usize];
        let offset = view["byteOffset"].as_u64().unwrap_or(0) as usize + acc["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = view["byteStride"].as_u64().unwrap_or(8) as usize;
        (0..acc["count"].as_u64().unwrap() as usize)
            .map(|i| {
                let at = offset + i * stride;
                [f32::from_le_bytes(data[at..at + 4].try_into().unwrap()), f32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap())]
            })
            .collect()
    }

    /// The JSON chunk of a GLB, read without craft-mesh: what any glTF consumer starts from.
    fn glb_json(bytes: &[u8]) -> serde_json::Value {
        assert_eq!(&bytes[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
        let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        assert_eq!(&bytes[16..20], b"JSON");
        serde_json::from_slice(&bytes[20..20 + len]).unwrap()
    }

    #[test]
    fn the_glb_a_consumer_receives_has_texcoords_for_every_primitive() {
        let json = glb_json(&export(&items(), Kind::Glb).unwrap());
        let prims = json["meshes"].as_array().unwrap().iter().flat_map(|m| m["primitives"].as_array().unwrap().clone()).collect::<Vec<_>>();
        assert!(!prims.is_empty());
        for p in &prims {
            let attrs = &p["attributes"];
            let uv = &json["accessors"][attrs["TEXCOORD_0"].as_u64().expect("TEXCOORD_0") as usize];
            let pos = &json["accessors"][attrs["POSITION"].as_u64().unwrap() as usize];
            assert_eq!(uv["type"], "VEC2");
            assert_eq!(uv["componentType"], 5126, "float");
            assert_eq!(uv["count"], pos["count"], "one UV per vertex");
        }
        // Without UVs there is no TEXCOORD_0 (a consumer then unwraps by itself).
        let mut bare = Primitive::Cube { size: Vec3::ONE, segments: 1 }.build().unwrap();
        bare.clear_uvs();
        let json = glb_json(&export(&one(bare), Kind::Glb).unwrap());
        assert!(json["meshes"][0]["primitives"][0]["attributes"].get("TEXCOORD_0").is_none());
    }
}
