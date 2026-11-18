//! The constraint operations of the `rig` command: constraints, IK, driven keys, expressions,
//! IK/FK matching and baking ([`modelcraft_scene::constraint`]).

use std::sync::Arc;

use glam::Vec3;
use modelcraft_anim::Curve;
use modelcraft_anim::expr::Expr;
use modelcraft_scene::anim::{BASE_LAYER, TRANSFORM_PATHS, unwrap_angle};
use modelcraft_scene::constraint::{Constraint, ConstraintKind, MAX_SOURCES, Source};
use modelcraft_scene::{ObjectId, ObjectKind, SceneError};

use crate::command::Outcome;
use crate::document::{Document, EngineError};
use crate::rig::{RigCommand, axis_index, invalid};

/// Most frames one bake samples.
pub const MAX_BAKE_FRAMES: usize = 100_000;

fn need(doc: &Document, id: ObjectId) -> Result<(), EngineError> {
    if doc.scene.get(id).is_some() { Ok(()) } else { Err(SceneError::NoObject(id).into()) }
}

fn constraint(doc: &Document, id: u32) -> Result<Constraint, EngineError> {
    doc.scene.constraints.get(id).cloned().ok_or_else(|| EngineError::Invalid(format!("no constraint {id}")))
}

fn position(doc: &Document, id: ObjectId) -> Vec3 {
    doc.scene.world_matrix(id).transform_point3(Vec3::ZERO)
}

/// Put an object at a world position by changing its own translation.
fn place(doc: &mut Document, id: ObjectId, world: Vec3) -> Result<(), EngineError> {
    let parent = doc.scene.get(id).and_then(|o| o.parent).map_or(glam::Mat4::IDENTITY, |p| doc.scene.world_matrix(p));
    let local = parent.inverse().transform_point3(world);
    if !local.is_finite() {
        return invalid("the object cannot be placed under a parent with zero scale");
    }
    doc.scene.edit(id)?.transform.translation = local;
    Ok(())
}

/// Report what the solve skipped as warnings of the command that caused it.
fn report(doc: &mut Document, out: &mut Outcome) {
    doc.scene.solve_constraints();
    out.warnings.extend(doc.scene.constraints.problems().iter().map(|p| p.message.clone()));
}

