//! Organising objects: tags, display layers, user attributes and attribute presets.
//!
//! - A **tag** gives one object a behaviour: how it is drawn in the editor ([`Tag::Display`]),
//!   whether renders include it ([`Tag::Render`]), which transform channels are protected
//!   ([`Tag::Protection`]) and which object it keeps facing ([`Tag::LookAt`], backed by an aim
//!   constraint).
//! - A **display layer** ([`DisplayLayer`]) is a named switch for many objects at once: shown or
//!   hidden, rendered or viewport-only, locked or free. An object is on at most one layer.
//! - **User attributes** are named values an object carries (a number, a switch, text, a colour
//!   or a vector). Numbers and switches are reachable as `user.<name>` attributes, so they can be
//!   keyed and scripted like any other number.
//! - An **attribute preset** is a named set of attributes saved from one object, applied to
//!   others.

use std::collections::BTreeMap;

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::{Material, ObjectId, Scene, SceneError, Transform};

/// Most tags on one object.
pub const MAX_TAGS: usize = 16;
/// Most display layers in a scene.
pub const MAX_LAYERS: usize = 256;
/// Most user attributes on one object.
pub const MAX_USER_ATTRS: usize = 64;
/// Most attribute presets in a scene.
pub const MAX_PRESETS: usize = 256;
/// Longest name of a layer, preset or user attribute.
pub const MAX_NAME: usize = 64;
/// Longest text value.
pub const MAX_TEXT: usize = 1024;

/// How surfaces are drawn in the editor. Per object through [`Tag::Display`]; for the whole view
/// through the viewport's display mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    /// Follow the view (for a tag), or smooth shading (for the view).
    #[default]
    Auto,
    /// Shaded with smoothed normals.
    Smooth,
    /// Shaded with one normal per face.
    Flat,
    /// Edges only.
    Wireframe,
    /// Faint surface with edges, the objects behind show through.
    SeeThrough,
    /// The object's bounding box only.
    BoundingBox,
}

impl DisplayMode {
    pub const ALL: [DisplayMode; 6] =
        [DisplayMode::Auto, DisplayMode::Smooth, DisplayMode::Flat, DisplayMode::Wireframe, DisplayMode::SeeThrough, DisplayMode::BoundingBox];

    pub fn id(self) -> &'static str {
        match self {
            DisplayMode::Auto => "auto",
            DisplayMode::Smooth => "smooth",
            DisplayMode::Flat => "flat",
            DisplayMode::Wireframe => "wireframe",
            DisplayMode::SeeThrough => "see_through",
            DisplayMode::BoundingBox => "bounding_box",
        }
    }

    pub fn from_id(s: &str) -> Option<DisplayMode> {
        DisplayMode::ALL.into_iter().find(|m| m.id() == s.trim())
    }
}

/// A behaviour attached to one object. An object has at most one tag of each kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Tag {
    /// The editor draws this object in `mode` whatever the view's mode is.
    Display { mode: DisplayMode },
    /// Whether renders include the object (it still shows in the viewport when `false`).
    Render { visible: bool },
    /// The checked transform channels cannot be changed until the tag is removed.
    Protection { position: bool, rotation: bool, scale: bool },
    /// The object's +Z axis keeps pointing at `target`. `constraint` is the aim constraint that
    /// does it (kept in step by the engine).
    LookAt {
        target: ObjectId,
        #[serde(default)]
        constraint: u32,
    },
}

/// The kinds of [`Tag`], for adding and removing by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TagKind {
    Display,
    Render,
    Protection,
    LookAt,
}

impl TagKind {
    pub const ALL: [TagKind; 4] = [TagKind::Display, TagKind::Render, TagKind::Protection, TagKind::LookAt];

    pub fn id(self) -> &'static str {
        match self {
            TagKind::Display => "display",
            TagKind::Render => "render",
            TagKind::Protection => "protection",
            TagKind::LookAt => "look_at",
        }
    }

    pub fn from_id(s: &str) -> Option<TagKind> {
        TagKind::ALL.into_iter().find(|k| k.id() == s.trim())
    }
}

