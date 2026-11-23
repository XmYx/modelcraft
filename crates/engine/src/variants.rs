//! Scene variant commands. One command, `variant`, with the operation in `op`:
//! `{"cmd":"variant","op":"switch","id":1}` ([`modelcraft_scene::variants`]).
//!
//! Switching is a command like any other: it is one undo step, journaled, and the variants and the
//! applied one are saved with the project. Overrides are added to the active variant only, from
//! the attribute's current value, which is still the main scene's, so override first and set the value after (switch to the variant, override, then edit).

use modelcraft_scene::ObjectId;
use serde::{Deserialize, Serialize};

use crate::command::Outcome;
use crate::document::{Document, EngineError};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum VariantCommand {
    /// Add an empty variant (it is not switched to).
    Add {
        #[serde(default)]
        name: Option<String>,
    },
    Remove {
        id: u32,
    },
    Rename {
        id: u32,
        name: String,
    },
    /// Apply a variant, or the main scene when `id` is missing.
    Switch {
        #[serde(default)]
        id: Option<u32>,
    },
    /// Override attributes of an object in the active variant, with their current values.
    Override {
        object: ObjectId,
        paths: Vec<String>,
    },
    /// Stop overriding attributes of an object in the active variant.
    ClearOverride {
        object: ObjectId,
        paths: Vec<String>,
    },
}

/// The `op` names of the `variant` command with one-line descriptions.
pub const COMMANDS: &[(&str, &str)] = &[
    ("add", "Add an empty variant: name?"),
    ("remove", "Remove a variant (the main scene comes back first when it was applied): id"),
    ("rename", "Rename a variant: id, name"),
    ("switch", "Apply a variant, or the main scene without id: id?"),
    (
        "override",
        "Override attributes of an object in the active variant with their current values: object, paths [translate.x, visible, material.metallic, …]",
    ),
    ("clear_override", "Stop overriding attributes in the active variant: object, paths"),
];

/// Most paths one override command takes.
const MAX_PATHS: usize = 256;

impl VariantCommand {
    pub fn label(&self) -> String {
        match self {
            VariantCommand::Add { .. } => "Add variant",
            VariantCommand::Remove { .. } => "Remove variant",
            VariantCommand::Rename { .. } => "Rename variant",
            VariantCommand::Switch { .. } => "Switch variant",
            VariantCommand::Override { .. } => "Override in variant",
            VariantCommand::ClearOverride { .. } => "Clear override",
        }
        .into()
    }
}

pub fn apply(doc: &mut Document, cmd: &VariantCommand, _out: &mut Outcome) -> Result<(), EngineError> {
    match cmd {
        VariantCommand::Add { name } => {
            doc.scene.add_variant(name.as_deref().unwrap_or(""))?;
        }
        VariantCommand::Remove { id } => doc.scene.remove_variant(*id)?,
        VariantCommand::Rename { id, name } => doc.scene.rename_variant(*id, name)?,
        VariantCommand::Switch { id } => doc.scene.switch_variant(*id)?,
        VariantCommand::Override { object, paths } => {
            if paths.len() > MAX_PATHS {
                return Err(EngineError::Invalid(format!("at most {MAX_PATHS} attributes at once")));
            }
            for p in paths {
                doc.scene.add_override(*object, p)?;
            }
        }
        VariantCommand::ClearOverride { object, paths } => {
            if paths.len() > MAX_PATHS {
                return Err(EngineError::Invalid(format!("at most {MAX_PATHS} attributes at once")));
            }
            for p in paths {
                doc.scene.remove_override(*object, p)?;
            }
        }
    }
    Ok(())
}
