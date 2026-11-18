//! An FBX 7.4 ASCII writer: meshes with their polygons kept whole, per-corner UVs and normals,
//! one material each, the object hierarchy with translation, rotation (Euler, X then Y then Z)
//! and scale, and `GlobalSettings` saying Y up and metres.
//!
//! Written from public descriptions of the ASCII syntax and of the scene objects (`Geometry`,
//! `Model`, `Material`, `Connections`) in the FBX SDK documentation; no exporter source was read
//! and nothing of the SDK is used. It is checked by reading its output back through craft-mesh's
//! FBX reader. A candidate to move to craft-mesh next to that reader.

use std::fmt::Write as _;

use glam::EulerRot;

use crate::Result;
use crate::exchange::Item;

/// Object ids: anything but 0 (the scene root).
fn id(kind: u64, i: usize) -> u64 {
    1_000_000 * kind + i as u64 + 1
}

/// FBX strings cannot hold a double quote or a line break; names are cleaned.
fn clean(name: &str) -> String {
    name.chars().filter(|c| *c != '"' && !c.is_control()).take(200).collect()
}

fn f(v: f64) -> String {
    if v.is_finite() { format!("{v}") } else { "0".into() }
}

fn array(out: &mut String, key: &str, values: impl ExactSizeIterator<Item = String>) {
    let n = values.len();
    let _ = write!(out, "\t\t{key}: *{n} {{\n\t\t\ta: ");
    for (i, v) in values.enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&v);
    }
    out.push_str("\n\t\t}\n");
}

fn p70(out: &mut String, name: &str, ty: &str, label: &str, flags: &str, values: &[f64]) {
    let vals: Vec<String> = values.iter().map(|v| f(*v)).collect();
    let _ = writeln!(out, "\t\t\tP: \"{name}\", \"{ty}\", \"{label}\", \"{flags}\",{}", vals.join(","));
}

/// The items as FBX 7.4 ASCII text, Y up in metres.
pub fn write(items: &[Item]) -> Result<Vec<u8>> {
    write_in(items, false, 1.0)
}

