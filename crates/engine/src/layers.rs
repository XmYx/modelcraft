//! Detail levels and shape layers of the sculpt layer being edited. Both change the layer in the
//! scene (one undo snapshot each); the sculpt session is then brought in line.

use std::sync::Arc;

use modelcraft_mesh::PolyMesh;
use modelcraft_ops::subdivide;
use modelcraft_scene::{ObjectId, ObjectKind, SculptLayer, ShapeLayer, Step, StepOp};
use modelcraft_sculpt::detail::{self, MAX_DETAIL_LEVEL, Refine, shape};

use crate::command::{DetailOp, ShapeOp};
use crate::document::{Document, EngineError, invalid};

/// Most shape layers on one sculpt layer.
pub const MAX_SHAPE_LAYERS: usize = 64;
/// Longest shape layer name, in characters.
pub const MAX_SHAPE_NAME: usize = 64;

/// The input of a sculpt layer subdivided to `level` (topology; positions are the smooth ones).
pub(crate) fn level_mesh(input: &PolyMesh, level: u8) -> Result<PolyMesh, EngineError> {
    if level == 0 {
        return Ok(input.clone());
    }
    subdivide::catmull_clark(input, u32::from(level)).map_err(|e| EngineError::Invalid(e.to_string()))
}

/// Bring the active shape layer's offsets up to date with what was sculpted since the anchor.
fn settle(layer: &mut SculptLayer) {
    if let Some(sh) = layer.active_shape.and_then(|k| layer.shapes.get_mut(k as usize))
        && layer.anchor.len() == layer.positions.len()
        && !Arc::ptr_eq(&layer.anchor, &layer.positions)
    {
        let strength = sh.strength;
        shape::settle(Arc::make_mut(&mut sh.delta).as_mut_slice(), &layer.positions, &layer.anchor, strength);
    }
    reanchor(layer);
}

/// Start measuring what is sculpted into the active shape layer from the current positions.
fn reanchor(layer: &mut SculptLayer) {
    layer.anchor = if layer.active_shape.is_some() { Arc::clone(&layer.positions) } else { Arc::default() };
}

fn shape_at(layer: &mut SculptLayer, index: u32) -> Result<&mut ShapeLayer, EngineError> {
    layer.shapes.get_mut(index as usize).ok_or_else(|| EngineError::Invalid(format!("there is no shape layer {index}")))
}

fn clean_name(name: &str, fallback: usize) -> String {
    let n: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_SHAPE_NAME).collect();
    if n.is_empty() { format!("Shape {fallback}") } else { n }
}

fn detail(layer: &mut SculptLayer, input: &PolyMesh, op: DetailOp) -> Result<(), EngineError> {
    settle(layer);
    let bad = |e: detail::DetailError| EngineError::Invalid(e.to_string());
    match op {
        DetailOp::Up => {
            if layer.level >= MAX_DETAIL_LEVEL {
                return invalid("this is the highest detail level");
            }
            let coarse = level_mesh(input, layer.level)?;
            let faces = subdivide::face_count_after(&coarse, 1);
            if faces > modelcraft_ops::MAX_RESULT_FACES {
                return invalid(format!("the next detail level would have {faces} faces, more than the limit"));
            }
            let fine = level_mesh(&coarse, 1)?;
            let refine = Refine::new(&coarse);
            let stored = layer.finer.first().cloned();
            let up = detail::step_up(&refine, &fine, &layer.positions, stored.as_deref().map(Vec::as_slice)).map_err(bad)?;
            let mask: Vec<f32> = refine.apply(layer.mask.as_slice()).into_iter().map(|m| m.clamp(0.0, 1.0)).collect();
            for sh in &mut layer.shapes {
                sh.delta = Arc::new(refine.apply(sh.delta.as_slice()));
            }
            if !layer.finer.is_empty() {
                layer.finer.remove(0);
            }
            layer.positions = Arc::new(up);
            layer.mask = Arc::new(mask);
            layer.level += 1;
        }
        DetailOp::Down => {
            if layer.level == 0 {
                return invalid("this is the base detail level");
            }
            if !layer.shapes.is_empty() {
                return invalid("merge or remove the shape layers before stepping down a detail level");
            }
            let coarse = level_mesh(input, layer.level - 1)?;
            let fine = level_mesh(&coarse, 1)?;
            let refine = Refine::new(&coarse);
            let (down, disp) = detail::step_down(&refine, &fine, &layer.positions).map_err(bad)?;
            let mask: Vec<f32> = layer.mask.iter().take(down.len()).copied().collect();
            layer.finer.insert(0, Arc::new(disp));
            layer.positions = Arc::new(down);
            layer.mask = Arc::new(mask);
            layer.level -= 1;
        }
    }
    reanchor(layer);
    Ok(())
}