impl Tag {
    pub fn kind(&self) -> TagKind {
        match self {
            Tag::Display { .. } => TagKind::Display,
            Tag::Render { .. } => TagKind::Render,
            Tag::Protection { .. } => TagKind::Protection,
            Tag::LookAt { .. } => TagKind::LookAt,
        }
    }

    /// The tag a new object of that kind starts with.
    pub fn default_of(kind: TagKind) -> Option<Tag> {
        Some(match kind {
            TagKind::Display => Tag::Display { mode: DisplayMode::Wireframe },
            TagKind::Render => Tag::Render { visible: false },
            TagKind::Protection => Tag::Protection { position: true, rotation: true, scale: true },
            // Needs a target.
            TagKind::LookAt => return None,
        })
    }
}

/// A named switch for many objects.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayLayer {
    pub id: u32,
    pub name: String,
    /// Colour of the layer's badge in the object manager.
    #[serde(default = "layer_colour")]
    pub color: [f32; 3],
    /// Hidden layers hide their objects in the viewport and in renders.
    #[serde(default = "yes")]
    pub visible: bool,
    /// Off: the objects show in the viewport but renders leave them out.
    #[serde(default = "yes")]
    pub render: bool,
    /// Locked layers protect their objects from edits.
    #[serde(default)]
    pub locked: bool,
}

fn yes() -> bool {
    true
}

fn layer_colour() -> [f32; 3] {
    [0.45, 0.62, 0.9]
}

/// A value an object carries under a name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum UserValue {
    Number(f64),
    Switch(bool),
    Text(String),
    Colour([f32; 3]),
    Vector([f32; 3]),
}

impl UserValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            UserValue::Number(_) => "number",
            UserValue::Switch(_) => "switch",
            UserValue::Text(_) => "text",
            UserValue::Colour(_) => "colour",
            UserValue::Vector(_) => "vector",
        }
    }

    /// A value of the named type with its starting content.
    pub fn default_of(type_name: &str) -> Option<UserValue> {
        Some(match type_name.trim() {
            "number" => UserValue::Number(0.0),
            "switch" => UserValue::Switch(false),
            "text" => UserValue::Text(String::new()),
            "colour" | "color" => UserValue::Colour([1.0; 3]),
            "vector" => UserValue::Vector([0.0; 3]),
            _ => return None,
        })
    }

    /// Replace non-finite numbers and cut long text.
    pub fn sanitized(self) -> UserValue {
        let fin = |v: f32| if v.is_finite() { v } else { 0.0 };
        match self {
            UserValue::Number(n) => UserValue::Number(if n.is_finite() { n } else { 0.0 }),
            UserValue::Text(t) => UserValue::Text(t.chars().take(MAX_TEXT).collect()),
            UserValue::Colour(c) => UserValue::Colour(c.map(fin)),
            UserValue::Vector(v) => UserValue::Vector(v.map(fin)),
            s @ UserValue::Switch(_) => s,
        }
    }

    /// The value as one number, for numbers and switches (the keyable ones).
    pub fn as_number(&self) -> Option<f64> {
        match self {
            UserValue::Number(n) => Some(*n),
            UserValue::Switch(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
}

/// Whether `name` can name a user attribute: 1 to [`MAX_NAME`] letters, digits, spaces, `_` or `-`.
pub fn valid_attr_name(name: &str) -> bool {
    let t = name.trim();
    !t.is_empty() && t == name && name.chars().count() <= MAX_NAME && name.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-'))
}

/// A named set of attributes saved from one object. Parts left `None` are not touched when the
/// preset is applied.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AttributePreset {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<Transform>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<Material>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// Tags other than look-at (that one names an object and a constraint of its own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<Tag>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<BTreeMap<String, UserValue>>,
}

/// Which parts of an object a preset keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetParts {
    pub transform: bool,
    pub material: bool,
    pub visible: bool,
    pub tags: bool,
    pub user: bool,
}

impl Default for PresetParts {
    fn default() -> Self {
        PresetParts { transform: false, material: true, visible: false, tags: true, user: true }
    }
}

