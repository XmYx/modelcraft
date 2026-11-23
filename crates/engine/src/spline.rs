//! Spline commands. One command, `spline`, with the operation in `op`:
//! `{"cmd":"spline","op":"add","kind":"circle"}`.
//!
//! A spline is an object ([`modelcraft_scene::splines`]): a primitive's numbers or points edited
//! by hand, and how finely the curve is tessellated. Generators (extrude, lathe, loft, sweep)
//! read spline children and follow them when they change.

use std::sync::Arc;

use modelcraft_scene::splines::SplineSource;
use modelcraft_scene::{ObjectId, ObjectKind, SceneError, Spline, SplineGen, SplinePrimitive, Tessellation};
use modelcraft_spline::SplineEdit;
use serde::{Deserialize, Serialize};

use crate::command::Outcome;
use crate::document::{Document, EngineError, invalid};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SplineCommand {
    /// Add a spline object: a primitive (`circle`, `rectangle`, `star`, `n_side`, `arc`,
    /// `helix`), or `empty` for a curve with no points yet (to draw into).
    Add {
        kind: String,
        #[serde(default)]
        parent: Option<ObjectId>,
        #[serde(default)]
        name: Option<String>,
    },
    /// Change a primitive spline's numbers.
    SetPrimitive { id: ObjectId, primitive: SplinePrimitive },
    /// Change how finely the curve is tessellated (fixed steps, uniform length or adaptive).
    SetTessellation { id: ObjectId, tess: Tessellation },
    /// Turn a primitive into points and handles that can be edited (the shape does not change).
    MakeEditable { id: ObjectId },
    /// Replace the curve with this one (segments, knots, handles); a primitive becomes editable.
    SetCurve { id: ObjectId, spline: Spline },
    /// Edit the points: add, move, set a handle, insert, remove, close, change the curve type
    /// ([`SplineEdit`]). A primitive becomes editable points first.
    Edit { id: ObjectId, edit: SplineEdit },
    /// Add a generator (`extrude`, `revolve`, `loft`, `sweep`) that shows a surface made from the
    /// splines inside it. The listed spline objects move inside it; with none, it starts with
    /// example splines so that a first click shows something.
    AddGenerator {
        kind: String,
        #[serde(default)]
        inputs: Vec<ObjectId>,
    },
    /// Change a generator's options.
    SetGenerator { id: ObjectId, generator: SplineGen },
    /// A mesh object with a flat face filling the closed curves of a spline (holes by nesting).
    ToMesh { id: ObjectId },
}

/// The `op` names of the `spline` command with one-line descriptions.
pub const COMMANDS: &[(&str, &str)] = &[
    ("add", "Add a spline: kind (circle, rectangle, star, n_side, arc, helix, empty), parent?, name?"),
    ("set_primitive", "Change a primitive spline's numbers: id, primitive {kind, …}"),
    ("set_tessellation", "Change the curve's tessellation: id, tess {mode: fixed {steps} | uniform {length} | adaptive {tolerance, angle_deg}}"),
    ("make_editable", "Turn a primitive spline into editable points and handles: id"),
    ("set_curve", "Replace the curve: id, spline {segments [{kind (linear, bezier, b_spline), closed, knots [{pos, tan_in, tan_out, mode}]}]}"),
    ("add_generator", "Add a generator (extrude, revolve, loft, sweep) around the spline objects inputs?; without inputs it starts with example splines"),
    (
        "set_generator",
        "Change a generator's options: id, generator {kind: extrude {distance, axis, steps, cap_start, cap_end} | revolve {angle, steps, axis, caps} | loft {steps, smooth, caps, points} | sweep {end_scale, twist, roll, caps}}",
    ),
    ("to_mesh", "Make a mesh object with a flat face filling the closed curves of a spline (holes by nesting): id"),
    (
        "edit",
        "Edit the points of a spline: id, edit {edit: append {segment, knot, kind?} | move_knots {moves [{at {segment, knot}, pos}]} | translate {knots, by} | set_handle {at, side (in, out), offset} | set_mode {at, mode (free, aligned, mirrored, auto)} | set_kind {segment, kind} | set_closed {segment, closed} | insert {segment, u} | remove {knots} | add_segment {segment} | remove_segment {segment} | reverse {segment} | chamfer {knots, distance} | fillet {knots, radius} | to_polygon {tess}}",
    ),
];

fn edit_label(e: &SplineEdit) -> &'static str {
    match e {
        SplineEdit::Append { .. } => "Add spline point",
        SplineEdit::MoveKnots { .. } | SplineEdit::Translate { .. } => "Move spline points",
        SplineEdit::SetHandle { .. } | SplineEdit::SetMode { .. } => "Spline handle",
        SplineEdit::SetKind { .. } => "Spline curve type",
        SplineEdit::SetClosed { .. } => "Open or close spline",
        SplineEdit::Insert { .. } => "Insert spline point",
        SplineEdit::Remove { .. } => "Delete spline points",
        SplineEdit::AddSegment { .. } | SplineEdit::RemoveSegment { .. } => "Spline curves",
        SplineEdit::Reverse { .. } => "Reverse spline",
        SplineEdit::Chamfer { .. } => "Chamfer spline points",
        SplineEdit::Fillet { .. } => "Fillet spline points",
        SplineEdit::ToPolygon { .. } => "Convert spline to polygon",
    }
}

