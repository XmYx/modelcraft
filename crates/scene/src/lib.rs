//! The scene: a tree of objects, each with a transform and a kind, and the evaluator that turns
//! it into meshes.
//!
//! - A **mesh object** has a *source* (a parametric [`Primitive`] or a stored mesh) and a *build
//!   history*: an ordered list of [`Step`]s (extrude, bevel, subdivide, sculpt layer…). Each step
//!   can be edited, switched off or removed later; [`Scene::collapse`] bakes them into the source.
//! - A **smooth cage** object is a generator: it shows its first mesh child subdivided.
//! - A **clone generator** lays out instances of its first mesh child ([`Cloner`]).
//! - A **deformer** object bends, twists… its parent mesh, in its own frame.
//! - A **group** only carries a transform.
//! - **Blend shapes, lattices and clusters** are children of a mesh too: its deformer stack
//!   ([`point_deform`]).
//! - **Constraints** pose objects from other objects, in dependency order ([`constraint`]).
//!
//! The [`Evaluator`] is the dependency graph: it caches every step's output per object and
//! re-evaluates only what an edit invalidated (each object has a revision counter; a generator's
//! or deformed mesh's key also covers its children).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod anim;
pub mod attr;
pub mod constraint;
mod eval;
pub mod hair;
pub mod organize;
pub mod pick;
pub mod point_deform;
pub mod rig;
mod rle;
pub mod sim;
pub mod snap;
pub mod spline_gen;
pub mod splines;
pub mod stage;

use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
pub use modelcraft_clone::{CloneInstance, CloneSet, Cloner, MAX_CLONES, Modifier};
pub use modelcraft_field::{Field, FieldStack};
pub use modelcraft_mesh::PolyMesh;
pub use modelcraft_ops::array::Array;
pub use modelcraft_ops::deform::{Deformer, Limits as DeformLimits};
pub use modelcraft_ops::primitives::Primitive;
pub use modelcraft_sim::{FrameCache, SimKind, SimObject};
pub use modelcraft_spline::{Polyline, Spline, SplinePrimitive, Tessellation};
pub use modelcraft_uv::UnwrapMethod;
pub use modelcraft_uvpack::{Density, Fit, Projection, Rotation};
use serde::{Deserialize, Serialize};

pub use anim::{Animation, BASE_LAYER, Layer, LayerMode, Track};
pub use eval::{Evaluated, Evaluator, Instances, StepError};
pub use organize::{AttributePreset, DisplayLayer, DisplayMode, PresetParts, Tag, TagKind, UserValue};
pub use spline_gen::SplineGen;
pub use splines::{SplineEval, SplineSource};

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
    /// A patch id for every face of the mesh this layer shows (patches made while sculpting).
    /// Empty: the faces keep the patches they come with.
    #[serde(default, skip_serializing_if = "arc_vec_is_empty", with = "rle::patches")]
    pub patches: Arc<Vec<u32>>,
    /// Hidden faces of the mesh this layer shows (a view state: nothing is removed). Empty:
    /// everything shows.
    #[serde(default, skip_serializing_if = "arc_vec_is_empty", with = "rle::hidden")]
    pub hidden: Arc<Vec<bool>>,
}