impl AttributePreset {
    /// Save the `parts` of an object under `name`.
    pub fn capture(scene: &Scene, id: ObjectId, name: &str, parts: PresetParts) -> Result<AttributePreset, SceneError> {
        let o = scene.get(id).ok_or(SceneError::NoObject(id))?;
        let name: String = name.trim().chars().take(MAX_NAME).collect();
        if name.is_empty() {
            return Err(SceneError::Invalid("a preset needs a name".into()));
        }
        Ok(AttributePreset {
            name,
            transform: parts.transform.then_some(o.transform),
            material: parts.material.then(|| o.material.clone()),
            visible: parts.visible.then_some(o.visible),
            tags: parts.tags.then(|| o.tags.iter().filter(|t| !matches!(t, Tag::LookAt { .. })).cloned().collect()),
            user: parts.user.then(|| o.user.clone()),
        })
    }

    /// The parts this preset holds.
    pub fn parts(&self) -> PresetParts {
        PresetParts {
            transform: self.transform.is_some(),
            material: self.material.is_some(),
            visible: self.visible.is_some(),
            tags: self.tags.is_some(),
            user: self.user.is_some(),
        }
    }

    /// Put the saved parts on an object. A look-at tag the object has is kept; user attributes
    /// of the preset are added or replaced, the object's other ones stay.
    pub fn apply(&self, scene: &mut Scene, id: ObjectId) -> Result<(), SceneError> {
        let o = scene.edit(id)?;
        if let Some(t) = self.transform {
            o.transform = t.sanitized();
        }
        if let Some(m) = &self.material {
            o.material = m.clone();
        }
        if let Some(v) = self.visible {
            o.visible = v;
        }
        if let Some(tags) = &self.tags {
            o.tags.retain(|t| matches!(t, Tag::LookAt { .. }));
            for t in tags.iter().take(MAX_TAGS) {
                if !o.tags.iter().any(|x| x.kind() == t.kind()) {
                    o.tags.push(t.clone());
                }
            }
        }
        if let Some(user) = &self.user {
            for (k, v) in user {
                if o.user.contains_key(k) || o.user.len() < MAX_USER_ATTRS {
                    o.user.insert(k.clone(), v.clone().sanitized());
                }
            }
        }
        Ok(())
    }
}

impl DisplayLayer {
    pub fn new(id: u32, name: &str) -> DisplayLayer {
        DisplayLayer { id, name: name.trim().chars().take(MAX_NAME).collect(), color: layer_colour(), visible: true, render: true, locked: false }
    }
}

/// The bounding box of an object's mesh points, as (min, max); `None` for an empty mesh.
pub fn bounds(points: &[Vec3]) -> Option<(Vec3, Vec3)> {
    let mut it = points.iter().filter(|p| p.is_finite());
    let first = *it.next()?;
    Some(it.fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p))))
}

impl Scene {
    /// The layer an object is on.
    pub fn layer_of(&self, id: ObjectId) -> Option<&DisplayLayer> {
        let l = self.get(id)?.layer?;
        self.layers.iter().find(|x| x.id == l)
    }

    /// The layer with this id.
    pub fn layer(&self, id: u32) -> Option<&DisplayLayer> {
        self.layers.iter().find(|l| l.id == id)
    }

    /// The tag of a kind on an object.
    pub fn tag(&self, id: ObjectId, kind: TagKind) -> Option<&Tag> {
        self.get(id)?.tags.iter().find(|t| t.kind() == kind)
    }

    /// The display mode an object asks for through its display tag (`Auto` when it has none).
    pub fn display_mode_of(&self, id: ObjectId) -> DisplayMode {
        match self.tag(id, TagKind::Display) {
            Some(Tag::Display { mode }) => *mode,
            _ => DisplayMode::Auto,
        }
    }

    /// Whether renders include the object: it is shown, no render tag on it or an ancestor turns
    /// it off, and its layer renders.
    pub fn renders(&self, id: ObjectId) -> bool {
        if !self.visible(id) {
            return false;
        }
        let mut c = self.get(id);
        for _ in 0..crate::MAX_DEPTH {
            let Some(o) = c else { return true };
            if o.tags.iter().any(|t| matches!(t, Tag::Render { visible: false })) {
                return false;
            }
            if o.layer.and_then(|l| self.layer(l)).is_some_and(|l| !l.render) {
                return false;
            }
            c = o.parent.and_then(|p| self.get(p));
        }
        true
    }

