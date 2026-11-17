//! ModelCraft UV tools, UI-free and built on [`modelcraft_mesh::PolyMesh`] only, so the crate can
//! move to a shared library unchanged.
//!
//! - [`SeamSet`]: seams, marked and cleared by edge, found by angle or taken from existing UVs.
//! - [`Charts`]: the islands a mesh falls into when it is cut along its seams, and
//!   [`cut_to_disks`], which adds the cuts an island needs to be flattenable.
//! - [`unwrap`]: least-squares conformal maps (LSCM, Lévy et al. 2002) of every island, with
//!   optional pinned vertices, then a simple layout in the unit square.
//! - [`measure`]: angle, stretch and area distortion of a UV layout, per triangle and overall.
//!
//! UV space follows glTF: the origin is the top left corner of the image and v grows downwards, so
//! a triangle that is not mirrored has a negative signed area in (u, v) (see [`measure`]).
//!
//! Only published papers were used (see each module). The linear solver is a block-sparse
//! preconditioned conjugate-gradient written here; the crate has no dependencies beyond `glam`,
//! `serde` and `thiserror`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod charts;
mod cholesky;
mod distortion;
mod layout;
mod lscm;
mod seams;
mod sparse;

pub use charts::{Charts, CutReport, Island, cut_to_disks, face_triangles};
pub use distortion::{Distortion, TriangleMetric, measure, per_face};
pub use layout::shelf;
pub use lscm::{IslandReport, Pin};
pub use seams::{MAX_SEAMS, SeamSet, shortest_path};

use glam::{DVec2, Vec2};
use modelcraft_mesh::{MeshError, PolyMesh};

