//! The scene's animation: curves bound to attributes, the current frame and the time settings.
//!
//! A [`Track`] drives one numeric attribute of one object ([`crate::attr`]) with a
//! [`modelcraft_anim::Curve`]. The scene always holds the pose of the current frame:
//! [`Scene::set_frame`] evaluates every track and writes the values into the objects, bumping the
//! revision of each object that changed, so the [`crate::Evaluator`] re-evaluates exactly what
//! the new frame touched. Editing an animated attribute without keying it lasts until the frame
//! changes, as in other animation tools; [`auto_key`] turns such edits into keys.

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::Vec3;
use modelcraft_anim::clock::clamp_frame;
use modelcraft_anim::{Curve, TimeSettings};
use serde::{Deserialize, Serialize};

use crate::{ObjectId, Scene, SceneError, attr};

/// Most tracks one scene holds.
pub const MAX_TRACKS: usize = 200_000;

/// The nine transform channels, in display order.
pub const TRANSFORM_PATHS: [&str; 9] = ["translate.x", "translate.y", "translate.z", "rotate.x", "rotate.y", "rotate.z", "scale.x", "scale.y", "scale.z"];
const ROTATE_PATHS: [&str; 3] = ["rotate.x", "rotate.y", "rotate.z"];

/// A curve driving one attribute of one object, in one layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub target: ObjectId,
    /// Attribute path ([`crate::attr`]).
    pub path: String,
    /// The layer the curve belongs to ([`BASE_LAYER`] or a [`Layer::id`]).
    #[serde(default, skip_serializing_if = "is_base")]
    pub layer: u32,
    pub curve: Arc<Curve>,
}

/// The layer every scene has: plain curves, always at full weight.
pub const BASE_LAYER: u32 = 0;
/// Most layers above the base.
pub const MAX_LAYERS: usize = 64;

fn is_base(l: &u32) -> bool {
    *l == BASE_LAYER
}
fn full() -> f64 {
    1.0
}

/// How a layer's curves combine with what lies below them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayerMode {
    /// The curve's value is added, scaled by the layer's weight.
    #[default]
    Add,
    /// The curve's value replaces the value below, blended by the layer's weight.
    Override,
}

/// An animation layer above the base: its curves adjust (or replace) the animation below
/// without touching it, and can be faded with `weight` or switched off with `mute`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    /// Stable id, never [`BASE_LAYER`].
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub mode: LayerMode,
    #[serde(default = "full")]
    pub weight: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mute: bool,
    /// Play only this layer (and other solo layers) on top of the base.
    #[serde(default, skip_serializing_if = "is_false")]
    pub solo: bool,
    /// Keys in this layer cannot be set, changed or removed.
    #[serde(default, skip_serializing_if = "is_false")]
    pub lock: bool,
    /// The weight over time: when it has keys it replaces `weight`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_curve: Option<Arc<Curve>>,
}

