//! The command set. `cmd` is the tag in JSON: `{"cmd":"add_primitive","kind":"cube"}`.

use modelcraft_ops::deform::Deformer;
use modelcraft_ops::primitives::Primitive;
use modelcraft_scene::{Material, ObjectId, StepOp, Transform};
use modelcraft_sculpt::Stroke;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskOp {
    Clear,
    Fill,
    Invert,
    /// Mask the patch of a triangle.
    Patch {
        triangle: u32,
    },
}

/// Step between detail levels (each level up subdivides once and keeps the sculpted detail).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailOp {
    Up,
    Down,
}

/// Edit the shape layers of a sculpt layer (named offsets, each blended with its own strength).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeOp {
    /// Add an empty shape layer and make it the one strokes are recorded into.
    Add {
        #[serde(default)]
        name: Option<String>,
    },
    /// Remove a shape layer and what it added.
    Remove {
        index: u32,
    },
    /// Remove a shape layer but keep what it added, at its current strength.
    Merge {
        index: u32,
    },
    /// Blend strength, -1..1.
    Strength {
        index: u32,
        strength: f32,
    },
    /// Record strokes into this shape layer (null: into the base sculpt).
    Select {
        index: Option<u32>,
    },
    Rename {
        index: u32,
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Add a parametric primitive (`cube`, `plane`, `sphere`, `round_cube`, `cylinder`, `cone`, `torus`).
    AddPrimitive {
        kind: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        parent: Option<ObjectId>,
    },
    /// Add an empty group.
    AddGroup {
        #[serde(default)]
        name: Option<String>,
    },
    /// Add a smooth cage generator; the given object becomes its input.
    AddSmoothCage {
        #[serde(default)]
        input: Option<ObjectId>,
        #[serde(default = "two")]
        levels: u32,
    },
    /// Add a deformer (`bend`, `twist`, `taper`, `ripple`, `round`) under a mesh.
    AddDeformer {
        kind: String,
        target: ObjectId,
    },
    Delete {
        ids: Vec<ObjectId>,
    },
    Duplicate {
        id: ObjectId,
    },
    Rename {
        id: ObjectId,
        name: String,
    },
    Reparent {
        id: ObjectId,
        parent: Option<ObjectId>,
    },
    SetTransform {
        id: ObjectId,
        transform: Transform,
    },
    SetVisible {
        id: ObjectId,
        visible: bool,
    },
    SetMaterial {
        id: ObjectId,
        material: Material,
    },
    SetPrimitive {
        id: ObjectId,
        primitive: Primitive,
    },
    SetCageLevels {
        id: ObjectId,
        levels: u32,
    },
    SetDeformer {
        id: ObjectId,
        deformer: Deformer,
    },
    /// Append a build-history step to a mesh object.
    AddStep {
        id: ObjectId,
        op: StepOp,
    },
    /// Change a step's parameters (its selection or amounts).
    SetStep {
        id: ObjectId,
        index: usize,
        op: StepOp,
    },
    EnableStep {
        id: ObjectId,
        index: usize,
        enabled: bool,
    },
    RemoveStep {
        id: ObjectId,
        index: usize,
    },
    /// Bake the history into the source mesh.
    Collapse {
        id: ObjectId,
    },
    /// Import a glTF/GLB/OBJ/STL/PLY file as new objects.
    Import {
        path: String,
        /// The file's up axis (`auto`: what the file says, FBX only; else Y).
        #[serde(default, skip_serializing_if = "is_auto")]
        up: modelcraft_formats::exchange::Up,
        /// Metres per file unit (`None`: what the file says, FBX only; else 1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale: Option<f32>,
    },
    /// Start sculpting a mesh object (adds a sculpt layer if its last step is not one).
    BeginSculpt {
        id: ObjectId,
    },
    EndSculpt,
    SculptStroke {
        id: ObjectId,
        stroke: Stroke,
    },
    SculptMask {
        id: ObjectId,
        op: MaskOp,
    },
    SculptDetail {
        id: ObjectId,
        op: DetailOp,
    },
    SculptShape {
        id: ObjectId,
        op: ShapeOp,
    },
    Undo,
    Redo,
    /// Keys, curves and time: `{"cmd":"anim","op":"set_key",…}` ([`crate::anim::AnimCommand`]).
    Anim(crate::anim::AnimCommand),
    /// Joints and skins: `{"cmd":"rig","op":"add_joint",…}` ([`crate::rig::RigCommand`]).
    Rig(crate::rig::RigCommand),
}

fn two() -> u32 {
    2
}

