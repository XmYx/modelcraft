//! UV transfer: give a mesh the UVs of another by looking up, for each corner, the closest point
//! on the other mesh's surface and reading its UV there.
//!
//! The lookup is a uniform grid over the source triangles with an expanding shell search, and the
//! closest point on a triangle is found by the classical region test on its barycentric
//! coordinates (the Voronoi regions of the vertices, edges and face). Between islands the source's
//! UVs jump, so with `per_island` every target face first picks the source island nearest its
//! centre and all its corners look up points on that island only: a face never straddles a
//! source seam, and the target gets seams where the source has them.

use glam::{DVec3, Vec2};
use modelcraft_mesh::PolyMesh;

use crate::charts::{Charts, face_triangles};
use crate::seams::SeamSet;
use crate::{MAX_FACES, UvError};

/// Largest source mesh a transfer accepts, in faces.
pub const MAX_TRANSFER_FACES: usize = MAX_FACES;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransferOptions {
    /// Corners whose closest point is further than this keep their own UV (or get zero when the
    /// target has none). `None` means no limit.
    pub max_distance: Option<f32>,
    /// Look up each target face on a single source island (see the module docs).
    pub per_island: bool,
}

impl Default for TransferOptions {
    fn default() -> Self {
        TransferOptions { max_distance: None, per_island: true }
    }
}

/// What [`transfer_uvs`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TransferReport {
    pub corners: usize,
    /// Corners that took a UV from the source.
    pub mapped: usize,
    /// Corners further than `max_distance` from the source.
    pub too_far: usize,
    pub mean_distance: f32,
    pub max_distance: f32,
}

struct Tri {
    p: [DVec3; 3],
    uv: [Vec2; 3],
    island: u32,
}

