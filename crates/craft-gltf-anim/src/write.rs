//! Writing a [`Document`] as a binary glTF (`.glb`).

use serde_json::{Map, Value, json};

use crate::{Document, Error, Interpolation, Limits, Result};

const FLOAT: u32 = 5126;
const U16: u32 = 5123;
const U32: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

#[derive(Default)]
struct Out {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Out {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len()});
        if let (Some(t), Some(m)) = (target, v.as_object_mut()) {
            m.insert("target".into(), json!(t));
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    /// An accessor of `width` floats per element; `bounds` adds the min and max the
    /// specification asks for on positions and key times.
    fn floats(&mut self, data: &[f32], width: usize, kind: &str, target: Option<u32>, bounds: bool) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, target);
        let mut a = json!({"bufferView": view, "componentType": FLOAT, "count": data.len() / width.max(1), "type": kind});
        if bounds && !data.is_empty() {
            let mut lo = vec![f32::MAX; width];
            let mut hi = vec![f32::MIN; width];
            for chunk in data.chunks(width.max(1)) {
                for ((l, h), v) in lo.iter_mut().zip(hi.iter_mut()).zip(chunk) {
                    *l = l.min(*v);
                    *h = h.max(*v);
                }
            }
            if let Some(m) = a.as_object_mut() {
                m.insert("min".into(), json!(lo));
                m.insert("max".into(), json!(hi));
            }
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn ints(&mut self, bytes: &[u8], count: usize, component: u32, kind: &str, target: Option<u32>) -> usize {
        let view = self.view(bytes, target);
        self.accessors.push(json!({"bufferView": view, "componentType": component, "count": count, "type": kind}));
        self.accessors.len() - 1
    }
}

fn finite(values: &[f32]) -> bool {
    values.iter().all(|v| v.is_finite())
}