fn arc_vec_is_empty<T>(v: &Arc<Vec<T>>) -> bool {
    v.is_empty()
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
        v3 * 12 + self.mask.len() * 4 + self.patches.len() * 4 + self.hidden.len() + self.topology.as_ref().map(|t| t.heap_bytes()).unwrap_or(0)
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
    /// Rebuild the patches from the shape (by angle or by connected shells).
    GroupPatches {
        grouping: modelcraft_ops::patches::Grouping,
    },
    /// Weld vertices closer than `distance` and remove faces with no area.
    CleanUp {
        distance: f32,
    },
    Sculpt {
        layer: SculptLayer,
    },
    /// Rebuild a closed mesh as even quads of about `cell` size (keeps the volume; loses UVs
    /// and patches).
    EvenRemesh {
        cell: f32,
    },
    /// Cut away everything on the side of a plane its normal points to; `cap` closes the cut.
    TrimPlane {
        point: Vec3,
        normal: Vec3,
        cap: bool,
    },
    /// Reduce the mesh to about `ratio` of its triangles (0..1), keeping the shape, the UVs and
    /// the patches. The result is triangles.
    Decimate {
        ratio: f32,
    },
    /// Flatten the surface into UVs: cut along `seams` (and along every edge sharper than
    /// `angle` degrees), solve a least-squares conformal map per island and lay the islands out
    /// in the unit square. Seams are vertex pairs and pins name a face corner, both in the
    /// numbering of the step's input mesh.
    Unwrap {
        #[serde(default)]
        seams: Vec<[u32; 2]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        angle: Option<f32>,
        #[serde(default)]
        pins: Vec<UvPin>,
        /// Add the cuts an island needs to be flattenable (a sphere gets a path, a tube a line).
        #[serde(default = "yes")]
        auto_cut: bool,
        /// Margin around and between islands, as a fraction of the unit square.
        #[serde(default = "default_margin")]
        margin: f32,
        /// The solver for each island: least-squares conformal or angle-based flattening.
        #[serde(default)]
        method: UnwrapMethod,
        /// Also cut along every border between two patches (polygon groups), so each patch
        /// becomes an island.
        #[serde(default)]
        from_patches: bool,
    },
    /// Pack the islands the UVs already form into the unit square: `margin` between them as a
    /// fraction of the square, optional rotation, and an even texel density.
    PackUvs {
        #[serde(default = "default_margin")]
        margin: f32,
        #[serde(default = "default_rotation")]
        rotation: Rotation,
        #[serde(default = "default_density")]
        density: Density,
        /// Spread the islands over this many UDIM tiles (1 is the unit square).
        #[serde(default = "default_tiles")]
        tiles: u32,
        /// Pack into as many tiles as this texel density needs (UV units per metre of surface),
        /// at most 100; overrides `tiles`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        texel_density: Option<f32>,
    },
    /// Give the corners of `faces` (all faces when empty) UVs from a planar, box, cylindrical,
    /// spherical or camera projection of their positions.
    ProjectUvs {
        projection: Projection,
        #[serde(default)]
        fit: Fit,
        #[serde(default)]
        faces: Vec<u32>,
    },
    /// Records seams (pairs of vertices of the step's input mesh) and changes nothing; the UV
    /// editor reads the latest one, and an unwrap step takes them.
    Seams {
        #[serde(default)]
        edges: Vec<[u32; 2]>,
    },
    /// Move, rotate and scale the UVs of the listed face corners (scale, then rotate, then
    /// translate, about `pivot`).
    TransformUvs {
        corners: Vec<u32>,
        #[serde(default)]
        translate: [f32; 2],
        #[serde(default)]
        rotate_deg: f32,
        #[serde(default = "unit_scale")]
        scale: [f32; 2],
        #[serde(default)]
        pivot: [f32; 2],
    },
    /// Weld the UVs on the two sides of the listed edges; with `align`, first move the islands
    /// on one side onto the other so the edges meet (and with `scale`, resize them to fit).
    StitchUvs {
        edges: Vec<u32>,
        #[serde(default)]
        align: bool,
        #[serde(default)]
        scale: bool,
    },
    /// Relax the UVs of the listed corners (everything when the list is empty) with a few
    /// iterations of local/global flattening; the other UVs stay where they are.
    RelaxUvs {
        #[serde(default)]
        corners: Vec<u32>,
        #[serde(default = "default_relax_iterations")]
        iterations: u32,
        #[serde(default)]
        mode: modelcraft_uv::RelaxMode,
    },
    /// Set the UV of every corner (in corner order), as computed by a UV transfer from another
    /// mesh. A mesh with another number of corners (the history before this step changed) keeps
    /// its own UVs and the step reports an error.
    SetUvs {
        uvs: Arc<Vec<[f32; 2]>>,
    },
    /// Lay the selected chains of UV edges on the line between their ends.
    StraightenUvs {
        #[serde(default)]
        corners: Vec<u32>,
        #[serde(default = "default_strength")]
        strength: f32,
    },
}

fn default_rotation() -> Rotation {
    Rotation::Fit
}

fn default_density() -> Density {
    Density::Uniform
}

fn unit_scale() -> [f32; 2] {
    [1.0, 1.0]
}

