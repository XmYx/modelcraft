//! Rigged and animated glTF: skins, morph targets and animation clips, through
//! `craft-gltf-anim`.
//!
//! `craft-mesh` writes static scenes (the posed mesh). When a scene has skins, blend shapes or
//! animation, [`export_glb`] writes them instead: joints as nodes, each bound mesh in its rest
//! shape with joints and weights (the four strongest per vertex), blend shapes as morph targets,
//! and one clip sampled every frame of the playback range from the *solved* scene, so curves,
//! layers, constraints and IK all end up in it. [`import_into`] reads such a file back into
//! joints, skins, blend shapes and curves.
//!
//! What glTF cannot hold is said in the warnings: lattices and clusters (the mesh is written
//! without them), in-between shapes (the shape at full weight is written), more than four
//! joints per vertex, more than one skin per mesh.

use std::collections::HashMap;
use std::sync::Arc;

use craft_gltf_anim as gl;
use glam::{Mat4, Quat, Vec2, Vec3};
use modelcraft_anim::deform::frame_shares;
use modelcraft_anim::skin::SkinWeights;
use modelcraft_anim::{Curve, Interp, Key};
use modelcraft_mesh::{PolyMesh, Polygon};
use modelcraft_scene::anim::{BASE_LAYER, unwrap_angle};
use modelcraft_scene::point_deform::{Frame, PointDeformer, Target};
use modelcraft_scene::rig::{JOINT_RADIUS, Skin};
use modelcraft_scene::{Evaluator, Material, ObjectId, ObjectKind, Scene, Source, Transform};

use crate::{FormatError, Result};

/// Most frames one exported or imported clip is sampled at.
pub const MAX_CLIP_FRAMES: usize = 100_000;

fn err(e: gl::Error) -> FormatError {
    FormatError::Malformed(e.to_string())
}

/// Whether a scene holds something only the rigged writer keeps: a skin, blend shapes, curves
/// or constraints.
pub fn scene_has_rig(scene: &Scene) -> bool {
    !scene.anim.is_empty()
        || !scene.constraints.list().is_empty()
        || scene.objects().iter().any(|o| matches!(o.kind, ObjectKind::Skin { .. } | ObjectKind::PointDeformer { deformer: PointDeformer::BlendShapes { .. } }))
}

/// Whether a file needs [`import_into`] to keep what it holds.
pub fn file_has_rig(bytes: &[u8]) -> bool {
    gl::has_rig_content(bytes)
}

