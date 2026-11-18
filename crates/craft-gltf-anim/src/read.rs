//! Reading `.glb` and `.gltf` into a [`Document`].

use serde_json::Value;

use crate::{Animation, Channel, Document, Error, Interpolation, Limits, Material, Mesh, Node, Path, Result, Skin, Target};

fn bad<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Malformed(m.into()))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// The JSON text and the binary chunk of a file (`.glb`), or the whole file as JSON (`.gltf`).
fn split(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>)> {
    if bytes.get(..4) != Some(b"glTF") {
        return Ok((bytes, None));
    }
    if le_u32(bytes, 4) != Some(2) {
        return Err(Error::Unsupported("a GLB container other than version 2".into()));
    }
    let json_len = le_u32(bytes, 12).ok_or_else(|| Error::Malformed("the GLB header is cut short".into()))? as usize;
    if bytes.get(16..20) != Some(b"JSON") {
        return bad("the first GLB chunk is not JSON");
    }
    let json = bytes.get(20..20usize.saturating_add(json_len)).ok_or_else(|| Error::Malformed("the JSON chunk is cut short".into()))?;
    let next = 20 + json_len;
    let bin = match (le_u32(bytes, next), bytes.get(next + 4..next + 8)) {
        (Some(len), Some(b"BIN\0")) => {
            Some(bytes.get(next + 8..(next + 8).saturating_add(len as usize)).ok_or_else(|| Error::Malformed("the binary chunk is cut short".into()))?)
        }
        _ => None,
    };
    Ok((json, bin))
}

/// Whether a glTF file holds skins, animations or morph targets (cheaply: only its JSON is
/// parsed). `false` for anything that is not glTF.
pub fn has_rig_content(bytes: &[u8]) -> bool {
    let Ok((json, _)) = split(bytes) else { return false };
    let Ok(v) = serde_json::from_slice::<Value>(json) else { return false };
    let filled = |key: &str| v.get(key).and_then(Value::as_array).is_some_and(|a| !a.is_empty());
    let targets = v.get("meshes").and_then(Value::as_array).is_some_and(|m| {
        m.iter().any(|m| {
            m.get("primitives")
                .and_then(Value::as_array)
                .is_some_and(|p| p.iter().any(|p| p.get("targets").and_then(Value::as_array).is_some_and(|t| !t.is_empty())))
        })
    });
    filled("skins") || filled("animations") || targets
}

fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn index(v: &Value, key: &str) -> Option<usize> {
    v.get(key).and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok())
}

fn name(v: &Value) -> String {
    v.get("name").and_then(Value::as_str).unwrap_or("").chars().take(256).collect()
}

fn number(v: &Value) -> f32 {
    let f = v.as_f64().unwrap_or(0.0) as f32;
    if f.is_finite() { f } else { 0.0 }
}

fn array<const N: usize>(v: Option<&Value>, default: [f32; N]) -> [f32; N] {
    let mut out = default;
    if let Some(a) = v.and_then(Value::as_array).filter(|a| a.len() == N) {
        for (slot, x) in out.iter_mut().zip(a) {
            *slot = number(x);
        }
    }
    out
}

struct Source<'a> {
    json: &'a Value,
    buffers: Vec<std::borrow::Cow<'a, [u8]>>,
    limits: &'a Limits,
}

/// Where an accessor's elements are.
struct Accessor<'a> {
    bytes: &'a [u8],
    stride: usize,
    count: usize,
    component: u64,
    width: usize,
    normalized: bool,
}