/// A face corner held at a UV position while unwrapping.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct UvPin {
    pub corner: u32,
    pub uv: [f32; 2],
}

fn default_margin() -> f32 {
    0.01
}

fn default_tiles() -> u32 {
    1
}

fn default_relax_iterations() -> u32 {
    10
}

fn default_strength() -> f32 {
    1.0
}

impl StepOp {
    /// A step that only records information and leaves the mesh as it is.
    pub fn is_marker(&self) -> bool {
        matches!(self, StepOp::Seams { .. })
    }

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
            StepOp::GroupPatches { .. } => "Group patches",
            StepOp::CleanUp { .. } => "Clean up",
            StepOp::Sculpt { .. } => "Sculpt layer",
            StepOp::EvenRemesh { .. } => "Even remesh",
            StepOp::TrimPlane { .. } => "Trim by plane",
            StepOp::Decimate { .. } => "Decimate",
            StepOp::Unwrap { .. } => "Unwrap UVs",
            StepOp::PackUvs { .. } => "Pack UV islands",
            StepOp::ProjectUvs { .. } => "Project UVs",
            StepOp::Seams { .. } => "Seams",
            StepOp::TransformUvs { .. } => "Transform UVs",
            StepOp::StitchUvs { align: true, .. } => "Stitch UV islands",
            StepOp::StitchUvs { .. } => "Weld UVs",
            StepOp::RelaxUvs { .. } => "Relax UVs",
            StepOp::SetUvs { .. } => "Transfer UVs",
            StepOp::StraightenUvs { .. } => "Straighten UVs",
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
            StepOp::Unwrap { seams, pins, .. } => seams.len() * 8 + pins.len() * 12,
            StepOp::ProjectUvs { faces, .. } => faces.len() * 4,
            StepOp::Seams { edges } => edges.len() * 8,
            StepOp::TransformUvs { corners, .. } => corners.len() * 4,
            StepOp::StitchUvs { edges, .. } => edges.len() * 4,
            StepOp::RelaxUvs { corners, .. } | StepOp::StraightenUvs { corners, .. } => corners.len() * 4,
            StepOp::SetUvs { uvs } => uvs.len() * 8,
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
        /// The box the deformer works over and how it treats what lies outside it.
        #[serde(default)]
        limits: modelcraft_ops::deform::Limits,
        /// The object a shrink-wrap pulls onto.
        #[serde(default)]
        target: Option<ObjectId>,
    },
    /// Shows real copies of its first mesh child joined into one mesh, in a row or a ring.
    Array {
        array: Array,
    },
    /// Shows its first mesh child mirrored across a plane through its origin (0 = X, 1 = Y,
    /// 2 = Z), joined at the seam.
    Symmetry {
        axis: u32,
        weld: f32,
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
    /// Blend shapes, a lattice or a cluster in its parent mesh's deformer stack ([`point_deform`]).
    PointDeformer {
        deformer: point_deform::PointDeformer,
    },
    /// A clone modifier inside a clone generator: it moves, turns, scales and tints the clones.
    /// Falloff fields inside it weigh where it acts; `target` is the object the `target` mode
    /// pulls toward.
    CloneModifier {
        modifier: Modifier,
        #[serde(default)]
        target: Option<ObjectId>,
    },
    /// Lays out clones of its first mesh child (or smooth cage child). The clones are instances of
    /// that child's evaluated mesh; `target` is the object the surface and vertices layouts sit on.
    Cloner {
        cloner: Cloner,
        #[serde(default)]
        target: Option<ObjectId>,
    },
    /// Shows another object's evaluated mesh (and clones) under its own transform and material.
    /// The mesh is shared, not copied: change the master and every instance follows.
    Instance {
        of: ObjectId,
    },
    /// Hair growing from its parent mesh: guide curves, how render hairs are made from them, and
    /// the maps that steer growth and grooming ([`hair`]).
    Hair {
        hair: Arc<hair::HairData>,
    },
    /// A simulation driving its parent mesh: settings, and the cache the last bake produced
    /// ([`sim`]).
    Sim {
        sim: Arc<SimObject>,
    },
    /// A light: it shines along its own -Z ([`stage`]).
    Light {
        light: stage::LightData,
    },
    /// A camera: it looks along its own -Z ([`stage`]).
    Camera {
        camera: stage::CameraData,
    },
    /// A curve: drawn as a line, never as a surface. Spline generators take spline children as
    /// their input ([`splines`]).
    Spline {
        source: SplineSource,
        #[serde(default)]
        tess: Tessellation,
    },
    /// Shows a surface made from the splines inside it: extrude, revolve, loft or sweep
    /// ([`spline_gen`]).
    SplineGenerator {
        generator: SplineGen,
    },
}

