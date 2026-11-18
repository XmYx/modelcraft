//! The deformer-stack operations of the `rig` command: blend shapes, lattices, clusters and
//! their order ([`modelcraft_scene::point_deform`]).

use std::sync::Arc;

use glam::{Mat4, Vec3};
use modelcraft_anim::deform::{self, MAX_LATTICE_DIVISIONS, MAX_SHAPE_FRAMES};
use modelcraft_scene::point_deform::{Frame, MAX_NAME, MAX_TARGETS, PointDeformer, Target};
use modelcraft_scene::{ObjectId, ObjectKind, SceneError, Transform};

use crate::command::Outcome;
use crate::document::{Document, EngineError};
use crate::rig::{RigCommand, invalid};

/// Offsets smaller than this are not stored in a blend shape.
const SHAPE_TOLERANCE: f32 = 1e-6;

fn need_mesh(doc: &Document, id: ObjectId) -> Result<(), EngineError> {
    match doc.scene.get(id).map(|o| &o.kind) {
        Some(ObjectKind::Mesh { .. }) => Ok(()),
        Some(_) => invalid(format!("object {} is not a mesh", id.0)),
        None => Err(SceneError::NoObject(id).into()),
    }
}

/// The mesh's points before its deformer stack runs (after its build history).
fn rest_points(doc: &mut Document, mesh: ObjectId) -> Vec<Vec3> {
    doc.eval.steps_output(&doc.scene, mesh, usize::MAX).mesh.positions().to_vec()
}

/// The blend-shape deformer of a mesh: its first such child.
fn shapes_of(doc: &Document, mesh: ObjectId) -> Option<ObjectId> {
    doc.scene
        .objects()
        .iter()
        .find(|o| o.parent == Some(mesh) && matches!(o.kind, ObjectKind::PointDeformer { deformer: PointDeformer::BlendShapes { .. } }))
        .map(|o| o.id)
}

fn targets_mut(doc: &mut Document, mesh: ObjectId) -> Result<&mut Vec<Target>, EngineError> {
    let id = shapes_of(doc, mesh).ok_or_else(|| EngineError::Invalid(format!("object {} has no blend shapes", mesh.0)))?;
    match &mut doc.scene.edit(id)?.kind {
        ObjectKind::PointDeformer { deformer: PointDeformer::BlendShapes { targets } } => Ok(targets),
        _ => invalid("not a blend-shape deformer"),
    }
}

fn clean_name(name: &Option<String>, fallback: &str) -> String {
    name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(fallback).chars().take(MAX_NAME).collect()
}

