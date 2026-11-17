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

pub mod anim;
pub mod attr;
mod eval;
pub mod pick;
pub mod rig;

use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
pub use modelcraft_field::{Field, FieldStack};
pub use modelcraft_mesh::PolyMesh;
pub use modelcraft_ops::deform::Deformer;
pub use modelcraft_ops::primitives::Primitive;
use serde::{Deserialize, Serialize};

pub use anim::{Animation, BASE_LAYER, Layer, LayerMode, Track};
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

/// A PBR surface: the glTF metallic-roughness core (the suite's shared material), plus the
/// layers the path tracer renders. A `.mcmat` from MaterialCraft can be linked by path. Fields
/// added after the first file format have defaults, so older projects open unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked: Option<String>,
    /// Emitted colour; with `emission_strength` above zero the surface is a light in renders.
    #[serde(default = "white", skip_serializing_if = "is_white")]
    pub emission: [f32; 3],
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub emission_strength: f32,
    /// A clear layer over the surface (lacquer, varnish).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub coat: f32,
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub coat_roughness: f32,
    /// Soft fibres on the surface (cloth, dust).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub sheen: f32,
    /// How much light passes through (glass, liquids).
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub transmission: f32,
    #[serde(default = "default_ior", skip_serializing_if = "is_default_ior")]
    pub ior: f32,
}

fn white() -> [f32; 3] {
    [1.0; 3]
}
fn is_white(c: &[f32; 3]) -> bool {
    *c == [1.0; 3]
}
fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}
fn default_ior() -> f32 {
    1.5
}
fn is_default_ior(v: &f32) -> bool {
    *v == 1.5
}

impl Default for Material {
    fn default() -> Self {
        Material {
            base_color: [0.62, 0.62, 0.64, 1.0],
            metallic: 0.0,
            roughness: 0.55,
            linked: None,
            emission: white(),
            emission_strength: 0.0,
            coat: 0.0,
            coat_roughness: 0.0,
            sheen: 0.0,
            transmission: 0.0,
            ior: default_ior(),
        }
    }
}

/// Where a mesh object's geometry starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    Primitive { primitive: Primitive },
    Mesh { mesh: Arc<PolyMesh> },
}

/// Sculpted positions (and mask) for every vertex of the step's input, subdivided `level` times
/// (detail levels). Stays valid while the steps before it keep the topology.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SculptLayer {
    pub positions: Arc<Vec<Vec3>>,
    #[serde(default)]
    pub mask: Arc<Vec<f32>>,
    /// The detail level being shown: how many times the input is subdivided.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub level: u8,
    /// Displacements of the levels above `level` (first: `level + 1`), kept while working lower.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub finer: Vec<Arc<Vec<Vec3>>>,
    /// Shape layers blended into `positions`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<ShapeLayer>,
    /// The shape layer new strokes are recorded into (none: the base sculpt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_shape: Option<u32>,
    /// `positions` when the active shape layer last had its offsets brought up to date; the
    /// difference to `positions` is what has been sculpted into it since.
    #[serde(default, skip_serializing_if = "arc_is_empty")]
    pub anchor: Arc<Vec<Vec3>>,
    /// Set once live detail has changed the triangles: the mesh this layer sculpts instead of
    /// the step's input (triangles; patches kept, UVs not).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<Arc<PolyMesh>>,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}
fn arc_is_empty(v: &Arc<Vec<Vec3>>) -> bool {
    v.is_empty()
}

/// A named offset per vertex, blended over the sculpt with its own strength.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShapeLayer {
    pub name: String,
    pub strength: f32,
    #[serde(default)]
    pub delta: Arc<Vec<Vec3>>,
}

impl SculptLayer {
    /// A fresh layer over `positions` at the base level.
    pub fn new(positions: Vec<Vec3>) -> SculptLayer {
        let mask = Arc::new(vec![0.0; positions.len()]);
        SculptLayer { positions: Arc::new(positions), mask, ..SculptLayer::default() }
    }
    pub fn heap_bytes(&self) -> usize {
        let v3 = self.positions.len()
            + self.anchor.len()
            + self.finer.iter().map(|f| f.len()).sum::<usize>()
            + self.shapes.iter().map(|s| s.delta.len()).sum::<usize>();
        v3 * 12 + self.mask.len() * 4 + self.topology.as_ref().map(|t| t.heap_bytes()).unwrap_or(0)
    }
}

