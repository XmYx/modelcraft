use proptest::prelude::*;

use super::*;

fn none(_: &str) -> Option<Vec<u8>> {
    None
}

/// A two-joint arm: a skinned strip of two triangles with one morph target and a clip that
/// moves, turns and scales the joints and animates the target's weight.
fn arm() -> Document {
    let nodes = vec![
        Node { name: "Root".into(), translation: [0.0, 1.0, 0.0], ..Node::default() },
        Node { name: "Elbow".into(), parent: Some(0), translation: [0.0, 1.0, 0.0], rotation: [0.0, 0.0, 0.38268343, 0.9238795], ..Node::default() },
        Node { name: "Arm".into(), mesh: Some(0), skin: Some(0), ..Node::default() },
        Node { name: "Prop".into(), parent: Some(1), scale: [2.0, 2.0, 2.0], mesh: Some(1), ..Node::default() },
    ];
    let positions = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [1.0, 2.0, 0.0]];
    let mesh = Mesh {
        name: "Arm".into(),
        normals: vec![[0.0, 0.0, 1.0]; 4],
        uvs: vec![[0.0, 1.0], [1.0, 1.0], [0.0, 0.0], [1.0, 0.0]],
        indices: vec![0, 1, 2, 2, 1, 3],
        joints: vec![[0, 0, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0], [1, 0, 0, 0]],
        weights: vec![[1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [0.75, 0.25, 0.0, 0.0], [0.5, 0.5, 0.0, 0.0]],
        targets: vec![Target { name: "Bulge".into(), deltas: vec![[0.0; 3], [0.5, 0.0, 0.0], [0.0; 3], [0.5, 0.0, 0.25]] }],
        target_weights: vec![0.25],
        material: Some(0),
        positions,
    };
    let prop = Mesh { name: "Prop".into(), positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], indices: vec![0, 1, 2], ..Mesh::default() };
    let mut bind = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    bind[13] = -1.0;
    let mut bind2 = bind;
    bind2[13] = -2.0;
    let clip = Animation {
        name: "Wave".into(),
        channels: vec![
            Channel {
                node: 1,
                path: Path::Rotation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 0.5, 1.0],
                values: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.70710677, 0.70710677, 0.0, 0.0, 0.0, 1.0],
            },
            Channel { node: 0, path: Path::Translation, interpolation: Interpolation::Step, times: vec![0.0, 1.0], values: vec![0.0, 1.0, 0.0, 0.0, 2.0, 0.0] },
            Channel { node: 3, path: Path::Scale, interpolation: Interpolation::CubicSpline, times: vec![0.0, 1.0], values: vec![0.0; 18] },
            Channel { node: 2, path: Path::Weights, interpolation: Interpolation::Linear, times: vec![0.0, 1.0], values: vec![0.0, 1.0] },
        ],
    };
    Document {
        nodes,
        meshes: vec![mesh, prop],
        skins: vec![Skin { name: "Arm skin".into(), joints: vec![0, 1], inverse_bind: vec![bind, bind2] }],
        animations: vec![clip],
        materials: vec![Material { name: "Skin".into(), base_color: [0.8, 0.6, 0.5, 1.0], metallic: 0.0, roughness: 0.5 }],
    }
}

#[test]
fn a_rigged_animated_document_round_trips_exactly() {
    let doc = arm();
    let glb = write_glb(&doc).unwrap();
    assert_eq!(&glb[..4], b"glTF");
    assert_eq!(u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize, glb.len());
    assert!(has_rig_content(&glb));
    let back = read(&glb, &mut none, &Limits::default()).unwrap();
    assert_eq!(back, doc);
    assert_eq!(back.animations[0].duration(), 1.0);
    assert_eq!(back.animations[0].channels[2].key(1), Some(&[0.0, 0.0, 0.0][..]));
    assert_eq!(back.animations[0].channels[0].width(), 4);
    // The JSON says what the specification asks for: bounds on positions and key times.
    let json: serde_json::Value = serde_json::from_slice(&glb[20..20 + u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize]).unwrap();
    assert_eq!(json["accessors"][0]["min"], serde_json::json!([0.0, 0.0, 0.0]));
    assert_eq!(json["accessors"][0]["max"], serde_json::json!([1.0, 2.0, 0.0]));
    assert_eq!(json["meshes"][0]["extras"]["targetNames"][0], "Bulge");
    assert_eq!(json["skins"][0]["joints"], serde_json::json!([0, 1]));
    assert_eq!(json["animations"][0]["channels"][3]["target"]["path"], "weights");
    assert_eq!(json["scenes"][0]["nodes"], serde_json::json!([0, 2]));
    // A static document has no rig content and no empty arrays.
    let plain = Document { nodes: vec![Node { mesh: Some(0), ..Node::default() }], meshes: vec![doc.meshes[1].clone()], ..Document::default() };
    let glb = write_glb(&plain).unwrap();
    assert!(!has_rig_content(&glb));
    assert_eq!(read(&glb, &mut none, &Limits::default()).unwrap(), plain);
    assert_eq!(read(&write_glb(&Document::default()).unwrap(), &mut none, &Limits::default()).unwrap(), Document::default());
}

