//! ModelCraft scripting: a sandboxed [Rhai](https://rhai.rs) engine over the command registry.
//!
//! Everything a script changes goes through [`modelcraft_engine::Command`]s on a
//! [`modelcraft_engine::Document`], so a script's work is journaled, undoable and identical to
//! what the app, `modelcraft-cli` and the control channel do. Scripts can read the scene (objects,
//! attributes, selection, the dependency graph, evaluated history nodes and meshes), build and edit
//! meshes with bounds-checked handles, and group their work into one undo step.
//!
//! A script has no file or network access of its own. The few functions that touch the disk check
//! a [`Permissions`] that grants nothing by default, and a [`Limits`] caps operations, wall-clock
//! time, call depth, container sizes and built geometry. `docs/scripting.md` has the language
//! choice and the API.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod api;
pub mod docs;
mod error;
pub mod highlight;
mod host;
mod limits;
<<<<<<< HEAD
pub mod macros;
=======
>>>>>>> origin/staging
mod pacing;
mod perm;
mod session;

pub use api::events::{Event, EventKind, ToolInfo};
pub use error::{ErrorKind, ScriptError};
pub use host::{LineKind, OutLine, RunContext};
pub use limits::Limits;
pub use pacing::{EventPacer, PlaybackPolicy};
pub use perm::{Access, Permissions};
pub use session::{Fired, RunReport, Session};

#[cfg(test)]
mod tests;
