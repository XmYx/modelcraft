//! The scene: a tree of objects, each with a transform and a kind, and the evaluator that turns
//! it into meshes.
//!
//! - A **mesh object** has a *source* (a parametric [`Primitive`] or a stored mesh) and a *build
//!   history*: an ordered list of [`Step`]s (extrude, bevel, subdivide, sculpt layer…). Each step
//!   can be edited, switched off or removed later; [`Scene::collapse`] bakes them into the source.
//! - A **smooth cage** object is a generator: it shows its first mesh child subdivided.
//! - A **deformer** object bends, twists… its parent mesh, in its own frame.
//! - A **group** only carries a transform.
//!
//! The [`Evaluator`] is the dependency graph: it caches every step's output per object and
//! re-evaluates only what an edit invalidated (each object has a revision counter; a generator's
//! or deformed mesh's key also covers its children).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod eval;
pub mod pick;

use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
pub use modelcraft_mesh::PolyMesh;
pub use modelcraft_ops::deform::Deformer;
pub use modelcraft_ops::primitives::Primitive;
use serde::{Deserialize, Serialize};

pub use eval::{Evaluated, Evaluator, StepError};

/// Deepest object nesting the scene accepts (hostile files, cycles).
pub const MAX_DEPTH: usize = 64;
/// Most objects in one scene.
pub const MAX_OBJECTS: usize = 100_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Transform { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE }
    }
}

impl Transform {
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
    /// Replace non-finite parts and a zero scale with defaults.
    pub fn sanitized(mut self) -> Self {
        if !self.translation.is_finite() {
            self.translation = Vec3::ZERO;
        }
        if !self.rotation.is_finite() || self.rotation.length() < 1e-6 {
            self.rotation = Quat::IDENTITY;
        }
        self.rotation = self.rotation.normalize();
        if !self.scale.is_finite() {
            self.scale = Vec3::ONE;
        }
        self.scale = self.scale.map(|s| if s.abs() < 1e-6 { 1e-6 } else { s });
        self
    }
    /// Rotation as XYZ Euler angles in degrees (for numeric entry).
    pub fn euler_degrees(&self) -> Vec3 {
        let (x, y, z) = self.rotation.to_euler(glam::EulerRot::XYZ);
        Vec3::new(x, y, z) * (180.0 / std::f32::consts::PI)
    }
    pub fn set_euler_degrees(&mut self, d: Vec3) {
        let r = d * (std::f32::consts::PI / 180.0);
        self.rotation = Quat::from_euler(glam::EulerRot::XYZ, r.x, r.y, r.z);
    }
}

/// A PBR surface in the glTF metallic-roughness model (the suite's shared material core). A
/// `.mcmat` from MaterialCraft can be linked by path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<String>,
}

impl Default for Material {
    fn default() -> Self {
        Material { base_color: [0.62, 0.62, 0.64, 1.0], metallic: 0.0, roughness: 0.55, linked: None }
    }
}

/// Where a mesh object's geometry starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    Primitive { primitive: Primitive },
    Mesh { mesh: Arc<PolyMesh> },
}

/// Sculpted positions (and mask) for every vertex of the step's input. Stays valid while the
/// steps before it keep the vertex count.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SculptLayer {
    pub positions: Arc<Vec<Vec3>>,
    #[serde(default)]
    pub mask: Arc<Vec<f32>>,
}

/// One entry of the build history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StepOp {
    Extrude { faces: Vec<u32>, distance: f32 },
    Inset { faces: Vec<u32>, amount: f32 },
    Bevel { edges: Vec<u32>, width: f32 },
    LoopCut { edge: u32, cuts: u32 },
    Merge { verts: Vec<u32> },
    Weld { distance: f32 },
    DeleteFaces { faces: Vec<u32> },
    DeleteEdges { edges: Vec<u32> },
    DeleteVertices { verts: Vec<u32> },
    MoveVertices { verts: Vec<u32>, translate: Vec3, rotate: Quat, scale: Vec3 },
    Flip { faces: Vec<u32> },
    Triangulate,
    Subdivide { levels: u32 },
    SetPatch { faces: Vec<u32>, patch: u32 },
    Sculpt { layer: SculptLayer },
}