impl<'a> Source<'a> {
    fn list(&self, key: &str) -> &'a [Value] {
        self.json.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
    }

    fn accessor(&self, i: usize) -> Result<Accessor<'_>> {
        let a = self.list("accessors").get(i).ok_or_else(|| Error::Malformed(format!("no accessor {i}")))?;
        if a.get("sparse").is_some() {
            return Err(Error::Unsupported("sparse accessors".into()));
        }
        let count = index(a, "count").unwrap_or(0);
        if count > self.limits.max_elements {
            return Err(Error::TooLarge(format!("an accessor with {count} elements")));
        }
        let component = a.get("componentType").and_then(Value::as_u64).unwrap_or(0);
        let size = match component {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            5125 | 5126 => 4,
            other => return bad(format!("component type {other}")),
        };
        let width = match a.get("type").and_then(Value::as_str).unwrap_or("") {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            "MAT4" => 16,
            other => return Err(Error::Unsupported(format!("accessor type {other:?}"))),
        };
        let element = size * width;
        let normalized = a.get("normalized").and_then(Value::as_bool).unwrap_or(false);
        let Some(view) = index(a, "bufferView") else {
            // No data: the specification says zeros.
            return Ok(Accessor { bytes: &[], stride: element, count, component, width, normalized });
        };
        let v = self.list("bufferViews").get(view).ok_or_else(|| Error::Malformed(format!("no buffer view {view}")))?;
        let buffer = self
            .buffers
            .get(index(v, "buffer").unwrap_or(usize::MAX))
            .ok_or_else(|| Error::Malformed("a buffer view names a buffer that is not there".into()))?;
        let stride = index(v, "byteStride").filter(|s| *s != 0).unwrap_or(element);
        let start = index(v, "byteOffset").unwrap_or(0).checked_add(index(a, "byteOffset").unwrap_or(0));
        let view_end = index(v, "byteOffset").unwrap_or(0).checked_add(index(v, "byteLength").unwrap_or(0));
        let need = if count == 0 { Some(0) } else { stride.checked_mul(count - 1).and_then(|n| n.checked_add(element)) };
        let (Some(start), Some(view_end), Some(need)) = (start, view_end, need) else { return bad("an accessor's offsets overflow") };
        let end = start.checked_add(need).filter(|e| *e <= view_end && *e <= buffer.len() && stride >= element);
        let Some(end) = end else { return bad(format!("accessor {i} reaches outside its buffer")) };
        Ok(Accessor { bytes: buffer.get(start..end).unwrap_or(&[]), stride, count, component, width, normalized })
    }

    /// The accessor's values as floats, `width` per element (integers are scaled when the
    /// accessor says they are normalized). Values that are not finite become 0.
    fn floats(&self, i: usize, width: usize) -> Result<Vec<f32>> {
        let a = self.accessor(i)?;
        if a.width != width {
            return bad(format!("accessor {i} has {} values per element, not {width}", a.width));
        }
        let mut out = Vec::with_capacity(a.count * width);
        if a.bytes.is_empty() {
            out.resize(a.count * width, 0.0);
            return Ok(out);
        }
        let size = a.stride.min(a.bytes.len());
        for e in 0..a.count {
            let at = e * a.stride;
            let Some(element) = a.bytes.get(at..at + size.min(a.bytes.len() - at)) else { break };
            for c in 0..width {
                let v = match a.component {
                    5126 => element.get(c * 4..c * 4 + 4).and_then(|b| b.try_into().ok()).map(f32::from_le_bytes),
                    5125 => element.get(c * 4..c * 4 + 4).and_then(|b| b.try_into().ok()).map(|b| u32::from_le_bytes(b) as f32),
                    5123 => element.get(c * 2..c * 2 + 2).and_then(|b| b.try_into().ok()).map(|b| {
                        let x = u16::from_le_bytes(b) as f32;
                        if a.normalized { x / 65535.0 } else { x }
                    }),
                    5122 => element.get(c * 2..c * 2 + 2).and_then(|b| b.try_into().ok()).map(|b| {
                        let x = i16::from_le_bytes(b) as f32;
                        if a.normalized { (x / 32767.0).max(-1.0) } else { x }
                    }),
                    5121 => element.get(c).map(|b| if a.normalized { *b as f32 / 255.0 } else { *b as f32 }),
                    _ => element.get(c).map(|b| {
                        let x = *b as i8 as f32;
                        if a.normalized { (x / 127.0).max(-1.0) } else { x }
                    }),
                };
                out.push(v.filter(|v| v.is_finite()).unwrap_or(0.0));
            }
        }
        out.resize(a.count * width, 0.0);
        Ok(out)
    }

    /// The accessor's values as whole numbers (indices, joints).
    fn uints(&self, i: usize, width: usize) -> Result<Vec<u32>> {
        let a = self.accessor(i)?;
        if a.width != width || a.component == 5126 {
            return bad(format!("accessor {i} does not hold whole numbers, {width} per element"));
        }
        let mut out = Vec::with_capacity(a.count * width);
        for e in 0..a.count {
            let at = e * a.stride;
            for c in 0..width {
                let v = match a.component {
                    5125 => a.bytes.get(at + c * 4..at + c * 4 + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes),
                    5123 | 5122 => a.bytes.get(at + c * 2..at + c * 2 + 2).and_then(|b| b.try_into().ok()).map(|b| u16::from_le_bytes(b) as u32),
                    _ => a.bytes.get(at + c).map(|b| *b as u32),
                };
                out.push(v.unwrap_or(0));
            }
        }
        Ok(out)
    }
}