impl Layer {
    /// The layer's weight at `frame`: its weight curve when keyed, else its plain weight.
    pub fn weight_at(&self, frame: f64) -> f64 {
        let w = self.weight_curve.as_ref().and_then(|c| c.eval(frame)).unwrap_or(self.weight);
        if w.is_finite() { w.clamp(0.0, 1.0) } else { 1.0 }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}
fn is_zero(f: &f64) -> bool {
    *f == 0.0
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Animation {
    #[serde(default, flatten)]
    pub time: TimeSettings,
    /// The current frame.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub frame: f64,
    /// Turn edits of transforms and of animated attributes into keys.
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_key: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tracks: Vec<Track>,
    /// Layers above the base, bottom first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    layers: Vec<Layer>,
    /// The layer keys are set and edited in.
    #[serde(default, skip_serializing_if = "is_base")]
    active_layer: u32,
}

impl Animation {
    /// True when nothing differs from a new scene (the block is then left out of the file).
    pub fn is_default(&self) -> bool {
        *self == Animation::default()
    }
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }
    /// The track of an attribute in the active layer.
    pub fn track(&self, id: ObjectId, path: &str) -> Option<&Track> {
        self.track_in(id, path, self.active_layer)
    }
    pub fn track_in(&self, id: ObjectId, path: &str, layer: u32) -> Option<&Track> {
        self.tracks.iter().find(|t| t.target == id && t.path == path && t.layer == layer)
    }
    /// True when the attribute has a curve in any layer.
    pub fn has_curve(&self, id: ObjectId, path: &str) -> bool {
        self.tracks.iter().any(|t| t.target == id && t.path == path)
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }
    pub fn layer(&self, id: u32) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }
    pub fn layer_mut(&mut self, id: u32) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }
    pub fn active_layer(&self) -> u32 {
        self.active_layer
    }
    /// Choose the layer keys go to; `false` when there is no such layer.
    pub fn set_active_layer(&mut self, id: u32) -> bool {
        let ok = id == BASE_LAYER || self.layer(id).is_some();
        if ok {
            self.active_layer = id;
        }
        ok
    }
    /// Add a layer on top; returns its id.
    pub fn add_layer(&mut self, name: &str, mode: LayerMode) -> Result<u32, SceneError> {
        if self.layers.len() >= MAX_LAYERS {
            return Err(SceneError::Invalid("too many animation layers".into()));
        }
        let id = self.layers.iter().map(|l| l.id).max().unwrap_or(BASE_LAYER) + 1;
        let name: String = name.trim().chars().take(128).collect();
        self.layers.push(Layer {
            id,
            name: if name.is_empty() { format!("Layer {id}") } else { name },
            mode,
            weight: 1.0,
            mute: false,
            solo: false,
            lock: false,
            weight_curve: None,
        });
        Ok(id)
    }
    /// Remove a layer and its curves; the base layer becomes active if it was the active one.
    pub fn remove_layer(&mut self, id: u32) -> bool {
        let n = self.layers.len();
        self.layers.retain(|l| l.id != id);
        if self.layers.len() == n {
            return false;
        }
        self.tracks.retain(|t| t.layer != id);
        if self.active_layer == id {
            self.active_layer = BASE_LAYER;
        }
        true
    }

    /// The value of an attribute at `frame` with every layer applied, bottom to top: the base
    /// curve, then each layer that is not muted adds its curve (times its weight) or blends
    /// towards it (by its weight, which may itself be keyed). With a solo layer, only solo layers
    /// play on top of the base. `None` when no layer has a curve with keys for it.
    pub fn value_at(&self, id: ObjectId, path: &str, frame: f64) -> Option<f64> {
        let mut v = self.track_in(id, path, BASE_LAYER).and_then(|t| t.curve.eval(frame));
        let solo = self.layers.iter().any(|l| l.solo);
        for l in self.layers.iter().filter(|l| !l.mute && (!solo || l.solo)) {
            let Some(x) = self.track_in(id, path, l.id).and_then(|t| t.curve.eval(frame)) else { continue };
            let w = l.weight_at(frame);
            v = Some(match (l.mode, v) {
                (LayerMode::Add, below) => below.unwrap_or(0.0) + x * w,
                (LayerMode::Override, Some(below)) => below + (x - below) * w,
                (LayerMode::Override, None) => x,
            });
        }
        v
    }

    /// Every animated attribute once: `(object, path)` in track order.
    pub fn animated(&self) -> Vec<(ObjectId, &str)> {
        let mut seen = std::collections::HashSet::new();
        self.tracks.iter().filter(|t| seen.insert((t.target, t.path.as_str()))).map(|t| (t.target, t.path.as_str())).collect()
    }
    pub fn curve(&self, id: ObjectId, path: &str) -> Option<&Curve> {
        self.track(id, path).map(|t| &*t.curve)
    }
    pub fn tracks_of(&self, id: ObjectId) -> impl Iterator<Item = &Track> {
        self.tracks.iter().filter(move |t| t.target == id)
    }
    pub fn is_animated(&self, id: ObjectId) -> bool {
        self.tracks.iter().any(|t| t.target == id)
    }

    /// The curve of an attribute in the active layer, created empty when missing.
    pub fn curve_mut(&mut self, id: ObjectId, path: &str) -> Result<&mut Curve, SceneError> {
        if self.active_locked() {
            return Err(SceneError::Invalid("the active animation layer is locked".into()));
        }
        self.curve_mut_in(id, path, self.active_layer)
    }

    /// Whether the active layer is locked against key edits.
    pub fn active_locked(&self) -> bool {
        self.layer(self.active_layer).is_some_and(|l| l.lock)
    }

    /// Move a layer `delta` places up the stack (negative: down); `true` when it moved.
    pub fn move_layer(&mut self, id: u32, delta: i32) -> bool {
        let Some(at) = self.layers.iter().position(|l| l.id == id) else { return false };
        let to = (at as i64 + delta as i64).clamp(0, self.layers.len() as i64 - 1) as usize;
        if to == at {
            return false;
        }
        let l = self.layers.remove(at);
        self.layers.insert(to, l);
        true
    }

    /// The curve of an attribute in a layer, created empty when missing.
    pub fn curve_mut_in(&mut self, id: ObjectId, path: &str, layer: u32) -> Result<&mut Curve, SceneError> {
        let i = match self.tracks.iter().position(|t| t.target == id && t.path == path && t.layer == layer) {
            Some(i) => i,
            None => {
                if self.tracks.len() >= MAX_TRACKS || path.len() > attr::MAX_PATH {
                    return Err(SceneError::Invalid("too many animation tracks".into()));
                }
                // Keep an object's tracks together, transform channels in their usual order.
                let rank = |p: &str| TRANSFORM_PATHS.iter().position(|t| *t == p).unwrap_or(TRANSFORM_PATHS.len());
                let at = self
                    .tracks
                    .iter()
                    .rposition(|t| t.target == id && rank(&t.path) <= rank(path))
                    .map(|i| i + 1)
                    .or_else(|| self.tracks.iter().position(|t| t.target == id))
                    .unwrap_or(self.tracks.len());
                self.tracks.insert(at, Track { target: id, path: path.to_string(), layer, curve: Arc::new(Curve::new()) });
                at
            }
        };
        match self.tracks.get_mut(i) {
            Some(t) => Ok(Arc::make_mut(&mut t.curve)),
            None => Err(SceneError::Invalid("no such track".into())),
        }
    }

    /// Remove an attribute's curve from the active layer; `true` when it existed.
    pub fn remove_track(&mut self, id: ObjectId, path: &str) -> bool {
        if self.active_locked() {
            return false;
        }
        let (n, layer) = (self.tracks.len(), self.active_layer);
        self.tracks.retain(|t| !(t.target == id && t.path == path && t.layer == layer));
        self.tracks.len() != n
    }
    /// Remove an attribute's curves from every layer; `true` when it had any.
    pub fn remove_path(&mut self, id: ObjectId, path: &str) -> bool {
        let n = self.tracks.len();
        self.tracks.retain(|t| !(t.target == id && t.path == path));
        self.tracks.len() != n
    }
    /// Remove an object's curves from the active layer; `true` when it had any there.
    pub fn remove_object_tracks(&mut self, id: ObjectId) -> bool {
        if self.active_locked() {
            return false;
        }
        let (n, layer) = (self.tracks.len(), self.active_layer);
        self.tracks.retain(|t| !(t.target == id && t.layer == layer));
        self.tracks.len() != n
    }
    /// Drop tracks without keys.
    pub fn prune(&mut self) {
        self.tracks.retain(|t| !t.curve.is_empty());
    }
    pub fn remove_object(&mut self, id: ObjectId) {
        self.tracks.retain(|t| t.target != id);
    }

    /// Key times of the given objects (all objects for an empty list), sorted, without repeats.
    pub fn key_times(&self, ids: &[ObjectId]) -> Vec<f64> {
        let mut v: Vec<f64> = self.tracks.iter().filter(|t| ids.is_empty() || ids.contains(&t.target)).flat_map(|t| t.curve.keys.iter().map(|k| k.t)).collect();
        v.sort_by(f64::total_cmp);
        v.dedup_by(|a, b| (*a - *b).abs() < modelcraft_anim::curve::TIME_EPSILON);
        v
    }
    pub fn next_key_time(&self, ids: &[ObjectId], frame: f64) -> Option<f64> {
        self.tracks.iter().filter(|t| ids.is_empty() || ids.contains(&t.target)).filter_map(|t| t.curve.next_key_time(frame)).min_by(f64::total_cmp)
    }
    pub fn prev_key_time(&self, ids: &[ObjectId], frame: f64) -> Option<f64> {
        self.tracks.iter().filter(|t| ids.is_empty() || ids.contains(&t.target)).filter_map(|t| t.curve.prev_key_time(frame)).max_by(f64::total_cmp)
    }

    pub fn heap_bytes(&self) -> usize {
        self.tracks.iter().map(|t| 64 + t.path.len() + t.curve.heap_bytes()).sum()
    }

    /// Repair after reading a file: finite settings, bounded and unique tracks on existing
    /// objects, repaired curves.
    pub(crate) fn repair(&mut self, exists: impl Fn(ObjectId) -> bool) {
        self.time = self.time.sanitized();
        self.frame = clamp_frame(self.frame);
        self.layers.truncate(MAX_LAYERS);
        let mut ids = std::collections::HashSet::new();
        self.layers.retain(|l| l.id != BASE_LAYER && ids.insert(l.id));
        for l in &mut self.layers {
            l.weight = if l.weight.is_finite() { l.weight.clamp(0.0, 1.0) } else { 1.0 };
            l.name = l.name.chars().take(128).collect();
            if let Some(c) = &mut l.weight_curve {
                Arc::make_mut(c).sanitize();
            }
        }
        if !ids.contains(&self.active_layer) {
            self.active_layer = BASE_LAYER;
        }
        let mut seen = std::collections::HashSet::new();
        self.tracks.retain(|t| {
            exists(t.target)
                && (t.layer == BASE_LAYER || ids.contains(&t.layer))
                && !t.path.is_empty()
                && t.path.len() <= attr::MAX_PATH
                && seen.insert((t.target, t.path.clone(), t.layer))
        });
        self.tracks.truncate(MAX_TRACKS);
        for t in &mut self.tracks {
            Arc::make_mut(&mut t.curve).sanitize();
        }
        self.prune();
    }
}

