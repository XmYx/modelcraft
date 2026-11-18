use std::sync::Arc;

use glam::{Quat, Vec3};
use modelcraft_ops::primitives::Primitive;
use modelcraft_scene::{Material, Transform};

use super::*;
use crate::exchange::{self, Convert, Item, Kind, Up};

fn cube_items() -> Vec<Item> {
    let cube = Arc::new(Primitive::Cube { size: Vec3::new(1.0, 2.0, 3.0), segments: 1 }.build().unwrap());
    vec![
        Item {
            name: "Root node".into(),
            parent: None,
            transform: Transform { translation: Vec3::new(1.0, 0.0, 0.0), rotation: Quat::from_rotation_y(0.4), scale: Vec3::ONE },
            mesh: None,
            material: Material::default(),
        },
        Item {
            name: "Box".into(),
            parent: Some(0),
            transform: Transform { translation: Vec3::new(0.0, 2.0, 0.0), scale: Vec3::new(1.0, 2.0, 1.0), ..Default::default() },
            mesh: Some(cube),
            material: Material { base_color: [0.25, 0.5, 0.75, 1.0], metallic: 0.5, roughness: 0.25, ..Default::default() },
        },
    ]
}

fn world_points(items: &[Item], i: usize) -> Vec<Vec3> {
    let mut m = glam::Mat4::IDENTITY;
    let mut c = Some(i);
    while let Some(k) = c {
        m = items[k].transform.matrix() * m;
        c = items[k].parent;
    }
    items[i].mesh.as_ref().unwrap().positions().iter().map(|p| m.transform_point3(*p)).collect()
}

fn sorted(mut v: Vec<Vec3>) -> Vec<[i64; 3]> {
    let mut out: Vec<[i64; 3]> = v.drain(..).map(|p| [(p.x * 1e4).round() as i64, (p.y * 1e4).round() as i64, (p.z * 1e4).round() as i64]).collect();
    out.sort();
    out
}

fn import(text: &str) -> Result<(Vec<Item>, Vec<String>)> {
    exchange::import(text.as_bytes(), Kind::Usda, &mut |_| None)
}

#[test]
fn a_written_scene_reads_back_with_hierarchy_placement_and_material() {
    let items = cube_items();
    let bytes = exchange::export(&items, Kind::Usda).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.starts_with("#usda 1.0"), "{}", &text[..40]);
    let (back, warnings) = exchange::import(&bytes, Kind::Usda, &mut |_| None).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let mesh_item = back.iter().position(|i| i.mesh.is_some()).expect("a mesh came back");
    let orig = world_points(&items, 1);
    let got = world_points(&back, mesh_item);
    assert_eq!(sorted(got), sorted(orig), "the same points in the same places");
    assert_eq!(back[mesh_item].mesh.as_ref().unwrap().face_count(), 6, "quads stay quads");
    let m = &back[mesh_item].material;
    assert!((m.base_color[0] - 0.25).abs() < 1e-4 && (m.base_color[2] - 0.75).abs() < 1e-4, "{m:?}");
    assert!((m.metallic - 0.5).abs() < 1e-4 && (m.roughness - 0.25).abs() < 1e-4);
    assert!(back.iter().any(|i| i.name == "Root_node"), "names are made valid prim names: {:?}", back.iter().map(|i| &i.name).collect::<Vec<_>>());
}

#[test]
fn texture_coordinates_survive_with_the_v_axis_flipped_both_ways() {
    let mut cube = Primitive::Cube { size: Vec3::ONE, segments: 1 }.build().unwrap();
    assert!(cube.has_uvs(), "the cube primitive carries UVs");
    let first = cube.uv(cube.face_half_edges(0).next().unwrap());
    let items = vec![Item {
        name: "C".into(),
        parent: None,
        transform: Transform::default(),
        mesh: Some(Arc::new(std::mem::take(&mut cube))),
        material: Material::default(),
    }];
    let bytes = exchange::export(&items, Kind::Usda).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("primvars:st") && text.contains("faceVarying"));
    let (back, _) = exchange::import(&bytes, Kind::Usda, &mut |_| None).unwrap();
    let m = back.iter().find_map(|i| i.mesh.clone()).unwrap();
    assert!(m.has_uvs());
    let got = m.uv(m.face_half_edges(0).next().unwrap());
    assert!((got - first).length() < 1e-4, "{got:?} vs {first:?}");
    // In the file v is measured from the bottom: the written value is 1 - v.
    let wanted = format!("({}, {})", first.x, 1.0 - first.y);
    assert!(text.contains(&wanted), "{wanted} in the file");
}

const HAND: &str = r#"#usda 1.0
(
    defaultPrim = "World"
    metersPerUnit = 0.01
    upAxis = "Z"
    doc = """A hand-written scene
    with a multi-line comment"""
)

