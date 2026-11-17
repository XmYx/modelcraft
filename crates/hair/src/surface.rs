//! The surface hair grows from: a triangle soup with vertex normals, and points on it.

use glam::Vec3;
use modelcraft_mesh::PolyMesh;

use crate::rng::{self, stream};
use crate::{HairError, MAX_SURFACE_TRIANGLES};

/// A point on a [`Surface`]: a triangle and barycentric coordinates (`bary` sums to 1).
///
/// Roots are stored this way, never as positions, so they follow the surface when it deforms.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SurfacePoint {
    pub tri: u32,
    pub bary: [f32; 3],
}

/// Triangles, positions and vertex normals. Built once from a mesh; hair operations borrow it.
#[derive(Clone, Debug)]
pub struct Surface {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
    areas: Vec<f32>,
}

impl Surface {
    /// The triangulated faces of `mesh` with its area-weighted vertex normals.
    pub fn from_mesh(mesh: &PolyMesh) -> Result<Surface, HairError> {
        if mesh.triangle_count() > MAX_SURFACE_TRIANGLES {
            return Err(HairError::SurfaceTooLarge(mesh.triangle_count()));
        }
        let (tris, _) = mesh.triangulate();
        Surface::from_triangles(mesh.positions().to_vec(), tris)
    }

    /// A surface from raw triangles; normals are area-weighted. Rejects non-finite positions and
    /// indices out of range. Degenerate triangles are kept but never receive hair.
    pub fn from_triangles(positions: Vec<Vec3>, tris: Vec<[u32; 3]>) -> Result<Surface, HairError> {
        if tris.len() > MAX_SURFACE_TRIANGLES {
            return Err(HairError::SurfaceTooLarge(tris.len()));
        }
        if positions.iter().any(|p| !p.is_finite()) {
            return Err(HairError::NotFinite("surface position"));
        }
        let n = positions.len();
        if tris.iter().flatten().any(|&v| v as usize >= n) {
            return Err(HairError::BadIndex);
        }
        let mut normals = vec![Vec3::ZERO; n];
        let mut areas = Vec::with_capacity(tris.len());
        for t in &tris {
            let [a, b, c] = corners(&positions, *t);
            let cross = (b - a).cross(c - a);
            areas.push(0.5 * cross.length());
            for &v in t {
                if let Some(nv) = normals.get_mut(v as usize) {
                    *nv += cross;
                }
            }
        }
        for v in &mut normals {
            *v = v.try_normalize().unwrap_or(Vec3::Y);
        }
        let total: f64 = areas.iter().map(|a| f64::from(*a)).sum();
        if tris.is_empty() || !(total > 0.0) || !total.is_finite() {
            return Err(HairError::NoSurface);
        }
        Ok(Surface { positions, normals, tris, areas })
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn positions(&self) -> &[Vec3] {
        &self.positions
    }

    pub fn normals(&self) -> &[Vec3] {
        &self.normals
    }

    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.tris
    }

    /// Surface area, in scene units squared.
    pub fn area(&self) -> f64 {
        self.areas.iter().map(|a| f64::from(*a)).sum()
    }

    pub fn triangle_area(&self, tri: usize) -> f32 {
        self.areas.get(tri).copied().unwrap_or(0.0)
    }

    /// Replace positions after the mesh deformed (same topology): keeps triangles, recomputes
    /// normals and areas. Roots stay valid because they are barycentric.
    pub fn with_positions(&self, positions: Vec<Vec3>) -> Result<Surface, HairError> {
        if positions.len() != self.positions.len() {
            return Err(HairError::LengthMismatch { what: "positions", expected: self.positions.len(), found: positions.len() });
        }
        Surface::from_triangles(positions, self.tris.clone())
    }

    /// Position of a surface point (the origin for a point whose triangle does not exist).
    pub fn position(&self, p: &SurfacePoint) -> Vec3 {
        let Some(t) = self.tris.get(p.tri as usize) else { return Vec3::ZERO };
        let [a, b, c] = corners(&self.positions, *t);
        a * p.bary[0] + b * p.bary[1] + c * p.bary[2]
    }

    /// Unit normal of a surface point: the interpolated vertex normals.
    pub fn normal(&self, p: &SurfacePoint) -> Vec3 {
        let Some(t) = self.tris.get(p.tri as usize) else { return Vec3::Y };
        let [a, b, c] = corners(&self.normals, *t);
        (a * p.bary[0] + b * p.bary[1] + c * p.bary[2]).try_normalize().unwrap_or(Vec3::Y)
    }

    /// Position and normal in one lookup.
    pub fn eval(&self, p: &SurfacePoint) -> (Vec3, Vec3) {
        (self.position(p), self.normal(p))
    }

    /// Is `p` a valid point here (triangle exists, barycentrics finite, non-negative, sum 1)?
    pub fn contains(&self, p: &SurfacePoint) -> bool {
        (p.tri as usize) < self.tris.len() && p.bary.iter().all(|b| b.is_finite() && *b >= -1e-4) && (p.bary.iter().sum::<f32>() - 1.0).abs() < 1e-3
    }