impl Scene {
    /// The value of a numeric attribute ([`crate::attr`]).
    pub fn attr(&self, id: ObjectId, path: &str) -> Option<f64> {
        if path.starts_with(crate::constraint::WEIGHT_PREFIX) {
            return self.constraints.weight_attr(id, path);
        }
        attr::get(self.get(id)?, path)
    }

    /// Set a numeric attribute; the object's revision is bumped only when the value changed.
    pub fn set_attr(&mut self, id: ObjectId, path: &str, value: f64) -> Result<bool, SceneError> {
        if path.starts_with(crate::constraint::WEIGHT_PREFIX) {
            return self.constraints.set_weight_attr(id, path, value).ok_or_else(|| SceneError::Invalid(format!("no numeric attribute {path:?}")));
        }
        let o = self.object_mut(id).ok_or(SceneError::NoObject(id))?;
        let changed = attr::set(o, path, value)?;
        if changed {
            o.rev = crate::next_rev();
        }
        Ok(changed)
    }

    /// Go to a frame: evaluate every track there and pose the objects.
    pub fn set_frame(&mut self, frame: f64) {
        self.anim.frame = clamp_frame(frame);
        self.apply_animation();
        self.solve_constraints();
    }

    /// Write the value of every track at the current frame into its attribute. Tracks whose
    /// attribute no longer exists are skipped. Returns how many objects changed.
    ///
    /// Curves set the transforms constraints start from, so the constraints' results are taken
    /// back first; call [`Scene::solve_constraints`] afterwards ([`Scene::set_frame`] does).
    pub fn apply_animation(&mut self) -> usize {
        if self.anim.tracks.is_empty() {
            return 0;
        }
        self.unsolve_constraints();
        let frame = self.anim.frame;
        // Values per object; rotation is set as one triple.
        let mut per: BTreeMap<ObjectId, Vec<(&str, f64)>> = BTreeMap::new();
        for (target, path) in self.anim.animated() {
            if let Some(v) = self.anim.value_at(target, path, frame) {
                per.entry(target).or_default().push((path, v));
            }
        }
        let mut changed = 0;
        for (target, values) in &per {
            for (path, v) in values {
                if path.starts_with(crate::constraint::WEIGHT_PREFIX) {
                    self.constraints.set_weight_attr(*target, path, *v);
                }
            }
        }
        for o in &mut self.objects {
            let Some(values) = per.get(&o.id) else { continue };
            let mut dirty = false;
            let mut rot: [Option<f64>; 3] = [None; 3];
            for (path, v) in values {
                match ROTATE_PATHS.iter().position(|p| p == path) {
                    Some(i) => rot[i] = Some(*v),
                    None => dirty |= attr::set(o, path, *v).unwrap_or(false),
                }
            }
            if rot.iter().any(Option::is_some) {
                let e = o.transform.euler_degrees().to_array();
                let d = Vec3::new(rot[0].map_or(e[0], |v| v as f32), rot[1].map_or(e[1], |v| v as f32), rot[2].map_or(e[2], |v| v as f32));
                dirty |= attr::set_rotation(o, d);
            }
            if dirty {
                o.rev = crate::next_rev();
                changed += 1;
            }
        }
        changed
    }