/// The output vertices of a mesh: one per point, or per point and UV where the mesh has UVs.
struct Split {
    /// The source point of each output vertex.
    point: Vec<u32>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

fn split(m: &PolyMesh) -> Split {
    let mut out = Split { point: Vec::new(), uvs: Vec::new(), indices: Vec::new() };
    let has_uvs = m.has_uvs();
    let mut seen: HashMap<(u32, [u32; 2]), u32> = HashMap::new();
    for f in 0..m.face_count() as u32 {
        let corners: Vec<u32> = m
            .face_half_edges(f)
            .map(|h| {
                let p = m.origin(h);
                let uv = if has_uvs { m.uv(h) } else { Vec2::ZERO };
                *seen.entry((p, [uv.x.to_bits(), uv.y.to_bits()])).or_insert_with(|| {
                    out.point.push(p);
                    out.uvs.push([uv.x, uv.y]);
                    (out.point.len() - 1) as u32
                })
            })
            .collect();
        let Some(first) = corners.first().copied() else { continue };
        for pair in corners.windows(2).skip(1) {
            if let [b, c] = pair {
                out.indices.extend([first, *b, *c]);
            }
        }
    }
    if !has_uvs {
        out.uvs.clear();
    }
    out
}

fn trs(t: Transform) -> ([f32; 3], [f32; 4], [f32; 3]) {
    (t.translation.to_array(), t.rotation.normalize().to_array(), t.scale.to_array())
}

/// Write the scene as a rigged, animated `.glb`. Returns the file and what could not be kept.
pub fn export_glb(scene: &Scene) -> Result<(Vec<u8>, Vec<String>)> {
    let mut warnings = Vec::new();
    // The rest scene: every stack deformer switched off, so meshes evaluate to what the stack
    // starts from (history and parametric deformers included).
    let mut rest = scene.clone();
    let stack: Vec<ObjectId> =
        scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Skin { .. } | ObjectKind::PointDeformer { .. })).map(|o| o.id).collect();
    for id in &stack {
        if let Ok(o) = rest.edit(*id) {
            o.visible = false;
        }
    }
    let mut eval = Evaluator::new();
    let mut posed_eval = Evaluator::new();

    let mut doc = gl::Document::default();
    let mut node_of: HashMap<ObjectId, usize> = HashMap::new();
    // (node, blend-shape object) of meshes whose weights may be animated.
    let mut shaped: Vec<(usize, ObjectId)> = Vec::new();
    // Every node first, so a skin can name joints that come later in the tree.
    let mut exported: Vec<ObjectId> = Vec::new();
    for (id, _) in scene.tree_order() {
        let Some(o) = scene.get(id) else { continue };
        if matches!(o.kind, ObjectKind::Deformer { .. } | ObjectKind::Skin { .. } | ObjectKind::PointDeformer { .. })
            || scene.is_cage_input(id)
            || !scene.visible(id)
        {
            continue;
        }
        let (translation, rotation, scale) = trs(o.transform);
        node_of.insert(id, doc.nodes.len());
        doc.nodes.push(gl::Node {
            name: o.name.clone(),
            parent: o.parent.and_then(|p| node_of.get(&p).copied()),
            translation,
            rotation,
            scale,
            mesh: None,
            skin: None,
        });
        exported.push(id);
    }
    for (index, id) in exported.iter().copied().enumerate() {
        let Some(o) = scene.get(id) else { continue };
        if matches!(o.kind, ObjectKind::Group | ObjectKind::Joint { .. }) {
            continue;
        }
        let mut node_skin = None;
        let skins = scene.skins_of(id);
        let children = || scene.objects().iter().filter(move |c| c.parent == Some(id) && c.visible);
        let shapes = children().find_map(|c| match &c.kind {
            ObjectKind::PointDeformer { deformer: PointDeformer::BlendShapes { targets } } => Some((c.id, targets.clone())),
            _ => None,
        });
        let other = children().any(|c| matches!(c.kind, ObjectKind::PointDeformer { deformer: PointDeformer::Lattice { .. } | PointDeformer::Cluster { .. } }));
        let rigged = !skins.is_empty() || shapes.is_some();
        // A rigged mesh is written in its rest shape; anything else as it is now.
        let mesh = if rigged { eval.evaluate(&rest, id).mesh } else { posed_eval.evaluate(scene, id).mesh };
        if mesh.face_count() == 0 {
            continue;
        }
        if rigged && other {
            warnings.push(format!("{}: lattices and clusters are not part of glTF and were left out", o.name));
        }
        let s = split(&mesh);
        let normals = mesh.vertex_normals();
        let mut out = gl::Mesh {
            name: o.name.clone(),
            positions: s.point.iter().map(|p| mesh.position(*p).to_array()).collect(),
            normals: s.point.iter().map(|p| normals.get(*p as usize).copied().unwrap_or(Vec3::Y).normalize_or(Vec3::Y).to_array()).collect(),
            uvs: s.uvs.clone(),
            indices: s.indices.clone(),
            ..gl::Mesh::default()
        };
        if let Some((_, skin)) = skins.first() {
            if skins.len() > 1 {
                warnings.push(format!("{}: only its first skin was written", o.name));
            }
            let joints: Option<Vec<usize>> = skin.joints.iter().map(|j| node_of.get(j).copied()).collect();
            match joints {
                Some(joints) if !joints.is_empty() && skin.weights.points() == mesh.vertex_count() => {
                    let mut weaker = false;
                    for p in &s.point {
                        let mut all: Vec<(u16, f32)> = skin.weights.of(*p as usize).filter(|(_, w)| *w > 0.0).collect();
                        all.sort_by(|a, b| b.1.total_cmp(&a.1));
                        weaker |= all.len() > 4;
                        all.truncate(4);
                        let total: f32 = all.iter().map(|(_, w)| w).sum();
                        let (mut j, mut w) = ([0u16; 4], [0.0f32; 4]);
                        for (slot, (joint, weight)) in all.iter().enumerate() {
                            if let (Some(js), Some(ws)) = (j.get_mut(slot), w.get_mut(slot)) {
                                *js = *joint;
                                *ws = if total > 0.0 { weight / total } else { 0.0 };
                            }
                        }
                        // A point no joint holds follows the first joint: a viewer needs one.
                        if total <= 0.0 {
                            w[0] = 1.0;
                        }
                        out.joints.push(j);
                        out.weights.push(w);
                    }
                    if weaker {
                        warnings.push(format!("{}: glTF holds four joints per vertex; the weakest were left out", o.name));
                    }
                    doc.skins.push(gl::Skin {
                        name: format!("{} skin", o.name),
                        joints,
                        inverse_bind: skin.inverse_bind.iter().map(Mat4::to_cols_array).collect(),
                    });
                    node_skin = Some(doc.skins.len() - 1);
                }
                _ => warnings.push(format!("{}: its skin names joints that are hidden or gone, or no longer fits; written without it", o.name)),
            }
        }
        if let Some((shapes_id, targets)) = shapes {
            let n = mesh.vertex_count();
            let mut betweens = false;
            for t in &targets {
                // The shape at full weight, in-betweens folded in.
                let mut dense = vec![Vec3::ZERO; n];
                let ats: Vec<f32> = t.frames.iter().map(|f| f.at).collect();
                betweens |= t.frames.len() > 1;
                for (frame, share) in frame_shares(&ats, 1.0) {
                    let Some(f) = t.frames.get(frame) else { continue };
                    for (i, d) in f.indices.iter().zip(f.deltas.iter()) {
                        if let Some(slot) = dense.get_mut(*i as usize) {
                            *slot += *d * share;
                        }
                    }
                }
                out.targets.push(gl::Target {
                    name: t.name.clone(),
                    deltas: s.point.iter().map(|p| dense.get(*p as usize).copied().unwrap_or(Vec3::ZERO).to_array()).collect(),
                });
                out.target_weights.push(t.weight);
            }
            if betweens {
                warnings.push(format!("{}: glTF has no in-between shapes; each shape was written at full weight only", o.name));
            }
            if !targets.is_empty() {
                shaped.push((index, shapes_id));
            }
        }
        let material = gl::Material {
            name: format!("{} material", o.name),
            base_color: o.material.base_color,
            metallic: o.material.metallic,
            roughness: o.material.roughness,
        };
        let same = |m: &gl::Material| m.base_color == material.base_color && m.metallic == material.metallic && m.roughness == material.roughness;
        out.material = Some(match doc.materials.iter().position(same) {
            Some(i) => i,
            None => {
                doc.materials.push(material);
                doc.materials.len() - 1
            }
        });
        doc.meshes.push(out);
        if let Some(node) = doc.nodes.get_mut(index) {
            node.mesh = Some(doc.meshes.len() - 1);
            node.skin = node_skin;
        }
    }

    // One clip: the solved scene sampled at every frame of the playback range.
    let moving = !scene.anim.is_empty() || scene.constraints.list().iter().any(|c| c.enabled);
    let time = scene.anim.time;
    let frames = (time.end - time.start).floor().max(0.0) as usize + 1;
    if moving && frames > 1 && time.fps > 0.0 {
        if frames > MAX_CLIP_FRAMES {
            return Err(FormatError::Malformed(format!("the playback range has {frames} frames; a clip holds at most {MAX_CLIP_FRAMES}")));
        }
        let mut order: Vec<(ObjectId, usize)> = node_of.iter().map(|(id, n)| (*id, *n)).collect();
        order.sort_by_key(|(_, n)| *n);
        let mut sampled = scene.clone();
        let mut tracks: Vec<Vec<Transform>> = vec![Vec::with_capacity(frames); order.len()];
        let mut weights: Vec<Vec<f32>> = vec![Vec::new(); shaped.len()];
        for f in 0..frames {
            sampled.set_frame(time.start + f as f64);
            for (slot, (id, _)) in tracks.iter_mut().zip(&order) {
                slot.push(sampled.get(*id).map(|o| o.transform).unwrap_or_default());
            }
            for (slot, (_, shapes_id)) in weights.iter_mut().zip(&shaped) {
                if let Some(ObjectKind::PointDeformer { deformer: PointDeformer::BlendShapes { targets } }) = sampled.get(*shapes_id).map(|o| &o.kind) {
                    slot.extend(targets.iter().map(|t| t.weight));
                }
            }
        }
        let times: Vec<f32> = (0..frames).map(|f| (f as f64 / time.fps) as f32).collect();
        let mut clip = gl::Animation { name: "Animation".into(), channels: Vec::new() };
        for ((_, node), track) in order.iter().zip(&tracks) {
            let node = *node;
            let Some(first) = track.first() else { continue };
            if track.iter().any(|t| (t.translation - first.translation).length() > 1e-6) {
                let values = track.iter().flat_map(|t| t.translation.to_array()).collect();
                clip.channels.push(gl::Channel { node, path: gl::Path::Translation, interpolation: gl::Interpolation::Linear, times: times.clone(), values });
            }
            if track.iter().any(|t| t.rotation.dot(first.rotation).abs() < 1.0 - 1e-7) {
                // Keep neighbours on the same side so a viewer's interpolation takes the short way.
                let mut last = first.rotation.normalize();
                let mut values = Vec::with_capacity(track.len() * 4);
                for t in track {
                    let mut q = t.rotation.normalize();
                    if q.dot(last) < 0.0 {
                        q = -q;
                    }
                    values.extend(q.to_array());
                    last = q;
                }
                clip.channels.push(gl::Channel { node, path: gl::Path::Rotation, interpolation: gl::Interpolation::Linear, times: times.clone(), values });
            }
            if track.iter().any(|t| (t.scale - first.scale).length() > 1e-6) {
                let values = track.iter().flat_map(|t| t.scale.to_array()).collect();
                clip.channels.push(gl::Channel { node, path: gl::Path::Scale, interpolation: gl::Interpolation::Linear, times: times.clone(), values });
            }
        }
        for ((node, _), values) in shaped.iter().zip(weights) {
            let count = doc.nodes.get(*node).and_then(|n| n.mesh).and_then(|m| doc.meshes.get(m)).map_or(0, |m| m.targets.len());
            let varies = count > 0 && values.len() == frames * count && values.chunks(count).any(|c| Some(c) != values.get(..count));
            if varies {
                clip.channels.push(gl::Channel {
                    node: *node,
                    path: gl::Path::Weights,
                    interpolation: gl::Interpolation::Linear,
                    times: times.clone(),
                    values,
                });
            }
        }
        if !clip.channels.is_empty() {
            doc.animations.push(clip);
        }
    }
    Ok((gl::write_glb(&doc).map_err(err)?, warnings))
}