pub fn apply(doc: &mut Document, cmd: &RigCommand, out: &mut Outcome) -> Result<(), EngineError> {
    match cmd {
        RigCommand::AddConstraint { kind, target, sources, weights, maintain_offset, aim, up, up_object, name } => {
            need(doc, *target)?;
            if sources.is_empty() || sources.len() > MAX_SOURCES {
                return invalid(format!("a constraint follows 1 to {MAX_SOURCES} objects"));
            }
            if !weights.is_empty() && weights.len() != sources.len() {
                return invalid("give one weight per source, or none");
            }
            let mut list = Vec::new();
            for (i, s) in sources.iter().enumerate() {
                need(doc, *s)?;
                if s == target {
                    return invalid("an object cannot be constrained to itself");
                }
                let weight = weights.get(i).copied().unwrap_or(1.0);
                if !weight.is_finite() || weight < 0.0 {
                    return invalid("a source weight is zero or more");
                }
                list.push(Source { weight, ..Source::new(*s) });
            }
            let mut k = match kind.as_str() {
                "point" => ConstraintKind::Point { sources: list },
                "orient" => ConstraintKind::Orient { sources: list },
                "scale" => ConstraintKind::Scale { sources: list },
                "parent" => ConstraintKind::Parent { sources: list },
                "aim" => {
                    let (aim, up) = (axis_index(aim)?, axis_index(up)?);
                    if aim == up {
                        return invalid("the aim and up axes must differ");
                    }
                    if let Some(u) = up_object {
                        need(doc, *u)?;
                    }
                    ConstraintKind::Aim { sources: list, aim: aim as u8, up: up as u8, up_object: *up_object, up_vector: Vec3::Y, offset: glam::Quat::IDENTITY }
                }
                other => return invalid(format!("no constraint kind {other:?}: point, orient, scale, parent or aim")),
            };
            if *maintain_offset {
                doc.scene.keep_offsets(*target, &mut k);
            }
            let label = name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| format!("{} constraint", k.type_name()));
            doc.scene.constraints.add(&label, *target, k)?;
            let _ = doc.scene.edit(*target)?;
            report(doc, out);
        }
        RigCommand::AddIk { end, goal, pole, bones, solver, name } => {
            need(doc, *end)?;
            let chain = doc.scene.ik_chain(*end, *bones);
            if chain.len() < 2 {
                return invalid("the end joint needs a parent joint to turn");
            }
            if let Some(p) = pole {
                need(doc, *p)?;
            }
            let goal = match goal {
                Some(g) => {
                    need(doc, *g)?;
                    if chain.contains(g) || doc.scene.subtree(*chain.first().unwrap_or(end)).contains(g) {
                        return invalid("the goal cannot be part of the chain it moves");
                    }
                    *g
                }
                None => {
                    let at = position(doc, *end);
                    let base = doc.scene.get(*end).map(|o| format!("{} goal", o.name)).unwrap_or_else(|| "IK goal".into());
                    let id = doc.scene.add(doc.scene.unique_name(&base), ObjectKind::Group, None)?;
                    doc.scene.edit(id)?.transform.translation = at;
                    out.created.push(id);
                    id
                }
            };
            let k = ConstraintKind::Ik { goal, pole: *pole, bones: (chain.len() - 1) as u32, solver: *solver, iterations: 24, tolerance: 1e-4 };
            let label = name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("IK");
            doc.scene.constraints.add(label, *end, k)?;
            let _ = doc.scene.edit(*end)?;
            report(doc, out);
        }
        RigCommand::SetDrivenKey { driver, driver_path, target, path, driver_value, value } => {
            let input = match driver_value {
                Some(v) => *v,
                None => doc
                    .scene
                    .attr(*driver, driver_path)
                    .ok_or_else(|| EngineError::Invalid(format!("object {} has no numeric attribute {driver_path:?}", driver.0)))?,
            };
            let output = match value {
                Some(v) => *v,
                None => doc.scene.attr(*target, path).ok_or_else(|| EngineError::Invalid(format!("object {} has no numeric attribute {path:?}", target.0)))?,
            };
            if doc.scene.attr(*driver, driver_path).is_none() || doc.scene.attr(*target, path).is_none() {
                return invalid("a driven key needs two numeric attributes");
            }
            if !input.is_finite() || !output.is_finite() {
                return invalid("a driven key needs finite values");
            }
            let existing = doc.scene.constraints.list().iter().find(|c| {
                c.target == *target
                    && matches!(&c.kind, ConstraintKind::Driven { path: p, driver: d, driver_path: dp, .. } if p == path && d == driver && dp == driver_path)
            });
            match existing.map(|c| c.id) {
                Some(id) => {
                    if let Some(ConstraintKind::Driven { curve, .. }) = doc.scene.constraints.get_mut(id).map(|c| &mut c.kind) {
                        Arc::make_mut(curve).set_key(input, output).ok_or_else(|| EngineError::Invalid("the curve is full".into()))?;
                    }
                }
                None => {
                    let curve = Arc::new(Curve::from_points(&[(input, output)]));
                    let k = ConstraintKind::Driven { path: path.clone(), driver: *driver, driver_path: driver_path.clone(), curve };
                    doc.scene.constraints.add(&format!("{path} driven by {driver_path}"), *target, k)?;
                }
            }
            let _ = doc.scene.edit(*target)?;
            report(doc, out);
        }
        RigCommand::SetExpression { target, path, expr } => {
            need(doc, *target)?;
            let existing = doc
                .scene
                .constraints
                .list()
                .iter()
                .find(|c| c.target == *target && matches!(&c.kind, ConstraintKind::Expression { path: p, .. } if p == path))
                .map(|c| c.id);
            if expr.trim().is_empty() {
                let id = existing.ok_or_else(|| EngineError::Invalid(format!("{path:?} has no expression")))?;
                doc.scene.constraints.remove(id);
            } else {
                if doc.scene.attr(*target, path).is_none() {
                    return invalid(format!("object {} has no numeric attribute {path:?}", target.0));
                }
                Expr::parse(expr).map_err(|e| EngineError::Invalid(e.to_string()))?;
                match existing {
                    Some(id) => {
                        if let Some(ConstraintKind::Expression { expr: e, .. }) = doc.scene.constraints.get_mut(id).map(|c| &mut c.kind) {
                            *e = expr.clone();
                        }
                    }
                    None => {
                        doc.scene.constraints.add(
                            &format!("{path} expression"),
                            *target,
                            ConstraintKind::Expression { path: path.clone(), expr: expr.clone() },
                        )?;
                    }
                }
            }
            let _ = doc.scene.edit(*target)?;
            report(doc, out);
        }
        RigCommand::SetConstraint { id, weight, enabled, name, source_weights, solver, iterations, pole } => {
            let mut c = constraint(doc, *id)?;
            if let Some(w) = weight {
                if !w.is_finite() {
                    return invalid("a weight is a number from 0 to 1");
                }
                c.weight = w.clamp(0.0, 1.0);
            }
            if let Some(e) = enabled {
                c.enabled = *e;
            }
            if let Some(n) = name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                c.name = n.chars().take(modelcraft_scene::constraint::MAX_NAME).collect();
            }
            if let Some(ws) = source_weights {
                let Some(sources) = c.kind.sources_mut() else { return invalid("this constraint has no sources") };
                if ws.len() != sources.len() || ws.iter().any(|w| !w.is_finite() || *w < 0.0) {
                    return invalid("give one weight of zero or more per source");
                }
                for (s, w) in sources.iter_mut().zip(ws) {
                    s.weight = *w;
                }
            }
            if solver.is_some() || iterations.is_some() || pole.is_some() {
                let ConstraintKind::Ik { solver: s, iterations: n, pole: p, .. } = &mut c.kind else { return invalid("only IK has a solver and a pole") };
                if let Some(v) = solver {
                    *s = *v;
                }
                if let Some(v) = iterations {
                    *n = (*v).clamp(1, modelcraft_anim::ik::MAX_ITERATIONS);
                }
                if let Some(v) = pole {
                    if *v != 0 {
                        need(doc, ObjectId(*v))?;
                    }
                    *p = (*v != 0).then_some(ObjectId(*v));
                }
            }
            let target = c.target;
            if let Some(slot) = doc.scene.constraints.get_mut(*id) {
                *slot = c;
            }
            let _ = doc.scene.edit(target)?;
            report(doc, out);
        }
        RigCommand::RemoveConstraint { id } => {
            let c = doc.scene.constraints.remove(*id).ok_or_else(|| EngineError::Invalid(format!("no constraint {id}")))?;
            let _ = doc.scene.edit(c.target);
            report(doc, out);
        }
        RigCommand::MatchIk { id, to } => {
            let c = constraint(doc, *id)?;
            let ConstraintKind::Ik { goal, pole, bones, .. } = &c.kind else { return invalid("only an IK constraint can be matched") };
            let chain = doc.scene.ik_chain(c.target, *bones);
            match to.as_str() {
                "fk" => {
                    // The joints keep the solved rotations as their own.
                    for j in chain.iter().take(chain.len().saturating_sub(1)) {
                        if let Some(o) = doc.scene.get(*j) {
                            let t = o.transform;
                            doc.scene.constraints.set_authored(*j, t);
                        }
                    }
                    if let Some(slot) = doc.scene.constraints.get_mut(*id) {
                        slot.weight = 0.0;
                    }
                }
                "ik" => {
                    // Where the chain is by its own rotations: solve a copy with the IK off.
                    let mut fk = doc.scene.clone();
                    if let Some(slot) = fk.constraints.get_mut(*id) {
                        slot.weight = 0.0;
                    }
                    fk.solve_constraints();
                    let at: Vec<Vec3> = chain.iter().map(|j| fk.world_matrix(*j).transform_point3(Vec3::ZERO)).collect();
                    let (Some(root), Some(end)) = (at.first().copied(), at.last().copied()) else { return invalid("the chain is empty") };
                    place(doc, *goal, end)?;
                    if let Some(p) = pole {
                        // Out from the root–end line through the joint that bends most.
                        let axis = (end - root).normalize_or_zero();
                        let bend = at
                            .iter()
                            .map(|p| (*p - root) - axis * (*p - root).dot(axis))
                            .max_by(|a, b| a.length().total_cmp(&b.length()))
                            .unwrap_or(Vec3::ZERO);
                        if bend.length() > 1e-5 {
                            let reach: f32 = at.windows(2).map(|w| w.first().zip(w.get(1)).map_or(0.0, |(a, b)| a.distance(*b))).sum();
                            place(doc, *p, (root + end) * 0.5 + bend.normalize() * reach.max(1e-3))?;
                        }
                    }
                    if let Some(slot) = doc.scene.constraints.get_mut(*id) {
                        slot.weight = 1.0;
                    }
                }
                _ => return invalid("match to fk or ik"),
            }
            report(doc, out);
        }
        RigCommand::BakeConstraints { objects, start, end, step, keep } => {
            let time = doc.scene.anim.time;
            let (start, end) = (start.unwrap_or(time.start), end.unwrap_or(time.end));
            if !start.is_finite() || !end.is_finite() || !step.is_finite() || end < start || *step <= 0.0 {
                return invalid("a bake needs start <= end and a positive step");
            }
            let frames = ((end - start) / step).floor() as usize + 1;
            if objects.is_empty() || frames.saturating_mul(objects.len()) > MAX_BAKE_FRAMES {
                return invalid(format!("a bake samples 1 to {MAX_BAKE_FRAMES} object frames"));
            }
            for id in objects {
                need(doc, *id)?;
            }
            // Sample the solved pose of every frame.
            let now = doc.scene.anim.frame;
            let mut samples: Vec<(f64, Vec<[f64; 9]>)> = Vec::with_capacity(frames);
            let mut last: Vec<[f64; 9]> = Vec::new();
            for i in 0..frames {
                let f = start + i as f64 * step;
                doc.scene.set_frame(f);
                let mut row = Vec::with_capacity(objects.len());
                for (n, id) in objects.iter().enumerate() {
                    let t = doc.scene.get(*id).map(|o| o.transform).unwrap_or_default();
                    let e = t.euler_degrees();
                    let mut v = [t.translation.x, t.translation.y, t.translation.z, e.x, e.y, e.z, t.scale.x, t.scale.y, t.scale.z].map(f64::from);
                    // Keep each angle near the frame before, so the curves do not jump by turns.
                    if let Some(prev) = last.get(n) {
                        for a in 3..6 {
                            if let (Some(slot), Some(p)) = (v.get_mut(a), prev.get(a)) {
                                *slot = unwrap_angle(*slot, *p);
                            }
                        }
                    }
                    row.push(v);
                }
                last.clone_from(&row);
                samples.push((f, row));
            }
            // Switch off what drove them, then write the keys over whatever curves were there.
            if !*keep {
                let ids: Vec<u32> = doc
                    .scene
                    .constraints
                    .list()
                    .iter()
                    .filter(|c| match &c.kind {
                        ConstraintKind::Driven { .. } | ConstraintKind::Expression { .. } => objects.contains(&c.target),
                        _ => doc.scene.constraint_drives(c).iter().any(|o| objects.contains(o)),
                    })
                    .map(|c| c.id)
                    .collect();
                for id in &ids {
                    if let Some(c) = doc.scene.constraints.get_mut(*id) {
                        c.enabled = false;
                    }
                }
                out.warnings.push(format!("{} constraints switched off", ids.len()));
            }
            for (n, id) in objects.iter().enumerate() {
                for (ch, path) in TRANSFORM_PATHS.iter().enumerate() {
                    let curve = doc.scene.anim.curve_mut_in(*id, path, BASE_LAYER)?;
                    let points: Vec<(f64, f64)> = samples.iter().filter_map(|(f, row)| Some((*f, *row.get(n)?.get(ch)?))).collect();
                    *curve = Curve::from_points(&points);
                }
                let _ = doc.scene.edit(*id)?;
            }
            doc.scene.set_frame(now);
            out.warnings.push(format!("{} keys set", frames * objects.len() * 9));
            out.warnings.extend(doc.scene.constraints.problems().iter().map(|p| p.message.clone()));
        }
        _ => return invalid("not a constraint operation"),
    }
    Ok(())
}