impl ObjectKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            ObjectKind::Group => "Group",
            ObjectKind::Mesh { .. } => "Mesh",
            ObjectKind::SmoothCage { .. } => "Smooth cage",
            ObjectKind::Deformer { .. } => "Deformer",
            ObjectKind::Array { .. } => "Array",
            ObjectKind::Symmetry { .. } => "Symmetry",
            ObjectKind::Field { .. } => "Falloff field",
            ObjectKind::Joint { .. } => "Joint",
            ObjectKind::Skin { .. } => "Skin",
            ObjectKind::PointDeformer { deformer } => deformer.type_name(),
            ObjectKind::Cloner { .. } => "Clone generator",
            ObjectKind::CloneModifier { .. } => "Clone modifier",
            ObjectKind::Instance { .. } => "Instance",
            ObjectKind::Hair { .. } => "Hair",
            ObjectKind::Sim { .. } => "Simulation",
            ObjectKind::Light { light } => light.kind.title(),
            ObjectKind::Camera { .. } => "Camera",
            ObjectKind::Spline { .. } => "Spline",
            ObjectKind::SplineGenerator { .. } => "Spline generator",
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
    /// Protected: edits to it are refused until it is unlocked.
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    /// Shown for reference: drawn, never picked in the viewport.
    #[serde(default, skip_serializing_if = "is_false")]
    pub template: bool,
    /// Behaviours: display, render, protection, look-at ([`organize::Tag`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tag>,
    /// The display layer the object is on ([`organize::DisplayLayer`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<u32>,
    /// Named values the object carries ([`organize::UserValue`]).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub user: std::collections::BTreeMap<String, UserValue>,
    /// The point, in the object's own space, that it turns and scales around, and that
    /// `transform.translation` places in the parent's space.
    #[serde(default, skip_serializing_if = "pivot_is_origin")]
    pub pivot: Vec3,
    /// Bumped on every change; the evaluator's cache key. Not saved.
    #[serde(skip)]
    pub rev: u64,
}

impl Object {
    /// The object's matrix in its parent's space: its transform about its pivot.
    pub fn local_matrix(&self) -> Mat4 {
        let m = self.transform.matrix();
        if self.pivot == Vec3::ZERO { m } else { m * Mat4::from_translation(-self.pivot) }
    }
    /// Move the pivot to `pivot` (own space). With `keep_in_place` the translation changes to
    /// match, so the object and its children stay where they are: `T R S (x - old)` must equal
    /// `T' R S (x - new)`, so `T' = T + R S (new - old)`. Returns false (and changes nothing)
    /// when that would leave the finite range.
    pub fn set_pivot(&mut self, pivot: Vec3, keep_in_place: bool) -> bool {
        if !pivot.is_finite() {
            return false;
        }
        if keep_in_place {
            let t = self.transform.translation + self.transform.rotation * (self.transform.scale * (pivot - self.pivot));
            if !t.is_finite() {
                return false;
            }
            self.transform.translation = t;
        }
        self.pivot = pivot;
        true
    }
    /// The transform with the pivot folded in: placing the object by this alone, with no pivot,
    /// gives the same matrix as [`Object::local_matrix`] (for file formats that have no pivot).
    pub fn effective_transform(&self) -> Transform {
        let mut t = self.transform;
        t.translation -= t.rotation * (t.scale * self.pivot);
        t
    }
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
            ObjectKind::Hair { hair } => hair.heap_bytes(),
            ObjectKind::Sim { sim } => sim.heap_bytes(),
            ObjectKind::Spline { source, .. } => source.heap_bytes(),
            ObjectKind::PointDeformer { deformer } => deformer.heap_bytes(),
            _ => 0,
        };
        256 + self.name.len()
            + self.tags.len() * 48
            + self.user.iter().map(|(k, v)| 48 + k.len() + if let UserValue::Text(t) = v { t.len() } else { 0 }).sum::<usize>()
            + src
            + self.steps().iter().map(|s| s.op.heap_bytes() + 32).sum::<usize>()
    }
}