    /// Key an attribute at `frame` in the active layer, with `value` or, when `None`, with
    /// whatever makes the result equal the attribute's current value.
    ///
    /// Keying one rotation angle keys all three: a rotation with only some angles keyed would
    /// have to read the others back from the quaternion, and the same rotation reads back as
    /// different triples (past ±90° on the middle axis), so the pose would jump. In a layer above
    /// the base, an attribute without any curve first gets a base key with its current value, so
    /// the layer always has something to adjust.
    pub fn set_key(&mut self, id: ObjectId, path: &str, frame: f64, value: Option<f64>) -> Result<(), SceneError> {
        let now = self.attr(id, path).ok_or_else(|| SceneError::Invalid(format!("object {} has no numeric attribute {path:?}", id.0)))?;
        if !frame.is_finite() || value.is_some_and(|v| !v.is_finite()) {
            return Err(SceneError::Invalid("a key needs a finite frame and value".into()));
        }
        let frame = clamp_frame(frame);
        let layer = self.anim.active_layer;
        let wanted: Vec<(&str, f64)> = match ROTATE_PATHS.iter().position(|p| *p == path) {
            Some(axis) => ROTATE_PATHS.iter().copied().zip(self.rotation_triple(id, frame, axis)).collect(),
            None => vec![(path, now)],
        };
        for (p, now) in wanted {
            let target = p == path;
            if layer != BASE_LAYER && !self.anim.has_curve(id, p) {
                self.anim.curve_mut_in(id, p, BASE_LAYER)?.set_key(frame, now);
            }
            if !target && self.anim.track(id, p).is_some() {
                continue;
            }
            let v = match (target, value) {
                (true, Some(v)) => v,
                _ => self.layer_value(id, p, frame, now),
            };
            self.anim.curve_mut(id, p)?.set_key(frame, v).ok_or_else(|| SceneError::Invalid("the curve is full".into()))?;
        }
        Ok(())
    }

    /// The value a key in the active layer needs at `frame` for the layered result to be `result`.
    fn layer_value(&self, id: ObjectId, path: &str, frame: f64, result: f64) -> f64 {
        let layer = self.anim.active_layer;
        let Some(l) = self.anim.layer(layer) else { return result };
        let (mode, w) = (l.mode, l.weight_at(frame));
        // What the layers give without this layer's curve. Layers above it are ignored: the key
        // makes the result right as seen from this layer.
        let mut below = self.anim.clone();
        let at = below.layers.iter().position(|x| x.id == layer).unwrap_or(0);
        below.layers.truncate(at);
        let under = below.value_at(id, path, frame).unwrap_or(result);
        match mode {
            LayerMode::Add if w > 1e-6 => (result - under) / w,
            LayerMode::Add => 0.0,
            LayerMode::Override if w > 1e-6 => under + (result - under) / w,
            LayerMode::Override => result,
        }
    }

