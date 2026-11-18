//! The `organize` command: tags, display layers, user attributes, attribute presets, pivots and
//! instances ([`modelcraft_scene::organize`]).
//!
//! `{"cmd":"organize","op":"set_tag","ids":[1],"tag":{"type":"protection","position":true,"rotation":false,"scale":false}}`

use std::sync::Arc;

use glam::{Quat, Vec3};
use modelcraft_scene::constraint::{ConstraintKind, Source as AimSource};
use modelcraft_scene::organize::{self, AttributePreset, DisplayLayer, PresetParts, Tag, TagKind, UserValue};
use modelcraft_scene::{ObjectId, ObjectKind, SceneError};
use serde::{Deserialize, Serialize};

use crate::command::Outcome;
use crate::document::{Document, EngineError};

/// Most objects one organize command touches.
pub const MAX_IDS: usize = 100_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OrganizeCommand {
    /// Put a tag on objects, replacing their tag of the same kind.
    SetTag {
        ids: Vec<ObjectId>,
        tag: Tag,
    },
    /// Take a tag off objects: `kind` is display, render, protection or look_at.
    RemoveTag {
        ids: Vec<ObjectId>,
        kind: String,
    },
    /// Add a display layer, and put `ids` on it.
    AddLayer {
        name: String,
        #[serde(default)]
        ids: Vec<ObjectId>,
    },
    /// Change a layer: any of its name, colour and its visible, render and locked switches.
    EditLayer {
        id: u32,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<[f32; 3]>,
        #[serde(default)]
        visible: Option<bool>,
        #[serde(default)]
        render: Option<bool>,
        #[serde(default)]
        locked: Option<bool>,
    },
    /// Delete a layer; its objects go back to no layer.
    RemoveLayer {
        id: u32,
    },
    /// Put objects on a layer (`None`: off any layer).
    AssignLayer {
        ids: Vec<ObjectId>,
        #[serde(default)]
        layer: Option<u32>,
    },
    /// Give objects a user attribute (or change its value).
    SetUserAttr {
        ids: Vec<ObjectId>,
        name: String,
        value: UserValue,
    },
    RemoveUserAttr {
        ids: Vec<ObjectId>,
        name: String,
    },
    /// Save parts of an object as a named preset (the default parts are the material, tags and
    /// user attributes).
    SavePreset {
        from: ObjectId,
        name: String,
        #[serde(default)]
        parts: Option<PresetParts>,
    },
    ApplyPreset {
        index: usize,
        ids: Vec<ObjectId>,
    },
    RemovePreset {
        index: usize,
    },
    /// Move an object's pivot to `pivot` (in the object's own space). With `keep_in_place` the
    /// object and its children stay where they are; otherwise they move with the pivot.
    SetPivot {
        ids: Vec<ObjectId>,
        pivot: [f32; 3],
        #[serde(default = "yes")]
        keep_in_place: bool,
    },
    /// Move pivots to the centre of the objects' bounding boxes (the object stays in place).
    PivotToCenter {
        ids: Vec<ObjectId>,
    },
    /// Add an instance of each object: it shows the same mesh, shared, under its own transform.
    Instance {
        ids: Vec<ObjectId>,
    },
    /// Turn instances into real, independent meshes.
    MakeUnique {
        ids: Vec<ObjectId>,
    },
}

fn yes() -> bool {
    true
}

impl OrganizeCommand {
    pub fn label(&self) -> String {
        match self {
            OrganizeCommand::SetTag { .. } => "Set tag",
            OrganizeCommand::RemoveTag { .. } => "Remove tag",
            OrganizeCommand::AddLayer { .. } => "Add layer",
            OrganizeCommand::EditLayer { .. } => "Edit layer",
            OrganizeCommand::RemoveLayer { .. } => "Remove layer",
            OrganizeCommand::AssignLayer { .. } => "Assign layer",
            OrganizeCommand::SetUserAttr { .. } => "Set user attribute",
            OrganizeCommand::RemoveUserAttr { .. } => "Remove user attribute",
            OrganizeCommand::SavePreset { .. } => "Save attribute preset",
            OrganizeCommand::ApplyPreset { .. } => "Apply attribute preset",
            OrganizeCommand::RemovePreset { .. } => "Remove attribute preset",
            OrganizeCommand::SetPivot { .. } => "Set pivot",
            OrganizeCommand::PivotToCenter { .. } => "Pivot to center",
            OrganizeCommand::Instance { .. } => "Instance",
            OrganizeCommand::MakeUnique { .. } => "Make unique",
        }
        .into()
    }

