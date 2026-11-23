//! Spline objects: where a spline comes from ([`SplineSource`]: a parametric primitive or points
//! the user edits) and its evaluated form ([`SplineEval`]: the spline and its tessellation), cached
//! per object by the [`Evaluator`](crate::Evaluator).
//!
//! A spline object has no mesh. It is drawn as a curve, and generators (extrude, lathe, loft,
//! sweep: [`crate::spline_gen`]) take spline children as their input.

use std::sync::Arc;

use modelcraft_spline::{Polyline, Spline, SplinePrimitive, Tessellation};
use serde::{Deserialize, Serialize};

use crate::{Evaluator, ObjectId, ObjectKind, Scene};

/// Where a spline object's curve comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SplineSource {
    /// A circle, rectangle, star, arc, helix…: the numbers are kept and the curve rebuilt.
    Primitive { primitive: SplinePrimitive },
    /// Points and handles edited by hand.
    Curve { spline: Arc<Spline> },
}

impl SplineSource {
    /// The curve (a primitive is built; an edited curve is shared).
    pub fn spline(&self) -> Arc<Spline> {
        match self {
            SplineSource::Primitive { primitive } => Arc::new(primitive.build()),
            SplineSource::Curve { spline } => spline.clone(),
        }
    }

    pub fn heap_bytes(&self) -> usize {
        match self {
            SplineSource::Primitive { .. } => 0,
            SplineSource::Curve { spline } => spline.knot_count() * 40 + spline.segments.len() * 32,
        }
    }
}

/// A spline object's curve, tessellated for drawing and for generators.
#[derive(Clone, Debug, Default)]
pub struct SplineEval {
    pub spline: Arc<Spline>,
    /// One polyline per segment, in the object's own space. Empty when `error` is set.
    pub lines: Vec<Polyline>,
    /// Why the curve could not be tessellated (not finite, too many points, too fine a mode).
    pub error: Option<String>,
}

impl SplineEval {
    /// Points over all polylines.
    pub fn point_count(&self) -> usize {
        self.lines.iter().map(|l| l.points.len()).sum()
    }
}

/// The evaluation of a spline source under a tessellation mode.
pub fn evaluate_source(source: &SplineSource, tess: Tessellation) -> SplineEval {
    let spline = source.spline();
    match spline.tessellate(tess) {
        Ok(lines) => SplineEval { spline, lines, error: None },
        Err(e) => SplineEval { spline, lines: Vec::new(), error: Some(e.to_string()) },
    }
}

/// Cache of evaluated splines, by object and revision.
#[derive(Default)]
pub struct SplineCache {
    pub(crate) items: std::collections::HashMap<ObjectId, (u64, Arc<SplineEval>)>,
}

impl SplineCache {
    pub fn clear(&mut self) {
        self.items.clear();
    }
    pub fn retain(&mut self, scene: &Scene) {
        self.items.retain(|id, _| scene.get(*id).is_some());
    }
}

impl Evaluator {
    /// The evaluated spline of a spline object; `None` for another kind of object.
    pub fn spline(&mut self, scene: &Scene, id: ObjectId) -> Option<Arc<SplineEval>> {
        let obj = scene.get(id)?;
        let ObjectKind::Spline { source, tess } = &obj.kind else { return None };
        if let Some((rev, e)) = self.splines.items.get(&id)
            && *rev == obj.rev
        {
            return Some(e.clone());
        }
        let e = Arc::new(evaluate_source(source, *tess));
        self.splines.items.insert(id, (obj.rev, e.clone()));
        Some(e)
    }
}

impl Scene {
    /// The spline objects directly inside `id`, in order (the inputs of a spline generator).
    pub fn spline_children(&self, id: ObjectId) -> Vec<ObjectId> {
        self.children(id).into_iter().filter(|c| self.get(*c).is_some_and(|o| matches!(o.kind, ObjectKind::Spline { .. }))).collect()
    }

    pub fn is_spline(&self, id: ObjectId) -> bool {
        self.get(id).is_some_and(|o| matches!(o.kind, ObjectKind::Spline { .. }))
    }
}
