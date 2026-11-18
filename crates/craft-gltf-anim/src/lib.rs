//! glTF 2.0 with skins, morph targets and animations.
//!
//! `craft-mesh` reads and writes static glTF scenes; it has no skins, morph targets or
//! animation. This crate carries exactly those (with the node tree, triangle meshes and plain
//! PBR factors they hang on) so an app can exchange rigged, animated characters. It uses no app
//! type and no maths library: positions, quaternions and matrices are plain arrays in glTF's own
//! conventions (Y up, metres, right-handed, quaternions `x y z w`, column-major matrices,
//! seconds).
//!
//! - [`Document`]: nodes, meshes (one triangle list each, up to four joints per vertex, morph
//!   targets), skins, animations (translation, rotation, scale and morph-weight channels) and
//!   materials.
//! - [`write_glb`]: a document as one binary `.glb`.
//! - [`read`]: a `.glb` or `.gltf` (with `data:` and external buffers) into a document.
//!
//! Written from the Khronos glTF 2.0 specification. Files are hostile: every count, offset and
//! index is checked, sizes are capped by [`Limits`], and nothing here panics.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod read;
mod write;

pub use read::{has_rig_content, read};
pub use write::write_glb;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("not a glTF file: {0}")]
    Malformed(String),
    #[error("the glTF file is too large: {0}")]
    TooLarge(String),
    #[error("the glTF file uses {0}, which is not supported")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Bounds on what [`read`] accepts and [`write_glb`] produces.
#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    /// Total bytes of all buffers.
    pub max_buffer_bytes: usize,
    /// Nodes, meshes, skins, animations, accessors and so on, each.
    pub max_items: usize,
    /// Elements of one accessor (vertices, indices, keys).
    pub max_elements: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_buffer_bytes: 1 << 30, max_items: 200_000, max_elements: 50_000_000 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    pub parent: Option<usize>,
    pub translation: [f32; 3],
    /// A unit quaternion, `x y z w`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub mesh: Option<usize>,
    pub skin: Option<usize>,
}

impl Default for Node {
    fn default() -> Self {
        Node { name: String::new(), parent: None, translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3], mesh: None, skin: None }
    }
}

/// A morph target: one offset per vertex.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Target {
    pub name: String,
    pub deltas: Vec<[f32; 3]>,
}

/// One triangle list. `normals`, `uvs`, `joints` and `weights` are empty or one per vertex.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Indices into the skin's joint list.
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub targets: Vec<Target>,
    /// The targets' weights at rest (empty: all zero).
    pub target_weights: Vec<f32>,
    pub material: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skin {
    pub name: String,
    /// Node indices.
    pub joints: Vec<usize>,
    /// Per joint, column-major: mesh space → joint space at bind (identity when empty).
    pub inverse_bind: Vec<[f32; 16]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    Translation,
    Rotation,
    Scale,
    /// Morph-target weights: as many values per key as the node's mesh has targets.
    Weights,
}

impl Path {
    pub fn id(self) -> &'static str {
        match self {
            Path::Translation => "translation",
            Path::Rotation => "rotation",
            Path::Scale => "scale",
            Path::Weights => "weights",
        }
    }
    /// Values per key (`None` for weights: it depends on the mesh).
    pub fn width(self) -> Option<usize> {
        match self {
            Path::Translation | Path::Scale => Some(3),
            Path::Rotation => Some(4),
            Path::Weights => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Interpolation {
    #[default]
    Linear,
    Step,
    /// Three values per key: in-tangent, value, out-tangent.
    CubicSpline,
}

/// One animated property of one node.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub node: usize,
    pub path: Path,
    pub interpolation: Interpolation,
    /// Key times in seconds, increasing.
    pub times: Vec<f32>,
    /// The keys' values, one after another.
    pub values: Vec<f32>,
}

impl Channel {
    /// Values per key, from the lengths.
    pub fn width(&self) -> usize {
        let per = if self.interpolation == Interpolation::CubicSpline { 3 } else { 1 };
        if self.times.is_empty() { 0 } else { self.values.len() / (self.times.len() * per) }
    }

    /// The value of key `i` (for cubic splines, the value between its tangents).
    pub fn key(&self, i: usize) -> Option<&[f32]> {
        let w = self.width();
        let (stride, offset) = if self.interpolation == Interpolation::CubicSpline { (3 * w, w) } else { (w, 0) };
        self.values.get(i * stride + offset..i * stride + offset + w)
    }
}

/// A clip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Animation {
    pub name: String,
    pub channels: Vec<Channel>,
}

