//! The functions a script can call. Each group registers itself on the engine; the table in
//! [`crate::docs`] describes every one of them, and a test keeps the two in step.

use std::rc::Rc;

use rhai::Engine;

use crate::host::Shared;

mod commands;
pub(crate) mod events;
pub(crate) use commands::script_name;
pub(crate) mod conv;
mod files;
mod mesh;
mod scene;
mod util;

pub(crate) fn register_all(engine: &mut Engine, s: &Rc<Shared>) {
    commands::register(engine, s);
    scene::register(engine, s);
    mesh::register(engine, s);
    files::register(engine, s);
    util::register(engine, s);
    events::register(engine, s);
}