/// Join vertices that are the same point with the same binding, as glTF splits them at UV
/// seams and hard edges. Returns the points and, per file vertex, its point.
fn weld(m: &gl::Mesh) -> (Vec<Vec3>, Vec<u32>) {
    let mut points = Vec::new();
    let mut map = Vec::with_capacity(m.positions.len());
    let mut seen: HashMap<([u32; 3], [u16; 4], [u32; 4]), u32> = HashMap::new();
    for (i, p) in m.positions.iter().enumerate() {
        let key = (p.map(f32::to_bits), m.joints.get(i).copied().unwrap_or_default(), m.weights.get(i).copied().unwrap_or_default().map(f32::to_bits));
        let at = *seen.entry(key).or_insert_with(|| {
            points.push(Vec3::from_array(*p));
            (points.len() - 1) as u32
        });
        map.push(at);
    }
    (points, map)
}

/// The value of a channel at `time` seconds, by the file's interpolation.
fn sample(c: &gl::Channel, time: f32, out: &mut Vec<f32>) {
    out.clear();
    let n = c.times.len();
    let after = c.times.partition_point(|t| *t <= time);
    let (a, b) = (after.saturating_sub(1), after.min(n.saturating_sub(1)));
    let (Some(ka), Some(kb)) = (c.key(a), c.key(b)) else { return };
    let (ta, tb) = (c.times.get(a).copied().unwrap_or(0.0), c.times.get(b).copied().unwrap_or(0.0));
    let span = tb - ta;
    let t = if a == b || span <= 0.0 { 0.0 } else { ((time - ta) / span).clamp(0.0, 1.0) };
    match c.interpolation {
        gl::Interpolation::Step => out.extend_from_slice(ka),
        gl::Interpolation::Linear if c.path == gl::Path::Rotation && ka.len() == 4 => {
            let q = Quat::from_slice(ka).normalize().slerp(Quat::from_slice(kb).normalize(), t);
            out.extend(q.to_array());
        }
        gl::Interpolation::Linear => out.extend(ka.iter().zip(kb).map(|(x, y)| x + (y - x) * t)),
        gl::Interpolation::CubicSpline => {
            // Hermite between the keys with the file's tangents (value units per second).
            let w = ka.len();
            let out_tangent = c.values.get(a * 3 * w + 2 * w..a * 3 * w + 3 * w);
            let in_tangent = c.values.get(b * 3 * w..b * 3 * w + w);
            let (t2, t3) = (t * t, t * t * t);
            for i in 0..w {
                let (p0, p1) = (ka.get(i).copied().unwrap_or(0.0), kb.get(i).copied().unwrap_or(0.0));
                let m0 = out_tangent.and_then(|v| v.get(i)).copied().unwrap_or(0.0) * span;
                let m1 = in_tangent.and_then(|v| v.get(i)).copied().unwrap_or(0.0) * span;
                out.push((2.0 * t3 - 3.0 * t2 + 1.0) * p0 + (t3 - 2.0 * t2 + t) * m0 + (-2.0 * t3 + 3.0 * t2) * p1 + (t3 - t2) * m1);
            }
            if c.path == gl::Path::Rotation && out.len() == 4 {
                let q = Quat::from_slice(out).normalize();
                out.clear();
                out.extend(q.to_array());
            }
        }
    }
}