fn chunks<const N: usize>(flat: Vec<f32>) -> Vec<[f32; N]> {
    flat.as_chunks::<N>().0.to_vec()
}

/// Translation, rotation and scale of a column-major matrix (shear is dropped).
fn decompose(m: [f32; 16]) -> ([f32; 3], [f32; 4], [f32; 3]) {
    let col = |i: usize| [m[i * 4], m[i * 4 + 1], m[i * 4 + 2]];
    let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let (mut x, y, z) = (col(0), col(1), col(2));
    let mut scale = [len(x), len(y), len(z)];
    // A mirrored matrix: put the flip on X so the rest is a rotation.
    let det = x[0] * (y[1] * z[2] - y[2] * z[1]) - x[1] * (y[0] * z[2] - y[2] * z[0]) + x[2] * (y[0] * z[1] - y[1] * z[0]);
    if det < 0.0 {
        scale[0] = -scale[0];
        x = [-x[0], -x[1], -x[2]];
    }
    let unit = |v: [f32; 3], l: f32| if l.abs() > 1e-12 { [v[0] / l.abs(), v[1] / l.abs(), v[2] / l.abs()] } else { [0.0; 3] };
    let (x, y, z) = (unit(x, scale[0]), unit(y, scale[1]), unit(z, scale[2]));
    // Rotation matrix (columns x, y, z) to a quaternion.
    let trace = x[0] + y[1] + z[2];
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(y[2] - z[1]) / s, (z[0] - x[2]) / s, (x[1] - y[0]) / s, 0.25 * s]
    } else if x[0] > y[1] && x[0] > z[2] {
        let s = (1.0 + x[0] - y[1] - z[2]).max(0.0).sqrt() * 2.0;
        [0.25 * s, (y[0] + x[1]) / s, (z[0] + x[2]) / s, (y[2] - z[1]) / s]
    } else if y[1] > z[2] {
        let s = (1.0 + y[1] - x[0] - z[2]).max(0.0).sqrt() * 2.0;
        [(y[0] + x[1]) / s, 0.25 * s, (z[1] + y[2]) / s, (z[0] - x[2]) / s]
    } else {
        let s = (1.0 + z[2] - x[0] - y[1]).max(0.0).sqrt() * 2.0;
        [(z[0] + x[2]) / s, (z[1] + y[2]) / s, 0.25 * s, (x[1] - y[0]) / s]
    };
    ([m[12], m[13], m[14]], unit_quaternion(q), scale.map(|s| if s.is_finite() && s.abs() > 1e-12 { s } else { 1.0 }))
}

