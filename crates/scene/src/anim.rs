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

/// A curve driving one attribute of one object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub target: ObjectId,
    /// Attribute path ([`crate::attr`]).
    pub path: String,
    pub curve: Arc<Curve>,
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
    pub fn track(&self, id: ObjectId, path: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.target == id && t.path == path)
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

    /// The curve of an attribute, created empty when missing.
    pub fn curve_mut(&mut self, id: ObjectId, path: &str) -> Result<&mut Curve, SceneError> {
        let i = match self.tracks.iter().position(|t| t.target == id && t.path == path) {
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
                self.tracks.insert(at, Track { target: id, path: path.to_string(), curve: Arc::new(Curve::new()) });
                at
            }
        };
        match self.tracks.get_mut(i) {
            Some(t) => Ok(Arc::make_mut(&mut t.curve)),
            None => Err(SceneError::Invalid("no such track".into())),
        }
    }

    /// Remove a track; `true` when it existed.
    pub fn remove_track(&mut self, id: ObjectId, path: &str) -> bool {
        let n = self.tracks.len();
        self.tracks.retain(|t| !(t.target == id && t.path == path));
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
        let mut seen = std::collections::HashSet::new();
        self.tracks.retain(|t| exists(t.target) && !t.path.is_empty() && t.path.len() <= attr::MAX_PATH && seen.insert((t.target, t.path.clone())));
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
        attr::get(self.get(id)?, path)
    }

    /// Set a numeric attribute; the object's revision is bumped only when the value changed.
    pub fn set_attr(&mut self, id: ObjectId, path: &str, value: f64) -> Result<bool, SceneError> {
        let o = self.objects.iter_mut().find(|o| o.id == id).ok_or(SceneError::NoObject(id))?;
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
    }

    /// Write the value of every track at the current frame into its attribute. Tracks whose
    /// attribute no longer exists are skipped. Returns how many objects changed.
    pub fn apply_animation(&mut self) -> usize {
        if self.anim.tracks.is_empty() {
            return 0;
        }
        let frame = self.anim.frame;
        // Values per object; rotation is set as one triple.
        let mut per: BTreeMap<ObjectId, Vec<(&str, f64)>> = BTreeMap::new();
        for t in &self.anim.tracks {
            if let Some(v) = t.curve.eval(frame) {
                per.entry(t.target).or_default().push((t.path.as_str(), v));
            }
        }
        let mut changed = 0;
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

    /// Key an attribute at `frame` with `value` (the attribute's current value when `None`).
    pub fn set_key(&mut self, id: ObjectId, path: &str, frame: f64, value: Option<f64>) -> Result<(), SceneError> {
        let current = self.attr(id, path).ok_or_else(|| SceneError::Invalid(format!("object {} has no numeric attribute {path:?}", id.0)))?;
        let mut v = value.unwrap_or(current);
        if !frame.is_finite() || !v.is_finite() {
            return Err(SceneError::Invalid("a key needs a finite frame and value".into()));
        }
        if value.is_none()
            && let Some(prev) = ROTATE_PATHS.iter().position(|p| *p == path).and_then(|_| self.anim.curve(id, path)).and_then(|c| nearest_key_value(c, frame))
        {
            v = unwrap_angle(v, prev);
        }
        self.anim.curve_mut(id, path)?.set_key(clamp_frame(frame), v).ok_or_else(|| SceneError::Invalid("the curve is full".into()))?;
        Ok(())
    }
}

/// The value of the key nearest to `frame`.
fn nearest_key_value(c: &Curve, frame: f64) -> Option<f64> {
    c.keys.iter().min_by(|a, b| (a.t - frame).abs().total_cmp(&(b.t - frame).abs())).map(|k| k.v)
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
        for t in after.anim.tracks_of(o.id) {
            if TRANSFORM_PATHS.contains(&t.path.as_str()) {
                continue;
            }
            if let (Some(now), Some(was)) = (attr::get(o, &t.path), attr::get(old, &t.path))
                && (now - was).abs() > 1e-9
            {
                keys.push((o.id, t.path.clone()));
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
            steps.push(Step { id: sid, enabled: true, op: StepOp::Extrude { faces: vec![0, 1, 2, 3, 4, 5], distance: 1.0 } });
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
        let d = s.add("Bend", ObjectKind::Deformer { deformer: Deformer::Bend { angle: 90.0 } }, Some(id)).unwrap();
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
        // A scene without animation saves no animation block, and old files still load.
        let plain = Scene::new();
        assert!(!serde_json::to_string(&plain).unwrap().contains("anim"));
        assert!(serde_json::from_str::<Scene>(r#"{"objects":[]}"#).unwrap().anim.is_default());
    }
}