#[test]
fn gltf_json_with_embedded_and_external_buffers_matrices_and_packed_types() {
    // One triangle; positions as floats in a data URI, joints as bytes and weights as
    // normalized bytes with a stride in an external file; the node placed by a matrix.
    let mut pos = Vec::new();
    for v in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
        for f in v {
            pos.extend_from_slice(&f.to_le_bytes());
        }
    }
    let b64 = {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in pos.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                s.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
            }
        }
        s
    };
    // Per vertex: 4 joint bytes, 4 weight bytes, 4 bytes of padding (stride 12).
    let mut skin = Vec::new();
    for _ in 0..3 {
        skin.extend_from_slice(&[1, 0, 0, 0, 255, 0, 0, 0, 9, 9, 9, 9]);
    }
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},
        "buffers":[{{"byteLength":36,"uri":"data:application/octet-stream;base64,{b64}"}},{{"byteLength":36,"uri":"skin.bin"}}],
        "bufferViews":[{{"buffer":0,"byteLength":36}},{{"buffer":1,"byteLength":36,"byteStride":12}}],
        "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},
                     {{"bufferView":1,"componentType":5121,"count":3,"type":"VEC4"}},
                     {{"bufferView":1,"byteOffset":4,"componentType":5121,"normalized":true,"count":3,"type":"VEC4"}}],
        "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"JOINTS_0":1,"WEIGHTS_0":2}}}}]}}],
        "skins":[{{"joints":[1,2]}}],
        "nodes":[{{"mesh":0,"skin":0}},{{"name":"A","children":[2],"matrix":[0,2,0,0, -2,0,0,0, 0,0,2,0, 5,6,7,1]}},{{"name":"B"}}]}}"#
    );
    let mut asked = Vec::new();
    let doc = read(
        json.as_bytes(),
        &mut |name: &str| {
            asked.push(name.to_string());
            (name == "skin.bin").then(|| skin.clone())
        },
        &Limits::default(),
    )
    .unwrap();
    assert_eq!(asked, vec!["skin.bin"]);
    let m = &doc.meshes[0];
    assert_eq!(m.positions, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    assert_eq!(m.indices, vec![0, 1, 2], "no index list: the vertices in order");
    assert_eq!((m.joints[2], m.weights[2]), ([1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]));
    let a = &doc.nodes[1];
    assert_eq!((a.translation, a.scale), ([5.0, 6.0, 7.0], [2.0, 2.0, 2.0]));
    let half = std::f32::consts::FRAC_1_SQRT_2;
    assert!(a.rotation.iter().zip([0.0, 0.0, half, half]).all(|(x, y)| (x - y).abs() < 1e-6), "a quarter turn about Z: {:?}", a.rotation);
    assert_eq!(doc.nodes[2].parent, Some(1));
    assert!(doc.skins[0].inverse_bind.is_empty());
    // Without the external file the read fails with a message naming it.
    let e = read(json.as_bytes(), &mut none, &Limits::default()).unwrap_err();
    assert!(e.to_string().contains("skin.bin"), "{e}");
}