    /// The point of the surface closest to `q` among the triangles in `candidates`, or among all
    /// triangles when `None`. Linear time; used by tools on small candidate sets and by tests.
    pub fn closest_point(&self, q: Vec3, candidates: Option<&[u32]>) -> Option<SurfacePoint> {
        let mut best: Option<(f32, SurfacePoint)> = None;
        let mut consider = |i: usize| {
            let Some(t) = self.tris.get(i) else { return };
            let [a, b, c] = corners(&self.positions, *t);
            let bary = closest_barycentric(q, a, b, c);
            let p = SurfacePoint { tri: i as u32, bary };
            let d = (self.position(&p) - q).length_squared();
            if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, p));
            }
        };
        match candidates {
            Some(c) => c.iter().for_each(|&i| consider(i as usize)),
            None => (0..self.tris.len()).for_each(consider),
        }
        best.map(|(_, p)| p)
    }
}

#[inline]
fn corners<T: Copy + Default>(data: &[T], t: [u32; 3]) -> [T; 3] {
    let g = |i: u32| data.get(i as usize).copied().unwrap_or_default();
    [g(t[0]), g(t[1]), g(t[2])]
}

/// Barycentric coordinates of the point of triangle `abc` nearest to `p` (Ericson, *Real-Time
/// Collision Detection*, closest point on a triangle).
pub(crate) fn closest_barycentric(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> [f32; 3] {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return [1.0, 0.0, 0.0];
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return [0.0, 1.0, 0.0];
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return [1.0 - v, v, 0.0];
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return [0.0, 0.0, 1.0];
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return [1.0 - w, 0.0, w];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [0.0, 1.0 - w, w];
    }
    let sum = va + vb + vc;
    if sum.abs() < f32::MIN_POSITIVE {
        return [1.0, 0.0, 0.0];
    }
    let v = vb / sum;
    let w = vc / sum;
    [1.0 - v - w, v, w]
}

/// Per-vertex scalar maps in `[0, 1]` that shape growth and interpolation.
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceMaps<'a> {
    /// Hair density: 0 grows nothing, 1 the full amount. Missing means 1 everywhere.
    pub density: Option<&'a [f32]>,
    /// Multiplier on a strand's length. Missing means 1 everywhere.
    pub length: Option<&'a [f32]>,
}

impl SurfaceMaps<'_> {
    pub(crate) fn check(&self, surface: &Surface) -> Result<(), HairError> {
        for (what, m) in [("density map", self.density), ("length map", self.length)] {
            if let Some(m) = m {
                if m.len() != surface.vertex_count() {
                    return Err(HairError::LengthMismatch { what, expected: surface.vertex_count(), found: m.len() });
                }
            }
        }
        Ok(())
    }
}

/// Value of a per-vertex map at a surface point (barycentric interpolation, non-finite is 0).
pub(crate) fn map_at(surface: &Surface, map: &[f32], p: &SurfacePoint) -> f32 {
    let Some(t) = surface.tris.get(p.tri as usize) else { return 0.0 };
    let [a, b, c] = corners(map, *t);
    let v = a * p.bary[0] + b * p.bary[1] + c * p.bary[2];
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// Picks surface points: triangles in proportion to `area × mean density`, then a uniform point in
/// the triangle thinned by the density at the point. Point `i` of a `seed` depends only on those two.
pub(crate) struct Sampler<'a> {
    surface: &'a Surface,
    density: Option<&'a [f32]>,
    cumulative: Vec<f64>,
    total: f64,
}

const MAX_ATTEMPTS: u64 = 16;

impl<'a> Sampler<'a> {
    pub(crate) fn new(surface: &'a Surface, density: Option<&'a [f32]>) -> Sampler<'a> {
        let mut cumulative = Vec::with_capacity(surface.tris.len());
        let mut total = 0.0f64;
        for (i, t) in surface.tris.iter().enumerate() {
            let w = match density {
                None => 1.0,
                Some(d) => {
                    let [a, b, c] = corners(d, *t);
                    f64::from(clean(a) + clean(b) + clean(c)) / 3.0
                }
            };
            total += f64::from(surface.triangle_area(i)) * w;
            cumulative.push(total);
        }
        Sampler { surface, density, cumulative, total }
    }

    /// `Σ area × mean density`: the area that counts for a hairs-per-area amount.
    pub(crate) fn weighted_area(&self) -> f64 {
        self.total
    }

    pub(crate) fn sample(&self, seed: u64, index: u64) -> SurfacePoint {
        let mut last = SurfacePoint { tri: 0, bary: [1.0, 0.0, 0.0] };
        for attempt in 0..MAX_ATTEMPTS {
            let k = attempt << 8;
            let x = f64::from(rng::rand(seed, index, stream::TRI + k)) * self.total;
            let tri = self.cumulative.partition_point(|c| *c <= x).min(self.cumulative.len().saturating_sub(1));
            let (u1, u2) = (rng::rand(seed, index, stream::BARY_A + k), rng::rand(seed, index, stream::BARY_B + k));
            let r = u1.sqrt();
            let bary = [1.0 - r, r * (1.0 - u2), r * u2];
            last = SurfacePoint { tri: tri as u32, bary };
            let Some(d) = self.density else { return last };
            let Some(t) = self.surface.tris.get(tri) else { return last };
            let [da, db, dc] = corners(d, *t);
            let dmax = clean(da).max(clean(db)).max(clean(dc));
            if dmax <= 0.0 {
                continue;
            }
            if rng::rand(seed, index, stream::ACCEPT + k) * dmax <= map_at(self.surface, d, &last) {
                return last;
            }
        }
        last
    }
}

#[inline]
fn clean(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}
