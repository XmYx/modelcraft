//! Procedural generators: objects that make one mesh from the meshes inside them (connect,
//! thicken, reduce, boolean). Each is data with a [`Generator::run`] that turns the evaluated
//! inputs (in the generator's space) into one mesh; the evaluator re-runs it whenever an input or
//! a setting changes, so the result stays live.

use glam::Mat4;
use modelcraft_ops::boolean::{self, BoolOp};
use modelcraft_ops::thicken::{self, Direction};
use modelcraft_ops::{OpError, OpResult, PolyMesh, connect, decimate};
use serde::{Deserialize, Serialize};

/// What a generator makes and how.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "gen", rename_all = "snake_case")]
pub enum Generator {
    /// Joins every mesh child into one mesh; `weld` merges points closer than that.
    Connect {
        #[serde(default)]
        weld: f32,
    },
    /// Gives its first mesh child a wall `amount` thick, an open sheet becoming a closed solid.
    Thicken {
        amount: f32,
        #[serde(default)]
        direction: Direction,
    },
    /// Keeps about `ratio` of the triangles of its first mesh child, live.
    Reduce { ratio: f32 },
    /// The first mesh child combined with each of the others in turn.
    Boolean { op: BoolOp },
}

/// Most meshes one boolean folds together (each fold is a whole boolean).
pub const MAX_BOOLEAN_INPUTS: usize = 16;

/// The generator names, in menu order.
pub const KINDS: &[&str] = &["connect", "thicken", "reduce", "boolean"];

impl Generator {
    pub fn default_of(kind: &str) -> Option<Generator> {
        Some(match kind {
            "connect" => Generator::Connect { weld: 0.0 },
            "thicken" => Generator::Thicken { amount: 0.2, direction: Direction::Both },
            "reduce" => Generator::Reduce { ratio: 0.5 },
            "boolean" => Generator::Boolean { op: BoolOp::Subtract },
            _ => return None,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Generator::Connect { .. } => "connect",
            Generator::Thicken { .. } => "thicken",
            Generator::Reduce { .. } => "reduce",
            Generator::Boolean { .. } => "boolean",
        }
    }

    /// The name shown for the object kind.
    pub fn type_name(&self) -> &'static str {
        match self {
            Generator::Connect { .. } => "Connect",
            Generator::Thicken { .. } => "Thicken",
            Generator::Reduce { .. } => "Reduce",
            Generator::Boolean { .. } => "Boolean",
        }
    }

    /// Whether every mesh child is an input (rather than only the first).
    pub fn takes_many(&self) -> bool {
        matches!(self, Generator::Connect { .. } | Generator::Boolean { .. })
    }

    /// Clamp every number into its safe range (a number that is not finite becomes the neutral one).
    pub fn sanitized(&self) -> Generator {
        let finite = |v: f32, d: f32| if v.is_finite() { v } else { d };
        match self {
            Generator::Connect { weld } => Generator::Connect { weld: finite(*weld, 0.0).clamp(0.0, 1_000.0) },
            Generator::Thicken { amount, direction } => {
                Generator::Thicken { amount: finite(*amount, 0.2).abs().clamp(thicken::MIN_AMOUNT, thicken::MAX_AMOUNT), direction: *direction }
            }
            Generator::Reduce { ratio } => Generator::Reduce { ratio: finite(*ratio, 0.5).clamp(0.0, 1.0) },
            Generator::Boolean { op } => Generator::Boolean { op: *op },
        }
    }

    /// The mesh the generator makes from `inputs` (each already placed in the generator's space),
    /// with notes about what it could not do.
    pub fn run_noted(&self, inputs: &[PolyMesh]) -> Result<(PolyMesh, Vec<String>), OpError> {
        let first = || inputs.first().ok_or_else(|| OpError::Invalid("needs a mesh inside it".into()));
        match self.sanitized() {
            Generator::Connect { weld } => {
                first()?;
                let parts: Vec<(&PolyMesh, Mat4)> = inputs.iter().map(|m| (m, Mat4::IDENTITY)).collect();
                connect::connect(&parts, weld).map(|m| (m, Vec::new()))
            }
            Generator::Thicken { amount, direction } => thicken::thicken(first()?, amount, direction).map(|m| (m, Vec::new())),
            Generator::Reduce { ratio } => decimate::decimate(first()?, ratio).map(|m| (m, Vec::new())),
            Generator::Boolean { op } => {
                let mut acc = first()?.clone();
                if inputs.len() < 2 {
                    return Err(OpError::Invalid("a boolean needs two meshes inside it".into()));
                }
                if inputs.len() > MAX_BOOLEAN_INPUTS {
                    return Err(OpError::Invalid(format!("a boolean combines at most {MAX_BOOLEAN_INPUTS} meshes")));
                }
                let mut notes = Vec::new();
                for b in &inputs[1..] {
                    let (m, stats) = boolean::combine_with(&acc, b, op, &boolean::Limits::interactive())?;
                    if stats.attempt > 0 && notes.is_empty() {
                        notes.push(
                            "some faces of the two meshes lie in one plane; the second was moved by a hair to resolve them, so the result is not exact"
                                .to_string(),
                        );
                    }
                    acc = m;
                }
                Ok((acc, notes))
            }
        }
    }

    /// [`Generator::run_noted`] without the notes.
    pub fn run(&self, inputs: &[PolyMesh]) -> OpResult {
        self.run_noted(inputs).map(|(m, _)| m)
    }
}