# a comment
def Xform "World" (
    kind = "assembly"
)
{
    double3 xformOp:translate = (10, 0, 0)
    float3 xformOp:rotateXYZ = (0, 0, 90)
    float3 xformOp:scale = (2, 2, 2)
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ", "xformOp:scale"]

    def Mesh "Quad" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        int[] faceVertexCounts = [4]
        int[] faceVertexIndices = [0, 1, 2, 3]
        point3f[] points = [(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)] (
            interpolation = "vertex"
        )
        texCoord2f[] primvars:st = [(0, 0), (1, 0), (1, 1), (0, 1)] (
            interpolation = "vertex"
        )
        rel material:binding = </World/Looks/Red>
        color3f[] primvars:displayColor = [(0.1, 0.2, 0.3)]
        double3 xformOp:translate.timeSamples = { 0: (0, 0, 0), 10: (5, 0, 0) }
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }

    def Scope "Looks"
    {
        def Material "Red"
        {
            token outputs:surface.connect = </World/Looks/Red/Shader.outputs:surface>
            def Shader "Shader"
            {
                uniform token info:id = "UsdPreviewSurface"
                color3f inputs:diffuseColor = (1, 0, 0)
                float inputs:roughness = 0.4
                float inputs:metallic = 1
                token outputs:surface
            }
        }
    }

    def Cube "Unsupported"
    {
        double size = 2
    }

    def "Ref" (
        references = @./other.usda@</Thing>
    )
    {
    }
}
"#;

#[test]
fn a_hand_written_scene_with_the_usual_syntax_is_read() {
    let (items, warnings) = import(HAND).unwrap();
    let quad = items.iter().position(|i| i.name == "Quad").expect("the mesh");
    let m = items[quad].mesh.as_ref().unwrap();
    assert_eq!(m.face_count(), 1);
    assert!(m.has_uvs());
    // Material through the binding; Z up and centimetres are turned into Y up and metres.
    assert!((items[quad].material.base_color[0] - 1.0).abs() < 1e-5 && (items[quad].material.metallic - 1.0).abs() < 1e-5);
    assert!((items[quad].material.roughness - 0.4).abs() < 1e-5);
    // Ops in order: scale 2, turn 90 degrees about Z, move 10 along X; the stage turns Z up
    // into Y up and centimetres into metres. The point (1, 0, 0) -> (2,0,0) -> (0,2,0) -> (10,2,0)
    // -> Y up: (10, 0, -2) -> metres: (0.1, 0, -0.02).
    let p = world_points(&items, quad).into_iter().find(|p| p.z.abs() > 0.01 && (p.x - 0.1).abs() < 1e-3).expect("the image of (1, 0, 0)");
    assert!(p.y.abs() < 1e-4 && (p.z + 0.02).abs() < 1e-4, "{p:?}");
    assert!(warnings.iter().any(|w| w.contains("Unsupported") || w.contains("Cube")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("references")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("first time sample")), "{warnings:?}");
    assert!(items.iter().all(|i| i.name != "Looks"), "the materials folder is not a scene object");
}

#[test]
fn the_axis_and_unit_settings_override_what_the_stage_says() {
    let bytes = HAND.as_bytes();
    let take = |c: Convert| {
        let (items, _) = exchange::import_with(bytes, Kind::Usda, &mut |_| None, c).unwrap();
        let q = items.iter().position(|i| i.name == "Quad").unwrap();
        world_points(&items, q).into_iter().find(|p| p.y > 1.0 || p.z.abs() > 0.01).unwrap()
    };
    let auto = take(Convert::default());
    let yup_m = take(Convert { up: Up::Y, scale: Some(1.0) });
    assert!((yup_m - Vec3::new(10.0, 2.0, 0.0)).length() < 1e-3, "no conversion: {yup_m:?}");
    assert!((auto - Vec3::new(0.1, 0.0, -0.02)).length() < 1e-3, "{auto:?}");
}

#[test]
fn binary_packages_and_other_files_are_named_not_misread() {
    let e = exchange::import(b"PXR-USDC\0\0\0\0", Kind::Usda, &mut |_| None).unwrap_err().to_string();
    assert!(e.contains("usdc"), "{e}");
    let e = exchange::import(b"PK\x03\x04junk", Kind::Usda, &mut |_| None).unwrap_err().to_string();
    assert!(e.contains("usdz"), "{e}");
    assert!(import("hello").is_err());
    assert!(import("").is_err());
}

#[test]
fn malformed_meshes_are_errors_with_the_prim_named() {
    let mesh = |counts: &str, indices: &str, points: &str| {
        format!(
            "#usda 1.0\ndef Mesh \"M\"\n{{\n int[] faceVertexCounts = [{counts}]\n int[] faceVertexIndices = [{indices}]\n point3f[] points = [{points}]\n}}\n"
        )
    };
    let pts = "(0,0,0),(1,0,0),(0,1,0)";
    assert!(import(&mesh("3", "0,1,2", pts)).is_ok());
    let e = import(&mesh("3", "0,1", pts)).unwrap_err().to_string();
    assert!(e.contains("`M`") && e.contains("indices"), "{e}");
    let e = import(&mesh("3", "0,1,9", pts)).unwrap_err().to_string();
    assert!(e.contains("out of range"), "{e}");
    let e = import(&mesh("-3,6", "0,1,2", pts)).unwrap_err().to_string();
    assert!(e.contains("corners") || e.contains("indices"), "{e}");
    // A degenerate face (two corners) is dropped, not an error; NaN and inf points are read as numbers.
    assert!(import(&mesh("2,3", "0,1,0,1,2", pts)).is_ok());
    let r = import(&mesh("3", "0,1,2", "(nan,0,0),(inf,0,0),(-inf,0,0)"));
    assert!(r.is_ok(), "{:?}", r.err());
}

