//! A 3MF writer: the package of the 3D Manufacturing Format (an Open Packaging Conventions ZIP
//! with `[Content_Types].xml`, `_rels/.rels` and `3D/3dmodel.model`), one `object` per mesh with
//! its triangles, a base material per object (`displaycolor`) and a `build` item per object
//! whose `transform` is the object's world matrix.
//!
//! Written from the public 3MF Core Specification (the `model`, `object`, `mesh`, `vertices`,
//! `triangles`, `basematerials`, `build` and `item` elements, the twelve-number row-vector
//! transform and the `unit` attribute). It is checked by reading its output back through
//! craft-mesh's 3MF reader. 3MF is Z up; the exporter turns the scene's Y up into it unless told
//! otherwise ([`crate::exchange::Convert`]).

use std::fmt::Write as _;

use glam::Mat4;

use crate::exchange::Item;
use crate::{FormatError, Result};

/// Most triangles one file may hold (the writer refuses more rather than build a huge string).
pub const MAX_TRIANGLES: usize = 20_000_000;

const NS: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";

/// The `unit` value for a scale in metres per unit, when the specification has one.
pub fn unit_name(metres_per_unit: f32) -> Option<&'static str> {
    [("micron", 1e-6f32), ("millimeter", 0.001), ("centimeter", 0.01), ("inch", 0.0254), ("foot", 0.3048), ("meter", 1.0)]
        .into_iter()
        .find(|(_, m)| (metres_per_unit - m).abs() <= m * 1e-4)
        .map(|(n, _)| n)
}

fn xml(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(200)
        .flat_map(|c| match c {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            '"' => "&quot;".chars().collect(),
            c => vec![c],
        })
        .collect()
}

fn num(v: f32) -> String {
    if v.is_finite() { format!("{v}") } else { "0".into() }
}

fn world(items: &[Item], mut i: usize) -> Mat4 {
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
}

/// The colour as `#RRGGBBAA`. 3MF colours are sRGB; the material's are linear.
fn hex(c: [f32; 4]) -> String {
    let encode = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
    };
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}{:02X}", b(encode(c[0])), b(encode(c[1])), b(encode(c[2])), b(c[3]))
}

/// The items as a 3MF package whose coordinates are already in the file's axes and unit
/// (`unit` is a name from [`unit_name`]).
pub fn write_in(items: &[Item], unit: &str) -> Result<Vec<u8>> {
    let mut model = String::new();
    let _ = write!(model, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"{unit}\" xml:lang=\"en-US\" xmlns=\"{NS}\">\n");
    model.push_str(" <metadata name=\"Application\">ModelCraft</metadata>\n <resources>\n");
    let mut builds = String::new();
    let mut triangles_total = 0usize;
    let mut next = 1usize;
    for (i, it) in items.iter().enumerate() {
        let Some(mesh) = it.mesh.as_ref().filter(|m| m.face_count() > 0) else { continue };
        let (tris, _) = mesh.triangulate();
        triangles_total += tris.len();
        if triangles_total > MAX_TRIANGLES {
            return Err(FormatError::Unsupported(format!("more than {MAX_TRIANGLES} triangles for one 3MF file")));
        }
        let (mat_id, obj_id) = (next, next + 1);
        next += 2;
        let _ = writeln!(
            model,
            "  <basematerials id=\"{mat_id}\"><base name=\"{}\" displaycolor=\"{}\"/></basematerials>",
            xml(&it.name),
            hex(it.material.base_color)
        );
        let _ =
            writeln!(model, "  <object id=\"{obj_id}\" type=\"model\" name=\"{}\" pid=\"{mat_id}\" pindex=\"0\">\n   <mesh>\n    <vertices>", xml(&it.name));
        for p in mesh.positions() {
            let _ = writeln!(model, "     <vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", num(p.x), num(p.y), num(p.z));
        }
        model.push_str("    </vertices>\n    <triangles>\n");
        for t in &tris {
            let _ = writeln!(model, "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>", t[0], t[1], t[2]);
        }
        model.push_str("    </triangles>\n   </mesh>\n  </object>\n");
        let c = world(items, i).to_cols_array_2d();
        let t: Vec<String> = c.iter().flat_map(|col| col.iter().take(3)).map(|v| num(*v)).collect();
        let _ = writeln!(builds, "  <item objectid=\"{obj_id}\" transform=\"{}\"/>", t.join(" "));
    }
    if builds.is_empty() {
        return Err(FormatError::Unsupported("nothing to write: no object has a mesh".into()));
    }
    let _ = write!(model, " </resources>\n <build>\n{builds} </build>\n</model>\n");
    let mut w = craft_zip::Writer::new();
    let zip = |e: craft_zip::Error| FormatError::Malformed(e.to_string());
    w.add(
        "[Content_Types].xml",
        b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/></Types>",
        craft_zip::Compression::Deflated,
    )
    .map_err(zip)?;
    w.add(
        "_rels/.rels",
        b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/></Relationships>",
        craft_zip::Compression::Deflated,
    )
    .map_err(zip)?;
    w.add("3D/3dmodel.model", model.as_bytes(), craft_zip::Compression::Deflated).map_err(zip)?;
    w.finish().map_err(zip)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::{Quat, Vec3};
    use modelcraft_ops::primitives::Primitive;
    use modelcraft_scene::{Material, Transform};

    use super::*;

    fn items() -> Vec<Item> {
        let cube = Arc::new(Primitive::Cube { size: Vec3::new(1.0, 2.0, 3.0), segments: 1 }.build().unwrap());
        vec![
            Item {
                name: "Root".into(),
                parent: None,
                transform: Transform { translation: Vec3::new(1.0, 0.0, 0.0), rotation: Quat::from_rotation_z(0.5), scale: Vec3::ONE },
                mesh: None,
                material: Material::default(),
            },
            Item {
                name: "Box <1> & \"2\"".into(),
                parent: Some(0),
                transform: Transform { translation: Vec3::new(0.0, 2.0, 0.0), ..Default::default() },
                mesh: Some(cube),
                material: Material { base_color: [1.0, 0.5, 0.0, 1.0], ..Default::default() },
            },
        ]
    }

    #[test]
    fn units_are_named_only_when_the_specification_has_them() {
        assert_eq!(unit_name(0.001), Some("millimeter"));
        assert_eq!(unit_name(1.0), Some("meter"));
        assert_eq!(unit_name(0.0254), Some("inch"));
        assert_eq!(unit_name(0.5), None);
    }

    #[test]
    fn a_written_file_reads_back_with_its_triangles_placement_and_colour() {
        let bytes = write_in(&items(), "meter").unwrap();
        let limits = craft_mesh::Limits::default();
        let model = craft_mesh::threemf::read_model(&bytes, &limits).unwrap();
        assert_eq!(model.unit.metres(), 1.0);
        let placed = model.scene.instances(&limits).unwrap();
        assert_eq!(placed.len(), 1);
        let (mesh, m) = &placed[0];
        let mesh = &model.scene.meshes[*mesh];
        assert_eq!(mesh.positions.len(), 8);
        let tris: usize = mesh.parts.iter().map(|p| p.faces().count()).sum();
        assert_eq!(tris, 12, "a cube's six quads are twelve triangles");
        assert_eq!(mesh.name, "Box <1> & \"2\"", "names survive the escaping");
        // The world matrix is parent * child: root turned by 0.5 about Z and moved to x = 1.
        let want = items()[0].transform.matrix() * items()[1].transform.matrix();
        let got = glam::DMat4::from_cols_array(m).as_mat4();
        assert!((got - want).abs_diff_eq(Mat4::ZERO, 1e-5), "{got:?} vs {want:?}");
        let colour = model.scene.materials.first().map(|m| m.base_color).unwrap_or_default();
        assert!((colour[0] - 1.0).abs() < 0.01 && (colour[1] - 0.5).abs() < 0.01 && colour[2] < 0.01, "linear colour in, linear colour out: {colour:?}");
    }

    #[test]
    fn nothing_to_write_and_too_much_to_write_are_errors() {
        assert!(write_in(&[], "meter").is_err());
        let group = vec![Item { name: "G".into(), parent: None, transform: Transform::default(), mesh: None, material: Material::default() }];
        assert!(write_in(&group, "meter").is_err());
    }
}