impl StepOp {
    /// Stable id of the operation (also the English label key).
    pub fn name(&self) -> &'static str {
        match self {
            StepOp::Extrude { .. } => "Extrude",
            StepOp::Inset { .. } => "Inset",
            StepOp::Bevel { .. } => "Bevel",
            StepOp::LoopCut { .. } => "Loop cut",
            StepOp::Merge { .. } => "Merge",
            StepOp::Weld { .. } => "Weld",
            StepOp::DeleteFaces { .. } => "Delete faces",
            StepOp::DeleteEdges { .. } => "Delete edges",
            StepOp::DeleteVertices { .. } => "Delete vertices",
            StepOp::MoveVertices { .. } => "Move components",
            StepOp::Flip { .. } => "Flip normals",
            StepOp::Triangulate => "Triangulate",
            StepOp::Subdivide { .. } => "Subdivide",
            StepOp::SetPatch { .. } => "Set patch",
            StepOp::Sculpt { .. } => "Sculpt layer",
        }
    }
    pub fn heap_bytes(&self) -> usize {
        match self {
            StepOp::Sculpt { layer } => layer.positions.len() * 12 + layer.mask.len() * 4,
            StepOp::Extrude { faces, .. }
            | StepOp::Inset { faces, .. }
            | StepOp::DeleteFaces { faces }
            | StepOp::Flip { faces }
            | StepOp::SetPatch { faces, .. } => faces.len() * 4,
            StepOp::Bevel { edges, .. } | StepOp::DeleteEdges { edges } => edges.len() * 4,
            StepOp::Merge { verts } | StepOp::DeleteVertices { verts } | StepOp::MoveVertices { verts, .. } => verts.len() * 4,
            _ => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub id: u64,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(flatten)]
    pub op: StepOp,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ObjectKind {
    Group,
    Mesh {
        source: Source,
        steps: Vec<Step>,
    },
    /// Shows its first mesh child subdivided `levels` times.
    SmoothCage {
        levels: u32,
    },
    Deformer {
        deformer: Deformer,
    },
}

impl ObjectKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            ObjectKind::Group => "Group",
            ObjectKind::Mesh { .. } => "Mesh",
            ObjectKind::SmoothCage { .. } => "Smooth cage",
            ObjectKind::Deformer { .. } => "Deformer",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    pub name: String,
    #[serde(default)]
    pub parent: Option<ObjectId>,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub material: Material,
    pub kind: ObjectKind,
    /// Bumped on every change; the evaluator's cache key. Not saved.
    #[serde(skip)]
    pub rev: u64,
}

impl Object {
    pub fn steps(&self) -> &[Step] {
        match &self.kind {
            ObjectKind::Mesh { steps, .. } => steps,
            _ => &[],
        }
    }
    pub fn heap_bytes(&self) -> usize {
        let src = match &self.kind {
            ObjectKind::Mesh { source: Source::Mesh { mesh }, .. } => mesh.heap_bytes(),
            _ => 0,
        };
        256 + self.name.len() + src + self.steps().iter().map(|s| s.op.heap_bytes() + 32).sum::<usize>()
    }
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum SceneError {
    #[error("no object {0:?}")]
    NoObject(ObjectId),
    #[error("{0}")]
    Invalid(String),
}

/// The scene: objects in tree order (siblings keep their relative order).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    objects: Vec<Object>,
    #[serde(default)]
    next_id: u64,
}

