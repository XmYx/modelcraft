//! Scene variants: named sets of attribute overrides on top of the main scene (a different
//! colour, a moved or hidden object), switched as one command.
//!
//! The scene always holds *live* values: with a variant active, its overrides are applied to the
//! objects. Everything the main scene had at those attributes is kept in `stash`, so switching to
//! another variant, or back to the main scene, first puts every stashed value back and only then
//! applies the new overrides. That makes the round trip exact: switching never changes the main
//! data, whatever happened in between. Edits made to an overridden attribute while its variant is
//! active belong to the variant (they are captured into it before the switch); edits to any other
//! attribute are edits of the main scene and stay.
//!
//! Overrides are numbers addressed by [`crate::attr`] paths, so a transform channel, visibility,
//! a material value, a shape or any other keyable number can be overridden. Animated attributes
//! cannot (their curves would overwrite the override on the next frame).

use serde::{Deserialize, Serialize};

use crate::{ObjectId, Scene, SceneError};

/// Most variants in a scene.
pub const MAX_VARIANTS: usize = 64;
/// Most overrides in one variant.
pub const MAX_OVERRIDES: usize = 20_000;
/// Longest variant name.
pub const MAX_NAME: usize = 128;

/// One overridden attribute.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Override {
    pub object: ObjectId,
    /// An attribute path ([`crate::attr`]).
    pub path: String,
    pub value: f64,
}

/// A named set of overrides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub id: u32,
    pub name: String,
    pub overrides: Vec<Override>,
}

/// The variants of a scene and which one is applied.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Variants {
    pub list: Vec<Variant>,
    /// The variant whose overrides are applied; `None` is the main scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<u32>,
    /// The main scene's values of the attributes the active variant overrides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stash: Vec<Override>,
    #[serde(default)]
    next_id: u32,
}

impl Variants {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.active.is_none()
    }
    pub fn get(&self, id: u32) -> Option<&Variant> {
        self.list.iter().find(|v| v.id == id)
    }
}

fn invalid<T>(m: impl Into<String>) -> Result<T, SceneError> {
    Err(SceneError::Invalid(m.into()))
}

fn clean_name(name: &str, fallback: &str) -> String {
    let n: String = name.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    if n.is_empty() { fallback.to_string() } else { n }
}

/// Drop what a file can get wrong: unknown objects, a missing active variant, over-long lists,
/// non-finite numbers.
pub(crate) fn repair(scene: &mut Scene) {
    let exists: std::collections::HashSet<ObjectId> = scene.objects().iter().map(|o| o.id).collect();
    let v = &mut scene.variants;
    v.list.truncate(MAX_VARIANTS);
    let mut seen = std::collections::HashSet::new();
    v.list.retain(|x| seen.insert(x.id));
    for x in &mut v.list {
        x.name = clean_name(&x.name, "Variant");
        x.overrides.truncate(MAX_OVERRIDES);
        x.overrides.retain(|o| o.value.is_finite() && o.path.len() <= crate::attr::MAX_PATH && exists.contains(&o.object));
    }
    v.next_id = v.next_id.max(v.list.iter().map(|x| x.id.saturating_add(1)).max().unwrap_or(0));
    if v.active.is_some_and(|a| v.get(a).is_none()) {
        v.active = None;
    }
    v.stash.truncate(MAX_OVERRIDES);
    v.stash.retain(|o| o.value.is_finite() && exists.contains(&o.object));
    if v.active.is_none() {
        // A stash without an active variant would be applied to nothing: put it back.
        let stash = std::mem::take(&mut v.stash);
        for o in stash {
            let _ = scene.set_attr(o.object, &o.path, o.value);
        }
    }
}

impl Scene {
    /// Add an empty variant; returns its id. It is not switched to.
    pub fn add_variant(&mut self, name: &str) -> Result<u32, SceneError> {
        if self.variants.list.len() >= MAX_VARIANTS {
            return invalid(format!("a scene holds at most {MAX_VARIANTS} variants"));
        }
        let id = self.variants.next_id;
        // An id that is already taken (a file can hold u32::MAX) is never handed out twice.
        let Some(next) = id.checked_add(1).filter(|_| self.variants.get(id).is_none()) else { return invalid("no more variant ids") };
        self.variants.next_id = next;
        let name = clean_name(name, &format!("Variant {}", id + 1));
        self.variants.list.push(Variant { id, name, overrides: Vec::new() });
        Ok(id)
    }

    /// Rename a variant.
    pub fn rename_variant(&mut self, id: u32, name: &str) -> Result<(), SceneError> {
        let v = self.variants.list.iter_mut().find(|v| v.id == id).ok_or_else(|| SceneError::Invalid(format!("no variant {id}")))?;
        v.name = clean_name(name, &v.name.clone());
        Ok(())
    }

