//! The clone generator: lays out many placed copies of one source object.
//!
//! A [`Cloner`] turns a [`Layout`] (linear, radial, grid, scatter, surface or vertices) into a
//! [`CloneSet`]: one transform and one colour per clone. A clone is an *instance*, never a copy
//! of the mesh, so the renderer and the viewport draw the one source mesh many times and a set of
//! ten thousand clones costs a few hundred kilobytes.
//!
//! Everything here is bounded and total: counts are clamped, a layout that would exceed
//! [`MAX_CLONES`] stops there and says so (`truncated`), every parameter is sanitized (a hostile
//! file with NaN or infinite numbers gives finite transforms), and random layouts use a small
//! seeded generator ([`Rng`]) so the same seed gives the same clones on every platform.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use glam::{EulerRot, Mat3, Mat4, Quat, Vec3};
pub mod modifier;
pub use modelcraft_mesh::PolyMesh;
pub use modifier::{Context, EASES, Ease, MODES, Mode, Modifier};
use serde::{Deserialize, Serialize};

/// The most clones one generator produces. The viewport draws every clone, so the cap is also the
/// worst-case draw cost; a layout beyond it is cut off and flagged ([`CloneSet::truncated`]).
pub const MAX_CLONES: usize = 10_000;

/// Largest magnitude accepted for lengths, spacings and extents.
const MAX_LEN: f32 = 100_000.0;
/// Largest magnitude accepted for angles, in degrees.
const MAX_DEG: f32 = 36_000.0;
/// Smallest clone scale (a clone never collapses or flips through zero).
const MIN_SCALE: f32 = 1e-3;

/// A small deterministic generator (SplitMix64): the same seed gives the same sequence on every
/// platform, which a saved scene needs.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(u64::from(seed).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// The axis a radial layout turns around.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    #[default]
    Y,
    Z,
}

impl Axis {
    pub fn vector(self) -> Vec3 {
        match self {
            Axis::X => Vec3::X,
            Axis::Y => Vec3::Y,
            Axis::Z => Vec3::Z,
        }
    }
    /// The two axes of the plane perpendicular to this one, in right-handed order.
    fn plane(self) -> (Vec3, Vec3) {
        match self {
            Axis::X => (Vec3::Y, Vec3::Z),
            Axis::Y => (Vec3::Z, Vec3::X),
            Axis::Z => (Vec3::X, Vec3::Y),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Axis::X => "x",
            Axis::Y => "y",
            Axis::Z => "z",
        }
    }
}

/// Where the clones go.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Layout {
    /// A row: each clone is moved by `offset`, turned by `rotation` degrees and grown by `scale`
    /// more than the one before.
    Linear { count: u32, offset: Vec3, rotation: Vec3, scale: Vec3 },
    /// A ring (or an arc, between `start` and `end` degrees) of `radius` around `axis`; `align`
    /// turns each clone to face outwards.
    Radial { count: u32, radius: f32, axis: Axis, start: f32, end: f32, align: bool },
    /// A box of `count` clones along X, Y and Z, `spacing` apart, centred on the origin.
    Grid { count: [u32; 3], spacing: Vec3 },
    /// `count` random positions inside a box of size `extent`, centred on the origin (seeded).
    Scatter { count: u32, extent: Vec3 },
    /// `count` random points on the surface of the target object, spread by area (seeded);
    /// `align` turns each clone to stand on the surface.
    Surface { count: u32, align: bool },
    /// One clone on every vertex of the target object; `align` follows the vertex normal.
    Vertices { align: bool },
}

/// The layout names, in menu order.
pub const KINDS: &[&str] = &["linear", "radial", "grid", "scatter", "surface", "vertices"];

