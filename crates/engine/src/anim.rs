//! Animation commands: keys, curves, time. One command, `anim`, with the operation in `op`:
//! `{"cmd":"anim","op":"set_key","id":1,"path":"translate.x","frame":12,"value":2}`.

use modelcraft_anim::clock::clamp_frame;
use modelcraft_anim::curve::TIME_EPSILON;
use modelcraft_anim::{Curve, Extrapolation, Interp, Tangent};
use modelcraft_scene::anim::TRANSFORM_PATHS;
use modelcraft_scene::{ObjectId, Scene, SceneError};
use serde::{Deserialize, Serialize};

use crate::command::Outcome;
use crate::document::EngineError;

/// One key, named by its object, attribute and frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeyRef {
    pub id: ObjectId,
    pub path: String,
    pub frame: f64,
}

fn yes() -> bool {
    true
}
fn one() -> f64 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AnimCommand {
    /// Key one attribute. `frame` defaults to the current frame, `value` to the attribute's
    /// current value.
    SetKey {
        id: ObjectId,
        path: String,
        #[serde(default)]
        frame: Option<f64>,
        #[serde(default)]
        value: Option<f64>,
    },
    /// Key position, rotation and scale of objects at once.
    KeyTransform {
        ids: Vec<ObjectId>,
        #[serde(default)]
        frame: Option<f64>,
        #[serde(default = "yes")]
        translate: bool,
        #[serde(default = "yes")]
        rotate: bool,
        #[serde(default = "yes")]
        scale: bool,
    },
    RemoveKeys {
        keys: Vec<KeyRef>,
    },
    /// Shift keys by `frames` and `value`.
    MoveKeys {
        keys: Vec<KeyRef>,
        #[serde(default)]
        frames: f64,
        #[serde(default)]
        value: f64,
    },
    /// Scale key times around the frame `pivot` and values around `value_pivot`.
    ScaleKeys {
        keys: Vec<KeyRef>,
        #[serde(default)]
        pivot: f64,
        #[serde(default = "one")]
        time: f64,
        #[serde(default)]
        value_pivot: f64,
        #[serde(default = "one")]
        value: f64,
    },
    /// Change one key: any of its value, interpolation, tangent mode, slopes and handle weights.
    EditKey {
        #[serde(flatten)]
        key: KeyRef,
        #[serde(default)]
        value: Option<f64>,
        #[serde(default)]
        interp: Option<Interp>,
        #[serde(default)]
        tangent: Option<Tangent>,
        #[serde(default)]
        in_slope: Option<f64>,
        #[serde(default)]
        out_slope: Option<f64>,
        #[serde(default)]
        in_weight: Option<f64>,
        #[serde(default)]
        out_weight: Option<f64>,
    },
    /// Set interpolation and/or tangent mode of many keys.
    SetKeyShape {
        keys: Vec<KeyRef>,
        #[serde(default)]
        interp: Option<Interp>,
        #[serde(default)]
        tangent: Option<Tangent>,
    },
    SetExtrapolation {
        id: ObjectId,
        path: String,
        #[serde(default)]
        pre: Option<Extrapolation>,
        #[serde(default)]
        post: Option<Extrapolation>,
    },
    /// Remove the curves of an object (one attribute, or all of them without `path`).
    RemoveTrack {
        id: ObjectId,
        #[serde(default)]
        path: Option<String>,
    },
    /// Replace a curve by one linear key every `step` frames over a span (default: the range).
    BakeTrack {
        id: ObjectId,
        path: String,
        #[serde(default)]
        start: Option<f64>,
        #[serde(default)]
        end: Option<f64>,
        #[serde(default = "one")]
        step: f64,
    },
    /// Remove keys a curve does not need, within `tolerance`.
    SimplifyTrack {
        id: ObjectId,
        path: String,
        tolerance: f64,
    },
    /// Go to a frame (not an undo step).
    SetTime {
        frame: f64,
    },
    /// Change the playback range and/or the frame rate.
    SetTimeRange {
        #[serde(default)]
        start: Option<f64>,
        #[serde(default)]
        end: Option<f64>,
        #[serde(default)]
        fps: Option<f64>,
    },
    SetAutoKey {
        on: bool,
    },
}

/// The `op` names of the `anim` command with one-line descriptions.
pub const COMMANDS: &[(&str, &str)] = &[
    (
        "set_key",
        "Key an attribute: id, path (translate.x, rotate.y, scale.z, visible, material.roughness, primitive.radius, step.<id>.distance, …), frame?, value?",
    ),
    ("key_transform", "Key position, rotation and scale: ids, frame?, translate?, rotate?, scale?"),
    ("remove_keys", "Remove keys: keys [{id, path, frame}]"),
    ("move_keys", "Shift keys in time and value: keys, frames, value"),
    ("scale_keys", "Scale keys around a pivot: keys, pivot, time, value_pivot, value"),
    (
        "edit_key",
        "Change one key: id, path, frame, value?, interp? (stepped, linear, bezier), tangent? (auto, flat, free, broken), in_slope?, out_slope?, in_weight?, out_weight?",
    ),
    ("set_key_shape", "Set interpolation and tangent mode of keys: keys, interp?, tangent?"),
    ("set_extrapolation", "What a curve does outside its keys: id, path, pre?, post? (hold, linear, cycle, cycle_offset, ping_pong)"),
    ("remove_track", "Remove an object's curves: id, path? (all without it)"),
    ("bake_track", "Sample a curve into linear keys: id, path, start?, end?, step"),
    ("simplify_track", "Remove keys a curve does not need: id, path, tolerance"),
    ("set_time", "Go to a frame: frame"),
    ("set_time_range", "Set the playback range and frame rate: start?, end?, fps?"),
    ("set_auto_key", "Key edits automatically: on"),
];

