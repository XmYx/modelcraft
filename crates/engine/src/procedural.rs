//! Procedural generator commands. One command, `procedural`, with the operation in `op`:
//! `{"cmd":"procedural","op":"add_generator","kind":"thicken","inputs":[3]}`.
//!
//! A generator is an object ([`modelcraft_scene::generator::Generator`]) that takes the meshes
//! inside it and makes one mesh; changing an input or a setting changes the result at once.

use glam::Vec3;
use modelcraft_ops::primitives::Primitive;
use modelcraft_scene::generator::{self, Generator};
use modelcraft_scene::{ObjectId, ObjectKind, SceneError, Source, Transform};
use serde::{Deserialize, Serialize};

use crate::command::Outcome;
use crate::document::{Document, EngineError};

/// Most inputs one generator is made around.
pub const MAX_INPUTS: usize = 1_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ProceduralCommand {
    /// Make a generator (`kind`: connect, thicken, reduce, boolean) around `inputs`, which move inside it;
    /// without any it makes small meshes of its own to work on, so a first click shows something.
    AddGenerator {
        kind: String,
        #[serde(default)]
        inputs: Vec<ObjectId>,
    },
    /// Replace a generator's settings (its `generator` with the fields of that kind).
    SetGenerator { id: ObjectId, generator: Generator },
}

/// The `op` names of the `procedural` command with one-line descriptions.
pub const COMMANDS: &[(&str, &str)] = &[
    ("add_generator", "Add a generator (connect, thicken, reduce, boolean) around inputs? (meshes or generators; none makes its own): kind, inputs?"),
    ("set_generator", "Change a generator: id, generator {gen: connect, weld} | {gen: thicken, amount, direction} | {gen: reduce, ratio} | {gen: boolean, op}"),
];

impl ProceduralCommand {
    pub fn label(&self) -> String {
        match self {
            ProceduralCommand::AddGenerator { kind, .. } => format!("Add {kind} generator"),
            ProceduralCommand::SetGenerator { .. } => "Generator settings".into(),
        }
    }
}

fn invalid<T>(m: impl Into<String>) -> Result<T, EngineError> {
    Err(EngineError::Invalid(m.into()))
}

fn title(kind: &str) -> String {
    let s = kind.replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => s,
    }
}

/// The objects a command edits, for the lock check.
pub fn check_unlocked(doc: &Document, cmd: &ProceduralCommand) -> Result<(), EngineError> {
    if let ProceduralCommand::SetGenerator { id, .. } = cmd
        && doc.scene.is_locked(*id)
    {
        let name = doc.scene.get(*id).map_or("an object", |o| o.name.as_str());
        return invalid(format!("{name} is locked; unlock it first"));
    }
    Ok(())
}

pub fn apply(doc: &mut Document, cmd: &ProceduralCommand, out: &mut Outcome) -> Result<(), EngineError> {
    match cmd {
        ProceduralCommand::AddGenerator { kind, inputs } => {
            let generator = Generator::default_of(kind).ok_or_else(|| EngineError::Invalid(format!("unknown generator {kind:?}; use one of {}", generator::KINDS.join(", "))))?;
            if inputs.len() > MAX_INPUTS {
                return invalid(format!("a generator takes at most {MAX_INPUTS} inputs"));
            }
            if !generator.takes_many() && inputs.len() > 1 {
                return invalid(format!("a {kind} generator takes one input"));
            }
            for (i, id) in inputs.iter().enumerate() {
                if inputs[..i].contains(id) {
                    return invalid("an input is listed twice");
                }
                match doc.scene.get(*id) {
                    None => return Err(SceneError::NoObject(*id).into()),
                    Some(o) if !o.kind.makes_geometry() => return invalid(format!("{} makes no mesh to use as an input", o.name)),
                    Some(_) => {}
                }
            }
            let parent = inputs.first().and_then(|i| doc.scene.get(*i)).and_then(|o| o.parent);
            let name = doc.scene.unique_name(&title(kind));
            let id = doc.scene.add(name, ObjectKind::Generator { generator }, parent)?;
            if inputs.is_empty() {
                add_defaults(doc, id, kind)?;
            }
            for input in inputs {
                doc.scene.reparent(*input, Some(id))?;
            }
            out.warnings.extend(doc.eval.evaluate(&doc.scene, id).errors.iter().map(|e| e.message.clone()));
            out.created.push(id);
        }
        ProceduralCommand::SetGenerator { id, generator } => match &mut doc.scene.edit(*id)?.kind {
            ObjectKind::Generator { generator: g } => *g = generator.sanitized(),
            _ => return invalid("not a generator"),
        },
    }
    Ok(())
}

/// The meshes a generator made without inputs works on.
fn add_defaults(doc: &mut Document, gen: ObjectId, kind: &str) -> Result<(), EngineError> {
    let add = |doc: &mut Document, name: &str, primitive: Primitive, at: Vec3| -> Result<(), EngineError> {
        let unique = doc.scene.unique_name(name);
        let id = doc.scene.add(unique, ObjectKind::Mesh { source: Source::Primitive { primitive }, steps: vec![] }, Some(gen))?;
        if at != Vec3::ZERO {
            doc.scene.edit(id)?.transform = Transform { translation: at, ..Transform::default() };
        }
        Ok(())
    };
    let cube = Primitive::Cube { size: Vec3::splat(1.0), segments: 1 };
    match kind {
        "connect" => {
            add(doc, "Cube", cube, Vec3::new(-0.75, 0.0, 0.0))?;
            add(doc, "Cube", cube, Vec3::new(0.75, 0.0, 0.0))?;
        }
        "boolean" => {
            add(doc, "Cube", Primitive::Cube { size: Vec3::splat(1.0), segments: 1 }, Vec3::ZERO)?;
            add(doc, "Sphere", Primitive::Sphere { radius: 0.35, segments: 24, rings: 12 }, Vec3::new(0.35, 0.35, 0.35))?;
        }
        "thicken" => add(doc, "Plane", Primitive::Plane { width: 2.0, depth: 2.0, segments: 4 }, Vec3::ZERO)?,
        _ => add(doc, "Sphere", Primitive::Sphere { radius: 1.0, segments: 24, rings: 12 }, Vec3::ZERO)?,
    }
    Ok(())
}