impl Layout {
    pub fn default_of(kind: &str) -> Option<Layout> {
        Some(match kind {
            "linear" => Layout::Linear { count: 5, offset: Vec3::new(1.0, 0.0, 0.0), rotation: Vec3::ZERO, scale: Vec3::ZERO },
            "radial" => Layout::Radial { count: 8, radius: 2.0, axis: Axis::Y, start: 0.0, end: 360.0, align: true },
            "grid" => Layout::Grid { count: [4, 1, 4], spacing: Vec3::splat(1.0) },
            "scatter" => Layout::Scatter { count: 30, extent: Vec3::splat(4.0) },
            "surface" => Layout::Surface { count: 60, align: true },
            "vertices" => Layout::Vertices { align: true },
            _ => return None,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Layout::Linear { .. } => "linear",
            Layout::Radial { .. } => "radial",
            Layout::Grid { .. } => "grid",
            Layout::Scatter { .. } => "scatter",
            Layout::Surface { .. } => "surface",
            Layout::Vertices { .. } => "vertices",
        }
    }

    /// True when the layout needs a target object to lay clones on.
    pub fn needs_target(&self) -> bool {
        matches!(self, Layout::Surface { .. } | Layout::Vertices { .. })
    }

    /// True when the layout draws random numbers (the seed matters).
    pub fn is_random(&self) -> bool {
        matches!(self, Layout::Scatter { .. } | Layout::Surface { .. })
    }

    /// Clamp every number into its safe range (non-finite numbers become the neutral value).
    pub fn sanitized(self) -> Layout {
        let len = |v: f32| if v.is_finite() { v.clamp(-MAX_LEN, MAX_LEN) } else { 0.0 };
        let v3 = |v: Vec3| Vec3::new(len(v.x), len(v.y), len(v.z));
        let deg = |v: f32| if v.is_finite() { v.clamp(-MAX_DEG, MAX_DEG) } else { 0.0 };
        let dv3 = |v: Vec3| Vec3::new(deg(v.x), deg(v.y), deg(v.z));
        let cap = |c: u32| c.clamp(1, MAX_CLONES as u32);
        match self {
            Layout::Linear { count, offset, rotation, scale } => {
                let s = |v: f32| if v.is_finite() { v.clamp(-100.0, 100.0) } else { 0.0 };
                Layout::Linear { count: cap(count), offset: v3(offset), rotation: dv3(rotation), scale: Vec3::new(s(scale.x), s(scale.y), s(scale.z)) }
            }
            Layout::Radial { count, radius, axis, start, end, align } => {
                Layout::Radial { count: cap(count), radius: len(radius).max(0.0), axis, start: deg(start), end: deg(end), align }
            }
            Layout::Grid { count, spacing } => Layout::Grid { count: count.map(cap), spacing: v3(spacing) },
            Layout::Scatter { count, extent } => Layout::Scatter { count: cap(count), extent: v3(extent).abs() },
            Layout::Surface { count, align } => Layout::Surface { count: cap(count), align },
            Layout::Vertices { align } => Layout::Vertices { align },
        }
    }

    /// How many clones the layout asks for before the cap (for the Properties readout).
    pub fn requested(&self, target_vertices: usize) -> u64 {
        match *self {
            Layout::Linear { count, .. } | Layout::Radial { count, .. } | Layout::Scatter { count, .. } | Layout::Surface { count, .. } => {
                u64::from(count.max(1))
            }
            Layout::Grid { count, .. } => count.iter().fold(1u64, |a, c| a.saturating_mul(u64::from((*c).max(1)))),
            Layout::Vertices { .. } => target_vertices as u64,
        }
    }
}

/// The clone generator's settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cloner {
    #[serde(flatten)]
    pub layout: Layout,
    /// Seeds the random layouts (and, later, the random clone modifiers).
    #[serde(default)]
    pub seed: u32,
}

impl Cloner {
    pub fn default_of(kind: &str) -> Option<Cloner> {
        Layout::default_of(kind).map(|layout| Cloner { layout, seed: 1 })
    }