impl AnimCommand {
    pub fn label(&self) -> String {
        match self {
            AnimCommand::SetKey { .. } => "Set key",
            AnimCommand::KeyTransform { .. } => "Key transform",
            AnimCommand::RemoveKeys { .. } => "Remove keys",
            AnimCommand::MoveKeys { .. } => "Move keys",
            AnimCommand::ScaleKeys { .. } => "Scale keys",
            AnimCommand::EditKey { .. } => "Edit key",
            AnimCommand::SetKeyShape { .. } => "Key interpolation",
            AnimCommand::SetExtrapolation { .. } => "Curve extrapolation",
            AnimCommand::RemoveTrack { .. } => "Remove animation",
            AnimCommand::BakeTrack { .. } => "Bake curve",
            AnimCommand::SimplifyTrack { .. } => "Simplify curve",
            AnimCommand::SetTime { .. } => "Go to frame",
            AnimCommand::SetTimeRange { .. } => "Time range",
            AnimCommand::SetAutoKey { .. } => "Auto-key",
        }
        .into()
    }
}

fn invalid<T>(m: impl Into<String>) -> Result<T, EngineError> {
    Err(EngineError::Invalid(m.into()))
}

fn curve_mut<'a>(scene: &'a mut Scene, id: ObjectId, path: &str) -> Result<&'a mut Curve, EngineError> {
    if scene.anim.track(id, path).is_none() {
        return invalid(format!("object {} has no curve on {path:?}", id.0));
    }
    Ok(scene.anim.curve_mut(id, path)?)
}

/// Group key references by track and resolve them to indices; unknown keys are an error.
fn resolve(scene: &Scene, keys: &[KeyRef]) -> Result<Vec<(ObjectId, String, Vec<usize>)>, EngineError> {
    let mut out: Vec<(ObjectId, String, Vec<usize>)> = Vec::new();
    for k in keys {
        let c = scene.anim.curve(k.id, &k.path).ok_or_else(|| EngineError::Invalid(format!("object {} has no curve on {:?}", k.id.0, k.path)))?;
        let i = c.key_at(k.frame).ok_or_else(|| EngineError::Invalid(format!("no key at frame {} on {:?}", k.frame, k.path)))?;
        match out.iter_mut().find(|(id, p, _)| *id == k.id && *p == k.path) {
            Some((_, _, v)) => v.push(i),
            None => out.push((k.id, k.path.clone(), vec![i])),
        }
    }
    Ok(out)
}