/// The items as FBX 7.4 ASCII text whose positions are already in the file's axes (`z_up`) and
/// unit (`metres_per_unit`); `GlobalSettings` says which, so readers can turn them back.
pub fn write_in(items: &[Item], z_up: bool, metres_per_unit: f32) -> Result<Vec<u8>> {
    let mut o = String::new();
    o.push_str("; FBX 7.4.0 project file\n; Written by ModelCraft\n\n");
    o.push_str("FBXHeaderExtension:  {\n\tFBXHeaderVersion: 1003\n\tFBXVersion: 7400\n\tCreator: \"ModelCraft\"\n}\n");
    o.push_str("GlobalSettings:  {\n\tVersion: 1000\n\tProperties70:  {\n");
    // Z up: +Z up, −Y towards the viewer, +X right.
    let axes = if z_up { [2, 1, 1, -1, 0, 1] } else { [1, 1, 2, 1, 0, 1] };
    for (name, v) in ["UpAxis", "UpAxisSign", "FrontAxis", "FrontAxisSign", "CoordAxis", "CoordAxisSign"].iter().zip(axes) {
        let _ = writeln!(o, "\t\tP: \"{name}\", \"int\", \"Integer\", \"\",{v}");
    }
    let cm = if metres_per_unit.is_finite() && metres_per_unit > 0.0 { f64::from(metres_per_unit) * 100.0 } else { 100.0 };
    let _ = write!(o, "\t\tP: \"UnitScaleFactor\", \"double\", \"Number\", \"\",{}\n\t}}\n}}\n", f(cm));
    o.push_str("Objects:  {\n");
    let mut links: Vec<(u64, u64)> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        let model = id(1, i);
        let name = clean(&it.name);
        let mesh = it.mesh.as_ref().filter(|m| m.face_count() > 0);
        if let Some(m) = mesh {
            let geo = id(2, i);
            let _ = writeln!(o, "\tGeometry: {geo}, \"Geometry::{name}\", \"Mesh\" {{");
            array(&mut o, "Vertices", m.positions().iter().flat_map(|p| [p.x, p.y, p.z]).map(|v| f(f64::from(v))).collect::<Vec<_>>().into_iter());
            // Polygon corners; the last of each polygon is written as -(index + 1).
            let mut idx = Vec::new();
            let mut uv_index = Vec::new();
            let mut uvs = Vec::new();
            let mut normals = Vec::new();
            let vn = m.vertex_normals();
            for face in 0..m.face_count() as u32 {
                let hs: Vec<u32> = m.face_half_edges(face).collect();
                for (k, h) in hs.iter().enumerate() {
                    let v = m.origin(*h);
                    let last = k + 1 == hs.len();
                    idx.push(if last { -(i64::from(v) + 1) } else { i64::from(v) });
                    let n = vn.get(v as usize).copied().unwrap_or(glam::Vec3::Y);
                    normals.extend([n.x, n.y, n.z]);
                    if m.has_uvs() {
                        let uv = m.uv(*h);
                        // ModelCraft keeps UVs with v down, as glTF does; FBX has v up.
                        uvs.extend([uv.x, 1.0 - uv.y]);
                        uv_index.push(uv_index.len() as i64);
                    }
                }
            }
            array(&mut o, "PolygonVertexIndex", idx.iter().map(|v| v.to_string()).collect::<Vec<_>>().into_iter());
            o.push_str("\t\tGeometryVersion: 124\n");
            o.push_str("\t\tLayerElementNormal: 0 {\n\t\t\tVersion: 101\n\t\t\tName: \"\"\n\t\t\tMappingInformationType: \"ByPolygonVertex\"\n\t\t\tReferenceInformationType: \"Direct\"\n");
            array(&mut o, "Normals", normals.iter().map(|v| f(f64::from(*v))).collect::<Vec<_>>().into_iter());
            o.push_str("\t\t}\n");
            if m.has_uvs() {
                o.push_str("\t\tLayerElementUV: 0 {\n\t\t\tVersion: 101\n\t\t\tName: \"UVMap\"\n\t\t\tMappingInformationType: \"ByPolygonVertex\"\n\t\t\tReferenceInformationType: \"IndexToDirect\"\n");
                array(&mut o, "UV", uvs.iter().map(|v| f(f64::from(*v))).collect::<Vec<_>>().into_iter());
                array(&mut o, "UVIndex", uv_index.iter().map(|v| v.to_string()).collect::<Vec<_>>().into_iter());
                o.push_str("\t\t}\n");
            }
            o.push_str("\t\tLayerElementMaterial: 0 {\n\t\t\tVersion: 101\n\t\t\tName: \"\"\n\t\t\tMappingInformationType: \"AllSame\"\n\t\t\tReferenceInformationType: \"IndexToDirect\"\n");
            array(&mut o, "Materials", std::iter::once("0".to_string()).collect::<Vec<_>>().into_iter());
            o.push_str("\t\t}\n");
            o.push_str("\t\tLayer: 0 {\n\t\t\tVersion: 100\n");
            let mut elements = vec!["LayerElementNormal", "LayerElementMaterial"];
            if m.has_uvs() {
                elements.push("LayerElementUV");
            }
            for e in elements {
                let _ = writeln!(o, "\t\t\tLayerElement:  {{\n\t\t\t\tType: \"{e}\"\n\t\t\t\tTypedIndex: 0\n\t\t\t}}");
            }
            o.push_str("\t\t}\n\t}\n");
            links.push((geo, model));
            let mat = id(3, i);
            let c = it.material.base_color;
            let _ =
                writeln!(o, "\tMaterial: {mat}, \"Material::{name} material\", \"\" {{\n\t\tVersion: 102\n\t\tShadingModel: \"phong\"\n\t\tProperties70:  {{");
            p70(&mut o, "DiffuseColor", "Color", "", "A", &[f64::from(c[0]), f64::from(c[1]), f64::from(c[2])]);
            p70(&mut o, "Opacity", "double", "Number", "", &[f64::from(c[3])]);
            o.push_str("\t\t}\n\t}\n");
            links.push((mat, model));
        }
        let kind = if mesh.is_some() { "Mesh" } else { "Null" };
        let _ = writeln!(o, "\tModel: {model}, \"Model::{name}\", \"{kind}\" {{\n\t\tVersion: 232\n\t\tProperties70:  {{");
        let t = it.transform;
        let (x, y, z) = t.rotation.to_euler(EulerRot::ZYX);
        p70(&mut o, "RotationOrder", "enum", "", "", &[0.0]);
        p70(&mut o, "Lcl Translation", "Lcl Translation", "", "A", &[t.translation.x, t.translation.y, t.translation.z].map(f64::from));
        p70(&mut o, "Lcl Rotation", "Lcl Rotation", "", "A", &[z.to_degrees(), y.to_degrees(), x.to_degrees()].map(f64::from));
        p70(&mut o, "Lcl Scaling", "Lcl Scaling", "", "A", &[t.scale.x, t.scale.y, t.scale.z].map(f64::from));
        o.push_str("\t\t}\n\t\tShading: T\n\t\tCulling: \"CullingOff\"\n\t}\n");
        let parent = it.parent.filter(|p| *p < i).map_or(0, |p| id(1, p));
        links.push((model, parent));
    }
    o.push_str("}\nConnections:  {\n");
    for (a, b) in links {
        let _ = writeln!(o, "\tC: \"OO\",{a},{b}");
    }
    o.push_str("}\n");
    Ok(o.into_bytes())
}