#[test]
fn hostile_files_are_refused_or_trimmed_never_trusted() {
    let limits = Limits::default();
    let read_json = |json: &str| read(json.as_bytes(), &mut none, &limits);
    for bad in [
        "",
        "{}",
        r#"{"asset":{"version":"1.0"}}"#,
        r#"{"asset":{"version":"2.0"},"extensionsRequired":["KHR_draco_mesh_compression"]}"#,
        // A node that is its own child, a child with two parents, a child that is not there.
        r#"{"asset":{"version":"2.0"},"nodes":[{"children":[0]}]}"#,
        r#"{"asset":{"version":"2.0"},"nodes":[{"children":[2]},{"children":[2]},{}]}"#,
        r#"{"asset":{"version":"2.0"},"nodes":[{"children":[7]}]}"#,
        r#"{"asset":{"version":"2.0"},"nodes":[{"children":[1]},{"children":[0]}]}"#,
        r#"{"asset":{"version":"2.0"},"nodes":[{"mesh":3}]}"#,
        // Accessors that reach outside their buffer, or whose offsets overflow.
        r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4,"uri":"data:,AAAAAA=="}],"bufferViews":[{"buffer":0,"byteLength":4}],
            "accessors":[{"bufferView":0,"componentType":5126,"count":9,"type":"VEC3"}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}]}"#,
        r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4,"uri":"data:,AAAAAA=="}],"bufferViews":[{"buffer":0,"byteOffset":18446744073709551615,"byteLength":4}],
            "accessors":[{"bufferView":0,"byteOffset":18446744073709551615,"componentType":5126,"count":1,"type":"VEC3"}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}]}"#,
        r#"{"asset":{"version":"2.0"},"accessors":[{"componentType":5126,"count":99999999999,"type":"VEC3"}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}]}"#,
        r#"{"asset":{"version":"2.0"},"accessors":[{"componentType":5126,"count":3,"type":"VEC3","sparse":{}}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}]}"#,
        r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":9999,"uri":"data:,AAAA"}]}"#,
        r#"{"asset":{"version":"2.0"},"buffers":[{"byteLength":4}]}"#,
        // A triangle naming a vertex that is not there; a skin without joints.
        r#"{"asset":{"version":"2.0"},"accessors":[{"componentType":5126,"count":3,"type":"VEC3"},{"componentType":5125,"count":3,"type":"SCALAR"}],
            "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":0}]}]}"#,
        r#"{"asset":{"version":"2.0"},"skins":[{"joints":[]}]}"#,
    ] {
        assert!(read_json(bad).is_err(), "should be refused: {bad}");
        assert!(!bad.is_empty() || !has_rig_content(bad.as_bytes()));
    }
    // An accessor without data is zeros; an index list of zeros with three vertices is fine.
    let zeros = read_json(
        r#"{"asset":{"version":"2.0"},"accessors":[{"componentType":5126,"count":3,"type":"VEC3"}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}]}"#,
    )
    .unwrap();
    assert_eq!(zeros.meshes[0].positions, vec![[0.0; 3]; 3]);
    // Channels that do not fit are dropped, the rest is kept; unknown paths are ignored.
    let doc = read_json(
        r#"{"asset":{"version":"2.0"},"nodes":[{}],"accessors":[{"componentType":5126,"count":2,"type":"SCALAR"},{"componentType":5126,"count":2,"type":"VEC3"},{"componentType":5126,"count":3,"type":"VEC3"}],
        "animations":[{"samplers":[{"input":0,"output":1},{"input":0,"output":2}],
          "channels":[{"sampler":0,"target":{"node":0,"path":"translation"}},{"sampler":1,"target":{"node":0,"path":"scale"}},{"sampler":0,"target":{"node":5,"path":"scale"}},
                      {"sampler":0,"target":{"node":0,"path":"pointer"}},{"sampler":9,"target":{"node":0,"path":"scale"}},{"sampler":0,"target":{"node":0,"path":"weights"}}]}]}"#,
    )
    .unwrap();
    assert!(doc.animations[0].channels.is_empty(), "key times of 0, 0 do not increase; the others do not fit");
    // Limits.
    let small = Limits { max_items: 1, ..Limits::default() };
    assert!(matches!(read(r#"{"asset":{"version":"2.0"},"nodes":[{},{}]}"#.as_bytes(), &mut none, &small), Err(Error::TooLarge(_))));
    let glb = write_glb(&arm()).unwrap();
    assert!(matches!(read(&glb, &mut none, &Limits { max_buffer_bytes: 16, ..Limits::default() }), Err(Error::TooLarge(_))));
    assert!(matches!(read(&glb, &mut none, &Limits { max_elements: 2, ..Limits::default() }), Err(Error::TooLarge(_))));
    // Truncated GLBs at every length fail cleanly.
    for cut in 0..glb.len() {
        let _ = read(&glb[..cut], &mut none, &limits);
    }
    // A writer refuses what it cannot write truthfully.
    let mut broken = arm();
    broken.meshes[0].positions[0][0] = f32::NAN;
    assert!(write_glb(&broken).is_err());
    let mut broken = arm();
    broken.meshes[0].joints[0][0] = 9;
    assert!(write_glb(&broken).is_err(), "a joint the skin does not have");
    let mut broken = arm();
    broken.animations[0].channels[0].times = vec![0.0, 0.0, 1.0];
    assert!(write_glb(&broken).is_err());
    let mut broken = arm();
    broken.nodes[0].parent = Some(1);
    assert!(write_glb(&broken).is_err(), "a cycle");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Flipping bytes anywhere in a valid file never panics, and whatever still reads is a
    /// document the writer accepts again.
    #[test]
    fn corrupted_files_never_panic(edits in proptest::collection::vec((0usize..4096, any::<u8>()), 1..12)) {
        let mut glb = write_glb(&arm()).unwrap();
        for (at, byte) in edits {
            let i = at % glb.len();
            glb[i] = byte;
        }
        if let Ok(doc) = read(&glb, &mut none, &Limits::default()) {
            prop_assert!(doc.validate(&Limits::default()).is_ok());
            // NaNs are scrubbed on the way in, so writing cannot fail on them.
            prop_assert!(write_glb(&doc).is_ok());
        }
    }

    #[test]
    fn arbitrary_json_never_panics(text in "\\{\"asset\":\\{\"version\":\"2.0\"\\},\"(nodes|meshes|skins|animations|accessors|buffers)\":\\[[\\[\\]{}\":,a-z0-9 ]{0,60}") {
        let _ = read(text.as_bytes(), &mut none, &Limits::default());
        let _ = has_rig_content(text.as_bytes());
    }
}