enum Slot {
    At(usize),
    Absent,
    Unknown,
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
    /// Constraints, IK, driven keys and expressions ([`constraint`]).
    /// Where each object sits in `objects` ([`Scene::get`]).
    #[serde(skip)]
    index: Box<Index>,
    /// Boxed: a scene is moved around by value (history entries, snapshots).
    #[serde(default, skip_serializing_if = "no_constraints")]
    pub constraints: Box<constraint::Constraints>,
    /// Named selections.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sets: Vec<SelectionSet>,
    /// Display and render layers ([`organize::DisplayLayer`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<DisplayLayer>,
    /// Saved attribute sets ([`organize::AttributePreset`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<AttributePreset>,
    /// What renders of the project use: the camera object, and (with the render settings) the rest
    /// ([`stage::RenderConfig`]).
    #[serde(default, skip_serializing_if = "stage::RenderConfig::is_default")]
    pub render: stage::RenderConfig,
}

#[allow(clippy::borrowed_box)]
fn no_constraints(c: &Box<constraint::Constraints>) -> bool {
    c.is_empty()
}

/// A named list of objects, to select them together again.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SelectionSet {
    pub name: String,
    pub ids: Vec<ObjectId>,
}

/// Most selection sets in a scene.
pub const MAX_SETS: usize = 1000;

fn is_false(v: &bool) -> bool {
    !*v
}

fn pivot_is_origin(v: &Vec3) -> bool {
    *v == Vec3::ZERO
}

/// Object id → position in the object list, so finding an object does not walk the list. It is
/// kept in step by everything that adds, removes or reorders objects; a scene straight from a
/// file has none until it is repaired, and lookups then walk the list as before. It is not part
/// of what a scene *is*: two scenes with the same objects are equal whatever their indexes hold.
#[derive(Clone, Debug, Default)]
struct Index(std::collections::HashMap<ObjectId, u32>);