    /// The objects the command changes, to check them for protection.
    pub(crate) fn targets(&self) -> &[ObjectId] {
        match self {
            OrganizeCommand::SetTag { ids, .. }
            | OrganizeCommand::RemoveTag { ids, .. }
            | OrganizeCommand::AssignLayer { ids, .. }
            | OrganizeCommand::SetUserAttr { ids, .. }
            | OrganizeCommand::RemoveUserAttr { ids, .. }
            | OrganizeCommand::ApplyPreset { ids, .. }
            | OrganizeCommand::SetPivot { ids, .. }
            | OrganizeCommand::PivotToCenter { ids }
            | OrganizeCommand::MakeUnique { ids } => ids,
            _ => &[],
        }
    }
}

fn invalid<T>(m: impl Into<String>) -> Result<T, EngineError> {
    Err(EngineError::Invalid(m.into()))
}

fn need(doc: &Document, id: ObjectId) -> Result<(), EngineError> {
    if doc.scene.get(id).is_some() { Ok(()) } else { Err(SceneError::NoObject(id).into()) }
}

fn check_ids(doc: &Document, ids: &[ObjectId]) -> Result<(), EngineError> {
    if ids.is_empty() {
        return invalid("no objects given");
    }
    if ids.len() > MAX_IDS {
        return invalid("too many objects");
    }
    ids.iter().try_for_each(|id| need(doc, *id))
}

/// Refuse a command that changes a locked object (the lock is an object's or its layer's).
pub(crate) fn check_unlocked(doc: &Document, cmd: &OrganizeCommand) -> Result<(), EngineError> {
    for id in cmd.targets() {
        if doc.scene.is_locked(*id) {
            let name = doc.scene.get(*id).map_or("an object", |o| o.name.as_str());
            return invalid(format!("{name} is locked; unlock it first"));
        }
    }
    Ok(())
}

fn find_layer(doc: &mut Document, id: u32) -> Result<&mut DisplayLayer, EngineError> {
    doc.scene.layers.iter_mut().find(|l| l.id == id).ok_or_else(|| EngineError::Invalid(format!("no layer {id}")))
}

/// The master an instance of `id` should point at: the object itself, or its master when it is an
/// instance already.
fn master_of(doc: &Document, id: ObjectId) -> Result<ObjectId, EngineError> {
    match doc.scene.get(id).map(|o| &o.kind) {
        Some(ObjectKind::Instance { of }) => {
            if doc.scene.get(*of).is_some() {
                Ok(*of)
            } else {
                invalid("the master of that instance is gone")
            }
        }
        Some(ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. } | ObjectKind::Cloner { .. }) => Ok(id),
        Some(_) => invalid("only meshes, smooth cages and clone generators can be instanced"),
        None => Err(SceneError::NoObject(id).into()),
    }
}

fn user_name(name: &str) -> Result<String, EngineError> {
    let name = name.trim().to_string();
    if organize::valid_attr_name(&name) { Ok(name) } else { invalid("a user attribute name is 1 to 64 letters, digits, spaces, _ or -") }
}

