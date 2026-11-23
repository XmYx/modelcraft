//! Spline generators: objects that show a surface made from the splines inside them.
//!
//! - **Extrude** and **Revolve** use every spline inside (nested outlines make holes).
//! - **Loft** blends through the splines inside, in order.
//! - **Sweep** moves the first spline (the profile) along the second (the path).
//!
//! The splines stay editable children; the generator re-evaluates whenever one of them changes
//! (its cache key covers its children), and it can itself be the input of an array, a symmetry
//! or another generator.

use std::sync::Arc;

use glam::Mat4;
use modelcraft_ops::spline_gen::{self, Axis, ExtrudeOptions, LoftOptions, RevolveOptions, SweepOptions};
use modelcraft_spline::Polyline;
use serde::{Deserialize, Serialize};

use crate::{Evaluated, Evaluator, ObjectId, ObjectKind, Scene, StepError};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SplineGen {
    Extrude(ExtrudeOptions),
    Revolve(RevolveOptions),
    Loft(LoftOptions),
    Sweep(SweepOptions),
}

/// The generator kinds, in menu order.
pub const KINDS: &[&str] = &["extrude", "revolve", "loft", "sweep"];

impl SplineGen {
    /// The default generator of a kind; `None` for an unknown kind.
    pub fn default_of(kind: &str) -> Option<SplineGen> {
        Some(match kind {
            "extrude" => SplineGen::Extrude(ExtrudeOptions::default()),
            "revolve" => SplineGen::Revolve(RevolveOptions::default()),
            "loft" => SplineGen::Loft(LoftOptions::default()),
            "sweep" => SplineGen::Sweep(SweepOptions::default()),
            _ => return None,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            SplineGen::Extrude(_) => "extrude",
            SplineGen::Revolve(_) => "revolve",
            SplineGen::Loft(_) => "loft",
            SplineGen::Sweep(_) => "sweep",
        }
    }

    pub fn sanitized(&self) -> SplineGen {
        match self {
            SplineGen::Extrude(o) => SplineGen::Extrude(o.sanitized()),
            SplineGen::Revolve(o) => SplineGen::Revolve(o.sanitized()),
            SplineGen::Loft(o) => SplineGen::Loft(o.sanitized()),
            SplineGen::Sweep(o) => SplineGen::Sweep(o.sanitized()),
        }
    }
}

/// One spline child, tessellated and placed in the generator's space.
struct Input {
    name: String,
    lines: Vec<Polyline>,
}

fn place(lines: &[Polyline], m: Mat4) -> Vec<Polyline> {
    lines.iter().map(|l| Polyline { points: l.points.iter().map(|p| m.transform_point3(*p)).collect(), closed: l.closed }).collect()
}

impl Evaluator {
    /// The mesh a spline generator shows.
    pub(crate) fn spline_generate(&mut self, scene: &Scene, id: ObjectId, generator: &SplineGen) -> Evaluated {
        let mut errors: Vec<StepError> = Vec::new();
        let mut err = |message: String| errors.push(StepError { step: usize::MAX, message });
        let mut inputs: Vec<Input> = Vec::new();
        let children = scene.spline_children(id);
        if children.len() > modelcraft_ops::spline_gen::MAX_INPUTS {
            err(format!("{} splines inside it are too many; at most {} are used", children.len(), modelcraft_ops::spline_gen::MAX_INPUTS));
            return Evaluated { errors, ..Evaluated::default() };
        }
        // Count the points before copying any: a file can hold far more than a generator takes.
        let mut points = 0usize;
        for c in &children {
            if let Some(e) = self.spline(scene, *c) {
                points = points.saturating_add(e.point_count());
            }
        }
        if points > modelcraft_ops::spline_gen::MAX_INPUT_POINTS {
            err(format!("the splines inside it have {points} points; at most {} are used", modelcraft_ops::spline_gen::MAX_INPUT_POINTS));
            return Evaluated { errors, ..Evaluated::default() };
        }
        for c in children {
            let Some(o) = scene.get(c) else { continue };
            match self.spline(scene, c) {
                Some(e) if e.error.is_none() => inputs.push(Input { name: o.name.clone(), lines: place(&e.lines, o.local_matrix()) }),
                Some(e) => err(format!("{}: {}", o.name, e.error.clone().unwrap_or_default())),
                None => {}
            }
        }
        let need = match generator {
            SplineGen::Extrude(_) | SplineGen::Revolve(_) => 1,
            SplineGen::Loft(_) | SplineGen::Sweep(_) => 2,
        };
        if inputs.len() < need {
            let what = match generator {
                SplineGen::Extrude(_) | SplineGen::Revolve(_) => "needs a spline inside it",
                SplineGen::Loft(_) => "needs at least two splines inside it",
                SplineGen::Sweep(_) => "needs two splines inside it: the profile, then the path",
            };
            err(what.into());
            return Evaluated { errors, ..Evaluated::default() };
        }
        self.runs += 1;
        let result = match generator {
            SplineGen::Extrude(o) => {
                let curves: Vec<Polyline> = inputs.iter().flat_map(|i| i.lines.iter().cloned()).collect();
                spline_gen::extrude(&curves, o)
            }
            SplineGen::Revolve(o) => {
                let curves: Vec<Polyline> = inputs.iter().flat_map(|i| i.lines.iter().cloned()).collect();
                spline_gen::revolve(&curves, o)
            }
            SplineGen::Loft(o) => {
                let sections: Vec<Polyline> = inputs.iter().filter_map(|i| i.lines.first().cloned()).collect();
                spline_gen::loft(&sections, o)
            }
            SplineGen::Sweep(o) => match inputs[1].lines.first() {
                Some(path) => spline_gen::sweep(&inputs[0].lines, path, o),
                None => Err(modelcraft_ops::OpError::Invalid(format!("{} has no points", inputs[1].name))),
            },
        };
        match result {
            Ok(mesh) => Evaluated { mesh: Arc::new(mesh), errors, ..Evaluated::default() },
            Err(e) => {
                err(e.to_string());
                Evaluated { errors, ..Evaluated::default() }
            }
        }
    }
}

impl Scene {
    /// True if the object is a spline generator.
    pub fn is_spline_generator(&self, id: ObjectId) -> bool {
        self.get(id).is_some_and(|o| matches!(o.kind, ObjectKind::SplineGenerator { .. }))
    }
}

/// Axis names for the Properties pickers.
pub const AXES: [Axis; 4] = Axis::ALL;