impl PartialEq for Index {
    fn eq(&self, _: &Self) -> bool {
        true
    }
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
        self.reindex();
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
        self.reindex();
        self.anim.repair(|id| ids.contains(&id));
        self.constraints.repair(|id| ids.contains(&id));
        rig::repair(&mut self);
        hair::repair(&mut self);
        sim::repair(&mut self);
        point_deform::repair(&mut self);
        self.sets.truncate(MAX_SETS);
        for s in &mut self.sets {
            s.ids.retain(|i| ids.contains(i));
            s.name = s.name.chars().take(256).collect();
        }
        self.repair_organization();
        Ok(self)
    }

    pub fn objects(&self) -> &[Object] {
        &self.objects
    }
    /// Mutable objects without a revision bump, for repairs that change nothing a cache holds.
    pub(crate) fn objects_mut_unbumped(&mut self) -> impl Iterator<Item = &mut Object> {
        self.objects.iter_mut()
    }
    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        match self.slot(id) {
            Slot::At(i) => self.objects.get(i),
            Slot::Absent => None,
            Slot::Unknown => self.objects.iter().find(|o| o.id == id),
        }
    }

    /// Where the index says `id` is. `Unknown` when the index is not in step with the list
    /// (a scene not repaired yet) or points at another object: the caller then walks the list.
    fn slot(&self, id: ObjectId) -> Slot {
        if self.index.0.len() != self.objects.len() {
            return Slot::Unknown;
        }
        match self.index.0.get(&id) {
            None => Slot::Absent,
            Some(i) if self.objects.get(*i as usize).is_some_and(|o| o.id == id) => Slot::At(*i as usize),
            Some(_) => Slot::Unknown,
        }
    }

    /// Rebuild the index after the list was reordered or objects were removed.
    fn reindex(&mut self) {
        self.index.0.clear();
        self.index.0.extend(self.objects.iter().enumerate().map(|(i, o)| (o.id, i as u32)));
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
        let o = self.object_mut(id).ok_or(SceneError::NoObject(id))?;
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
            locked: false,
            template: false,
            tags: Vec::new(),
            layer: None,
            user: Default::default(),
            pivot: Vec3::ZERO,
            rev: next_rev(),
        });
        // Adding at the end keeps a complete index complete.
        if self.index.0.len() + 1 == self.objects.len() {
            self.index.0.insert(id, (self.objects.len() - 1) as u32);
        } else {
            self.reindex();
        }
        Ok(id)
    }

    /// Put back an object removed earlier (undo), at its old index.
    pub fn insert_at(&mut self, index: usize, mut object: Object) {
        object.rev = next_rev();
        self.next_id = self.next_id.max(object.id.0 + 1);
        let i = index.min(self.objects.len());
        self.objects.insert(i, object);
        self.reindex();
    }

    pub fn index_of(&self, id: ObjectId) -> Option<usize> {
        match self.slot(id) {
            Slot::At(i) => Some(i),
            Slot::Absent => None,
            Slot::Unknown => self.objects.iter().position(|o| o.id == id),
        }
    }

    /// An object for changing in place, without bumping its revision (the caller does).
    pub(crate) fn object_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        match self.slot(id) {
            Slot::At(i) => self.objects.get_mut(i),
            Slot::Absent => None,
            Slot::Unknown => self.objects.iter_mut().find(|o| o.id == id),
        }
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
        self.reindex();
        self.bump_ancestors_of_removed(&out);
        for (_, o) in &out {
            self.anim.remove_object(o.id);
            self.constraints.remove_object(o.id);
        }
        for s in &mut self.sets {
            s.ids.retain(|i| !doomed.contains(i));
        }
        self.prune_organization();
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
            self.reindex();
        }
        Ok(())
    }

    /// Object → world matrix.
    pub fn world_matrix(&self, id: ObjectId) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        let mut c = self.get(id);
        for _ in 0..MAX_DEPTH {
            let Some(o) = c else { break };
            m = o.local_matrix() * m;
            c = o.parent.and_then(|p| self.get(p));
        }
        m
    }

    /// True if the object is the input of its parent generator (a smooth cage, array, symmetry
    /// or clone generator) and so is shown through it, not directly. The input of a generator is
    /// its first child that makes geometry; other children are ordinary objects and draw as they are.
    pub fn is_cage_input(&self, id: ObjectId) -> bool {
        let Some(parent) = self.get(id).and_then(|o| o.parent) else { return false };
        match self.get(parent).map(|p| &p.kind) {
            Some(ObjectKind::SmoothCage { .. }) => true,
            Some(ObjectKind::Array { .. } | ObjectKind::Symmetry { .. } | ObjectKind::Cloner { .. }) => self.generator_input(parent) == Some(id),
            Some(ObjectKind::Sim { .. }) => self.is_particles(parent) && self.generator_input(parent) == Some(id),
            _ => false,
        }
    }

    /// The input a generator shows: its first child that makes geometry (a mesh or another
    /// generator).
    pub fn generator_input(&self, id: ObjectId) -> Option<ObjectId> {
        self.children(id).into_iter().find(|c| {
            self.get(*c).is_some_and(|o| {
                matches!(
                    o.kind,
                    ObjectKind::Mesh { .. }
                        | ObjectKind::SmoothCage { .. }
                        | ObjectKind::Array { .. }
                        | ObjectKind::Symmetry { .. }
                        | ObjectKind::SplineGenerator { .. }
                )
            })
        })
    }

    pub fn visible(&self, id: ObjectId) -> bool {
        let mut c = self.get(id);
        for _ in 0..MAX_DEPTH {
            let Some(o) = c else { return true };
            if !o.visible || o.layer.and_then(|l| self.layer(l)).is_some_and(|l| !l.visible) {
                return false;
            }
            c = o.parent.and_then(|p| self.get(p));
        }
        true
    }

    /// The visible clone modifiers inside the clone generator `id`, in order.
    pub fn clone_modifiers(&self, id: ObjectId) -> Vec<ObjectId> {
        self.children(id).into_iter().filter(|c| self.get(*c).is_some_and(|o| o.visible && matches!(o.kind, ObjectKind::CloneModifier { .. }))).collect()
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
mod spline_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod uv_tests;