/// Command names with a one-line description (for `modelcraft-cli commands` and the agent channel).
pub const COMMANDS: &[(&str, &str)] = &[
    ("add_primitive", "Add a parametric primitive: kind (cube, plane, sphere, round_cube, cylinder, cone, torus), name?, parent?"),
    ("add_group", "Add an empty group: name?"),
    ("add_smooth_cage", "Add a smooth cage generator around input? with levels"),
    ("add_deformer", "Add a deformer (bend, twist, taper, ripple, round) under target"),
    ("delete", "Delete objects and their children: ids"),
    ("duplicate", "Duplicate an object and its children: id"),
    ("rename", "Rename an object: id, name"),
    ("reparent", "Move an object under parent (null: top level)"),
    ("set_transform", "Set translation, rotation (quaternion x,y,z,w) and scale: id, transform"),
    ("set_visible", "Show or hide an object: id, visible"),
    ("set_material", "Set base_color, metallic, roughness: id, material"),
    ("set_primitive", "Change a primitive's parameters: id, primitive"),
    ("set_cage_levels", "Change a smooth cage's levels: id, levels"),
    ("set_deformer", "Change a deformer's parameters: id, deformer"),
    (
        "add_step",
        "Append a history step (extrude, inset, bevel, loop_cut, merge, weld, delete_faces, delete_edges, delete_vertices, move_vertices, flip, triangulate, subdivide, set_patch, bridge, fill_hole, mirror, even_remesh): id, op",
    ),
    ("set_step", "Change a history step: id, index, op"),
    ("enable_step", "Switch a history step on or off: id, index, enabled"),
    ("remove_step", "Remove a history step: id, index"),
    ("collapse", "Bake an object's history into its mesh: id"),
    ("import", "Import a glTF, GLB, OBJ, STL, PLY, FBX or 3MF file: path, up? (auto, y, z), scale? (metres per file unit)"),
    ("begin_sculpt", "Start sculpting a mesh object: id"),
    ("end_sculpt", "Stop sculpting"),
    ("sculpt_stroke", "Apply a brush stroke: id, stroke {brush {…, live_detail?: edge length}, dabs}"),
    ("sculpt_mask", "Clear, fill or invert the mask, or mask a patch: id, op"),
    ("sculpt_detail", "Step up or down a detail level, keeping the sculpted detail: id, op (up, down)"),
    (
        "sculpt_shape",
        "Edit shape layers: id, op (add {name?}, remove {index}, merge {index}, strength {index, strength}, select {index or null}, rename {index, name})",
    ),
    ("undo", "Undo the last change"),
    ("redo", "Redo the last undone change"),
];

fn is_auto(u: &modelcraft_formats::exchange::Up) -> bool {
    *u == modelcraft_formats::exchange::Up::Auto
}

impl Command {
    /// The English label for menus and the undo history.
    pub fn label(&self) -> String {
        match self {
            Command::AddPrimitive { kind, .. } => format!("Add {kind}"),
            Command::AddGroup { .. } => "Add group".into(),
            Command::AddSmoothCage { .. } => "Add smooth cage".into(),
            Command::AddDeformer { kind, .. } => format!("Add {kind}"),
            Command::Delete { .. } => "Delete".into(),
            Command::Duplicate { .. } => "Duplicate".into(),
            Command::Rename { .. } => "Rename".into(),
            Command::Reparent { .. } => "Move in tree".into(),
            Command::SetTransform { .. } => "Transform".into(),
            Command::SetVisible { .. } => "Visibility".into(),
            Command::SetMaterial { .. } => "Material".into(),
            Command::SetPrimitive { .. } => "Primitive settings".into(),
            Command::SetCageLevels { .. } => "Smooth cage levels".into(),
            Command::SetDeformer { .. } => "Deformer settings".into(),
            Command::AddStep { op, .. } => op.name().into(),
            Command::SetStep { op, .. } => format!("Edit {}", op.name()),
            Command::EnableStep { .. } => "Toggle step".into(),
            Command::RemoveStep { .. } => "Remove step".into(),
            Command::Collapse { .. } => "Collapse history".into(),
            Command::Import { .. } => "Import".into(),
            Command::BeginSculpt { .. } => "Start sculpting".into(),
            Command::EndSculpt => "Stop sculpting".into(),
            Command::SculptStroke { stroke, .. } => format!("{} stroke", stroke.brush.kind.label()),
            Command::SculptMask { .. } => "Mask".into(),
            Command::SculptDetail { op: DetailOp::Up, .. } => "Detail level up".into(),
            Command::SculptDetail { op: DetailOp::Down, .. } => "Detail level down".into(),
            Command::SculptShape { op, .. } => match op {
                ShapeOp::Add { .. } => "Add shape layer",
                ShapeOp::Remove { .. } => "Remove shape layer",
                ShapeOp::Merge { .. } => "Merge shape layer",
                ShapeOp::Strength { .. } => "Shape layer strength",
                ShapeOp::Select { .. } => "Select shape layer",
                ShapeOp::Rename { .. } => "Rename shape layer",
            }
            .into(),
            Command::Undo => "Undo".into(),
            Command::Redo => "Redo".into(),
            Command::Anim(a) => a.label(),
            Command::Rig(r) => r.label(),
        }
    }
}

/// What a command did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    /// Objects it created (the first is the main one).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<ObjectId>,
    /// Warnings worth showing (import repairs, stale steps).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}