/// Closest point to `p` on triangle `a b c`: returns it with its barycentric coordinates.
fn closest_on_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> (DVec3, [f64; 3]) {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, [1.0, 0.0, 0.0]);
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return (b, [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a + ab * v, [1.0 - v, v, 0.0]);
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return (c, [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a + ac * w, [1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * w, [0.0, 1.0 - w, w]);
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    (a + ab * v + ac * w, [1.0 - v - w, v, w])
}

/// The UV at `p` of the affine map of triangle `t` (barycentric coordinates of `p`'s projection on
/// the triangle's plane, not limited to the triangle).
fn affine_uv(t: &Tri, p: DVec3) -> Vec2 {
    let (v0, v1, v2) = (t.p[1] - t.p[0], t.p[2] - t.p[0], p - t.p[0]);
    let (d00, d01, d11, d20, d21) = (v0.dot(v0), v0.dot(v1), v1.dot(v1), v2.dot(v0), v2.dot(v1));
    let denom = d00 * d11 - d01 * d01;
    if denom.abs() < 1e-30 {
        return t.uv[0];
    }
    let v = (d11 * d20 - d01 * d21) / denom;
    let w = (d00 * d21 - d01 * d20) / denom;
    let uv = t.uv[0] * (1.0 - v - w) as f32 + t.uv[1] * v as f32 + t.uv[2] * w as f32;
    if uv.is_finite() { uv } else { t.uv[0] }
}

struct Grid {
    lo: DVec3,
    cell: f64,
    res: [usize; 3],
    start: Vec<u32>,
    items: Vec<u32>,
    /// Triangles too big for the grid: checked by every query.
    big: Vec<u32>,
}

/// Cells per triangle above which it goes to the `big` list.
const MAX_CELLS_PER_TRIANGLE: usize = 512;

impl Grid {
    fn build(tris: &[Tri]) -> Option<Grid> {
        let (mut lo, mut hi) = (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN));
        for t in tris {
            for p in t.p {
                lo = lo.min(p);
                hi = hi.max(p);
            }
        }
        let size = (hi - lo).max(DVec3::splat(1e-9));
        // A surface fills a thin shell of the grid, so aim for about two triangles per occupied cell.
        let per_axis = ((tris.len() as f64 / 2.0).sqrt().ceil() as usize).clamp(1, 160);
        let cell = (size.max_element() / per_axis as f64).max(1e-12);
        let res = [0, 1, 2].map(|k| ((size[k] / cell).ceil() as usize).clamp(1, 160));
        let cells = res[0].checked_mul(res[1])?.checked_mul(res[2])?;
        let idx = |p: DVec3| [0, 1, 2].map(|k| (((p[k] - lo[k]) / cell).floor().max(0.0) as usize).min(res[k] - 1));
        let mut counts = vec![0u32; cells + 1];
        let mut big = Vec::new();
        let mut spans: Vec<Option<([usize; 3], [usize; 3])>> = Vec::with_capacity(tris.len());
        for (i, t) in tris.iter().enumerate() {
            let (tl, th) = (t.p[0].min(t.p[1]).min(t.p[2]), t.p[0].max(t.p[1]).max(t.p[2]));
            let (a, b) = (idx(tl), idx(th));
            let n = (b[0] - a[0] + 1) * (b[1] - a[1] + 1) * (b[2] - a[2] + 1);
            if n > MAX_CELLS_PER_TRIANGLE {
                big.push(i as u32);
                spans.push(None);
                continue;
            }
            spans.push(Some((a, b)));
            for z in a[2]..=b[2] {
                for y in a[1]..=b[1] {
                    for x in a[0]..=b[0] {
                        counts[(z * res[1] + y) * res[0] + x + 1] += 1;
                    }
                }
            }
        }
        for c in 0..cells {
            counts[c + 1] += counts[c];
        }
        let mut items = vec![0u32; counts[cells] as usize];
        let mut next = counts.clone();
        for (i, s) in spans.iter().enumerate() {
            let Some((a, b)) = s else { continue };
            for z in a[2]..=b[2] {
                for y in a[1]..=b[1] {
                    for x in a[0]..=b[0] {
                        let c = (z * res[1] + y) * res[0] + x;
                        items[next[c] as usize] = i as u32;
                        next[c] += 1;
                    }
                }
            }
        }
        Some(Grid { lo, cell, res, start: counts, items, big })
    }

    /// The closest point to `p` among the triangles `accept` allows: `(distance, triangle, barycentric)`.
    ///
    /// Candidates whose distances differ by no more than `tie` count as equally close; among them
    /// the one whose UV is nearest `prefer` wins (a point on a seam has one candidate per side).
    fn closest(&self, tris: &[Tri], p: DVec3, limit: f64, accept: &dyn Fn(&Tri) -> bool, prefer: Option<Vec2>, tie: f64) -> Option<(f64, usize, [f64; 3])> {
        let idx = |q: DVec3| [0, 1, 2].map(|k| (((q[k] - self.lo[k]) / self.cell).floor().max(0.0) as usize).min(self.res[k] - 1));
        let c0 = idx(p);
        // How far p lies outside the grid: ring bounds are measured from the clamped point.
        let inside = DVec3::new(
            p.x.clamp(self.lo.x, self.lo.x + self.res[0] as f64 * self.cell),
            p.y.clamp(self.lo.y, self.lo.y + self.res[1] as f64 * self.cell),
            p.z.clamp(self.lo.z, self.lo.z + self.res[2] as f64 * self.cell),
        );
        let out = p.distance(inside);
        // (distance, triangle, barycentric, distance of its UV from the preferred one)
        let mut best: Option<(f64, usize, [f64; 3], f32)> = None;
        let consider = |i: usize, best: &mut Option<(f64, usize, [f64; 3], f32)>| {
            let Some(t) = tris.get(i) else { return };
            if !accept(t) {
                return;
            }
            let (q, bary) = closest_on_triangle(p, t.p[0], t.p[1], t.p[2]);
            let d = p.distance(q);
            if !d.is_finite() {
                return;
            }
            let off = prefer.map_or(0.0, |u| (t.uv[0] * bary[0] as f32 + t.uv[1] * bary[1] as f32 + t.uv[2] * bary[2] as f32).distance(u));
            if best.as_ref().is_none_or(|b| d < b.0 - tie || (d <= b.0 + tie && off < b.3)) {
                *best = Some((d, i, bary, off));
            }
        };
        for &i in &self.big {
            consider(i as usize, &mut best);
        }
        let max_ring = self.res[0].max(self.res[1]).max(self.res[2]);
        for k in 0..=max_ring {
            let lo = [0, 1, 2].map(|a| c0[a].saturating_sub(k));
            let hi = [0, 1, 2].map(|a| (c0[a] + k).min(self.res[a] - 1));
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        // Only the shell at Chebyshev distance k from the start cell.
                        let on_shell = x.abs_diff(c0[0]) == k || y.abs_diff(c0[1]) == k || z.abs_diff(c0[2]) == k;
                        if !on_shell {
                            continue;
                        }
                        let c = (z * self.res[1] + y) * self.res[0] + x;
                        for &i in &self.items[self.start[c] as usize..self.start[c + 1] as usize] {
                            consider(i as usize, &mut best);
                        }
                    }
                }
            }
            // Everything in shell k + 1 and beyond is at least k cells away (less the distance p lies outside).
            let reach = k as f64 * self.cell - out;
            if best.as_ref().is_some_and(|b| b.0 <= reach) || reach > limit {
                break;
            }
        }
        best.filter(|b| b.0 <= limit).map(|b| (b.0, b.1, b.2))
    }
}