/// Apply a scene-changing animation command. The pose is re-evaluated afterwards, so the scene
/// shows the edited curves at the current frame.
pub fn apply(scene: &mut Scene, cmd: &AnimCommand, out: &mut Outcome) -> Result<(), EngineError> {
    let now = scene.anim.frame;
    match cmd {
        AnimCommand::SetKey { id, path, frame, value } => {
            if scene.get(*id).is_none() {
                return Err(SceneError::NoObject(*id).into());
            }
            scene.set_key(*id, path, frame.unwrap_or(now), *value)?;
        }
        AnimCommand::KeyTransform { ids, frame, translate, rotate, scale } => {
            if ids.is_empty() {
                return invalid("nothing to key");
            }
            for id in ids {
                if scene.get(*id).is_none() {
                    return Err(SceneError::NoObject(*id).into());
                }
                for (i, path) in TRANSFORM_PATHS.iter().enumerate() {
                    if [*translate, *rotate, *scale].get(i / 3).copied().unwrap_or(false) {
                        scene.set_key(*id, path, frame.unwrap_or(now), None)?;
                    }
                }
            }
        }
        AnimCommand::RemoveKeys { keys } => {
            for (id, path, mut idx) in resolve(scene, keys)? {
                idx.sort_unstable();
                idx.dedup();
                let c = curve_mut(scene, id, &path)?;
                for i in idx.into_iter().rev() {
                    c.remove(i);
                }
            }
            scene.anim.prune();
        }
        AnimCommand::MoveKeys { keys, frames, value } => {
            if !frames.is_finite() || !value.is_finite() {
                return invalid("a move needs finite numbers");
            }
            for (id, path, idx) in resolve(scene, keys)? {
                curve_mut(scene, id, &path)?.move_keys(&idx, *frames, *value);
            }
        }
        AnimCommand::ScaleKeys { keys, pivot, time, value_pivot, value } => {
            if ![*pivot, *time, *value_pivot, *value].iter().all(|v| v.is_finite()) || time.abs() < 1e-6 {
                return invalid("a scale needs finite numbers and a time factor that is not zero");
            }
            for (id, path, idx) in resolve(scene, keys)? {
                curve_mut(scene, id, &path)?.scale_keys(&idx, *pivot, *time, *value_pivot, *value);
            }
        }
        AnimCommand::EditKey { key, value, interp, tangent, in_slope, out_slope, in_weight, out_weight } => {
            let numbers = [value, in_slope, out_slope, in_weight, out_weight];
            if numbers.iter().any(|n| n.is_some_and(|v| !v.is_finite())) {
                return invalid("a key needs finite numbers");
            }
            let c = curve_mut(scene, key.id, &key.path)?;
            let i = c.key_at(key.frame).ok_or_else(|| EngineError::Invalid(format!("no key at frame {}", key.frame)))?;
            let Some(k) = c.keys.get_mut(i) else { return invalid("no such key") };
            if let Some(v) = value {
                k.v = *v;
            }
            if let Some(i) = interp {
                k.interp = *i;
            }
            // Giving a slope by hand makes the tangent the user's, unless a mode is named too.
            match (tangent, in_slope, out_slope) {
                (Some(t), _, _) => k.tangent = *t,
                (None, Some(_), Some(_)) => k.tangent = Tangent::Broken,
                (None, Some(_), None) | (None, None, Some(_)) if !matches!(k.tangent, Tangent::Broken) => k.tangent = Tangent::Free,
                _ => {}
            }
            if let Some(s) = out_slope {
                k.out_slope = *s;
            }
            if let Some(s) = in_slope {
                k.in_slope = *s;
                if k.tangent == Tangent::Free && out_slope.is_none() {
                    k.out_slope = *s;
                }
            }
            if let Some(w) = in_weight {
                k.in_weight = w.clamp(0.0, 1.0);
            }
            if let Some(w) = out_weight {
                k.out_weight = w.clamp(0.0, 1.0);
            }
            c.retangent();
        }
        AnimCommand::SetKeyShape { keys, interp, tangent } => {
            for (id, path, idx) in resolve(scene, keys)? {
                let c = curve_mut(scene, id, &path)?;
                for i in idx {
                    if let Some(k) = c.keys.get_mut(i) {
                        k.interp = interp.unwrap_or(k.interp);
                        k.tangent = tangent.unwrap_or(k.tangent);
                    }
                }
                c.retangent();
            }
        }
        AnimCommand::SetExtrapolation { id, path, pre, post } => {
            let c = curve_mut(scene, *id, path)?;
            c.pre = pre.unwrap_or(c.pre);
            c.post = post.unwrap_or(c.post);
        }
        AnimCommand::RemoveTrack { id, path } => match path {
            Some(p) => {
                if !scene.anim.remove_track(*id, p) {
                    return invalid(format!("object {} has no curve on {p:?}", id.0));
                }
            }
            None => {
                if !scene.anim.is_animated(*id) {
                    return invalid(format!("object {} is not animated", id.0));
                }
                scene.anim.remove_object(*id);
            }
        },
        AnimCommand::BakeTrack { id, path, start, end, step } => {
            let (a, b) = (start.unwrap_or(scene.anim.time.start), end.unwrap_or(scene.anim.time.end));
            if !(a.is_finite() && b.is_finite() && step.is_finite()) || b < a || *step <= 0.0 {
                return invalid("bake needs start <= end and a positive step");
            }
            if (b - a) / step > 1_000_000.0 {
                return invalid("too many samples");
            }
            curve_mut(scene, *id, path)?.bake(clamp_frame(a), clamp_frame(b), *step);
        }
        AnimCommand::SimplifyTrack { id, path, tolerance } => {
            let removed = curve_mut(scene, *id, path)?.simplify(*tolerance);
            out.warnings.push(format!("{removed} keys removed"));
        }
        AnimCommand::SetTimeRange { start, end, fps } => {
            let mut t = scene.anim.time;
            t.start = start.unwrap_or(t.start);
            t.end = end.unwrap_or(t.end);
            t.fps = fps.unwrap_or(t.fps);
            if ![t.start, t.end, t.fps].iter().all(|v| v.is_finite()) || t.end < t.start || t.fps <= 0.0 {
                return invalid("the range needs start <= end and a positive frame rate");
            }
            scene.anim.time = t.sanitized();
        }
        AnimCommand::SetAutoKey { on } => scene.anim.auto_key = *on,
        AnimCommand::SetTime { .. } => return invalid("not a scene command"),
    }
    scene.apply_animation();
    Ok(())
}

/// The frame a key reference names, snapped to the key that is there.
pub fn key_ref(id: ObjectId, path: &str, frame: f64) -> KeyRef {
    KeyRef { id, path: path.to_string(), frame }
}

/// True when two frames name the same key.
pub fn same_frame(a: f64, b: f64) -> bool {
    (a - b).abs() < TIME_EPSILON
}
