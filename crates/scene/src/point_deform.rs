//! Deformers that live in a mesh's deformer stack: blend shapes, lattices and clusters.
//!
//! Like a skin, each is a child object of the mesh it deforms ([`crate::ObjectKind::PointDeformer`]).
//! The children of a mesh form its **deformer stack**: after the parametric deformers (bend,
//! twist…), the blend shapes, lattices, clusters and skins run in child order, each on the
//! result of the one before ([`crate::rig::apply_stack`]). Reordering the children reorders the
//! stack ([`Scene::move_in_stack`]); hiding one switches it off.
//!
//! - **Blend shapes**: named targets, each a sparse set of point offsets with optional
//!   in-between shapes, blended by a keyable weight (`target.<index>.weight`).
//! - **Lattice**: a box of control points around (part of) the mesh; moving a control point
//!   bends what is inside. The object's transform places the box (a unit cube) in the mesh's
//!   space; control points are keyable (`point.<index>.x`).
//! - **Cluster**: a weighted set of points that follows the cluster object's own transform.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use modelcraft_anim::deform::{self, MAX_LATTICE_DIVISIONS, MAX_SHAPE_FRAMES, ShapeFrame};
use serde::{Deserialize, Serialize};

use crate::{ObjectId, ObjectKind, Scene, SceneError};

/// Most targets of one blend-shape deformer.
pub const MAX_TARGETS: usize = 512;
/// Longest target name.
pub const MAX_NAME: usize = 64;
/// Lattices with more control points than this do not list them as attributes (they can still
/// be read, set and keyed by path).
pub const LISTED_POINTS: usize = 64;

fn is_false(b: &bool) -> bool {
    !*b
}
fn one() -> f32 {
    1.0
}

/// One shape of a target: where the listed points are, relative to the rest shape, when the
/// target's weight is `at`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    #[serde(default = "one")]
    pub at: f32,
    pub indices: Arc<Vec<u32>>,
    pub deltas: Arc<Vec<Vec3>>,
}

/// A blend-shape target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    #[serde(default)]
    pub weight: f32,
    /// The full shape (`at` 1) and any in-betweens.
    pub frames: Vec<Frame>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PointDeformer {
    BlendShapes {
        targets: Vec<Target>,
    },
    Lattice {
        /// Cells along X, Y and Z (1 to 8 each).
        divisions: [u32; 3],
        /// Interpolate cell by cell instead of as one Bézier volume.
        #[serde(default, skip_serializing_if = "is_false")]
        linear: bool,
        /// Control points in the lattice's space (rest: a grid over the cube −0.5..0.5).
        points: Arc<Vec<Vec3>>,
    },
    Cluster {
        points: Arc<Vec<u32>>,
        weights: Arc<Vec<f32>>,
        /// The inverse of the cluster object's matrix when it was made: where it started.
        bind: Mat4,
    },
}