    /// Whether edits of the object are refused: it is locked or its layer is.
    pub fn is_locked(&self, id: ObjectId) -> bool {
        let Some(o) = self.get(id) else { return false };
        o.locked || o.layer.and_then(|l| self.layer(l)).is_some_and(|l| l.locked)
    }

    /// Why the transform change from `before` to `after` is refused by a protection tag.
    pub fn protected_channel(&self, id: ObjectId, before: &Transform, after: &Transform) -> Option<&'static str> {
        let Some(Tag::Protection { position, rotation, scale }) = self.tag(id, TagKind::Protection) else { return None };
        if *position && before.translation != after.translation {
            Some("position")
        } else if *rotation && before.rotation != after.rotation {
            Some("rotation")
        } else if *scale && before.scale != after.scale {
            Some("scale")
        } else {
            None
        }
    }

    /// Drop references that no longer hold: look-at tags whose target is gone, layers of objects
    /// that name a missing layer, empty-named presets.
    pub fn prune_organization(&mut self) {
        let ids: std::collections::HashSet<ObjectId> = self.objects().iter().map(|o| o.id).collect();
        let layer_ids: std::collections::HashSet<u32> = self.layers.iter().map(|l| l.id).collect();
        let constraints: std::collections::HashSet<u32> = self.constraints.list().iter().map(|c| c.id).collect();
        for o in self.objects_mut_unbumped() {
            o.tags.retain(|t| !matches!(t, Tag::LookAt { target, constraint } if !ids.contains(target) || !constraints.contains(constraint)));
            if o.layer.is_some_and(|l| !layer_ids.contains(&l)) {
                o.layer = None;
            }
            // An instance whose master is gone shows nothing; it becomes an empty group.
            if matches!(o.kind, crate::ObjectKind::Instance { of } if !ids.contains(&of) || of == o.id) {
                o.kind = crate::ObjectKind::Group;
            }
        }
    }

    /// Make a hostile file's organisation sane: bounded counts and names, one tag per kind,
    /// unique layer ids, finite values.
    pub(crate) fn repair_organization(&mut self) {
        self.layers.truncate(MAX_LAYERS);
        let mut seen = std::collections::HashSet::new();
        self.layers.retain(|l| seen.insert(l.id));
        for l in &mut self.layers {
            l.name = l.name.chars().take(MAX_NAME).collect();
            l.color = l.color.map(|c| if c.is_finite() { c.clamp(0.0, 1.0) } else { 0.5 });
        }
        self.presets.truncate(MAX_PRESETS);
        for p in &mut self.presets {
            p.name = p.name.chars().take(MAX_NAME).collect();
            if let Some(t) = &mut p.transform {
                *t = t.sanitized();
            }
            if let Some(tags) = &mut p.tags {
                tags.retain(|t| !matches!(t, Tag::LookAt { .. }));
                tags.truncate(MAX_TAGS);
            }
            if let Some(u) = &mut p.user {
                u.retain(|k, _| valid_attr_name(k));
                while u.len() > MAX_USER_ATTRS {
                    u.pop_last();
                }
                for v in u.values_mut() {
                    *v = std::mem::replace(v, UserValue::Switch(false)).sanitized();
                }
            }
        }
        for o in self.objects_mut_unbumped() {
            let mut kinds = std::collections::HashSet::new();
            o.tags.retain(|t| kinds.insert(t.kind()));
            o.tags.truncate(MAX_TAGS);
            o.user.retain(|k, _| valid_attr_name(k));
            while o.user.len() > MAX_USER_ATTRS {
                o.user.pop_last();
            }
            for v in o.user.values_mut() {
                *v = std::mem::replace(v, UserValue::Switch(false)).sanitized();
            }
            if !o.pivot.is_finite() {
                o.pivot = Vec3::ZERO;
            }
        }
        self.prune_organization();
    }
}
