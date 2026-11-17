//! From pointer samples to dabs. A [`DabMaker`] takes the points the pointer hits on the
//! surface and places dabs according to the brush's [`StrokeMode`]: evenly spaced, one per
//! sample, scattered, trailing behind the pointer, or one dab dragged out from where the stroke
//! began. It holds no mesh, so it is tested on its own and the app only feeds it.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::brush::{BrushSettings, Dab};

/// How a stroke turns pointer movement into dabs.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum StrokeMode {
    /// Dabs at even steps along the path.
    #[default]
    Spaced,
    /// Separate dabs, a brush width apart.
    Dots,
    /// Dabs scattered around the path, each with a random size.
    Spray {
        /// How far from the path, in brush radii.
        scatter: f32,
    },
    /// The brush trails the pointer on a leash, smoothing out a shaky hand.
    Trail {
        /// Leash length in brush radii.
        leash: f32,
    },
    /// One dab at the starting point; dragging away sets its size.
    Drag,
}

/// Most dabs one pointer sample may produce.
const MAX_STEPS: usize = 64;

/// What one pointer sample produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DabOut {
    /// Take back everything this stroke has done so far before applying `dabs`.
    pub rewind: bool,
    pub dabs: Vec<Dab>,
}

/// Places dabs for one stroke. See the module documentation.
#[derive(Clone, Debug)]
pub struct DabMaker {
    mode: StrokeMode,
    radius: f32,
    spacing: f32,
    /// Where the last dab went (or the stroke began).
    last: Option<Vec3>,
    /// Trail mode: where the brush is.
    brush_at: Option<Vec3>,
    rng: u32,
}

impl DabMaker {
    pub fn new(brush: &BrushSettings) -> DabMaker {
        let radius = if brush.radius.is_finite() { brush.radius.clamp(1e-6, 1e6) } else { 1.0 };
        let spacing = if brush.spacing.is_finite() { brush.spacing.clamp(0.02, 4.0) } else { 0.2 };
        DabMaker { mode: brush.stroke, radius, spacing: spacing * radius, last: None, brush_at: None, rng: 0x9E37_79B9 }
    }

    fn random(&mut self) -> f32 {
        // xorshift32: the same sequence every stroke, so a spray looks the same on any machine.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / 16_777_216.0
    }

    /// Feed one pointer sample: the surface point, the view direction and the pen pressure.
    pub fn push(&mut self, point: Vec3, view_dir: Vec3, pressure: f32) -> DabOut {
        let mut out = DabOut::default();
        if !(point.is_finite() && view_dir.is_finite()) {
            return out;
        }
        let pressure = if pressure.is_finite() { pressure.clamp(0.0, 1.0) } else { 1.0 };
        let dab = |center: Vec3, from: Vec3, pressure: f32, scale: f32| Dab { center, view_dir, pressure, grab_delta: center - from, scale };
        let Some(last) = self.last else {
            // The first sample always dabs.
            self.last = Some(point);
            self.brush_at = Some(point);
            let scale = if self.mode == StrokeMode::Drag { 0.05 } else { 1.0 };
            out.dabs.push(dab(point, point, pressure, scale));
            return out;
        };
        match self.mode {
            StrokeMode::Drag => {
                out.rewind = true;
                let scale = (point.distance(last) / self.radius).clamp(0.05, 20.0);
                out.dabs.push(dab(last, last, pressure, scale));
            }
            StrokeMode::Dots => {
                if point.distance(last) >= self.radius.max(self.spacing) {
                    out.dabs.push(dab(point, last, pressure, 1.0));
                    self.last = Some(point);
                }
            }
            StrokeMode::Spaced | StrokeMode::Spray { .. } | StrokeMode::Trail { .. } => {
                let target = match self.mode {
                    StrokeMode::Trail { leash } => {
                        let leash = if leash.is_finite() { leash.clamp(0.0, 10.0) } else { 1.0 } * self.radius;
                        let at = self.brush_at.unwrap_or(last);
                        let d = point.distance(at);
                        let now = if d > leash && d > 0.0 { at.lerp(point, 1.0 - leash / d) } else { at };
                        self.brush_at = Some(now);
                        now
                    }
                    _ => point,
                };
                let dist = target.distance(last);
                if dist < self.spacing {
                    return out;
                }
                let steps = ((dist / self.spacing) as usize).clamp(1, MAX_STEPS);
                let mut from = last;
                for k in 1..=steps {
                    let on_path = last.lerp(target, k as f32 / steps as f32);
                    if let StrokeMode::Spray { scatter } = self.mode {
                        let scatter = if scatter.is_finite() { scatter.clamp(0.0, 5.0) } else { 1.0 } * self.radius;
                        let n = view_dir.normalize_or(Vec3::Z);
                        let u = n.any_orthonormal_vector();
                        let v = n.cross(u);
                        let (a, r) = (self.random() * std::f32::consts::TAU, self.random().sqrt() * scatter);
                        let c = on_path + (u * a.cos() + v * a.sin()) * r;
                        let size = 0.4 + 0.6 * self.random();
                        out.dabs.push(dab(c, c, pressure, size));
                    } else {
                        out.dabs.push(dab(on_path, from, pressure, 1.0));
                    }
                    from = on_path;
                }
                self.last = Some(target);
            }
        }
        out
    }
}