    /// Remove a variant. Removing the active one goes back to the main scene first.
    pub fn remove_variant(&mut self, id: u32) -> Result<(), SceneError> {
        if self.variants.get(id).is_none() {
            return invalid(format!("no variant {id}"));
        }
        if self.variants.active == Some(id) {
            self.switch_variant(None)?;
        }
        self.variants.list.retain(|v| v.id != id);
        Ok(())
    }

    /// Make `which` the applied variant (`None`: the main scene). The main scene's values come
    /// back first, so the result never depends on the variant that was applied before.
    pub fn switch_variant(&mut self, which: Option<u32>) -> Result<(), SceneError> {
        if let Some(w) = which
            && self.variants.get(w).is_none()
        {
            return invalid(format!("no variant {w}"));
        }
        self.capture_variant();
        let stash = std::mem::take(&mut self.variants.stash);
        for o in &stash {
            let _ = self.set_attr(o.object, &o.path, o.value);
        }
        self.variants.active = None;
        let Some(w) = which else { return Ok(()) };
        let overrides = self.variants.get(w).map(|v| v.overrides.clone()).unwrap_or_default();
        for o in overrides {
            let Some(main) = self.attr(o.object, &o.path) else { continue };
            // Animated attributes are not overridable; a curve added later wins.
            if self.anim.has_curve(o.object, &o.path) {
                continue;
            }
            self.variants.stash.push(Override { object: o.object, path: o.path.clone(), value: main });
            let _ = self.set_attr(o.object, &o.path, o.value);
        }
        self.variants.active = Some(w);
        Ok(())
    }

    /// Read the live values of the active variant's overrides back into it, so edits made while
    /// it was applied are kept.
    pub fn capture_variant(&mut self) {
        let Some(a) = self.variants.active else { return };
        let live: Vec<Option<f64>> = match self.variants.get(a) {
            Some(v) => v.overrides.iter().map(|o| self.attr(o.object, &o.path)).collect(),
            None => return,
        };
        if let Some(v) = self.variants.list.iter_mut().find(|v| v.id == a) {
            for (o, now) in v.overrides.iter_mut().zip(live) {
                if let Some(now) = now.filter(|n| n.is_finite()) {
                    o.value = now;
                }
            }
        }
    }

    /// Override an attribute in the active variant, with its current value. The live value is
    /// still the main scene's at this point (the variant did not change it yet), which is what
    /// is kept for the way back: override first, edit after. Overriding again re-reads the value.
    pub fn add_override(&mut self, object: ObjectId, path: &str) -> Result<(), SceneError> {
        let Some(a) = self.variants.active else { return invalid("switch to a variant first: the main scene has no overrides") };
        let Some(now) = self.attr(object, path) else { return invalid(format!("object {} has no numeric attribute {path:?}", object.0)) };
        if self.anim.has_curve(object, path) {
            return invalid(format!("{path:?} is animated, so a variant cannot override it"));
        }
        if self.variants.get(a).is_some_and(|v| v.overrides.len() >= MAX_OVERRIDES && !v.overrides.iter().any(|o| o.object == object && o.path == path)) {
            return invalid(format!("a variant holds at most {MAX_OVERRIDES} overrides"));
        }
        let already = self.variants.get(a).is_some_and(|v| v.overrides.iter().any(|o| o.object == object && o.path == path));
        if !already {
            // The live value is still the main scene's: keep it for the way back.
            self.variants.stash.push(Override { object, path: path.to_string(), value: now });
        }
        if let Some(v) = self.variants.list.iter_mut().find(|v| v.id == a) {
            match v.overrides.iter_mut().find(|o| o.object == object && o.path == path) {
                Some(o) => o.value = now,
                None => v.overrides.push(Override { object, path: path.to_string(), value: now }),
            }
        }
        Ok(())
    }

    /// Stop overriding an attribute in the active variant: it goes back to the main value.
    pub fn remove_override(&mut self, object: ObjectId, path: &str) -> Result<(), SceneError> {
        let Some(a) = self.variants.active else { return invalid("switch to a variant first") };
        let had = self.variants.get(a).is_some_and(|v| v.overrides.iter().any(|o| o.object == object && o.path == path));
        if !had {
            return invalid(format!("the variant does not override {path:?} of object {}", object.0));
        }
        if let Some(i) = self.variants.stash.iter().position(|o| o.object == object && o.path == path) {
            let o = self.variants.stash.remove(i);
            let _ = self.set_attr(o.object, &o.path, o.value);
        }
        if let Some(v) = self.variants.list.iter_mut().find(|v| v.id == a) {
            v.overrides.retain(|o| !(o.object == object && o.path == path));
        }
        Ok(())
    }
}