/// UVs for every corner of `target` (in corner order) taken from `source`, which must have UVs;
/// both meshes must be in the same space.
pub fn transfer_uvs(source: &PolyMesh, target: &PolyMesh, opts: &TransferOptions) -> Result<(Vec<Vec2>, TransferReport), UvError> {
    if !source.has_uvs() {
        return Err(UvError::NoUvs);
    }
    if source.face_count() == 0 || target.face_count() == 0 {
        return Err(UvError::NoFaces);
    }
    if source.face_count() > MAX_TRANSFER_FACES || target.face_count() > MAX_TRANSFER_FACES {
        return Err(UvError::TooLarge(source.face_count().max(target.face_count())));
    }
    if opts.max_distance.is_some_and(|d| d.is_nan() || d < 0.0) {
        return Err(UvError::Invalid("the distance limit is not a number".into()));
    }
    let charts = Charts::build(source, &SeamSet::from_uv_discontinuities(source))?;
    let mut tris: Vec<Tri> = Vec::with_capacity(source.face_count() * 2);
    let mut buf = Vec::new();
    for f in 0..source.face_count() as u32 {
        buf.clear();
        face_triangles(source, f, &mut buf);
        let island = charts.face_island.get(f as usize).copied().unwrap_or(0);
        for t in &buf {
            let p = t.map(|c| source.position(source.origin(c)).as_dvec3());
            if p.iter().all(|q| q.is_finite()) {
                tris.push(Tri { p, uv: t.map(|c| source.uv(c)), island });
            }
        }
    }
    let grid = Grid::build(&tris).ok_or_else(|| UvError::Invalid("the source mesh is too large to index".into()))?;
    let limit = opts.max_distance.map_or(f64::INFINITY, f64::from);
    let mut out: Vec<Vec2> = if target.has_uvs() { target.corner_uvs().to_vec() } else { vec![Vec2::ZERO; target.corner_count()] };
    let mut report = TransferReport { corners: target.corner_count(), ..TransferReport::default() };
    let mut sum = 0.0f64;
    let any = |_: &Tri| true;
    let tie = 1e-6 * (grid.cell * grid.res[0].max(grid.res[1]).max(grid.res[2]) as f64) + 1e-12;
    for f in 0..target.face_count() as u32 {
        let corners: Vec<u32> = target.face_half_edges(f).collect();
        // The face's anchor: the point of the source nearest its centre, which fixes the island and
        // the side of a seam its corners should stay on.
        let anchor = if opts.per_island && !corners.is_empty() {
            let centre = corners.iter().map(|c| target.position(target.origin(*c)).as_dvec3()).sum::<DVec3>() / corners.len() as f64;
            grid.closest(&tris, centre, f64::INFINITY, &any, None, tie).and_then(|(_, i, _)| tris.get(i))
        } else {
            None
        };
        let island = anchor.map(|t| t.island);
        for &c in &corners {
            let p = target.position(target.origin(c)).as_dvec3();
            if !p.is_finite() {
                report.too_far += 1;
                continue;
            }
            // Where the anchor triangle's own map would put this corner: the side of a seam it
            // should stay on.
            let prefer = anchor.map(|t| affine_uv(t, p));
            let found = match island {
                Some(isl) => grid.closest(&tris, p, limit, &|t: &Tri| t.island == isl, prefer, tie),
                None => grid.closest(&tris, p, limit, &any, None, tie),
            };
            match found.and_then(|(d, i, b)| Some((d, tris.get(i)?, b))) {
                Some((d, t, b)) => {
                    let uv = t.uv[0] * b[0] as f32 + t.uv[1] * b[1] as f32 + t.uv[2] * b[2] as f32;
                    if let Some(o) = out.get_mut(c as usize) {
                        *o = uv;
                    }
                    report.mapped += 1;
                    sum += d;
                    report.max_distance = report.max_distance.max(d as f32);
                }
                None => report.too_far += 1,
            }
        }
    }
    report.mean_distance = if report.mapped > 0 { (sum / report.mapped as f64) as f32 } else { 0.0 };
    Ok((out, report))
}
