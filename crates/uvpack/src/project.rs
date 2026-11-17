//! UV projections: planar, box, cylindrical, spherical and camera.
//!
//! Each gives every corner of the chosen faces a UV from its position; corners of other faces keep
//! their UVs. UV space follows glTF: the origin is the top left, u grows to the right and v
//! downwards, so a surface seen from outside along the projection direction is not mirrored.
//! Positions are in the mesh's own space; the caller transforms them first for a world-space
//! projection.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec2, Vec3, Vec4};
use modelcraft_mesh::PolyMesh;
use serde::{Deserialize, Serialize};

use crate::{MAX_FACES, PackError};

/// A signed coordinate axis: the side a planar or box projection looks from, or the axis of a
/// cylinder or sphere (the sign is ignored there).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    PosX,
    NegX,
    #[default]
    PosY,
    NegY,
    PosZ,
    NegZ,
}

impl Axis {
    pub const ALL: [Axis; 6] = [Axis::PosX, Axis::NegX, Axis::PosY, Axis::NegY, Axis::PosZ, Axis::NegZ];

    /// The unit vector the projection looks from.
    pub fn dir(self) -> Vec3 {
        match self {
            Axis::PosX => Vec3::X,
            Axis::NegX => Vec3::NEG_X,
            Axis::PosY => Vec3::Y,
            Axis::NegY => Vec3::NEG_Y,
            Axis::PosZ => Vec3::Z,
            Axis::NegZ => Vec3::NEG_Z,
        }
    }

    /// `(right, down)` on the image plane of a viewer standing on this side, looking at the
    /// origin, with +Y up (looking along Y, -Z up).
    pub fn frame(self) -> (Vec3, Vec3) {
        let up = match self {
            Axis::PosY => Vec3::NEG_Z,
            Axis::NegY => Vec3::Z,
            _ => Vec3::Y,
        };
        let forward = -self.dir();
        (forward.cross(up), -up)
    }

    /// The axis index (0 X, 1 Y, 2 Z).
    pub fn index(self) -> usize {
        match self {
            Axis::PosX | Axis::NegX => 0,
            Axis::PosY | Axis::NegY => 1,
            Axis::PosZ | Axis::NegZ => 2,
        }
    }
}