/// Write `doc` as a `.glb`. Fails when the document is inconsistent ([`Document::validate`]),
/// holds a number that is not finite, or is too large for a GLB (4 GiB).
pub fn write_glb(doc: &Document) -> Result<Vec<u8>> {
    doc.validate(&Limits { max_buffer_bytes: u32::MAX as usize, ..Limits::default() })?;
    let mut out = Out::default();
    let mut root = Map::new();
    root.insert("asset".into(), json!({"version": "2.0", "generator": "craft-gltf-anim"}));

    // Nodes and the scene's roots.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); doc.nodes.len()];
    let mut roots = Vec::new();
    for (i, n) in doc.nodes.iter().enumerate() {
        match n.parent.and_then(|p| children.get_mut(p)) {
            Some(list) => list.push(i),
            None => roots.push(i),
        }
    }
    let mut nodes = Vec::with_capacity(doc.nodes.len());
    for (n, kids) in doc.nodes.iter().zip(&children) {
        if !finite(&n.translation) || !finite(&n.rotation) || !finite(&n.scale) {
            return Err(Error::Malformed(format!("node {} has a transform that is not finite", n.name)));
        }
        let mut m = Map::new();
        m.insert("name".into(), json!(n.name));
        if n.translation != [0.0; 3] {
            m.insert("translation".into(), json!(n.translation));
        }
        if n.rotation != [0.0, 0.0, 0.0, 1.0] {
            m.insert("rotation".into(), json!(n.rotation));
        }
        if n.scale != [1.0; 3] {
            m.insert("scale".into(), json!(n.scale));
        }
        if !kids.is_empty() {
            m.insert("children".into(), json!(kids));
        }
        if let Some(mesh) = n.mesh {
            m.insert("mesh".into(), json!(mesh));
        }
        if let Some(skin) = n.skin {
            m.insert("skin".into(), json!(skin));
        }
        nodes.push(Value::Object(m));
    }
    root.insert("scene".into(), json!(0));
    root.insert("scenes".into(), json!([{"nodes": roots}]));
    if !nodes.is_empty() {
        root.insert("nodes".into(), Value::Array(nodes));
    }

    let mut meshes = Vec::with_capacity(doc.meshes.len());
    for mesh in &doc.meshes {
        let flat3 = |v: &[[f32; 3]]| v.iter().flatten().copied().collect::<Vec<f32>>();
        let positions = flat3(&mesh.positions);
        if !finite(&positions) {
            return Err(Error::Malformed(format!("mesh {} has a position that is not finite", mesh.name)));
        }
        let mut attributes = Map::new();
        attributes.insert("POSITION".into(), json!(out.floats(&positions, 3, "VEC3", Some(ARRAY_BUFFER), true)));
        if !mesh.normals.is_empty() {
            attributes.insert("NORMAL".into(), json!(out.floats(&flat3(&mesh.normals), 3, "VEC3", Some(ARRAY_BUFFER), false)));
        }
        if !mesh.uvs.is_empty() {
            let uv: Vec<f32> = mesh.uvs.iter().flatten().copied().collect();
            attributes.insert("TEXCOORD_0".into(), json!(out.floats(&uv, 2, "VEC2", Some(ARRAY_BUFFER), false)));
        }
        if !mesh.joints.is_empty() {
            let j: Vec<u8> = mesh.joints.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
            attributes.insert("JOINTS_0".into(), json!(out.ints(&j, mesh.joints.len(), U16, "VEC4", Some(ARRAY_BUFFER))));
            let w: Vec<f32> = mesh.weights.iter().flatten().copied().collect();
            if !finite(&w) {
                return Err(Error::Malformed(format!("mesh {} has a weight that is not finite", mesh.name)));
            }
            attributes.insert("WEIGHTS_0".into(), json!(out.floats(&w, 4, "VEC4", Some(ARRAY_BUFFER), false)));
        }
        let mut primitive = Map::new();
        primitive.insert("attributes".into(), Value::Object(attributes));
        primitive.insert("mode".into(), json!(4));
        if !mesh.indices.is_empty() {
            let bytes: Vec<u8> = mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            primitive.insert("indices".into(), json!(out.ints(&bytes, mesh.indices.len(), U32, "SCALAR", Some(ELEMENT_ARRAY_BUFFER))));
        }
        if let Some(m) = mesh.material {
            primitive.insert("material".into(), json!(m));
        }
        let mut m = Map::new();
        m.insert("name".into(), json!(mesh.name));
        if !mesh.targets.is_empty() {
            let mut targets = Vec::new();
            for t in &mesh.targets {
                let d = flat3(&t.deltas);
                if !finite(&d) {
                    return Err(Error::Malformed(format!("morph target {} has an offset that is not finite", t.name)));
                }
                targets.push(json!({"POSITION": out.floats(&d, 3, "VEC3", Some(ARRAY_BUFFER), true)}));
            }
            primitive.insert("targets".into(), Value::Array(targets));
            let weights = if mesh.target_weights.is_empty() { vec![0.0; mesh.targets.len()] } else { mesh.target_weights.clone() };
            m.insert("weights".into(), json!(weights));
            // The usual place for target names; viewers and other apps read it.
            m.insert("extras".into(), json!({"targetNames": mesh.targets.iter().map(|t| t.name.as_str()).collect::<Vec<_>>()}));
        }
        m.insert("primitives".into(), json!([Value::Object(primitive)]));
        meshes.push(Value::Object(m));
    }
    if !meshes.is_empty() {
        root.insert("meshes".into(), Value::Array(meshes));
    }

    let mut skins = Vec::with_capacity(doc.skins.len());
    for s in &doc.skins {
        let mut m = Map::new();
        m.insert("name".into(), json!(s.name));
        m.insert("joints".into(), json!(s.joints));
        if !s.inverse_bind.is_empty() {
            let flat: Vec<f32> = s.inverse_bind.iter().flatten().copied().collect();
            if !finite(&flat) {
                return Err(Error::Malformed(format!("skin {} has a bind matrix that is not finite", s.name)));
            }
            m.insert("inverseBindMatrices".into(), json!(out.floats(&flat, 16, "MAT4", None, false)));
        }
        skins.push(Value::Object(m));
    }
    if !skins.is_empty() {
        root.insert("skins".into(), Value::Array(skins));
    }

    let mut animations = Vec::new();
    for a in doc.animations.iter().filter(|a| !a.channels.is_empty()) {
        let (mut samplers, mut channels) = (Vec::new(), Vec::new());
        for c in &a.channels {
            if !finite(&c.values) {
                return Err(Error::Malformed(format!("animation {} has a value that is not finite", a.name)));
            }
            let input = out.floats(&c.times, 1, "SCALAR", None, true);
            let (width, kind) = match c.path.width() {
                Some(3) => (3, "VEC3"),
                Some(_) => (4, "VEC4"),
                None => (1, "SCALAR"),
            };
            let output = out.floats(&c.values, width, kind, None, false);
            let interpolation = match c.interpolation {
                Interpolation::Linear => "LINEAR",
                Interpolation::Step => "STEP",
                Interpolation::CubicSpline => "CUBICSPLINE",
            };
            channels.push(json!({"sampler": samplers.len(), "target": {"node": c.node, "path": c.path.id()}}));
            samplers.push(json!({"input": input, "output": output, "interpolation": interpolation}));
        }
        animations.push(json!({"name": a.name, "samplers": samplers, "channels": channels}));
    }
    if !animations.is_empty() {
        root.insert("animations".into(), Value::Array(animations));
    }

    if !doc.materials.is_empty() {
        let list: Vec<Value> = doc
            .materials
            .iter()
            .map(|m| json!({"name": m.name, "pbrMetallicRoughness": {"baseColorFactor": m.base_color, "metallicFactor": m.metallic, "roughnessFactor": m.roughness}}))
            .collect();
        root.insert("materials".into(), Value::Array(list));
    }

    while !out.bin.len().is_multiple_of(4) {
        out.bin.push(0);
    }
    if !out.bin.is_empty() {
        root.insert("accessors".into(), Value::Array(out.accessors));
        root.insert("bufferViews".into(), Value::Array(out.views));
        root.insert("buffers".into(), json!([{"byteLength": out.bin.len()}]));
    }
    let mut json = serde_json::to_vec(&Value::Object(root)).map_err(|e| Error::Malformed(e.to_string()))?;
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let bin_chunk = if out.bin.is_empty() { 0 } else { 8 + out.bin.len() };
    let total = 12 + 8 + json.len() + bin_chunk;
    if total > u32::MAX as usize {
        return Err(Error::TooLarge("more than 4 GiB".into()));
    }
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(&json);
    if !out.bin.is_empty() {
        glb.extend_from_slice(&(out.bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&out.bin);
    }
    Ok(glb)
}