    /// The object's current rotation as XYZ Euler degrees, as the triple that continues its
    /// rotation curves at `frame` (nearest whole turns, flipped or not); without curves, the
    /// plain triple for `axis` ([`plain_euler`]).
    fn rotation_triple(&self, id: ObjectId, frame: f64, axis: usize) -> [f64; 3] {
        let e = self.get(id).map(|o| o.transform.euler_degrees().to_array().map(f64::from)).unwrap_or_default();
        let curves = ROTATE_PATHS.map(|p| self.anim.value_at(id, p, frame));
        if curves.iter().all(Option::is_none) {
            return plain_euler(e, axis);
        }
        let fit = |c: [f64; 3]| {
            let mut out = c;
            let mut dist = 0.0;
            for ((o, a), curve) in out.iter_mut().zip(c).zip(curves) {
                *o = unwrap_angle(a, curve.unwrap_or(0.0));
                dist += curve.map_or(0.0, |k| (*o - k).abs());
            }
            (out, dist)
        };
        let (a, da) = fit(e);
        let (b, db) = fit([e[0] + 180.0, 180.0 - e[1], e[2] + 180.0]);
        if db + 1e-6 < da { b } else { a }
    }
}

/// Most samples one motion path holds.
pub const MAX_PATH_SAMPLES: usize = 4000;

impl Scene {
    /// An object's transform at `frame`: its stored transform with every keyed transform channel
    /// replaced by its layered curve value. The scene is not changed.
    pub fn transform_at(&self, id: ObjectId, frame: f64) -> Option<crate::Transform> {
        let o = self.get(id)?;
        let mut t = o.transform;
        let mut rot: [Option<f32>; 3] = [None; 3];
        for (i, path) in TRANSFORM_PATHS.iter().enumerate() {
            let Some(v) = self.anim.value_at(id, path, frame) else { continue };
            let v = v as f32;
            match i / 3 {
                0 => t.translation[i % 3] = v,
                1 => rot[i % 3] = Some(v),
                _ => t.scale[i % 3] = v,
            }
        }
        if rot.iter().any(Option::is_some) {
            let e = t.euler_degrees();
            t.set_euler_degrees(Vec3::new(rot[0].unwrap_or(e.x), rot[1].unwrap_or(e.y), rot[2].unwrap_or(e.z)));
        }
        Some(t.sanitized())
    }

    /// Object → world matrix at `frame`, following the animated transforms of the ancestors.
    pub fn world_matrix_at(&self, id: ObjectId, frame: f64) -> glam::Mat4 {
        let mut m = glam::Mat4::IDENTITY;
        let mut c = Some(id);
        for _ in 0..crate::MAX_DEPTH {
            let Some(cur) = c else { break };
            let Some(t) = self.transform_at(cur, frame) else { break };
            let pivot = self.get(cur).map_or(Vec3::ZERO, |o| o.pivot);
            m = t.matrix() * glam::Mat4::from_translation(-pivot) * m;
            c = self.get(cur).and_then(|o| o.parent);
        }
        m
    }

    /// True when the object or one of its ancestors has a keyed transform channel.
    pub fn moves(&self, id: ObjectId) -> bool {
        let mut c = Some(id);
        for _ in 0..crate::MAX_DEPTH {
            let Some(cur) = c else { break };
            if self.anim.tracks_of(cur).any(|t| TRANSFORM_PATHS.contains(&t.path.as_str())) {
                return true;
            }
            c = self.get(cur).and_then(|o| o.parent);
        }
        false
    }

    /// The world position of an object's origin at every `step` frames from `start` to `end`:
    /// `(frame, position)` pairs, at most [`MAX_PATH_SAMPLES`].
    pub fn motion_path(&self, id: ObjectId, start: f64, end: f64, step: f64) -> Vec<(f64, Vec3)> {
        if !(start.is_finite() && end.is_finite() && step.is_finite()) || end < start || self.get(id).is_none() {
            return Vec::new();
        }
        let step = step.max((end - start) / (MAX_PATH_SAMPLES as f64 - 1.0)).max(1e-3);
        let n = (((end - start) / step).floor() as usize).min(MAX_PATH_SAMPLES - 1);
        (0..=n).map(|i| start + i as f64 * step).map(|f| (f, self.world_matrix_at(id, f).transform_point3(Vec3::ZERO))).collect()
    }
}

/// Of the two Euler triples (degrees) for one rotation, the one whose angles other than `axis`
/// are smallest: a turn of 170° about Y reads back as (180, 10, 180) and is returned as
/// (0, 170, 0), so changing the `axis` angle turns the object the plain way.
pub fn plain_euler(e: [f64; 3], axis: usize) -> [f64; 3] {
    let flipped = [unwrap_angle(e[0] + 180.0, 0.0), unwrap_angle(180.0 - e[1], 0.0), unwrap_angle(e[2] + 180.0, 0.0)];
    let others = |c: &[f64; 3]| c.iter().enumerate().filter(|(i, _)| *i != axis).map(|(_, v)| v.abs()).sum::<f64>();
    if others(&flipped) + 1e-6 < others(&e) { flipped } else { e }
}

/// The angle equal to `degrees` (mod 360) that is nearest to `near`: an angle read back from a
/// rotation continues the curve instead of jumping a full turn.
pub fn unwrap_angle(degrees: f64, near: f64) -> f64 {
    degrees + ((near - degrees) / 360.0).round() * 360.0
}