pub fn apply(doc: &mut Document, cmd: &OrganizeCommand, out: &mut Outcome) -> Result<(), EngineError> {
    match cmd {
        OrganizeCommand::SetTag { ids, tag } => {
            check_ids(doc, ids)?;
            let mut tag = tag.clone();
            if let Tag::LookAt { target, .. } = &mut tag {
                need(doc, *target)?;
            }
            for id in ids {
                set_tag(doc, *id, tag.clone())?;
            }
        }
        OrganizeCommand::RemoveTag { ids, kind } => {
            check_ids(doc, ids)?;
            let kind = TagKind::from_id(kind).ok_or_else(|| EngineError::Invalid(format!("no tag kind {kind:?}: display, render, protection or look_at")))?;
            for id in ids {
                remove_tag(doc, *id, kind)?;
            }
        }
        OrganizeCommand::AddLayer { name, ids } => {
            let name = name.trim();
            if name.is_empty() {
                return invalid("a layer needs a name");
            }
            if doc.scene.layers.len() >= organize::MAX_LAYERS {
                return invalid("too many layers");
            }
            for id in ids {
                need(doc, *id)?;
            }
            let id = doc.scene.layers.iter().map(|l| l.id).max().map_or(1, |m| m.saturating_add(1));
            doc.scene.layers.push(DisplayLayer::new(id, name));
            for o in ids {
                doc.scene.edit(*o)?.layer = Some(id);
            }
        }
        OrganizeCommand::EditLayer { id, name, color, visible, render, locked } => {
            let l = find_layer(doc, *id)?;
            if let Some(n) = name {
                let n = n.trim();
                if n.is_empty() {
                    return invalid("a layer needs a name");
                }
                l.name = n.chars().take(organize::MAX_NAME).collect();
            }
            if let Some(c) = color {
                l.color = c.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.5 });
            }
            if let Some(v) = visible {
                l.visible = *v;
            }
            if let Some(v) = render {
                l.render = *v;
            }
            if let Some(v) = locked {
                l.locked = *v;
            }
            // Cached results do not depend on layers, but the objects on it changed what shows.
            let on: Vec<ObjectId> = doc.scene.objects().iter().filter(|o| o.layer == Some(*id)).map(|o| o.id).collect();
            for o in on {
                doc.scene.edit(o)?;
            }
        }
        OrganizeCommand::RemoveLayer { id } => {
            find_layer(doc, *id)?;
            doc.scene.layers.retain(|l| l.id != *id);
            let on: Vec<ObjectId> = doc.scene.objects().iter().filter(|o| o.layer == Some(*id)).map(|o| o.id).collect();
            for o in on {
                doc.scene.edit(o)?.layer = None;
            }
        }
        OrganizeCommand::AssignLayer { ids, layer } => {
            check_ids(doc, ids)?;
            if let Some(l) = layer
                && doc.scene.layer(*l).is_none()
            {
                return invalid(format!("no layer {l}"));
            }
            for id in ids {
                doc.scene.edit(*id)?.layer = *layer;
            }
        }
        OrganizeCommand::SetUserAttr { ids, name, value } => {
            check_ids(doc, ids)?;
            let name = user_name(name)?;
            let value = value.clone().sanitized();
            for id in ids {
                let o = doc.scene.edit(*id)?;
                if !o.user.contains_key(&name) && o.user.len() >= organize::MAX_USER_ATTRS {
                    return invalid(format!("{} already has {} user attributes", o.name, organize::MAX_USER_ATTRS));
                }
                o.user.insert(name.clone(), value.clone());
            }
        }
        OrganizeCommand::RemoveUserAttr { ids, name } => {
            check_ids(doc, ids)?;
            let mut any = false;
            for id in ids {
                any |= doc.scene.edit(*id)?.user.remove(name.trim()).is_some();
            }
            if !any {
                return invalid(format!("no object has a user attribute {:?}", name.trim()));
            }
        }
        OrganizeCommand::SavePreset { from, name, parts } => {
            if doc.scene.presets.len() >= organize::MAX_PRESETS {
                return invalid("too many presets");
            }
            let preset = AttributePreset::capture(&doc.scene, *from, name, parts.unwrap_or_default())?;
            // Saving under a name that exists replaces that preset.
            match doc.scene.presets.iter_mut().find(|p| p.name == preset.name) {
                Some(p) => *p = preset,
                None => doc.scene.presets.push(preset),
            }
        }
        OrganizeCommand::ApplyPreset { index, ids } => {
            check_ids(doc, ids)?;
            let preset = doc.scene.presets.get(*index).cloned().ok_or_else(|| EngineError::Invalid(format!("no preset {index}")))?;
            for id in ids {
                preset.apply(&mut doc.scene, *id)?;
            }
        }
        OrganizeCommand::RemovePreset { index } => {
            if *index >= doc.scene.presets.len() {
                return invalid(format!("no preset {index}"));
            }
            doc.scene.presets.remove(*index);
        }
        OrganizeCommand::SetPivot { ids, pivot, keep_in_place } => {
            check_ids(doc, ids)?;
            let p = Vec3::from_array(*pivot);
            if !p.is_finite() {
                return invalid("the pivot must be finite numbers");
            }
            for id in ids {
                set_pivot(doc, *id, p, *keep_in_place)?;
            }
        }
        OrganizeCommand::PivotToCenter { ids } => {
            check_ids(doc, ids)?;
            for id in ids {
                let mesh = doc.eval.evaluate(&doc.scene, *id).mesh;
                let Some((lo, hi)) = organize::bounds(mesh.positions()) else {
                    return invalid(format!("{} has no geometry to centre on", doc.scene.get(*id).map_or("the object", |o| o.name.as_str())));
                };
                set_pivot(doc, *id, (lo + hi) * 0.5, true)?;
            }
        }
        OrganizeCommand::Instance { ids } => {
            check_ids(doc, ids)?;
            let masters: Vec<ObjectId> = ids.iter().map(|id| master_of(doc, *id)).collect::<Result<_, _>>()?;
            for master in masters {
                let Some(m) = doc.scene.get(master).cloned() else { continue };
                let name = doc.scene.unique_name(&format!("{} instance", m.name));
                let id = doc.scene.add(name, ObjectKind::Instance { of: master }, m.parent)?;
                let n = doc.scene.edit(id)?;
                n.transform = m.transform;
                n.pivot = m.pivot;
                n.material = m.material.clone();
                n.layer = m.layer;
                out.created.push(id);
            }
        }
        OrganizeCommand::MakeUnique { ids } => {
            check_ids(doc, ids)?;
            for id in ids {
                make_unique(doc, *id)?;
            }
        }
    }
    Ok(())
}

