//! The ModelCraft engine: a [`Document`] (scene, evaluator, undo history, crash journal and the
//! active sculpt session) changed only through [`Command`]s.
//!
//! Every action of the app, the CLI and the agent channel is a [`Command`]: plain serializable
//! data. That gives three things at once: an unlimited undo history (bounded by a memory budget,
//! oldest entries dropped first), a journal that can rebuild the document after a crash, and
//! scriptability (the CLI and the JSON-lines control channel send the same commands).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod command;
mod document;
mod layers;
mod undo;

pub use command::{COMMANDS, Command, DetailOp, MaskOp, Outcome, ShapeOp};
pub use document::{Document, EngineError, SculptSession};
pub use layers::{MAX_SHAPE_LAYERS, MAX_SHAPE_NAME};
pub use undo::{UndoEntry, UndoStack};

pub use modelcraft_formats as formats;
pub use modelcraft_mesh as mesh;
pub use modelcraft_ops as ops;
pub use modelcraft_scene as scene;
pub use modelcraft_sculpt as sculpt;
pub use modelcraft_uv as uv;