#[cfg(test)]
mod exchange_tests {
    use std::sync::Arc;

    use glam::Vec3;
    use modelcraft_ops::primitives::Primitive;
    use modelcraft_scene::{Material, Transform};

    use crate::exchange::{self, Convert, Item, Kind, Up};

    fn one_cube() -> Vec<Item> {
        let cube = Arc::new(Primitive::Cube { size: Vec3::new(1.0, 2.0, 3.0), segments: 1 }.build().unwrap());
        vec![Item {
            name: "Box".into(),
            parent: None,
            transform: Transform { translation: Vec3::new(0.0, 5.0, 0.0), ..Default::default() },
            mesh: Some(cube),
            material: Material::default(),
        }]
    }

    fn top(items: &[Item]) -> f32 {
        let m = items[0].transform.matrix();
        items[0].mesh.as_ref().unwrap().positions().iter().map(|p| m.transform_point3(*p).y).fold(f32::MIN, f32::max)
    }

    #[test]
    fn exports_are_z_up_millimetres_by_default_settings_and_come_back_the_same_way_up() {
        let items = one_cube();
        // Default settings: Z up (3MF's own), metres.
        let bytes = exchange::export(&items, Kind::ThreeMf).unwrap();
        let (back, _) = exchange::import(&bytes, Kind::ThreeMf, &mut |_| None).unwrap();
        assert!((top(&back) - top(&items)).abs() < 1e-4, "{} vs {}", top(&back), top(&items));
        // The file itself has the height along Z: the cube is 5 up, 1 tall (y size 2): z = 5 + 1.
        let text = {
            let model = craft_mesh::threemf::read_model(&bytes, &craft_mesh::Limits::default()).unwrap();
            let t = model.scene.instances(&craft_mesh::Limits::default()).unwrap()[0].1;
            model.scene.meshes[0].positions.iter().map(|p| t[2] * p[0] + t[6] * p[1] + t[10] * p[2] + t[14]).fold(f64::MIN, f64::max)
        };
        assert!((text - 6.0).abs() < 1e-4, "height along Z in the file: {text}");
        // Millimetres: the unit is named and the numbers are a thousand times larger.
        let mm = Convert { up: Up::Auto, scale: Some(0.001) };
        let bytes = exchange::export_with(&items, Kind::ThreeMf, mm).unwrap();
        let model = craft_mesh::threemf::read_model(&bytes, &craft_mesh::Limits::default()).unwrap();
        assert_eq!(model.unit.metres(), 0.001);
        let (back, _) = exchange::import(&bytes, Kind::ThreeMf, &mut |_| None).unwrap();
        assert!((top(&back) - 6.0).abs() < 1e-3, "back in metres: {}", top(&back));
        // A unit 3MF has no name for is written in metres.
        let odd = Convert { up: Up::Auto, scale: Some(0.5) };
        let bytes = exchange::export_with(&items, Kind::ThreeMf, odd).unwrap();
        let (back, _) = exchange::import(&bytes, Kind::ThreeMf, &mut |_| None).unwrap();
        assert!((top(&back) - 6.0).abs() < 1e-3);
    }
}