impl SplineCommand {
    pub fn label(&self) -> String {
        match self {
            SplineCommand::Add { kind, .. } => format!("Add {kind} spline"),
            SplineCommand::SetPrimitive { .. } => "Spline shape".into(),
            SplineCommand::SetTessellation { .. } => "Spline tessellation".into(),
            SplineCommand::MakeEditable { .. } => "Make spline editable".into(),
            SplineCommand::SetCurve { .. } => "Edit spline".into(),
            SplineCommand::Edit { edit, .. } => edit_label(edit).into(),
            SplineCommand::AddGenerator { kind, .. } => format!("Add {kind}"),
            SplineCommand::SetGenerator { .. } => "Spline generator settings".into(),
            SplineCommand::ToMesh { .. } => "Spline to mesh".into(),
        }
    }

    /// The object the command changes (for the lock check).
    pub fn target(&self) -> Option<ObjectId> {
        match self {
            SplineCommand::Add { .. } | SplineCommand::AddGenerator { .. } => None,
            SplineCommand::SetPrimitive { id, .. }
            | SplineCommand::SetTessellation { id, .. }
            | SplineCommand::MakeEditable { id }
            | SplineCommand::SetCurve { id, .. }
            | SplineCommand::Edit { id, .. }
            | SplineCommand::SetGenerator { id, .. }
            | SplineCommand::ToMesh { id } => Some(*id),
        }
    }
}