#[test]
fn hostile_text_is_an_error_and_never_a_panic() {
    for text in [
        "#usda 1.0\ndef Xform \"A\" {",
        "#usda 1.0\ndef Xform \"A\" ( {",
        "#usda 1.0\ndef \"A\" { string s = \"unclosed }",
        "#usda 1.0\ndef \"A\" { float[] v = [1, 2, (3, 4)] }",
        "#usda 1.0\ndef \"A\" { float[] v = [(1, 2), (3)] }",
        "#usda 1.0\ndef \"A\" { asset a = @unclosed }",
        "#usda 1.0\n@@@",
        "#usda 1.0\n)))))",
        "#usda 1.0\ndef \"A\" { x = = }",
        "#usda 1.0\ndef \"A\" { float x = 1e999999 }",
        "#usda 1.0\ndef \"A\" { float x = -- }",
        "#usda 1.0\ndef 5 \"A\" {}",
        "#usda 1.0\n(\n upAxis = \n)",
    ] {
        let _ = import(text);
    }
    // Nesting: prims, tuples and arrays are bounded.
    let deep_prims = format!("#usda 1.0\n{}{}", "def \"A\" {\n".repeat(200), "}\n".repeat(200));
    let e = import(&deep_prims).unwrap_err().to_string();
    assert!(e.contains("deeper"), "{e}");
    let deep_value = format!("#usda 1.0\ndef \"A\" {{ x = {}{} }}", "(".repeat(500), ")".repeat(500));
    assert!(import(&deep_value).is_err());
}

#[test]
fn the_prim_and_number_budgets_stop_the_work_early() {
    let many = format!("#usda 1.0\n{}", "def \"A\" {}\n".repeat(MAX_PRIMS + 1));
    let e = import(&many).unwrap_err().to_string();
    assert!(e.contains("more than") && e.contains("prims"), "{e}");
    // The number counter trips before the memory does.
    let mut lx = Lexer::new(b"1 2 3 4");
    lx.numbers = MAX_NUMBERS - 2;
    assert!(lx.next().is_ok());
    assert!(lx.next().is_ok());
    assert!(lx.next().is_err(), "the third number is over the budget");
    // A mesh over the vertex limit is refused before its arrays are used.
    let limits = cm::Limits { max_vertices: 2, ..Default::default() };
    let text =
        "#usda 1.0\ndef Mesh \"M\"\n{\n int[] faceVertexCounts = [3]\n int[] faceVertexIndices = [0,1,2]\n point3f[] points = [(0,0,0),(1,0,0),(0,1,0)]\n}\n";
    let e = read(text.as_bytes(), &limits).unwrap_err().to_string();
    assert!(e.contains("more than 2 points"), "{e}");
}

mod properties {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        /// Random bytes, and random cuts of a real file, are errors or scenes, never panics.
        #[test]
        fn garbage_and_truncations_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..400), cut in 0usize..HAND.len()) {
            let _ = read(&bytes, &cm::Limits::default());
            let mut with_header = b"#usda 1.0\n".to_vec();
            with_header.extend_from_slice(&bytes);
            let _ = read(&with_header, &cm::Limits::default());
            let _ = read(HAND.as_bytes().get(..cut).unwrap_or_default(), &cm::Limits::default());
        }

        /// What the writer says about any transform, the reader gives back.
        #[test]
        fn transforms_round_trip(t in (-100.0f32..100.0, -100.0f32..100.0, -100.0f32..100.0), a in -3.0f32..3.0, s in (0.1f32..5.0, 0.1f32..5.0, 0.1f32..5.0)) {
            let mut items = cube_items();
            items[1].transform = Transform { translation: Vec3::new(t.0, t.1, t.2), rotation: Quat::from_rotation_x(a), scale: Vec3::new(s.0, s.1, s.2) };
            items[0].transform = Transform::default();
            let bytes = exchange::export(&items, Kind::Usda).unwrap();
            let (back, _) = exchange::import(&bytes, Kind::Usda, &mut |_| None).unwrap();
            let i = back.iter().position(|i| i.mesh.is_some()).unwrap();
            let (want, got) = (sorted(world_points(&items, 1)), sorted(world_points(&back, i)));
            prop_assert_eq!(want.len(), got.len());
            for (w, g) in want.iter().zip(&got) {
                for k in 0..3 {
                    prop_assert!((w[k] - g[k]).abs() < 30 + (w[k].abs() / 5000), "{:?} vs {:?}", w, g);
                }
            }
        }
    }
}