fn set_tag(doc: &mut Document, id: ObjectId, mut tag: Tag) -> Result<(), EngineError> {
    // A look-at tag is an aim constraint; replacing the tag replaces the constraint.
    if let Some(Tag::LookAt { constraint, .. }) = doc.scene.tag(id, TagKind::LookAt).cloned() {
        doc.scene.constraints.remove(constraint);
    }
    if let Tag::LookAt { target, constraint } = &mut tag {
        if *target == id {
            return invalid("an object cannot look at itself");
        }
        let kind = ConstraintKind::Aim { sources: vec![AimSource::new(*target)], aim: 2, up: 1, up_object: None, up_vector: Vec3::Y, offset: Quat::IDENTITY };
        let name = doc.scene.get(id).map_or_else(|| "Look at".to_string(), |o| format!("{} look at", o.name));
        *constraint = doc.scene.constraints.add(&name, id, kind)?;
    }
    let o = doc.scene.edit(id)?;
    match o.tags.iter().position(|t| t.kind() == tag.kind()) {
        Some(i) => o.tags[i] = tag,
        None if o.tags.len() >= organize::MAX_TAGS => return invalid(format!("{} already has {} tags", o.name, organize::MAX_TAGS)),
        None => o.tags.push(tag),
    }
    doc.scene.solve_constraints();
    Ok(())
}

fn remove_tag(doc: &mut Document, id: ObjectId, kind: TagKind) -> Result<(), EngineError> {
    if let Some(Tag::LookAt { constraint, .. }) = doc.scene.tag(id, TagKind::LookAt).cloned()
        && kind == TagKind::LookAt
    {
        doc.scene.constraints.remove(constraint);
    }
    doc.scene.edit(id)?.tags.retain(|t| t.kind() != kind);
    doc.scene.solve_constraints();
    Ok(())
}