fn shapes(layer: &mut SculptLayer, op: &ShapeOp) -> Result<(), EngineError> {
    settle(layer);
    match op {
        ShapeOp::Add { name } => {
            if layer.shapes.len() >= MAX_SHAPE_LAYERS {
                return invalid(format!("a sculpt layer holds at most {MAX_SHAPE_LAYERS} shape layers"));
            }
            let name = clean_name(name.as_deref().unwrap_or(""), layer.shapes.len() + 1);
            layer.shapes.push(ShapeLayer { name, strength: 1.0, delta: Arc::new(vec![glam::Vec3::ZERO; layer.positions.len()]) });
            layer.active_shape = Some(layer.shapes.len() as u32 - 1);
        }
        ShapeOp::Remove { index } | ShapeOp::Merge { index } => {
            let sh = shape_at(layer, *index)?.clone();
            if matches!(op, ShapeOp::Remove { .. }) {
                shape::add_scaled(Arc::make_mut(&mut layer.positions).as_mut_slice(), &sh.delta, -sh.strength);
            }
            layer.shapes.remove(*index as usize);
            layer.active_shape = match layer.active_shape {
                Some(a) if a == *index => None,
                Some(a) if a > *index => Some(a - 1),
                a => a,
            };
        }
        ShapeOp::Strength { index, strength } => {
            if !strength.is_finite() {
                return invalid("the strength is not a number");
            }
            let to = strength.clamp(*shape::STRENGTH.start(), *shape::STRENGTH.end());
            let sh = shape_at(layer, *index)?;
            let (delta, from) = (Arc::clone(&sh.delta), sh.strength);
            sh.strength = to;
            shape::add_scaled(Arc::make_mut(&mut layer.positions).as_mut_slice(), &delta, to - from);
        }
        ShapeOp::Select { index } => {
            if let Some(i) = index {
                shape_at(layer, *i)?;
            }
            layer.active_shape = *index;
        }
        ShapeOp::Rename { index, name } => {
            let fallback = *index as usize + 1;
            shape_at(layer, *index)?.name = clean_name(name, fallback);
        }
    }
    reanchor(layer);
    Ok(())
}

impl Document {
    /// The sculpt layer of `id` as last synced (its level and shape layers), if it has one.
    pub fn sculpt_layer(&self, id: ObjectId) -> Option<&SculptLayer> {
        match self.scene.get(id)?.steps().last() {
            Some(Step { op: StepOp::Sculpt { layer }, .. }) => Some(layer),
            _ => None,
        }
    }

    fn edit_sculpt_layer(&mut self, id: ObjectId, f: impl FnOnce(&mut SculptLayer, &PolyMesh) -> Result<(), EngineError>) -> Result<(), EngineError> {
        let last = self.scene.get(id).map(|o| o.steps().len()).unwrap_or(0).saturating_sub(1);
        let input = self.eval.steps_output(&self.scene, id, last).mesh;
        if let ObjectKind::Mesh { steps, .. } = &mut self.scene.edit(id)?.kind
            && let Some(Step { op: StepOp::Sculpt { layer }, .. }) = steps.last_mut()
        {
            return f(layer, &input);
        }
        invalid("the object has no sculpt layer")
    }

    pub(crate) fn apply_detail(&mut self, id: ObjectId, op: DetailOp) -> Result<(), EngineError> {
        self.edit_sculpt_layer(id, |layer, input| detail(layer, input, op))
    }

    pub(crate) fn apply_shape(&mut self, id: ObjectId, op: &ShapeOp) -> Result<(), EngineError> {
        self.edit_sculpt_layer(id, |layer, _| shapes(layer, op))
    }
}