/// Turn an edit into keys: compare the scene before and after a command and key what changed at
/// the current frame. Changed transform channels are always keyed (all three angles when the
/// rotation changed); any other attribute is keyed when it already has a track. Returns the
/// number of keys set.
pub fn auto_key(before: &Scene, after: &mut Scene) -> usize {
    let frame = after.anim.frame;
    let mut keys: Vec<(ObjectId, String)> = Vec::new();
    for o in after.objects() {
        let Some(old) = before.get(o.id) else { continue };
        if old.rev == o.rev {
            continue;
        }
        let (a, b) = (old.transform, o.transform);
        for (i, axis) in ["x", "y", "z"].iter().enumerate() {
            if (a.translation.to_array()[i] - b.translation.to_array()[i]).abs() > 1e-6 {
                keys.push((o.id, format!("translate.{axis}")));
            }
            if (a.scale.to_array()[i] - b.scale.to_array()[i]).abs() > 1e-6 {
                keys.push((o.id, format!("scale.{axis}")));
            }
        }
        if a.rotation.dot(b.rotation).abs() < 1.0 - 1e-7 {
            keys.extend(ROTATE_PATHS.iter().map(|p| (o.id, p.to_string())));
        }
        for (target, path) in after.anim.animated() {
            if target != o.id || TRANSFORM_PATHS.contains(&path) {
                continue;
            }
            if let (Some(now), Some(was)) = (attr::get(o, path), attr::get(old, path))
                && (now - was).abs() > 1e-9
            {
                keys.push((o.id, path.to_string()));
            }
        }
    }
    keys.into_iter().filter(|(id, path)| after.set_key(*id, path, frame, None).is_ok()).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Evaluator, ObjectKind, Source, Step, StepOp};
    use modelcraft_anim::Interp;
    use modelcraft_ops::deform::Deformer;
    use modelcraft_ops::primitives::Primitive;

    fn cube(s: &mut Scene) -> ObjectId {
        let kind = ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::Cube { size: Vec3::splat(2.0), segments: 1 } }, steps: vec![] };
        s.add("Cube", kind, None).unwrap()
    }

    #[test]
    fn every_number_of_an_object_is_an_attribute() {
        let mut s = Scene::new();
        let id = cube(&mut s);
        let sid = s.fresh_id();
        if let ObjectKind::Mesh { steps, .. } = &mut s.edit(id).unwrap().kind {
            steps.push(Step { id: sid, enabled: true, op: StepOp::extrude(vec![0, 1, 2, 3, 4, 5], 1.0) });
        }
        let names: Vec<String> = attr::list(s.get(id).unwrap()).into_iter().map(|(n, _)| n).collect();
        for want in ["translate.x", "rotate.z", "scale.y", "visible", "material.roughness", "material.base_color.3", "primitive.size.0", "primitive.segments"] {
            assert!(names.iter().any(|n| n == want), "{want} missing from {names:?}");
        }
        assert!(names.contains(&format!("step.{sid}.distance")));
        assert!(names.contains(&format!("step.{sid}.enabled")));
        assert!(!names.iter().any(|n| n.contains("faces")), "selections are not attributes");
        // Every listed attribute reads back and can be set to its own value without a change.
        for (name, v) in attr::list(s.get(id).unwrap()) {
            assert_eq!(s.attr(id, &name), Some(v), "{name}");
            assert_eq!(s.set_attr(id, &name, v), Ok(false), "{name}");
        }
        // Whole numbers stay whole, switches flip at one half, floats are free.
        let rev = s.get(id).unwrap().rev;
        assert_eq!(s.set_attr(id, "primitive.segments", 2.6), Ok(true));
        assert_eq!(s.attr(id, "primitive.segments"), Some(3.0));
        assert_ne!(s.get(id).unwrap().rev, rev);
        assert_eq!(s.set_attr(id, &format!("step.{sid}.enabled"), 0.2), Ok(true));
        assert!(!s.get(id).unwrap().steps()[0].enabled);
        assert_eq!(s.set_attr(id, "material.metallic", 0.25), Ok(true));
        assert_eq!(s.get(id).unwrap().material.metallic, 0.25);
        assert!(s.set_attr(id, "primitive.kind", 1.0).is_err(), "text is not a number");
        assert!(s.set_attr(id, "nope.x", 1.0).is_err());
        assert!(s.set_attr(id, "translate.x", f64::NAN).is_err());
        assert!(s.set_attr(ObjectId(99), "translate.x", 1.0).is_err());
        // Other kinds are walked as they are saved.
        let d = s.add("Bend", ObjectKind::Deformer { deformer: Deformer::Bend { angle: 90.0 }, limits: Default::default(), target: None }, Some(id)).unwrap();
        assert_eq!(s.attr(d, "deformer.angle"), Some(90.0));
        let cage = s.add("Cage", ObjectKind::SmoothCage { levels: 2 }, None).unwrap();
        assert_eq!(s.set_attr(cage, "levels", 3.0), Ok(true));
    }

    #[test]
    fn frames_pose_the_scene_and_only_changed_objects_are_re_evaluated() {
        let mut s = Scene::new();
        let mut ev = Evaluator::new();
        let a = cube(&mut s);
        let b = cube(&mut s);
        s.set_key(a, "translate.x", 0.0, Some(0.0)).unwrap();
        s.set_key(a, "translate.x", 10.0, Some(10.0)).unwrap();
        s.set_key(a, "primitive.segments", 0.0, Some(1.0)).unwrap();
        s.set_key(a, "primitive.segments", 10.0, Some(3.0)).unwrap();
        s.anim.curve_mut(a, "translate.x").unwrap().keys[0].interp = Interp::Linear;
        s.anim.curve_mut(a, "primitive.segments").unwrap().keys[0].interp = Interp::Stepped;
        let _ = (ev.evaluate(&s, a), ev.evaluate(&s, b));
        let rev_b = s.get(b).unwrap().rev;
        s.set_frame(5.0);
        assert_eq!(s.get(a).unwrap().transform.translation.x, 5.0);
        assert_eq!(ev.evaluate(&s, a).mesh.face_count(), 6, "stepped: still one segment");
        assert_eq!(s.get(b).unwrap().rev, rev_b, "an object without tracks is untouched");
        let rev_a = s.get(a).unwrap().rev;
        s.set_frame(5.0);
        assert_eq!(s.get(a).unwrap().rev, rev_a, "the same frame changes nothing");
        s.set_frame(10.0);
        assert_eq!(ev.evaluate(&s, a).mesh.face_count(), 54, "the keyed parameter rebuilt the primitive");
        // Removing an object removes its tracks.
        s.remove(a);
        assert!(s.anim.is_empty());
    }

    #[test]
    fn rotation_is_keyed_as_three_angles_without_flips() {
        let mut s = Scene::new();
        let id = cube(&mut s);
        for (f, deg) in [(0.0, 0.0), (10.0, 170.0), (20.0, 340.0)] {
            for (p, v) in [("rotate.x", 0.0), ("rotate.y", deg), ("rotate.z", 0.0)] {
                s.set_key(id, p, f, Some(v)).unwrap();
            }
        }
        // Past 90° the quaternion reads back as a different Euler triple; the curves still drive it.
        s.set_frame(10.0);
        let q = s.get(id).unwrap().transform.rotation;
        let want = glam::Quat::from_rotation_y(170f32.to_radians());
        assert!(q.dot(want).abs() > 0.9999, "{q:?}");
        // Keying the current pose continues the curve instead of jumping a turn.
        s.set_frame(20.0);
        s.set_key(id, "rotate.y", 20.0, None).unwrap();
        let v = s.anim.curve(id, "rotate.y").unwrap().eval(20.0).unwrap();
        assert!((v - 340.0).abs() < 1e-3, "{v}");
        assert_eq!(unwrap_angle(-170.0, 180.0), 190.0);
    }

    #[test]
    fn keying_one_angle_keys_all_three_so_the_pose_never_flips() {
        let mut s = Scene::new();
        let id = cube(&mut s);
        s.set_key(id, "rotate.y", 0.0, Some(0.0)).unwrap();
        s.set_key(id, "rotate.y", 20.0, Some(340.0)).unwrap();
        assert_eq!(s.anim.tracks().len(), 3);
        s.anim.curve_mut(id, "rotate.y").unwrap().keys[0].interp = Interp::Linear;
        // Scrub through 90°, 180° and 270° and back: always a pure turn about Y.
        for f in [1.0, 5.0, 6.0, 10.0, 11.0, 16.0, 20.0, 15.0, 9.0, 5.5, 0.0] {
            s.set_frame(f);
            let want = glam::Quat::from_rotation_y((f as f32 * 17.0).to_radians());
            let q = s.get(id).unwrap().transform.rotation;
            assert!(q.dot(want).abs() > 0.9999, "frame {f}: {q:?}");
        }
        // Keying an angle on an object that is already turned starts from the matching triple.
        let other = cube(&mut s);
        s.edit(other).unwrap().transform.rotation = glam::Quat::from_rotation_y(170f32.to_radians());
        s.set_key(other, "rotate.y", 0.0, Some(170.0)).unwrap();
        let x = s.anim.curve(other, "rotate.x").unwrap().eval(0.0).unwrap();
        assert!(x.abs() < 1e-3, "not the flipped triple (180, 10, 180): {x}");
    }

    #[test]
    fn auto_key_keys_changed_channels_and_animated_attributes() {
        let mut s = Scene::new();
        let id = cube(&mut s);
        s.set_key(id, "material.roughness", 0.0, Some(0.5)).unwrap();
        s.set_frame(12.0);
        let before = s.clone();
        {
            let o = s.edit(id).unwrap();
            o.transform.translation.y = 3.0;
            o.material.roughness = 0.9;
            o.material.metallic = 1.0;
        }
        assert_eq!(auto_key(&before, &mut s), 2);
        assert_eq!(s.anim.curve(id, "translate.y").unwrap().eval(12.0), Some(3.0));
        assert!((s.anim.curve(id, "material.roughness").unwrap().eval(12.0).unwrap() - 0.9).abs() < 1e-6);
        assert!(s.anim.track(id, "material.metallic").is_none(), "an attribute without a curve is not keyed");
        assert!(s.anim.track(id, "translate.x").is_none(), "an unchanged channel is not keyed");
    }

    #[test]
    fn motion_paths_follow_animated_parents_without_changing_the_scene() {
        let mut s = Scene::new();
        let parent = s.add("P", ObjectKind::Group, None).unwrap();
        let child = cube(&mut s);
        s.reparent(child, Some(parent)).unwrap();
        s.edit(child).unwrap().transform.translation = Vec3::new(1.0, 0.0, 0.0);
        for (f, deg) in [(0.0, 0.0), (10.0, 90.0)] {
            for p in ["rotate.x", "rotate.y", "rotate.z"] {
                s.set_key(parent, p, f, Some(if p == "rotate.y" { deg } else { 0.0 })).unwrap();
            }
        }
        s.anim.curve_mut(parent, "rotate.y").unwrap().keys[0].interp = Interp::Linear;
        assert!(s.moves(child) && s.moves(parent));
        let before = s.clone();
        let path = s.motion_path(child, 0.0, 10.0, 1.0);
        assert_eq!(s, before);
        assert_eq!(path.len(), 11);
        assert!((path[0].1 - Vec3::X).length() < 1e-5);
        assert!((path[10].1 - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5, "{:?}", path[10]);
        assert!(path.iter().all(|(_, p)| (p.length() - 1.0).abs() < 1e-5), "a quarter circle");
        // Bounded and safe on bad input.
        assert_eq!(s.motion_path(child, 0.0, 1e9, 1.0).len(), MAX_PATH_SAMPLES);
        assert!(s.motion_path(child, 5.0, 1.0, 1.0).is_empty());
        assert!(s.motion_path(child, 0.0, f64::NAN, 1.0).is_empty());
        assert!(s.motion_path(ObjectId(99), 0.0, 1.0, 1.0).is_empty());
        let still = cube(&mut s);
        assert!(!s.moves(still));
    }

    #[test]
    fn hostile_animation_is_repaired() {
        let json = r#"{"objects":[{"id":1,"name":"G","kind":{"type":"group"}}],"anim":{"fps":-5,"start":9,"end":1,"frame":1e300,"tracks":[
            {"target":1,"path":"translate.x","curve":{"keys":[{"t":5,"v":1},{"t":"NaN","v":1}]}}]}}"#;
        assert!(serde_json::from_str::<Scene>(json).is_err(), "NaN is not JSON");
        let json = r#"{"objects":[{"id":1,"name":"G","kind":{"type":"group"}}],"anim":{"fps":-5,"start":9,"end":1,"frame":1e300,"tracks":[
            {"target":1,"path":"translate.x","curve":{"keys":[{"t":5,"v":1},{"t":5,"v":2},{"t":1,"v":0,"in_weight":50}]}},
            {"target":1,"path":"translate.x","curve":{"keys":[{"t":0,"v":9}]}},
            {"target":7,"path":"translate.x","curve":{"keys":[{"t":0,"v":9}]}},
            {"target":1,"path":"no.such.thing","curve":{"keys":[{"t":0,"v":9}]}},
            {"target":1,"path":"scale.x","curve":{"keys":[]}}]}}"#;
        let mut s = serde_json::from_str::<Scene>(json).unwrap().repaired().unwrap();
        assert_eq!((s.anim.time.fps, s.anim.time.start, s.anim.time.end), (1.0, 9.0, 9.0));
        assert_eq!(s.anim.frame, modelcraft_anim::clock::MAX_FRAME);
        assert_eq!(s.anim.tracks().len(), 2, "duplicates, missing objects and empty curves are dropped");
        let c = s.anim.curve(ObjectId(1), "translate.x").unwrap();
        assert_eq!(c.keys.iter().map(|k| (k.t, k.v)).collect::<Vec<_>>(), vec![(1.0, 0.0), (5.0, 2.0)]);
        // A track on an attribute that does not exist is skipped when posing.
        s.set_frame(5.0);
        assert_eq!(s.get(ObjectId(1)).unwrap().transform.translation.x, 2.0);
        // Layers: duplicate and base ids go, weights are clamped, tracks on unknown layers go.
        let json = r#"{"objects":[{"id":1,"name":"G","kind":{"type":"group"}}],"anim":{"active_layer":9,
            "layers":[{"id":2,"name":"A","weight":7},{"id":2,"name":"B"},{"id":0,"name":"C"}],"tracks":[
            {"target":1,"path":"translate.x","layer":2,"curve":{"keys":[{"t":0,"v":4}]}},
            {"target":1,"path":"translate.x","layer":5,"curve":{"keys":[{"t":0,"v":9}]}}]}}"#;
        let mut l = serde_json::from_str::<Scene>(json).unwrap().repaired().unwrap();
        assert_eq!((l.anim.layers().len(), l.anim.layers()[0].weight, l.anim.active_layer(), l.anim.tracks().len()), (1, 1.0, BASE_LAYER, 1));
        l.set_frame(0.0);
        assert_eq!(l.get(ObjectId(1)).unwrap().transform.translation.x, 4.0, "an adding layer without a base adds to zero");
        // A scene without animation saves no animation block, and old files still load.
        let plain = Scene::new();
        assert!(!serde_json::to_string(&plain).unwrap().contains("anim"));
        assert!(serde_json::from_str::<Scene>(r#"{"objects":[]}"#).unwrap().anim.is_default());
    }
}