impl Animation {
    /// The time of the last key.
    pub fn duration(&self) -> f32 {
        self.channels.iter().filter_map(|c| c.times.last().copied()).fold(0.0, f32::max)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
}

impl Default for Material {
    fn default() -> Self {
        Material { name: String::new(), base_color: [1.0; 4], metallic: 1.0, roughness: 1.0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    pub nodes: Vec<Node>,
    pub meshes: Vec<Mesh>,
    pub skins: Vec<Skin>,
    pub animations: Vec<Animation>,
    pub materials: Vec<Material>,
}

impl Document {
    /// Check that every index points at something and every list has the length it must have.
    /// [`write_glb`] calls this; [`read`] only returns documents that pass.
    pub fn validate(&self, limits: &Limits) -> Result<()> {
        let bad = |m: String| Err(Error::Malformed(m));
        for (name, n) in [
            ("nodes", self.nodes.len()),
            ("meshes", self.meshes.len()),
            ("skins", self.skins.len()),
            ("animations", self.animations.len()),
            ("materials", self.materials.len()),
        ] {
            if n > limits.max_items {
                return Err(Error::TooLarge(format!("{n} {name}")));
            }
        }
        for (i, n) in self.nodes.iter().enumerate() {
            if n.parent.is_some_and(|p| p >= self.nodes.len() || p == i)
                || n.mesh.is_some_and(|m| m >= self.meshes.len())
                || n.skin.is_some_and(|s| s >= self.skins.len())
            {
                return bad(format!("node {i} points at something that is not there"));
            }
            // No cycles: walking up ends within as many steps as there are nodes.
            let mut at = n.parent;
            let mut steps = 0;
            while let Some(p) = at {
                steps += 1;
                if steps > self.nodes.len() {
                    return bad(format!("node {i} is its own ancestor"));
                }
                at = self.nodes.get(p).and_then(|x| x.parent);
            }
        }
        for (i, m) in self.meshes.iter().enumerate() {
            let n = m.positions.len();
            if n > limits.max_elements || m.indices.len() > limits.max_elements {
                return Err(Error::TooLarge(format!("mesh {i}")));
            }
            let per_vertex = [m.normals.len(), m.uvs.len(), m.joints.len(), m.weights.len()];
            if per_vertex.iter().any(|l| *l != 0 && *l != n) || m.joints.len() != m.weights.len() {
                return bad(format!("mesh {i} has attribute lists of different lengths"));
            }
            if !m.indices.len().is_multiple_of(3) || m.indices.iter().any(|x| *x as usize >= n) {
                return bad(format!("mesh {i} has a bad triangle list"));
            }
            if m.targets.iter().any(|t| t.deltas.len() != n) || (!m.target_weights.is_empty() && m.target_weights.len() != m.targets.len()) {
                return bad(format!("mesh {i} has morph targets that do not fit it"));
            }
            if m.material.is_some_and(|x| x >= self.materials.len()) {
                return bad(format!("mesh {i} names a material that is not there"));
            }
        }
        for (i, s) in self.skins.iter().enumerate() {
            if s.joints.is_empty() || s.joints.iter().any(|j| *j >= self.nodes.len()) || (!s.inverse_bind.is_empty() && s.inverse_bind.len() != s.joints.len())
            {
                return bad(format!("skin {i} has bad joints"));
            }
        }
        for n in &self.nodes {
            if let (Some(m), Some(s)) = (n.mesh.and_then(|m| self.meshes.get(m)), n.skin.and_then(|s| self.skins.get(s)))
                && m.joints.iter().flatten().any(|j| *j as usize >= s.joints.len())
            {
                return bad(format!("mesh {} uses joints its skin does not have", m.name));
            }
        }
        for (i, a) in self.animations.iter().enumerate() {
            for c in &a.channels {
                let per = if c.interpolation == Interpolation::CubicSpline { 3 } else { 1 };
                let width = match c.path.width() {
                    Some(w) => w,
                    None => self.nodes.get(c.node).and_then(|n| n.mesh).and_then(|m| self.meshes.get(m)).map_or(0, |m| m.targets.len()),
                };
                if c.node >= self.nodes.len() || c.times.len() > limits.max_elements || width == 0 || c.values.len() != c.times.len() * per * width {
                    return bad(format!("animation {i} has a channel that does not fit its node"));
                }
                if c.times.iter().any(|t| !t.is_finite() || *t < 0.0) || c.times.windows(2).any(|w| w.first() >= w.get(1)) {
                    return bad(format!("animation {i} has key times that do not increase"));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