/// Most faces one unwrap accepts.
pub const MAX_FACES: usize = 4_000_000;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum UvError {
    #[error("the mesh has {0} faces, more than an unwrap accepts")]
    TooLarge(usize),
    #[error("the mesh has no faces")]
    NoFaces,
    #[error(transparent)]
    Mesh(#[from] MeshError),
}

/// What [`unwrap`] does besides the seams.
#[derive(Clone, Debug, PartialEq)]
pub struct UnwrapOptions {
    /// Vertices fixed to a UV position. An island with pins is solved around them and keeps its
    /// place and scale; the other islands are scaled and laid out.
    pub pins: Vec<Pin>,
    /// Add the cuts an island needs to be a disc (see [`cut_to_disks`]).
    pub auto_cut: bool,
    /// Lay the islands out in the unit square; when false each keeps its own origin at 0,0.
    pub layout: bool,
    /// Margin around and between islands, as a fraction of the unit square.
    pub margin: f32,
}

impl Default for UnwrapOptions {
    fn default() -> Self {
        UnwrapOptions { pins: Vec::new(), auto_cut: true, layout: true, margin: 0.01 }
    }
}

/// The result of [`unwrap`].
#[derive(Clone, Debug)]
pub struct UnwrapResult {
    /// One UV per corner, in corner order (see [`PolyMesh::corner_count`]).
    pub corner_uvs: Vec<Vec2>,
    /// The seams used: the given ones plus any added by `auto_cut`.
    pub seams: SeamSet,
    pub cut: CutReport,
    /// How each island was solved, in island order.
    pub islands: Vec<IslandReport>,
}

impl UnwrapResult {
    /// Islands that could not be solved and were projected onto a plane.
    pub fn fallbacks(&self) -> usize {
        self.islands.iter().filter(|i| i.fallback).count()
    }
}

/// Unwrap `mesh` along `seams`: solve LSCM per island, scale each island to the surface area it
/// covers, rotate it so its long axis is horizontal and lay the islands out.
pub fn unwrap(mesh: &PolyMesh, seams: &SeamSet, opts: &UnwrapOptions) -> Result<UnwrapResult, UvError> {
    if mesh.face_count() == 0 {
        return Err(UvError::NoFaces);
    }
    let (seams, cut) = if opts.auto_cut { cut_to_disks(mesh, seams)? } else { (seams.clone(), CutReport::default()) };
    let charts = Charts::build(mesh, &seams)?;
    let pins: Vec<(u32, DVec2)> = opts
        .pins
        .iter()
        .filter(|p| p.uv.is_finite())
        .filter_map(|p| Some((*charts.corner_vertex.get(p.corner as usize)?, DVec2::new(f64::from(p.uv.x), -f64::from(p.uv.y)))))
        .collect();

    struct Placed {
        island: usize,
        uv: Vec<DVec2>,
        fixed: bool,
        min: DVec2,
        size: DVec2,
    }
    let mut placed: Vec<Placed> = Vec::with_capacity(charts.islands.len());
    let mut reports = Vec::with_capacity(charts.islands.len());
    let mut tris = Vec::new();
    for (i, island) in charts.islands.iter().enumerate() {
        let mine: Vec<(u32, DVec2)> = pins.iter().copied().filter(|(v, _)| island.vertices.binary_search(v).is_ok()).collect();
        // The solver works with v up; ModelCraft's UV space has v down (origin at the top left, as
        // glTF), so pins go in and results come out mirrored in v.
        let mut solved = lscm::solve_island(mesh, &charts, island, &mine);
        for u in &mut solved.uv {
            u.y = -u.y;
        }
        let fixed = !mine.is_empty();
        if !fixed {
            // Scale to the surface area, then rotate the long axis to u.
            let (mut a3, mut auv) = (0.0, 0.0);
            for &f in &island.faces {
                tris.clear();
                face_triangles(mesh, f, &mut tris);
                for t in &tris {
                    let at = |c: u32| {
                        island.vertices.binary_search(&charts.corner_vertex.get(c as usize).copied().unwrap_or(0)).ok().and_then(|l| solved.uv.get(l).copied())
                    };
                    let (Some(u0), Some(u1), Some(u2)) = (at(t[0]), at(t[1]), at(t[2])) else { continue };
                    auv += 0.5 * (u1 - u0).perp_dot(u2 - u0).abs();
                    let p = t.map(|c| mesh.position(mesh.origin(c)).as_dvec3());
                    a3 += 0.5 * (p[1] - p[0]).cross(p[2] - p[0]).length();
                }
            }
            let s = if auv > 0.0 && a3 > 0.0 { (a3 / auv).sqrt() } else { 1.0 };
            let n = solved.uv.len().max(1) as f64;
            let mean = solved.uv.iter().fold(DVec2::ZERO, |a, b| a + *b) / n;
            let (mut cxx, mut cyy, mut cxy) = (0.0, 0.0, 0.0);
            for u in &solved.uv {
                let d = *u - mean;
                cxx += d.x * d.x;
                cyy += d.y * d.y;
                cxy += d.x * d.y;
            }
            let theta = 0.5 * (2.0 * cxy).atan2(cxx - cyy);
            let (sn, cs) = (-theta).sin_cos();
            for u in &mut solved.uv {
                let d = (*u - mean) * s;
                *u = DVec2::new(cs * d.x - sn * d.y, sn * d.x + cs * d.y);
            }
        }
        let (mut lo, mut hi) = (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN));
        for u in &solved.uv {
            lo = lo.min(*u);
            hi = hi.max(*u);
        }
        if !(lo.is_finite() && hi.is_finite()) {
            lo = DVec2::ZERO;
            hi = DVec2::ZERO;
        }
        placed.push(Placed { island: i, uv: solved.uv, fixed, min: lo, size: (hi - lo).max(DVec2::splat(1e-9)) });
        reports.push(solved.report);
    }

    let movable: Vec<usize> = (0..placed.len()).filter(|i| !placed[*i].fixed).collect();
    let mut offset = vec![DVec2::ZERO; placed.len()];
    let mut scale = vec![1.0f64; placed.len()];
    if opts.layout {
        let sizes: Vec<DVec2> = movable.iter().map(|i| placed[*i].size).collect();
        let (places, s) = shelf(&sizes, f64::from(opts.margin));
        for (k, i) in movable.iter().enumerate() {
            scale[*i] = s;
            offset[*i] = places.get(k).copied().unwrap_or(DVec2::ZERO);
        }
    }
    let mut corner_uvs = vec![Vec2::ZERO; mesh.corner_count()];
    for c in 0..corner_uvs.len() {
        let f = mesh.face_of(c as u32);
        let Some(&isl) = charts.face_island.get(f as usize) else { continue };
        let Some(pl) = placed.get(isl as usize) else { continue };
        let Some(island) = charts.islands.get(pl.island) else { continue };
        let Some(l) = charts.corner_vertex.get(c).and_then(|v| island.vertices.binary_search(v).ok()) else { continue };
        let Some(u) = pl.uv.get(l) else { continue };
        let v = if pl.fixed { *u } else { (*u - pl.min) * scale[isl as usize] + offset[isl as usize] };
        if let Some(o) = corner_uvs.get_mut(c) {
            *o = if v.is_finite() { v.as_vec2() } else { Vec2::ZERO };
        }
    }
    Ok(UnwrapResult { corner_uvs, seams, cut, islands: reports })
}

/// [`unwrap`] and write the UVs into a copy of the mesh.
pub fn unwrap_mesh(mesh: &PolyMesh, seams: &SeamSet, opts: &UnwrapOptions) -> Result<(PolyMesh, UnwrapResult), UvError> {
    let result = unwrap(mesh, seams, opts)?;
    let mut out = mesh.clone();
    out.set_uvs(&result.corner_uvs)?;
    Ok((out, result))
}

#[cfg(test)]
mod tests;