/// One entry of the build history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StepOp {
    Extrude {
        faces: Vec<u32>,
        distance: f32,
    },
    Inset {
        faces: Vec<u32>,
        amount: f32,
    },
    Bevel {
        edges: Vec<u32>,
        width: f32,
    },
    LoopCut {
        edge: u32,
        cuts: u32,
    },
    Merge {
        verts: Vec<u32>,
    },
    Weld {
        distance: f32,
    },
    DeleteFaces {
        faces: Vec<u32>,
    },
    DeleteEdges {
        edges: Vec<u32>,
    },
    DeleteVertices {
        verts: Vec<u32>,
    },
    MoveVertices {
        verts: Vec<u32>,
        translate: Vec3,
        rotate: Quat,
        scale: Vec3,
    },
    Flip {
        faces: Vec<u32>,
    },
    Triangulate,
    Subdivide {
        levels: u32,
    },
    SetPatch {
        faces: Vec<u32>,
        patch: u32,
    },
    /// Join two faces with a tube.
    Bridge {
        a: u32,
        b: u32,
    },
    /// Close the hole bordered by an edge.
    FillHole {
        edge: u32,
    },
    /// Mirror across a plane through the object's origin (0 = X, 1 = Y, 2 = Z), welding the seam.
    Mirror {
        axis: u32,
        weld: f32,
    },
    Sculpt {
        layer: SculptLayer,
    },
    /// Rebuild a closed mesh as even quads of about `cell` size (keeps the volume; loses UVs
    /// and patches).
    EvenRemesh {
        cell: f32,
    },
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
            StepOp::Bridge { .. } => "Bridge",
            StepOp::FillHole { .. } => "Fill hole",
            StepOp::Mirror { .. } => "Mirror",
            StepOp::Sculpt { .. } => "Sculpt layer",
            StepOp::EvenRemesh { .. } => "Even remesh",
        }
    }
    pub fn heap_bytes(&self) -> usize {
        match self {
            StepOp::Sculpt { layer } => layer.heap_bytes(),
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
    /// A falloff field. Inside a deformer it sets how strongly the deformer acts at each point
    /// (the field's own transform places it in the deformer's frame); several fields combine.
    Field {
        field: Field,
    },
    /// A joint of a skeleton: a transform drawn as a bone to each child joint ([`rig`]).
    Joint {
        radius: f32,
    },
    /// Binds its parent mesh to joints ([`rig::Skin`]).
    Skin {
        skin: Arc<rig::Skin>,
    },
}

impl ObjectKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            ObjectKind::Group => "Group",
            ObjectKind::Mesh { .. } => "Mesh",
            ObjectKind::SmoothCage { .. } => "Smooth cage",
            ObjectKind::Deformer { .. } => "Deformer",
            ObjectKind::Field { .. } => "Falloff field",
            ObjectKind::Joint { .. } => "Joint",
            ObjectKind::Skin { .. } => "Skin",
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
            ObjectKind::Skin { skin } => skin.heap_bytes(),
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
    /// Curves, the current frame and the time settings ([`anim`]).
    #[serde(default, skip_serializing_if = "Animation::is_default")]
    pub anim: Animation,
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
        self.anim.repair(|id| ids.contains(&id));
        rig::repair(&mut self);
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
        for (_, o) in &out {
            self.anim.remove_object(o.id);
        }
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

    /// The falloff fields inside `id` (a deformer), each with the matrix taking the deformer's
    /// frame into its own. Hidden fields are off.
    pub fn field_stack(&self, id: ObjectId) -> FieldStack {
        let mut stack = FieldStack::new();
        for c in self.children(id) {
            let Some(o) = self.get(c) else { continue };
            if let (true, ObjectKind::Field { field }) = (o.visible, &o.kind) {
                stack.push(o.transform.matrix().inverse(), field.clone());
            }
        }
        stack
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