/// Revisions come from one process-wide counter, so a scene restored from a snapshot (undo, a
/// drag preview) can never reuse a revision that a cache has already seen.
fn next_rev() -> u64 {
    static REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    REV.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Scene {
    pub fn new() -> Self {
        Scene::default()
    }

    /// Check and repair a scene read from a file: unique ids, existing parents, no cycles,
    /// finite transforms, bounded depth and size.
    pub fn repaired(mut self) -> Result<Scene, SceneError> {
        if self.objects.len() > MAX_OBJECTS {
            return Err(SceneError::Invalid(format!("{} objects", self.objects.len())));
        }
        let mut seen = std::collections::HashSet::new();
        self.objects.retain(|o| seen.insert(o.id));
        let ids: std::collections::HashSet<ObjectId> = self.objects.iter().map(|o| o.id).collect();
        for o in &mut self.objects {
            if o.parent.is_some_and(|p| !ids.contains(&p) || p == o.id) {
                o.parent = None;
            }
            o.transform = o.transform.sanitized();
        }
        // Break cycles and over-deep chains.
        for i in 0..self.objects.len() {
            let Some(id) = self.objects.get(i).map(|o| o.id) else { continue };
            if self.depth(id).is_none()
                && let Some(o) = self.objects.get_mut(i)
            {
                o.parent = None;
            }
        }
        let max = self.objects.iter().map(|o| o.id.0).max().unwrap_or(0);
        self.next_id = self.next_id.max(max + 1);
        for o in &mut self.objects {
            for s in o.steps() {
                self.next_id = self.next_id.max(s.id + 1);
            }
        }
        for o in &mut self.objects {
            o.rev = next_rev();
        }
        Ok(self)
    }

    pub fn objects(&self) -> &[Object] {
        &self.objects
    }
    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }
    pub fn len(&self) -> usize {
        self.objects.len()
    }
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
    /// The id [`Scene::fresh_id`] would hand out next.
    pub fn fresh_id_peek(&self) -> u64 {
        self.next_id.max(1)
    }
    pub fn fresh_id(&mut self) -> u64 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Mutable access; bumps the object's revision (and so invalidates its cached meshes).
    pub fn edit(&mut self, id: ObjectId) -> Result<&mut Object, SceneError> {
        let rev = next_rev();
        let o = self.objects.iter_mut().find(|o| o.id == id).ok_or(SceneError::NoObject(id))?;
        o.rev = rev;
        Ok(o)
    }

    /// Add an object (as the last child of `parent`); returns its id.
    pub fn add(&mut self, name: impl Into<String>, kind: ObjectKind, parent: Option<ObjectId>) -> Result<ObjectId, SceneError> {
        if self.objects.len() >= MAX_OBJECTS {
            return Err(SceneError::Invalid("too many objects".into()));
        }
        if let Some(p) = parent {
            self.depth(p).filter(|d| *d + 1 < MAX_DEPTH).ok_or(SceneError::NoObject(p))?;
        }
        let id = ObjectId(self.fresh_id());
        self.objects.push(Object {
            id,
            name: name.into(),
            parent,
            transform: Transform::default(),
            visible: true,
            material: Material::default(),
            kind,
            rev: next_rev(),
        });
        Ok(id)
    }

    /// Put back an object removed earlier (undo), at its old index.
    pub fn insert_at(&mut self, index: usize, mut object: Object) {
        object.rev = next_rev();
        self.next_id = self.next_id.max(object.id.0 + 1);
        let i = index.min(self.objects.len());
        self.objects.insert(i, object);
    }

    pub fn index_of(&self, id: ObjectId) -> Option<usize> {
        self.objects.iter().position(|o| o.id == id)
    }

    /// Remove an object and its whole subtree; returns them with their indices (for undo).
    pub fn remove(&mut self, id: ObjectId) -> Vec<(usize, Object)> {
        let doomed: std::collections::HashSet<ObjectId> = self.subtree(id).into_iter().collect();
        let mut out = Vec::new();
        let mut i = 0;
        let mut orig = 0;
        while i < self.objects.len() {
            if self.objects.get(i).is_some_and(|o| doomed.contains(&o.id)) {
                out.push((orig, self.objects.remove(i)));
            } else {
                i += 1;
            }
            orig += 1;
        }
        self.bump_ancestors_of_removed(&out);
        out
    }

    fn bump_ancestors_of_removed(&mut self, removed: &[(usize, Object)]) {
        for (_, o) in removed {
            if let Some(p) = o.parent {
                let _ = self.edit(p);
            }
        }
    }

    /// `id` and all its descendants, parents first.
    pub fn subtree(&self, id: ObjectId) -> Vec<ObjectId> {
        let mut out = Vec::new();
        if self.get(id).is_none() {
            return out;
        }
        let mut stack = vec![(id, 0usize)];
        while let Some((c, d)) = stack.pop() {
            out.push(c);
            if d < MAX_DEPTH {
                for k in self.children(c).iter().rev() {
                    stack.push((*k, d + 1));
                }
            }
        }
        out
    }

    pub fn children(&self, id: ObjectId) -> Vec<ObjectId> {
        self.objects.iter().filter(|o| o.parent == Some(id)).map(|o| o.id).collect()
    }
    pub fn roots(&self) -> Vec<ObjectId> {
        self.objects.iter().filter(|o| o.parent.is_none()).map(|o| o.id).collect()
    }

    /// Depth of an object (0 for a root); `None` when missing, cyclic or deeper than [`MAX_DEPTH`].
    pub fn depth(&self, id: ObjectId) -> Option<usize> {
        let mut c = self.get(id)?;
        for d in 0..MAX_DEPTH {
            match c.parent {
                None => return Some(d),
                Some(p) => c = self.get(p)?,
            }
        }
        None
    }

    /// Objects in display order (depth-first), with their depth.
    pub fn tree_order(&self) -> Vec<(ObjectId, usize)> {
        let mut out = Vec::with_capacity(self.objects.len());
        let mut stack: Vec<(ObjectId, usize)> = self.roots().into_iter().rev().map(|r| (r, 0)).collect();
        while let Some((id, d)) = stack.pop() {
            out.push((id, d));
            if d < MAX_DEPTH {
                for c in self.children(id).into_iter().rev() {
                    stack.push((c, d + 1));
                }
            }
        }
        out
    }

    /// Move an object under a new parent (`None`: to the top level). Refuses cycles.
    pub fn reparent(&mut self, id: ObjectId, parent: Option<ObjectId>) -> Result<(), SceneError> {
        if let Some(p) = parent {
            if self.subtree(id).contains(&p) {
                return Err(SceneError::Invalid("an object cannot go inside itself".into()));
            }
            let pd = self.depth(p).ok_or(SceneError::NoObject(p))?;
            let height = self.subtree(id).iter().filter_map(|c| self.depth(*c)).max().unwrap_or(0).saturating_sub(self.depth(id).unwrap_or(0));
            if pd + 1 + height >= MAX_DEPTH {
                return Err(SceneError::Invalid("too deep".into()));
            }
        }
        let old = self.get(id).ok_or(SceneError::NoObject(id))?.parent;
        if let Some(o) = old {
            let _ = self.edit(o);
        }
        if let Some(p) = parent {
            let _ = self.edit(p);
        }
        self.edit(id)?.parent = parent;
        // Keep the moved object after its new siblings.
        if let Some(i) = self.index_of(id) {
            let o = self.objects.remove(i);
            self.objects.push(o);
        }
        Ok(())
    }

    /// Object → world matrix.
    pub fn world_matrix(&self, id: ObjectId) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        let mut c = self.get(id);
        for _ in 0..MAX_DEPTH {
            let Some(o) = c else { break };
            m = o.transform.matrix() * m;
            c = o.parent.and_then(|p| self.get(p));
        }
        m
    }

    /// True if an ancestor is a smooth cage (the object is shown through it, not directly).
    pub fn is_cage_input(&self, id: ObjectId) -> bool {
        self.get(id).and_then(|o| o.parent).and_then(|p| self.get(p)).is_some_and(|p| matches!(p.kind, ObjectKind::SmoothCage { .. }))
    }

    pub fn visible(&self, id: ObjectId) -> bool {
        let mut c = self.get(id);
        for _ in 0..MAX_DEPTH {
            let Some(o) = c else { return true };
            if !o.visible {
                return false;
            }
            c = o.parent.and_then(|p| self.get(p));
        }
        true
    }

    /// Combined revision of an object and everything its output depends on (children for
    /// generators, deformer children for meshes).
    pub fn deep_rev(&self, id: ObjectId) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for c in self.subtree(id) {
            let r = self.get(c).map_or(0, |o| o.rev ^ o.id.0.rotate_left(32));
            h = (h ^ r).wrapping_mul(0x0100_0000_01b3);
        }
        h
    }

    /// Bake the build history (and source) of a mesh object into a stored mesh.
    pub fn collapse(&mut self, id: ObjectId, eval: &mut Evaluator) -> Result<(), SceneError> {
        let mesh = eval.steps_output(self, id, usize::MAX).mesh;
        match &mut self.edit(id)?.kind {
            ObjectKind::Mesh { source, steps } => {
                *source = Source::Mesh { mesh };
                steps.clear();
                Ok(())
            }
            _ => Err(SceneError::Invalid("only mesh objects have a history".into())),
        }
    }

    /// Unique display name based on `base` ("Cube", "Cube.1", …).
    pub fn unique_name(&self, base: &str) -> String {
        if !self.objects.iter().any(|o| o.name == base) {
            return base.to_string();
        }
        (1..).map(|i| format!("{base}.{i}")).find(|n| !self.objects.iter().any(|o| &o.name == n)).unwrap_or_else(|| base.to_string())
    }

    pub fn heap_bytes(&self) -> usize {
        self.objects.iter().map(Object::heap_bytes).sum()
    }
}

#[cfg(test)]
mod tests;