fn set_curve(scene: &mut Scene, id: ObjectId, path: &str, keys: Vec<Key>) {
    if keys.is_empty() {
        return;
    }
    if let Ok(curve) = scene.anim.curve_mut_in(id, path, BASE_LAYER) {
        let mut c = Curve::new();
        for k in keys {
            c.insert(k);
        }
        *curve = c;
    }
}

/// Read a rigged glTF into the scene: nodes as groups and joints, meshes with their skins and
/// blend shapes, and clip number `clip` (the first without it) as curves starting at the
/// scene's first frame, at the scene's frame rate. Everything is added under `parent`.
/// Returns the new objects (parents first) and what could not be kept.
pub fn import_into(
    scene: &mut Scene,
    bytes: &[u8],
    resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    parent: Option<ObjectId>,
    clip: Option<usize>,
) -> Result<(Vec<ObjectId>, Vec<String>)> {
    let doc = gl::read(bytes, resolve, &gl::Limits::default()).map_err(err)?;
    let mut warnings = Vec::new();
    let mut created = Vec::new();
    let is_joint: std::collections::HashSet<usize> = doc.skins.iter().flat_map(|s| s.joints.iter().copied()).collect();

    // Parents before children.
    let mut depth_of = vec![0usize; doc.nodes.len()];
    for (i, slot) in depth_of.iter_mut().enumerate() {
        let mut at = doc.nodes.get(i).and_then(|n| n.parent);
        let mut d = 0;
        while let Some(p) = at {
            d += 1;
            if d > doc.nodes.len() {
                break;
            }
            at = doc.nodes.get(p).and_then(|n| n.parent);
        }
        *slot = d;
    }
    let mut order: Vec<usize> = (0..doc.nodes.len()).collect();
    order.sort_by_key(|i| depth_of.get(*i).copied().unwrap_or(0));

    let mut object_of: HashMap<usize, ObjectId> = HashMap::new();
    // (mesh object, node, per-file-vertex point, point count) of meshes whose skin and shapes
    // are bound once every joint exists.
    let mut bound: Vec<(ObjectId, usize, Vec<u32>, usize)> = Vec::new();
    for i in order {
        let Some(n) = doc.nodes.get(i) else { continue };
        let above = n.parent.and_then(|p| object_of.get(&p).copied()).or(parent);
        let transform =
            Transform { translation: Vec3::from_array(n.translation), rotation: Quat::from_array(n.rotation), scale: Vec3::from_array(n.scale) }.sanitized();
        let label = if n.name.trim().is_empty() { format!("Node {i}") } else { n.name.clone() };
        let mesh = n.mesh.and_then(|m| doc.meshes.get(m)).filter(|m| !m.indices.is_empty());
        let joint = is_joint.contains(&i);
        let kind = if joint { ObjectKind::Joint { radius: JOINT_RADIUS } } else { ObjectKind::Group };
        let id = match scene.add(scene.unique_name(&label), kind, above) {
            Ok(id) => id,
            Err(e) => {
                warnings.push(format!("{label}: {e}"));
                continue;
            }
        };
        if let Ok(o) = scene.edit(id) {
            o.transform = transform;
        }
        object_of.insert(i, id);
        created.push(id);
        let Some(m) = mesh else { continue };
        let (points, map) = weld(m);
        let polygons: Vec<Polygon> = m
            .indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let verts: Vec<u32> = t.iter().map(|v| map.get(*v as usize).copied().unwrap_or(0)).collect();
                if m.uvs.is_empty() {
                    Polygon::new(verts)
                } else {
                    Polygon::with_uvs(verts, t.iter().map(|v| m.uvs.get(*v as usize).map_or(Vec2::ZERO, |uv| Vec2::from_array(*uv))).collect())
                }
            })
            .collect();
        let count = points.len();
        let built = match PolyMesh::build(points, polygons) {
            Ok((pm, report)) => {
                if !report.is_clean() {
                    warnings.push(format!("mesh `{}`: {} faces left out, {} turned, {} detached", m.name, report.dropped, report.flipped, report.detached));
                }
                pm
            }
            Err(e) => {
                warnings.push(format!("mesh `{}`: {e}", m.name));
                continue;
            }
        };
        let fits = built.vertex_count() == count;
        let mesh_kind = ObjectKind::Mesh { source: Source::Mesh { mesh: Arc::new(built) }, steps: vec![] };
        // The mesh itself: on the node, or as a child when the node is also a joint.
        let mesh_id = if joint {
            match scene.add(scene.unique_name(&format!("{label} mesh")), mesh_kind, Some(id)) {
                Ok(c) => {
                    created.push(c);
                    c
                }
                Err(e) => {
                    warnings.push(format!("{label}: {e}"));
                    continue;
                }
            }
        } else {
            if let Ok(o) = scene.edit(id) {
                o.kind = mesh_kind;
            }
            id
        };
        if let (Some(mat), Ok(o)) = (m.material.and_then(|x| doc.materials.get(x)), scene.edit(mesh_id)) {
            o.material = Material { base_color: mat.base_color, metallic: mat.metallic, roughness: mat.roughness, ..Material::default() };
        }
        if fits {
            bound.push((mesh_id, i, map, count));
        } else if !m.joints.is_empty() || !m.targets.is_empty() {
            warnings.push(format!("mesh `{}` had to be repaired, so its skin and shapes no longer fit and were left out", m.name));
        }
    }

    // Shapes first, then the skin: the order glTF applies them in.
    let mut shapes_of: HashMap<usize, ObjectId> = HashMap::new();
    for (mesh_id, node, map, count) in &bound {
        let Some(n) = doc.nodes.get(*node) else { continue };
        let Some(m) = n.mesh.and_then(|m| doc.meshes.get(m)) else { continue };
        if !m.targets.is_empty() {
            let targets: Vec<Target> = m
                .targets
                .iter()
                .enumerate()
                .map(|(t, target)| {
                    // One offset per point: the first file vertex of each point speaks for it.
                    let mut dense: Vec<Option<Vec3>> = vec![None; *count];
                    for (v, d) in target.deltas.iter().enumerate() {
                        if let Some(slot) = map.get(v).and_then(|p| dense.get_mut(*p as usize))
                            && slot.is_none()
                        {
                            *slot = Some(Vec3::from_array(*d));
                        }
                    }
                    let (indices, deltas): (Vec<u32>, Vec<Vec3>) =
                        dense.iter().enumerate().filter_map(|(i, d)| d.filter(|d| d.length() > 1e-7).map(|d| (i as u32, d))).unzip();
                    Target {
                        name: target.name.clone(),
                        weight: m.target_weights.get(t).copied().unwrap_or(0.0),
                        frames: vec![Frame { at: 1.0, indices: Arc::new(indices), deltas: Arc::new(deltas) }],
                    }
                })
                .collect();
            let mut deformer = PointDeformer::BlendShapes { targets };
            deformer.sanitize();
            if let Ok(id) = scene.add(scene.unique_name("Blend shapes"), ObjectKind::PointDeformer { deformer }, Some(*mesh_id)) {
                shapes_of.insert(*node, id);
                created.push(id);
            }
        }
        let Some(s) = n.skin.and_then(|s| doc.skins.get(s)) else { continue };
        let joints: Option<Vec<ObjectId>> = s.joints.iter().map(|j| object_of.get(j).copied()).collect();
        let Some(joints) = joints else {
            warnings.push(format!("mesh `{}`: a joint of its skin could not be made; left unskinned", m.name));
            continue;
        };
        let mut weights = SkinWeights::new(*count, 4);
        let mut done = vec![false; *count];
        for (v, (j, w)) in m.joints.iter().zip(&m.weights).enumerate() {
            let Some(p) = map.get(v).map(|p| *p as usize) else { continue };
            if done.get(p).copied().unwrap_or(true) {
                continue;
            }
            let entries: Vec<(u16, f32)> = j.iter().copied().zip(w.iter().copied()).filter(|(_, w)| *w > 0.0).collect();
            weights.set(p, &entries);
            if let Some(d) = done.get_mut(p) {
                *d = true;
            }
        }
        let inverse_bind: Vec<Mat4> =
            if s.inverse_bind.is_empty() { vec![Mat4::IDENTITY; joints.len()] } else { s.inverse_bind.iter().map(Mat4::from_cols_array).collect() };
        let mut skin = Skin { joints, inverse_bind, weights, dual_quaternion: false };
        if !skin.sanitize() {
            warnings.push(format!("mesh `{}`: its skin is not usable; left unskinned", m.name));
            continue;
        }
        if let Ok(id) = scene.add(scene.unique_name("Skin"), ObjectKind::Skin { skin: Arc::new(skin) }, Some(*mesh_id)) {
            created.push(id);
        }
    }

    // The clip, as curves.
    if doc.animations.len() > 1 {
        let names: Vec<String> =
            doc.animations.iter().enumerate().map(|(i, a)| if a.name.is_empty() { format!("{i}") } else { format!("{i} ({})", a.name) }).collect();
        warnings.push(format!("the file holds {} clips: {}; one is read at a time (`clip`)", names.len(), names.join(", ")));
    }
    let chosen = clip.unwrap_or(0);
    if let Some(a) = doc.animations.get(chosen) {
        let fps = if scene.anim.time.fps > 0.0 { scene.anim.time.fps } else { 24.0 };
        let start = scene.anim.time.start;
        let last = (a.duration() as f64 * fps).ceil() as usize;
        if last + 1 > MAX_CLIP_FRAMES {
            warnings.push(format!("the clip is longer than {MAX_CLIP_FRAMES} frames and was left out"));
        } else {
            let mut value = Vec::new();
            for c in &a.channels {
                let exact = c.interpolation != gl::Interpolation::CubicSpline && c.path != gl::Path::Rotation;
                let interp = if c.interpolation == gl::Interpolation::Step { Interp::Stepped } else { Interp::Linear };
                // Key times: the file's own for what a linear or stepped curve reproduces
                // exactly; every frame for rotations (the file turns the short way between
                // keys, our curves hold angles) and for splines.
                let times: Vec<f64> = if exact {
                    c.times.iter().map(|t| *t as f64 * fps).collect()
                } else {
                    let (first, end) = (c.times.first().copied().unwrap_or(0.0) as f64 * fps, c.times.last().copied().unwrap_or(0.0) as f64 * fps);
                    let mut t: Vec<f64> = ((first.ceil() as usize)..=(end.floor() as usize)).map(|f| f as f64).collect();
                    if t.first().is_none_or(|x| *x > first + 1e-6) {
                        t.insert(0, first);
                    }
                    if t.last().is_some_and(|x| *x < end - 1e-6) {
                        t.push(end);
                    }
                    t
                };
                let (target, paths): (Option<ObjectId>, Vec<String>) = match c.path {
                    gl::Path::Translation => (object_of.get(&c.node).copied(), ["x", "y", "z"].iter().map(|a| format!("translate.{a}")).collect()),
                    gl::Path::Scale => (object_of.get(&c.node).copied(), ["x", "y", "z"].iter().map(|a| format!("scale.{a}")).collect()),
                    gl::Path::Rotation => (object_of.get(&c.node).copied(), ["x", "y", "z"].iter().map(|a| format!("rotate.{a}")).collect()),
                    gl::Path::Weights => (shapes_of.get(&c.node).copied(), (0..c.width()).map(|i| format!("target.{i}.weight")).collect()),
                };
                let Some(target) = target else { continue };
                let mut keys: Vec<Vec<Key>> = vec![Vec::with_capacity(times.len()); paths.len()];
                let mut before = [0.0f64; 3];
                for (n, frame) in times.iter().enumerate() {
                    sample(c, (*frame / fps) as f32, &mut value);
                    let numbers: Vec<f64> = if c.path == gl::Path::Rotation {
                        if value.len() != 4 {
                            continue;
                        }
                        let e = Transform { rotation: Quat::from_slice(&value), ..Transform::default() }.sanitized().euler_degrees();
                        let mut e = e.to_array().map(f64::from);
                        if n > 0 {
                            for (angle, prev) in e.iter_mut().zip(before) {
                                *angle = unwrap_angle(*angle, prev);
                            }
                        }
                        before = e;
                        e.to_vec()
                    } else {
                        value.iter().map(|v| *v as f64).collect()
                    };
                    for (list, v) in keys.iter_mut().zip(numbers) {
                        list.push(Key::new(start + *frame, v).with_interp(interp));
                    }
                }
                for (path, list) in paths.iter().zip(keys) {
                    set_curve(scene, target, path, list);
                }
            }
            let time = &mut scene.anim.time;
            time.end = time.end.max(start + last as f64);
        }
    } else if clip.is_some() {
        warnings.push(format!("the file has no clip {chosen}"));
    }
    scene.apply_animation();
    scene.solve_constraints();
    Ok((created, warnings))
}