    pub fn kind(&self) -> &'static str {
        self.layout.kind()
    }

    pub fn sanitized(self) -> Cloner {
        Cloner { layout: self.layout.sanitized(), seed: self.seed }
    }

    /// Lay out the clones. `target` is the mesh the surface and vertices layouts sit on, in the
    /// generator's own space.
    pub fn generate(&self, target: Option<&PolyMesh>) -> Result<CloneSet, CloneError> {
        self.generate_in(target, Mat4::IDENTITY)
    }

    /// Lay out the clones with the target mesh seen through `frame` (the matrix from the target's
    /// space into the generator's). Points are spread by area in the generator's space, and a
    /// clone that stands on the surface follows the true surface normal even when the frame
    /// scales unevenly (normals move by the inverse transpose, not as directions).
    pub fn generate_in(&self, target: Option<&PolyMesh>, frame: Mat4) -> Result<CloneSet, CloneError> {
        let frame = if frame.is_finite() && frame.determinant().abs() > 1e-20 { frame } else { Mat4::IDENTITY };
        let flip = if frame.determinant() < 0.0 { -1.0 } else { 1.0 };
        let normal_matrix = frame.inverse().transpose();
        let layout = self.layout.sanitized();
        let mut rng = Rng::new(self.seed);
        let mut set = CloneSet::default();
        match layout {
            Layout::Linear { count, offset, rotation, scale } => {
                for i in 0..count {
                    let k = i as f32;
                    let rot = Quat::from_euler(EulerRot::XYZ, (rotation.x * k).to_radians(), (rotation.y * k).to_radians(), (rotation.z * k).to_radians());
                    let s = (Vec3::ONE + scale * k).max(Vec3::splat(MIN_SCALE));
                    set.push(Mat4::from_scale_rotation_translation(s, rot, offset * k));
                }
            }
            Layout::Radial { count, radius, axis, start, end, align } => {
                let (u, v) = axis.plane();
                let a = axis.vector();
                let span = end - start;
                // A full turn must not put the last clone on top of the first.
                let steps = if span.abs() >= 360.0 { count } else { count.saturating_sub(1).max(1) };
                for i in 0..count {
                    let theta = (start + span * i as f32 / steps as f32).to_radians();
                    let radial = u * theta.cos() + v * theta.sin();
                    let rot = if align { Quat::from_mat3(&Mat3::from_cols(radial, a, radial.cross(a))) } else { Quat::IDENTITY };
                    set.push(Mat4::from_rotation_translation(rot, radial * radius));
                }
            }
            Layout::Grid { count, spacing } => {
                let [nx, ny, nz] = count.map(u64::from);
                let total = nx.saturating_mul(ny).saturating_mul(nz);
                let n = total.min(MAX_CLONES as u64);
                set.truncated = total > n;
                let centre = |i: u64, n: u64| i as f32 - (n as f32 - 1.0) * 0.5;
                for idx in 0..n {
                    let (x, y, z) = (idx % nx, (idx / nx) % ny, idx / (nx * ny));
                    let p = Vec3::new(centre(x, nx), centre(y, ny), centre(z, nz)) * spacing;
                    set.push(Mat4::from_translation(p));
                }
            }
            Layout::Scatter { count, extent } => {
                for _ in 0..count {
                    let p = Vec3::new(rng.next_f32() - 0.5, rng.next_f32() - 0.5, rng.next_f32() - 0.5) * extent;
                    set.push(Mat4::from_translation(p));
                }
            }
            Layout::Surface { count, align } => {
                let mesh = target.ok_or(CloneError::NeedsTarget)?;
                let (tris, _) = mesh.triangulate();
                let pos = mesh.positions();
                let corner = |i: u32| frame.transform_point3(pos.get(i as usize).copied().unwrap_or(Vec3::ZERO));
                let mut cumulative = Vec::with_capacity(tris.len());
                let mut total = 0.0f32;
                for t in &tris {
                    let area = (corner(t[1]) - corner(t[0])).cross(corner(t[2]) - corner(t[0])).length() * 0.5;
                    total += if area.is_finite() { area } else { 0.0 };
                    cumulative.push(total);
                }
                if tris.is_empty() || total <= 0.0 {
                    return Err(CloneError::EmptyTarget);
                }
                for _ in 0..count {
                    let pick = rng.next_f32() * total;
                    let ti = cumulative.partition_point(|c| *c <= pick).min(tris.len() - 1);
                    let Some(t) = tris.get(ti) else { continue };
                    let (a, b, c) = (corner(t[0]), corner(t[1]), corner(t[2]));
                    let (r1, r2) = (rng.next_f32().sqrt(), rng.next_f32());
                    let p = a * (1.0 - r1) + b * (r1 * (1.0 - r2)) + c * (r1 * r2);
                    let rot = if align { up_to((b - a).cross(c - a) * flip) } else { Quat::IDENTITY };
                    set.push(Mat4::from_rotation_translation(rot, p));
                }
            }
            Layout::Vertices { align } => {
                let mesh = target.ok_or(CloneError::NeedsTarget)?;
                if mesh.vertex_count() == 0 {
                    return Err(CloneError::EmptyTarget);
                }
                let n = mesh.vertex_count().min(MAX_CLONES);
                set.truncated = mesh.vertex_count() > n;
                let normals = if align { mesh.vertex_normals() } else { Vec::new() };
                for (i, p) in mesh.positions().iter().take(n).enumerate() {
                    let rot = if align { up_to(normal_matrix.transform_vector3(normals.get(i).copied().unwrap_or(Vec3::Y))) } else { Quat::IDENTITY };
                    set.push(Mat4::from_rotation_translation(rot, frame.transform_point3(*p)));
                }
            }
        }
        // Counts past the cap on the row layouts were clamped by `sanitized`; flag any layout that
        // asked for more than it got.
        set.truncated |= self.layout.requested(target.map_or(0, PolyMesh::vertex_count)) > set.instances.len() as u64;
        Ok(set)
    }
}

/// The rotation that stands the +Y axis on `normal` (identity for a degenerate normal).
fn up_to(normal: Vec3) -> Quat {
    let n = normal.normalize_or_zero();
    if n == Vec3::ZERO { Quat::IDENTITY } else { Quat::from_rotation_arc(Vec3::Y, n) }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CloneError {
    #[error("this layout needs a target object: choose one in the Properties panel")]
    NeedsTarget,
    #[error("the target object has no surface to place clones on")]
    EmptyTarget,
}

/// One clone: where it is (relative to the generator), its colour tint and its position in the
/// layout.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CloneInstance {
    pub matrix: Mat4,
    /// Multiplies the source's colour.
    pub color: [f32; 4],
    pub index: u32,
}

/// The clones of one generator.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CloneSet {
    pub instances: Vec<CloneInstance>,
    /// The layout asked for more clones than [`MAX_CLONES`] (or the target had more vertices).
    pub truncated: bool,
}

impl CloneSet {
    fn push(&mut self, matrix: Mat4) {
        if self.instances.len() >= MAX_CLONES {
            self.truncated = true;
            return;
        }
        let matrix = if matrix.is_finite() { matrix } else { Mat4::IDENTITY };
        self.instances.push(CloneInstance { matrix, color: [1.0; 4], index: self.instances.len() as u32 });
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// Move every clone by `to_generator` (a target's frame into the generator's frame).
    pub fn transform_all(&mut self, to_generator: Mat4) {
        if !to_generator.is_finite() {
            return;
        }
        for i in &mut self.instances {
            i.matrix = to_generator * i.matrix;
        }
    }
}

#[cfg(test)]
mod tests;