impl PointDeformer {
    pub fn type_name(&self) -> &'static str {
        match self {
            PointDeformer::BlendShapes { .. } => "Blend shapes",
            PointDeformer::Lattice { .. } => "Lattice",
            PointDeformer::Cluster { .. } => "Cluster",
        }
    }

    /// An undeformed lattice.
    pub fn lattice(divisions: [u32; 3], linear: bool) -> PointDeformer {
        let divisions = divisions.map(|d| d.clamp(1, MAX_LATTICE_DIVISIONS));
        PointDeformer::Lattice { divisions, linear, points: Arc::new(deform::lattice_rest(divisions)) }
    }

    pub fn heap_bytes(&self) -> usize {
        match self {
            PointDeformer::BlendShapes { targets } => {
                targets.iter().map(|t| 64 + t.name.len() + t.frames.iter().map(|f| f.indices.len() * 4 + f.deltas.len() * 12).sum::<usize>()).sum()
            }
            PointDeformer::Lattice { points, .. } => points.len() * 12,
            PointDeformer::Cluster { points, weights, .. } => points.len() * 4 + weights.len() * 4,
        }
    }

    /// Repair a deformer read from a file.
    pub fn sanitize(&mut self) {
        match self {
            PointDeformer::BlendShapes { targets } => {
                targets.truncate(MAX_TARGETS);
                for t in targets {
                    t.name = t.name.chars().take(MAX_NAME).collect();
                    t.weight = if t.weight.is_finite() { t.weight.clamp(-10.0, 10.0) } else { 0.0 };
                    t.frames.truncate(MAX_SHAPE_FRAMES);
                    t.frames.retain(|f| f.at.is_finite() && f.at > 1e-6 && f.indices.len() == f.deltas.len());
                    for f in &mut t.frames {
                        if f.deltas.iter().any(|d| !d.is_finite()) {
                            for d in Arc::make_mut(&mut f.deltas) {
                                if !d.is_finite() {
                                    *d = Vec3::ZERO;
                                }
                            }
                        }
                    }
                }
            }
            PointDeformer::Lattice { divisions, points, .. } => {
                *divisions = divisions.map(|d| d.clamp(1, MAX_LATTICE_DIVISIONS));
                let rest = deform::lattice_rest(*divisions);
                if points.len() != rest.len() {
                    *points = Arc::new(rest);
                } else if points.iter().any(|p| !p.is_finite()) {
                    for (p, r) in Arc::make_mut(points).iter_mut().zip(rest) {
                        if !p.is_finite() {
                            *p = r;
                        }
                    }
                }
            }
            PointDeformer::Cluster { points, weights, bind } => {
                if weights.len() != points.len() {
                    Arc::make_mut(weights).resize(points.len(), 1.0);
                }
                if weights.iter().any(|w| !w.is_finite() || !(0.0..=1.0).contains(w)) {
                    for w in Arc::make_mut(weights) {
                        *w = if w.is_finite() { w.clamp(0.0, 1.0) } else { 0.0 };
                    }
                }
                if !bind.is_finite() {
                    *bind = Mat4::IDENTITY;
                }
            }
        }
    }

    /// Deform `points` (in the mesh's space); `local` is the deformer object's own matrix.
    /// Returns a message when something could not be applied.
    pub fn apply(&self, points: &mut [Vec3], local: Mat4) -> Option<String> {
        match self {
            PointDeformer::BlendShapes { targets } => {
                let mut stale = 0;
                for t in targets.iter().filter(|t| t.weight != 0.0) {
                    if t.frames.iter().any(|f| f.indices.iter().any(|i| *i as usize >= points.len())) {
                        stale += 1;
                        continue;
                    }
                    let frames: Vec<ShapeFrame<'_>> = t.frames.iter().map(|f| ShapeFrame { at: f.at, indices: &f.indices, deltas: &f.deltas }).collect();
                    deform::morph(points, &frames, t.weight);
                }
                (stale > 0).then(|| format!("{stale} blend shapes were made for a mesh with more points: make them again"))
            }
            PointDeformer::Lattice { divisions, linear, points: control } => {
                let to_local = local.inverse();
                if !local.is_finite() || !to_local.is_finite() {
                    return Some("the lattice has no size".into());
                }
                for p in points.iter_mut() {
                    let offset = deform::ffd_offset(to_local.transform_point3(*p), *divisions, control, *linear);
                    if offset != Vec3::ZERO {
                        let moved = *p + local.transform_vector3(offset);
                        if moved.is_finite() {
                            *p = moved;
                        }
                    }
                }
                None
            }
            PointDeformer::Cluster { points: indices, weights, bind } => {
                deform::cluster(points, indices, weights, local * *bind);
                indices.iter().any(|i| *i as usize >= points.len()).then(|| "the cluster holds points the mesh no longer has: make it again".into())
            }
        }
    }
}

impl Scene {
    /// The mesh a stack deformer (or a skin) deforms: its parent, when that is a mesh.
    pub fn deformed_mesh(&self, deformer: ObjectId) -> Option<ObjectId> {
        let p = self.get(deformer)?.parent?;
        matches!(self.get(p)?.kind, ObjectKind::Mesh { .. }).then_some(p)
    }

    /// Whether an object takes part in its parent's deformer stack.
    pub fn in_stack(&self, id: ObjectId) -> bool {
        self.get(id).is_some_and(|o| matches!(o.kind, ObjectKind::PointDeformer { .. } | ObjectKind::Skin { .. } | ObjectKind::Deformer { .. }))
    }