/// Move the pivot; with `keep` the object does not move (its translation changes to match).
fn set_pivot(doc: &mut Document, id: ObjectId, pivot: Vec3, keep: bool) -> Result<(), EngineError> {
    if doc.scene.edit(id)?.set_pivot(pivot, keep) { Ok(()) } else { invalid("the pivot would place the object out of range") }
}

/// Replace an instance with a mesh that holds a copy of what it shows.
fn make_unique(doc: &mut Document, id: ObjectId) -> Result<(), EngineError> {
    let Some(ObjectKind::Instance { of }) = doc.scene.get(id).map(|o| o.kind.clone()) else {
        return invalid(format!("{} is not an instance", doc.scene.get(id).map_or("the object", |o| o.name.as_str())));
    };
    realize(doc, id, of)
}

/// Make `id` (an instance of `master`) a real mesh holding the master's evaluated mesh.
fn realize(doc: &mut Document, id: ObjectId, master: ObjectId) -> Result<(), EngineError> {
    let e = doc.eval.evaluate(&doc.scene, master);
    if e.instances.is_some() {
        return invalid("an instance of a clone generator cannot become a mesh; duplicate the generator instead");
    }
    let mesh = (*e.mesh).clone();
    doc.scene.edit(id)?.kind = ObjectKind::Mesh { source: modelcraft_scene::Source::Mesh { mesh: Arc::new(mesh) }, steps: Vec::new() };
    Ok(())
}

/// Before objects are deleted: instances of them (that stay) become real meshes, so deleting a
/// master does not leave an empty shell.
pub(crate) fn realize_instances_of(doc: &mut Document, doomed: &[ObjectId]) {
    let mut gone = std::collections::HashSet::new();
    for id in doomed {
        gone.extend(doc.scene.subtree(*id));
    }
    let orphans: Vec<(ObjectId, ObjectId)> = doc
        .scene
        .objects()
        .iter()
        .filter_map(|o| match o.kind {
            ObjectKind::Instance { of } if gone.contains(&of) && !gone.contains(&o.id) => Some((o.id, of)),
            _ => None,
        })
        .collect();
    for (id, master) in orphans {
        if realize(doc, id, master).is_err() {
            // A generator's instance cannot be a mesh; it becomes an empty group.
            if let Ok(o) = doc.scene.edit(id) {
                o.kind = ObjectKind::Group;
            }
        }
    }
}

/// The `op` names of the `organize` command with one-line descriptions.
pub const COMMANDS: &[(&str, &str)] = &[
    ("set_tag", "Tag objects: ids, tag {type: display + mode | render + visible | protection + position, rotation, scale | look_at + target}"),
    ("remove_tag", "Take a tag off: ids, kind (display, render, protection, look_at)"),
    ("add_layer", "Add a display layer: name, ids? (objects to put on it)"),
    ("edit_layer", "Change a layer: id, name?, color?, visible?, render?, locked?"),
    ("remove_layer", "Delete a layer (its objects stay): id"),
    ("assign_layer", "Put objects on a layer: ids, layer? (none: off any layer)"),
    ("set_user_attr", "Give objects a named value: ids, name, value {type: number | switch | text | colour | vector, value}"),
    ("remove_user_attr", "Remove a user attribute: ids, name"),
    ("save_preset", "Save an object's attributes as a preset: from, name, parts? {transform, material, visible, tags, user}"),
    ("apply_preset", "Put a preset on objects: index, ids"),
    ("remove_preset", "Delete a preset: index"),
    ("set_pivot", "Move the pivot (object space): ids, pivot [x,y,z], keep_in_place? (default true)"),
    ("pivot_to_center", "Move pivots to the centre of the bounding boxes: ids"),
    ("instance", "Add instances that share the object's mesh: ids"),
    ("make_unique", "Turn instances into independent meshes: ids"),
];