fn unit_quaternion(q: [f32; 4]) -> [f32; 4] {
    let l = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    if !l.is_finite() || l <= 1e-6 {
        [0.0, 0.0, 0.0, 1.0]
    } else if (l - 1.0).abs() < 1e-5 {
        // Already a unit quaternion: keep its exact numbers.
        q
    } else {
        q.map(|v| v / l)
    }
}

/// Read a `.glb` or `.gltf`. `resolve` supplies external buffer files by the name the file
/// gives them; it may decline. Extensions the file *requires* are refused; meshes keep their
/// triangle primitives (merged into one list), up to four joints per vertex and their position
/// morph targets.
pub fn read(bytes: &[u8], resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>, limits: &Limits) -> Result<Document> {
    let (json_bytes, bin) = split(bytes)?;
    let json: Value = serde_json::from_slice(json_bytes).map_err(|e| Error::Malformed(e.to_string()))?;
    if json.get("asset").and_then(|a| a.get("version")).and_then(Value::as_str).is_none_or(|v| !v.starts_with("2.")) {
        return bad("it does not say it is glTF 2");
    }
    if let Some(required) = json.get("extensionsRequired").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str) {
        return Err(Error::Unsupported(format!("the required extension {required}")));
    }
    for key in ["nodes", "meshes", "skins", "animations", "materials", "accessors", "bufferViews", "buffers"] {
        let n = json.get(key).and_then(Value::as_array).map_or(0, Vec::len);
        if n > limits.max_items {
            return Err(Error::TooLarge(format!("{n} {key}")));
        }
    }

    let mut buffers: Vec<std::borrow::Cow<'_, [u8]>> = Vec::new();
    let mut total = 0usize;
    for (i, b) in json.get("buffers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]).iter().enumerate() {
        let want = index(b, "byteLength").unwrap_or(0);
        total = total.saturating_add(want);
        if total > limits.max_buffer_bytes {
            return Err(Error::TooLarge(format!("{total} bytes of buffers")));
        }
        let data: std::borrow::Cow<'_, [u8]> = match b.get("uri").and_then(Value::as_str) {
            None => match (i, bin) {
                (0, Some(bin)) => bin.into(),
                _ => return bad(format!("buffer {i} has no data")),
            },
            Some(uri) if uri.starts_with("data:") => {
                let text = uri.split_once(',').map(|(_, t)| t).unwrap_or("");
                if text.len() / 4 * 3 > limits.max_buffer_bytes {
                    return Err(Error::TooLarge("an embedded buffer".into()));
                }
                base64(text).ok_or_else(|| Error::Malformed(format!("buffer {i} is not base64")))?.into()
            }
            Some(uri) => resolve(uri).ok_or_else(|| Error::Malformed(format!("the buffer file {uri:?} is missing")))?.into(),
        };
        if data.len() < want || data.len() > limits.max_buffer_bytes {
            return bad(format!("buffer {i} is shorter than it says"));
        }
        buffers.push(data);
    }
    let src = Source { json: &json, buffers, limits };

    let mut doc = Document::default();
    for m in src.list("materials") {
        let pbr = m.get("pbrMetallicRoughness");
        let factor = |key: &str| pbr.and_then(|p| p.get(key)).map(number).unwrap_or(1.0).clamp(0.0, 1.0);
        doc.materials.push(Material {
            name: name(m),
            base_color: array(pbr.and_then(|p| p.get("baseColorFactor")), [1.0; 4]),
            metallic: factor("metallicFactor"),
            roughness: factor("roughnessFactor"),
        });
    }

    for m in src.list("meshes") {
        let mut mesh = Mesh { name: name(m), ..Mesh::default() };
        let names: Vec<String> = m
            .get("extras")
            .and_then(|e| e.get("targetNames"))
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|n| n.as_str().unwrap_or("").chars().take(256).collect()).collect())
            .unwrap_or_default();
        // Attributes every primitive must have for the merged mesh to keep them.
        let (mut all_normals, mut all_uvs, mut all_skin, mut first) = (true, true, true, true);
        for p in m.get("primitives").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            if p.get("mode").and_then(Value::as_u64).unwrap_or(4) != 4 {
                continue;
            }
            let attr = |key: &str| p.get("attributes").and_then(|a| index(a, key));
            let Some(position) = attr("POSITION") else { continue };
            let positions: Vec<[f32; 3]> = chunks(src.floats(position, 3)?);
            let n = positions.len();
            let base = mesh.positions.len();
            if base + n > limits.max_elements {
                return Err(Error::TooLarge(format!("mesh {}", mesh.name)));
            }
            let indices = match index(p, "indices") {
                Some(i) => src.uints(i, 1)?,
                None => (0..n as u32).collect(),
            };
            if indices.iter().any(|i| *i as usize >= n) {
                return bad(format!("mesh {} has a triangle that names a vertex it does not have", mesh.name));
            }
            mesh.indices.extend(indices.as_chunks::<3>().0.iter().flatten().map(|i| i + base as u32));
            mesh.positions.extend(positions);
            match attr("NORMAL").map(|a| src.floats(a, 3)).transpose()?.map(chunks::<3>).filter(|v| v.len() == n) {
                Some(v) if all_normals => mesh.normals.extend(v),
                _ => all_normals = false,
            }
            match attr("TEXCOORD_0").map(|a| src.floats(a, 2)).transpose()?.map(chunks::<2>).filter(|v| v.len() == n) {
                Some(v) if all_uvs => mesh.uvs.extend(v),
                _ => all_uvs = false,
            }
            let joints = attr("JOINTS_0").map(|a| src.uints(a, 4)).transpose()?.filter(|v| v.len() == n * 4);
            let weights = attr("WEIGHTS_0").map(|a| src.floats(a, 4)).transpose()?.filter(|v| v.len() == n * 4);
            match (joints, weights) {
                (Some(j), Some(w)) if all_skin => {
                    mesh.joints.extend(j.as_chunks::<4>().0.iter().map(|c| c.map(|v| v.min(65535) as u16)));
                    mesh.weights.extend(chunks::<4>(w));
                }
                _ => all_skin = false,
            }
            // Morph targets: every primitive must bring the same number.
            let targets = p.get("targets").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
            if first {
                mesh.targets = (0..targets.len())
                    .map(|i| Target { name: names.get(i).cloned().unwrap_or_else(|| format!("Target {}", i + 1)), deltas: vec![[0.0; 3]; base] })
                    .collect();
            }
            for (i, t) in mesh.targets.iter_mut().enumerate() {
                let deltas = targets.get(i).and_then(|t| index(t, "POSITION")).map(|a| src.floats(a, 3)).transpose()?.map(chunks::<3>).filter(|v| v.len() == n);
                t.deltas.extend(deltas.unwrap_or_else(|| vec![[0.0; 3]; n]));
            }
            if first {
                mesh.material = index(p, "material").filter(|i| *i < doc.materials.len());
            }
            first = false;
        }
        if !all_normals {
            mesh.normals.clear();
        }
        if !all_uvs {
            mesh.uvs.clear();
        }
        if !all_skin {
            mesh.joints.clear();
            mesh.weights.clear();
        }
        if let Some(w) = m.get("weights").and_then(Value::as_array).filter(|w| w.len() == mesh.targets.len()) {
            mesh.target_weights = w.iter().map(number).collect();
        }
        doc.meshes.push(mesh);
    }

    for s in src.list("skins") {
        let joints: Vec<usize> = s
            .get("joints")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|j| j.as_u64().and_then(|j| usize::try_from(j).ok())).collect())
            .unwrap_or_default();
        let inverse_bind = match index(s, "inverseBindMatrices") {
            Some(a) => chunks::<16>(src.floats(a, 16)?),
            None => Vec::new(),
        };
        doc.skins.push(Skin { name: name(s), joints, inverse_bind });
    }

    let nodes = src.list("nodes");
    for n in nodes {
        let (translation, rotation, scale) = match n.get("matrix") {
            Some(m) => decompose(array(Some(m), [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0])),
            None => (array(n.get("translation"), [0.0; 3]), unit_quaternion(array(n.get("rotation"), [0.0, 0.0, 0.0, 1.0])), array(n.get("scale"), [1.0; 3])),
        };
        doc.nodes.push(Node { name: name(n), parent: None, translation, rotation, scale, mesh: index(n, "mesh"), skin: index(n, "skin") });
    }
    for (i, n) in nodes.iter().enumerate() {
        for c in n.get("children").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            let child = c.as_u64().and_then(|c| usize::try_from(c).ok()).and_then(|c| doc.nodes.get_mut(c));
            match child {
                Some(child) if child.parent.is_none() => child.parent = Some(i),
                _ => return bad(format!("node {i} has a child that is missing or already has a parent")),
            }
        }
    }
    // A skinned mesh without usable joint data is shown unskinned rather than refused.
    for n in &mut doc.nodes {
        if n.skin.is_some() && n.mesh.and_then(|m| doc.meshes.get(m)).is_none_or(|m| m.joints.is_empty()) {
            n.skin = None;
        }
    }

    for a in src.list("animations") {
        let samplers = a.get("samplers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
        let mut clip = Animation { name: name(a), channels: Vec::new() };
        for c in a.get("channels").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            let target = c.get("target");
            let (Some(node), Some(sampler)) =
                (target.and_then(|t| index(t, "node")).filter(|n| *n < doc.nodes.len()), index(c, "sampler").and_then(|s| samplers.get(s)))
            else {
                continue;
            };
            let path = match target.and_then(|t| t.get("path")).and_then(Value::as_str) {
                Some("translation") => Path::Translation,
                Some("rotation") => Path::Rotation,
                Some("scale") => Path::Scale,
                Some("weights") => Path::Weights,
                _ => continue,
            };
            let interpolation = match sampler.get("interpolation").and_then(Value::as_str) {
                Some("STEP") => Interpolation::Step,
                Some("CUBICSPLINE") => Interpolation::CubicSpline,
                _ => Interpolation::Linear,
            };
            let (Some(input), Some(output)) = (index(sampler, "input"), index(sampler, "output")) else { continue };
            // A sampler whose data does not read is skipped like any channel that does not fit.
            let (Ok(times), Ok(values)) = (src.floats(input, 1), src.floats(output, path.width().unwrap_or(1))) else { continue };
            let mut channel = Channel { node, path, interpolation, times, values };
            if path == Path::Rotation && interpolation != Interpolation::CubicSpline {
                for q in channel.values.as_chunks_mut::<4>().0 {
                    *q = unit_quaternion(*q);
                }
            }
            clip.channels.push(channel);
        }
        doc.animations.push(clip);
    }
    // Channels that do not fit (wrong value count, times out of order) are left out, as a viewer
    // would ignore them; everything else must hold.
    let targets: Vec<usize> = doc.nodes.iter().map(|n| n.mesh.and_then(|m| doc.meshes.get(m)).map_or(0, |m| m.targets.len())).collect();
    for clip in &mut doc.animations {
        clip.channels.retain(|c| {
            let per = if c.interpolation == Interpolation::CubicSpline { 3 } else { 1 };
            let width = c.path.width().unwrap_or_else(|| targets.get(c.node).copied().unwrap_or(0));
            width > 0 && c.values.len() == c.times.len() * per * width && c.times.iter().all(|t| *t >= 0.0) && c.times.windows(2).all(|w| w.first() < w.get(1))
        });
    }
    doc.validate(limits)?;
    Ok(doc)
}