    /// Move an object `delta` places among its siblings (negative: earlier). For a deformer
    /// this reorders the stack. Returns whether anything moved.
    pub fn move_in_stack(&mut self, id: ObjectId, delta: i32) -> Result<bool, SceneError> {
        let parent = self.get(id).ok_or(SceneError::NoObject(id))?.parent;
        let siblings: Vec<usize> = self.objects.iter().enumerate().filter(|(_, o)| o.parent == parent).map(|(i, _)| i).collect();
        let Some(at) = siblings.iter().position(|i| self.objects.get(*i).is_some_and(|o| o.id == id)) else { return Ok(false) };
        let to = (at as i64 + delta as i64).clamp(0, siblings.len() as i64 - 1) as usize;
        if to == at {
            return Ok(false);
        }
        // Slide the siblings between the two places by one; other objects keep their slots.
        let step: i64 = if to > at { 1 } else { -1 };
        let mut i = at as i64;
        while i != to as i64 {
            if let (Some(a), Some(b)) = (siblings.get(i as usize), siblings.get((i + step) as usize)) {
                self.objects.swap(*a, *b);
            }
            i += step;
        }
        let _ = self.edit(id);
        if let Some(p) = parent {
            let _ = self.edit(p);
        }
        Ok(true)
    }
}

/// Repair the stack deformers of a scene read from a file.
pub(crate) fn repair(scene: &mut Scene) {
    let ids: Vec<ObjectId> = scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::PointDeformer { .. })).map(|o| o.id).collect();
    for id in ids {
        if let Ok(o) = scene.edit(id)
            && let ObjectKind::PointDeformer { deformer } = &mut o.kind
        {
            deformer.sanitize();
        }
    }
}

/// Read `target.<i>.weight`, `point.<i>.<axis>` or `linear`.
pub(crate) fn attr_get(d: &PointDeformer, path: &str) -> Option<f64> {
    let mut parts = path.split('.');
    match (d, parts.next()?, parts.next(), parts.next(), parts.next()) {
        (PointDeformer::BlendShapes { targets }, "target", Some(i), Some("weight"), None) => targets.get(i.parse::<usize>().ok()?).map(|t| t.weight as f64),
        (PointDeformer::Lattice { points, .. }, "point", Some(i), Some(axis), None) => {
            let p = points.get(i.parse::<usize>().ok()?)?;
            ["x", "y", "z"].iter().position(|a| *a == axis).and_then(|a| p.to_array().get(a).copied()).map(f64::from)
        }
        (PointDeformer::Lattice { linear, .. }, "linear", None, None, None) => Some(if *linear { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Write one of the paths [`attr_get`] reads; `Some(changed)` when the path exists.
pub(crate) fn attr_set(d: &mut PointDeformer, path: &str, value: f64) -> Option<bool> {
    let mut parts = path.split('.');
    let v = value as f32;
    match (d, parts.next()?, parts.next(), parts.next(), parts.next()) {
        (PointDeformer::BlendShapes { targets }, "target", Some(i), Some("weight"), None) => {
            let t = targets.get_mut(i.parse::<usize>().ok()?)?;
            let v = v.clamp(-10.0, 10.0);
            Some(std::mem::replace(&mut t.weight, v) != v)
        }
        (PointDeformer::Lattice { points, .. }, "point", Some(i), Some(axis), None) => {
            let i = i.parse::<usize>().ok()?;
            let a = ["x", "y", "z"].iter().position(|a| *a == axis)?;
            let mut now = points.get(i)?.to_array();
            let slot = now.get_mut(a)?;
            if *slot == v {
                return Some(false);
            }
            *slot = v;
            if let Some(p) = Arc::make_mut(points).get_mut(i) {
                *p = Vec3::from_array(now);
            }
            Some(true)
        }
        (PointDeformer::Lattice { linear, .. }, "linear", None, None, None) => Some(std::mem::replace(linear, value >= 0.5) != (value >= 0.5)),
        _ => None,
    }
}

/// The attribute paths of a stack deformer with their values.
pub(crate) fn attr_list(d: &PointDeformer, out: &mut Vec<(String, f64)>) {
    match d {
        PointDeformer::BlendShapes { targets } => out.extend(targets.iter().enumerate().map(|(i, t)| (format!("target.{i}.weight"), t.weight as f64))),
        PointDeformer::Lattice { points, linear, .. } => {
            out.push(("linear".into(), if *linear { 1.0 } else { 0.0 }));
            if points.len() <= LISTED_POINTS {
                for (i, p) in points.iter().enumerate() {
                    for (axis, v) in ["x", "y", "z"].iter().zip(p.to_array()) {
                        out.push((format!("point.{i}.{axis}"), v as f64));
                    }
                }
            }
        }
        PointDeformer::Cluster { .. } => {}
    }
}