fn title(kind: &str) -> String {
    match kind {
        "n_side" => "N-Side".into(),
        "empty" => "Spline".into(),
        k => {
            let mut c = k.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

pub fn apply(doc: &mut Document, cmd: &SplineCommand, out: &mut Outcome) -> Result<(), EngineError> {
    match cmd {
        SplineCommand::Add { kind, parent, name } => {
            let source = match kind.as_str() {
                "empty" => SplineSource::Curve { spline: Arc::new(Spline::default()) },
                k => SplineSource::Primitive {
                    primitive: SplinePrimitive::default_of(k).ok_or_else(|| EngineError::Invalid(format!("unknown spline {kind:?}")))?,
                },
            };
            let base = name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| title(kind), str::to_string);
            let name = doc.scene.unique_name(&base);
            let id = doc.scene.add(name, ObjectKind::Spline { source, tess: Tessellation::default() }, *parent)?;
            out.created.push(id);
        }
        SplineCommand::SetPrimitive { id, primitive } => match &mut doc.scene.edit(*id)?.kind {
            ObjectKind::Spline { source: SplineSource::Primitive { primitive: p }, .. } => *p = primitive.sanitized(),
            ObjectKind::Spline { .. } => return invalid("this spline has its own points; its shape is not a primitive any more"),
            _ => return invalid("not a spline"),
        },
        SplineCommand::SetTessellation { id, tess } => match &mut doc.scene.edit(*id)?.kind {
            ObjectKind::Spline { tess: t, .. } => *t = tess.sanitized(),
            _ => return invalid("not a spline"),
        },
        SplineCommand::MakeEditable { id } => match &mut doc.scene.edit(*id)?.kind {
            ObjectKind::Spline { source, .. } => {
                let spline = source.spline();
                *source = SplineSource::Curve { spline };
            }
            _ => return invalid("not a spline"),
        },
        SplineCommand::SetCurve { id, spline } => {
            spline.validate().map_err(|e| EngineError::Invalid(e.to_string()))?;
            match &mut doc.scene.edit(*id)?.kind {
                ObjectKind::Spline { source, .. } => *source = SplineSource::Curve { spline: Arc::new(spline.clone()) },
                _ => return invalid("not a spline"),
            }
        }
        SplineCommand::Edit { id, edit } => match &mut doc.scene.edit(*id)?.kind {
            ObjectKind::Spline { source, .. } => {
                let mut spline = (*source.spline()).clone();
                spline.apply(edit).map_err(|e| EngineError::Invalid(e.to_string()))?;
                *source = SplineSource::Curve { spline: Arc::new(spline) };
            }
            _ => return invalid("not a spline"),
        },
        SplineCommand::AddGenerator { kind, inputs } => add_generator(doc, kind, inputs, out)?,
        SplineCommand::ToMesh { id } => {
            let (name, parent, transform) = match doc.scene.get(*id) {
                Some(o) if matches!(o.kind, ObjectKind::Spline { .. }) => (o.name.clone(), o.parent, o.transform),
                Some(_) => return invalid("not a spline"),
                None => return Err(SceneError::NoObject(*id).into()),
            };
            let eval = doc.eval.spline(&doc.scene, *id).ok_or_else(|| EngineError::Invalid("not a spline".into()))?;
            if let Some(e) = &eval.error {
                return invalid(e.clone());
            }
            let mesh = modelcraft_ops::spline_gen::fill(&eval.lines).map_err(|e| EngineError::Invalid(e.to_string()))?;
            let unique = doc.scene.unique_name(&format!("{name} Mesh"));
            let new = doc.scene.add(unique, ObjectKind::Mesh { source: modelcraft_scene::Source::Mesh { mesh: Arc::new(mesh) }, steps: vec![] }, parent)?;
            doc.scene.edit(new)?.transform = transform;
            out.created.push(new);
        }
        SplineCommand::SetGenerator { id, generator } => match &mut doc.scene.edit(*id)?.kind {
            ObjectKind::SplineGenerator { generator: g } => *g = generator.sanitized(),
            _ => return invalid("not a spline generator"),
        },
    }
    Ok(())
}

/// Example splines for a generator that starts with none: a first click shows a surface.
fn demo_inputs(doc: &mut Document, generator: ObjectId, kind: &str) -> Result<(), EngineError> {
    use modelcraft_scene::Transform;
    use modelcraft_spline::Plane;
    let add = |doc: &mut Document, name: &str, primitive: SplinePrimitive, translation: glam::Vec3, scale: f32| -> Result<(), EngineError> {
        let n = doc.scene.unique_name(name);
        let id = doc.scene.add(n, ObjectKind::Spline { source: SplineSource::Primitive { primitive }, tess: Tessellation::default() }, Some(generator))?;
        doc.scene.edit(id)?.transform = Transform { translation, scale: glam::Vec3::splat(scale), ..Transform::default() };
        Ok(())
    };
    let circle = SplinePrimitive::Circle { radius_x: 1.0, radius_y: 1.0, plane: Plane::Xy };
    match kind {
        "extrude" => add(doc, "Star", SplinePrimitive::Star { points: 5, inner: 0.45, outer: 1.0, plane: Plane::Xy }, glam::Vec3::ZERO, 1.0),
        "revolve" => {
            add(doc, "Profile", SplinePrimitive::Rectangle { width: 1.0, height: 2.0, corner: 0.0, plane: Plane::Xy }, glam::Vec3::new(1.5, 0.0, 0.0), 1.0)
        }
        "loft" => {
            add(doc, "Circle", circle, glam::Vec3::ZERO, 1.0)?;
            add(doc, "Circle", circle, glam::Vec3::new(0.0, 0.0, 2.0), 0.5)
        }
        _ => {
            add(doc, "Profile", circle, glam::Vec3::ZERO, 0.25)?;
            add(
                doc,
                "Path",
                SplinePrimitive::Helix { radius_start: 1.0, radius_end: 1.0, height: 3.0, turns: 3.0, knots_per_turn: 12, plane: Plane::Zx },
                glam::Vec3::ZERO,
                1.0,
            )
        }
    }
}

fn add_generator(doc: &mut Document, kind: &str, inputs: &[ObjectId], out: &mut Outcome) -> Result<(), EngineError> {
    let generator = SplineGen::default_of(kind).ok_or_else(|| EngineError::Invalid(format!("unknown spline generator {kind:?}")))?;
    if inputs.len() > modelcraft_ops::spline_gen::MAX_INPUTS {
        return invalid("too many splines for one generator");
    }
    for (i, id) in inputs.iter().enumerate() {
        if !doc.scene.is_spline(*id) {
            return invalid("a spline generator takes spline objects as its input");
        }
        if inputs[..i].contains(id) {
            return invalid("a spline is listed twice");
        }
    }
    let parent = inputs.first().and_then(|i| doc.scene.get(*i)).and_then(|o| o.parent);
    let base = match kind {
        "extrude" => "Extrude",
        "revolve" => "Revolve",
        "loft" => "Loft",
        _ => "Sweep",
    };
    let name = doc.scene.unique_name(base);
    let id = doc.scene.add(name, ObjectKind::SplineGenerator { generator }, parent)?;
    if inputs.is_empty() {
        demo_inputs(doc, id, kind)?;
    } else {
        for i in inputs {
            doc.scene.reparent(*i, Some(id))?;
        }
    }
    out.warnings.extend(doc.eval.evaluate(&doc.scene, id).errors.iter().map(|e| e.message.clone()));
    out.created.push(id);
    Ok(())
}

/// Refuse an edit of a locked spline.
pub(crate) fn check_unlocked(doc: &Document, cmd: &SplineCommand) -> Result<(), EngineError> {
    if let Some(id) = cmd.target() {
        if doc.scene.get(id).is_none() {
            return Err(SceneError::NoObject(id).into());
        }
        if doc.scene.is_locked(id) {
            let name = doc.scene.get(id).map_or("an object", |o| o.name.as_str());
            return invalid(format!("{name} is locked; unlock it first"));
        }
    }
    Ok(())
}