/// How the projected UVs are scaled afterwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    /// Keep the projection's own scale.
    None,
    /// Scale evenly (and shift) so the result fills the unit square along its longer side.
    #[default]
    Uniform,
    /// Stretch to fill the unit square on both sides.
    Stretch,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Projection {
    /// Parallel projection onto a plane seen from `from`.
    Planar { from: Axis },
    /// Each face is projected along the axis its normal points along most, the six views laid out in a 3×2 grid.
    Box,
    /// Angle around `axis` as u, distance along it as v.
    Cylindrical { axis: Axis },
    /// Longitude around `axis` as u, latitude as v.
    Spherical { axis: Axis },
    /// Perspective or orthographic view-projection matrix (column-major, as `Mat4::to_cols_array`).
    Camera { view_proj: [f32; 16] },
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectOptions {
    pub projection: Projection,
    pub fit: Fit,
    /// Faces to project; empty means every face.
    pub faces: Vec<u32>,
}

impl ProjectOptions {
    pub fn new(projection: Projection) -> Self {
        ProjectOptions { projection, fit: Fit::Uniform, faces: Vec::new() }
    }
}

/// Wrap angles to within half a turn of the first, so a face that straddles the 0/2π cut keeps
/// its corners together.
fn unwrap_angles(a: &mut [f32]) {
    if let Some(&first) = a.first() {
        for x in a.iter_mut() {
            *x += TAU * ((first - *x) / TAU).round();
        }
    }
}

fn cyclic(axis: Axis, p: Vec3) -> (f32, f32, f32) {
    // (along the axis, next, next-next) with the cyclic order X → Y → Z → X.
    match axis.index() {
        0 => (p.x, p.y, p.z),
        1 => (p.y, p.z, p.x),
        _ => (p.z, p.x, p.y),
    }
}

/// Project `mesh`: one UV per corner, in corner order (see [`PolyMesh::corner_count`]). Corners
/// of faces that are not projected keep their UV (zero for a mesh without UVs).
pub fn project(mesh: &PolyMesh, opts: &ProjectOptions) -> Result<Vec<Vec2>, PackError> {
    let nf = mesh.face_count();
    if nf == 0 {
        return Err(PackError::NoFaces);
    }
    if nf > MAX_FACES {
        return Err(PackError::TooLarge(nf));
    }
    let mut uvs: Vec<Vec2> = if mesh.has_uvs() { mesh.corner_uvs().to_vec() } else { vec![Vec2::ZERO; mesh.corner_count()] };
    let faces: Vec<u32> = if opts.faces.is_empty() { (0..nf as u32).collect() } else { opts.faces.iter().copied().filter(|f| (*f as usize) < nf).collect() };
    if faces.is_empty() {
        return Ok(uvs);
    }
    let (lo, hi) = mesh.bounds();
    let centre = 0.5 * (lo + hi);
    let size = (hi - lo).max_element().max(1e-6);

    // Raw projected coordinates per (face, corner).
    let mut raw: Vec<(u32, Vec2)> = Vec::new();
    let mut corners: Vec<u32> = Vec::new();
    for &f in &faces {
        corners.clear();
        corners.extend(mesh.face_half_edges(f));
        let pts: Vec<Vec3> = corners.iter().map(|h| mesh.position(mesh.origin(*h)) - centre).collect();
        match opts.projection {
            Projection::Planar { from } => {
                let (right, down) = from.frame();
                raw.extend(corners.iter().zip(&pts).map(|(h, p)| (*h, Vec2::new(p.dot(right), p.dot(down)))));
            }
            Projection::Box => {
                let n = mesh.face_normal(f);
                let best = Axis::ALL.into_iter().max_by(|a, b| n.dot(a.dir()).total_cmp(&n.dot(b.dir()))).unwrap_or(Axis::PosY);
                let (right, down) = best.frame();
                let cell = Axis::ALL.iter().position(|a| *a == best).unwrap_or(0);
                // Grid of 3 columns by 2 rows, each cell the object's largest size plus a gap.
                let pitch = size * 1.1;
                let origin = Vec2::new((cell % 3) as f32 * pitch, (cell / 3) as f32 * pitch);
                raw.extend(corners.iter().zip(&pts).map(|(h, p)| (*h, origin + Vec2::new(p.dot(right), p.dot(down)))));
            }
            Projection::Cylindrical { axis } | Projection::Spherical { axis } => {
                let mut theta: Vec<f32> = Vec::with_capacity(pts.len());
                let mut along: Vec<f32> = Vec::with_capacity(pts.len());
                let mut radial: Vec<f32> = Vec::with_capacity(pts.len());
                for p in &pts {
                    let (a, n1, n2) = cyclic(axis, *p);
                    // On the axis the angle is undefined: mark it and fill it in from the face.
                    let r = (n1 * n1 + n2 * n2).sqrt();
                    theta.push(if r > 1e-6 * size { n2.atan2(n1) } else { f32::NAN });
                    along.push(a);
                    radial.push(r);
                }
                let known: Vec<f32> = theta.iter().copied().filter(|t| t.is_finite()).collect();
                let mut ref_angle = known.first().copied().unwrap_or(0.0);
                let mut fixed: Vec<f32> = known.clone();
                unwrap_angles(&mut fixed);
                if !fixed.is_empty() {
                    ref_angle = fixed.iter().sum::<f32>() / fixed.len() as f32;
                }
                for t in &mut theta {
                    if !t.is_finite() {
                        *t = ref_angle;
                    }
                }
                unwrap_angles(&mut theta);
                for (i, h) in corners.iter().enumerate() {
                    let (t, a, r) = (theta.get(i).copied().unwrap_or(0.0), along.get(i).copied().unwrap_or(0.0), radial.get(i).copied().unwrap_or(0.0));
                    let uv = match opts.projection {
                        Projection::Cylindrical { .. } => Vec2::new(0.5 + t / TAU, 0.5 - a / size),
                        _ => Vec2::new(0.5 + t / TAU, 0.5 - a.atan2(r) / PI),
                    };
                    raw.push((*h, uv));
                }
            }
            Projection::Camera { view_proj } => {
                let m = Mat4::from_cols_array(&view_proj);
                for (h, p) in corners.iter().zip(&pts) {
                    let c = m * Vec4::new(p.x + centre.x, p.y + centre.y, p.z + centre.z, 1.0);
                    let w = if c.w.abs() < 1e-9 { 1e-9 } else { c.w };
                    raw.push((*h, Vec2::new(0.5 + 0.5 * c.x / w, 0.5 - 0.5 * c.y / w)));
                }
            }
        }
    }
    if raw.iter().any(|(_, p)| !p.is_finite()) {
        return Err(PackError::NonFinite);
    }

    // Fit the projected corners to the unit square.
    let (mut blo, mut bhi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for (_, p) in &raw {
        blo = blo.min(*p);
        bhi = bhi.max(*p);
    }
    let span = (bhi - blo).max(Vec2::splat(1e-9));
    let map = |p: Vec2| match opts.fit {
        Fit::None => p,
        Fit::Uniform => (p - blo) / span.max_element() + (Vec2::ONE - span / span.max_element()) * 0.5,
        Fit::Stretch => (p - blo) / span,
    };
    for (h, p) in raw {
        if let Some(u) = uvs.get_mut(h as usize) {
            *u = map(p);
        }
    }
    Ok(uvs)
}

/// Write [`project`]'s result into a copy of the mesh.
pub fn project_mesh(mesh: &PolyMesh, opts: &ProjectOptions) -> Result<PolyMesh, PackError> {
    let uvs = project(mesh, opts)?;
    let mut out = mesh.clone();
    out.set_uvs(&uvs)?;
    Ok(out)
}