pub fn apply(doc: &mut Document, cmd: &RigCommand, out: &mut Outcome) -> Result<(), EngineError> {
    match cmd {
        RigCommand::AddShape { mesh, from, name, target, at } => {
            need_mesh(doc, *mesh)?;
            if mesh == from {
                return invalid("a blend shape comes from another object with the same points");
            }
            if doc.scene.get(*from).is_none() {
                return Err(SceneError::NoObject(*from).into());
            }
            if !at.is_finite() || *at <= 1e-3 || *at > 10.0 {
                return invalid("an in-between sits at a weight above 0");
            }
            let base = rest_points(doc, *mesh);
            let shape = doc.eval.evaluate(&doc.scene, *from).mesh.positions().to_vec();
            let Some((indices, deltas)) = deform::shape_delta(&base, &shape, SHAPE_TOLERANCE) else {
                return invalid(format!("the shape has {} points but the mesh has {}: a blend shape needs the same points", shape.len(), base.len()));
            };
            let moved = indices.len();
            let frame = Frame { at: *at, indices: Arc::new(indices), deltas: Arc::new(deltas) };
            let fallback = doc.scene.get(*from).map(|o| o.name.clone()).unwrap_or_else(|| "Shape".into());
            if shapes_of(doc, *mesh).is_none() {
                if target.is_some() {
                    return invalid("the mesh has no blend shapes to add an in-between to");
                }
                // Shapes belong before skins and lattices: first in the stack.
                let id = doc.scene.add(
                    doc.scene.unique_name("Blend shapes"),
                    ObjectKind::PointDeformer { deformer: PointDeformer::BlendShapes { targets: Vec::new() } },
                    Some(*mesh),
                )?;
                doc.scene.move_in_stack(id, i32::MIN)?;
                out.created.push(id);
            }
            let targets = targets_mut(doc, *mesh)?;
            match target {
                Some(t) => {
                    let slot = targets.get_mut(*t).ok_or_else(|| EngineError::Invalid(format!("no blend shape {t}")))?;
                    if slot.frames.len() >= MAX_SHAPE_FRAMES {
                        return invalid(format!("a blend shape holds at most {MAX_SHAPE_FRAMES} shapes"));
                    }
                    // A shape at the same weight replaces the one there.
                    slot.frames.retain(|f| (f.at - *at).abs() > 1e-4);
                    slot.frames.push(frame);
                    slot.frames.sort_by(|a, b| a.at.total_cmp(&b.at));
                }
                None => {
                    if targets.len() >= MAX_TARGETS {
                        return invalid(format!("a mesh holds at most {MAX_TARGETS} blend shapes"));
                    }
                    targets.push(Target { name: clean_name(name, &fallback), weight: 0.0, frames: vec![frame] });
                }
            }
            out.warnings.push(format!("{moved} points move in the shape"));
        }
        RigCommand::SetShape { mesh, target, weight, name } => {
            need_mesh(doc, *mesh)?;
            if weight.is_some_and(|w| !w.is_finite()) {
                return invalid("a weight is a number");
            }
            let t = targets_mut(doc, *mesh)?.get_mut(*target).ok_or_else(|| EngineError::Invalid(format!("no blend shape {target}")))?;
            if let Some(w) = weight {
                t.weight = w.clamp(-10.0, 10.0);
            }
            if name.is_some() {
                t.name = clean_name(name, &t.name.clone());
            }
        }
        RigCommand::RemoveShape { mesh, target, frame } => {
            need_mesh(doc, *mesh)?;
            let targets = targets_mut(doc, *mesh)?;
            let t = targets.get_mut(*target).ok_or_else(|| EngineError::Invalid(format!("no blend shape {target}")))?;
            match frame {
                Some(f) if t.frames.len() > 1 => {
                    if *f >= t.frames.len() {
                        return invalid(format!("the blend shape has no shape {f}"));
                    }
                    t.frames.remove(*f);
                }
                _ => {
                    targets.remove(*target);
                }
            }
            // Curves on the shapes after it would now drive the wrong shape: a removed shape
            // takes its own curve along and the later ones move up.
            if frame.is_none()
                && let Some(id) = shapes_of(doc, *mesh)
            {
                renumber_weight_tracks(doc, id, *target);
            }
        }
        RigCommand::AddLattice { mesh, divisions, linear } => {
            need_mesh(doc, *mesh)?;
            if divisions.iter().any(|d| *d == 0 || *d > MAX_LATTICE_DIVISIONS) {
                return invalid(format!("a lattice has 1 to {MAX_LATTICE_DIVISIONS} cells along each axis"));
            }
            // Fit the box to the mesh as it is deformed so far, a little larger so that points on
            // the surface are inside.
            let points = doc.eval.evaluate(&doc.scene, *mesh).mesh.positions().to_vec();
            let (lo, hi) = points.iter().filter(|p| p.is_finite()).fold((Vec3::MAX, Vec3::MIN), |(a, b), p| (a.min(*p), b.max(*p)));
            let (center, size) = if lo.cmple(hi).all() { ((lo + hi) * 0.5, ((hi - lo) * 1.02).max(Vec3::splat(1e-3))) } else { (Vec3::ZERO, Vec3::ONE) };
            let id = doc.scene.add(
                doc.scene.unique_name("Lattice"),
                ObjectKind::PointDeformer { deformer: PointDeformer::lattice(*divisions, *linear) },
                Some(*mesh),
            )?;
            doc.scene.edit(id)?.transform = Transform { translation: center, scale: size, ..Transform::default() };
            out.created.push(id);
        }
        RigCommand::EditLattice { id, points, translate, to, reset, linear } => {
            let ObjectKind::PointDeformer { deformer: PointDeformer::Lattice { divisions, points: control, linear: is_linear } } =
                &mut doc.scene.edit(*id)?.kind
            else {
                return invalid(format!("object {} is not a lattice", id.0));
            };
            for v in [translate, to].into_iter().flatten() {
                if !Vec3::from_array(*v).is_finite() {
                    return invalid("a lattice point needs finite numbers");
                }
            }
            if translate.is_some() && to.is_some() {
                return invalid("give translate or to, not both");
            }
            if points.iter().any(|p| *p as usize >= control.len()) {
                return invalid(format!("the lattice has {} points", control.len()));
            }
            if *reset {
                *control = Arc::new(deform::lattice_rest(*divisions));
            }
            if let Some(l) = linear {
                *is_linear = *l;
            }
            if translate.is_some() || to.is_some() {
                let list = Arc::make_mut(control);
                for p in points {
                    if let Some(slot) = list.get_mut(*p as usize) {
                        *slot = match (translate, to) {
                            (Some(t), _) => *slot + Vec3::from_array(*t),
                            (_, Some(t)) => Vec3::from_array(*t),
                            _ => *slot,
                        };
                    }
                }
            }
        }
        RigCommand::AddCluster { mesh, points, weights, radius, center, name } => {
            need_mesh(doc, *mesh)?;
            // The points as the stack has deformed them so far: what is on screen.
            let now = doc.eval.evaluate(&doc.scene, *mesh).mesh.positions().to_vec();
            if points.iter().any(|p| *p as usize >= now.len()) {
                return invalid(format!("the mesh has {} points", now.len()));
            }
            if !weights.is_empty() && weights.len() != points.len() || weights.iter().any(|w| !w.is_finite()) {
                return invalid("give one weight per point, or none");
            }
            let middle = match center {
                Some(c) => Vec3::from_array(*c),
                None if !points.is_empty() => points.iter().filter_map(|p| now.get(*p as usize)).sum::<Vec3>() / points.len() as f32,
                None => return invalid("a cluster needs points or a centre"),
            };
            if !middle.is_finite() {
                return invalid("a cluster needs a finite centre");
            }
            let mut members: std::collections::BTreeMap<u32, f32> = std::collections::BTreeMap::new();
            if let Some(r) = radius {
                if !r.is_finite() || *r <= 0.0 {
                    return invalid("the radius is above 0");
                }
                let (i, w) = deform::radial_weights(&now, middle, *r);
                members.extend(i.into_iter().zip(w));
            }
            for (n, p) in points.iter().enumerate() {
                members.insert(*p, weights.get(n).copied().unwrap_or(1.0).clamp(0.0, 1.0));
            }
            members.retain(|_, w| *w > 0.0);
            if members.is_empty() {
                return invalid("no points are within the cluster");
            }
            let count = members.len();
            let handle = Transform { translation: middle, ..Transform::default() };
            let deformer = PointDeformer::Cluster {
                points: Arc::new(members.keys().copied().collect()),
                weights: Arc::new(members.values().copied().collect()),
                bind: Mat4::from_translation(-middle),
            };
            let id = doc.scene.add(doc.scene.unique_name(&clean_name(name, "Cluster")), ObjectKind::PointDeformer { deformer }, Some(*mesh))?;
            doc.scene.edit(id)?.transform = handle;
            out.created.push(id);
            out.warnings.push(format!("{count} points in the cluster"));
        }
        RigCommand::SetClusterWeights { id, points, weight } => {
            if !weight.is_finite() {
                return invalid("a weight is a number from 0 to 1");
            }
            let ObjectKind::PointDeformer { deformer: PointDeformer::Cluster { points: members, weights, .. } } = &mut doc.scene.edit(*id)?.kind else {
                return invalid(format!("object {} is not a cluster", id.0));
            };
            let mut map: std::collections::BTreeMap<u32, f32> = members.iter().copied().zip(weights.iter().copied()).collect();
            for p in points {
                map.insert(*p, weight.clamp(0.0, 1.0));
            }
            map.retain(|_, w| *w > 0.0);
            *members = Arc::new(map.keys().copied().collect());
            *weights = Arc::new(map.values().copied().collect());
        }
        RigCommand::MoveInStack { id, delta } => {
            if !doc.scene.in_stack(*id) {
                return invalid(format!("object {} is not a deformer", id.0));
            }
            if !doc.scene.move_in_stack(*id, *delta)? {
                out.warnings.push("already at the end of the stack".into());
            }
        }
        _ => return invalid("not a deformer operation"),
    }
    Ok(())
}

/// After blend shape `removed` is gone: drop its weight curve and move the later ones down.
fn renumber_weight_tracks(doc: &mut Document, shapes: ObjectId, removed: usize) {
    let paths: Vec<String> = doc.scene.anim.tracks_of(shapes).map(|t| t.path.clone()).collect();
    let index = |p: &str| p.strip_prefix("target.").and_then(|r| r.strip_suffix(".weight")).and_then(|i| i.parse::<usize>().ok());
    let mut later: Vec<(usize, String)> = paths.iter().filter_map(|p| index(p).map(|i| (i, p.clone()))).filter(|(i, _)| *i >= removed).collect();
    later.sort();
    later.dedup();
    for (i, path) in later {
        let moved: Vec<modelcraft_scene::Track> = doc.scene.anim.tracks_of(shapes).filter(|t| t.path == path).cloned().collect();
        doc.scene.anim.remove_path(shapes, &path);
        if i == removed {
            continue;
        }
        for t in moved {
            if let Ok(curve) = doc.scene.anim.curve_mut_in(shapes, &format!("target.{}.weight", i - 1), t.layer) {
                *curve = (*t.curve).clone();
            }
        }
    }
}
