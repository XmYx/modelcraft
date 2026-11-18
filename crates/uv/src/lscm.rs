//! Least-squares conformal maps (LSCM), after Lévy, Petitjean, Ray and Maillot, "Least Squares
//! Conformal Maps for Automatic Texture Atlas Generation", ACM SIGGRAPH 2002.
//!
//! Treating the flattening `(x, y) → (u, v)` as one complex function `f = u + i v`, a map is
//! conformal (it preserves angles) when `∂f/∂z̄ = 0`, the Cauchy–Riemann equations
//! `u_x = v_y` and `u_y = −v_x`. On a triangle with the local frame `(x_j, y_j)` and area `A`, the
//! gradient of the barycentric function of corner `j` is `∇φ_j = (y_{j+1} − y_{j+2}, x_{j+2} −
//! x_{j+1}) / 2A`, so the two residuals are
//!
//! ```text
//! r = Σ_j (a_j u_j − b_j v_j)        s = Σ_j (b_j u_j + a_j v_j)        (a_j, b_j) = ∇φ_j
//! ```
//!
//! and the energy `Σ_T A_T (r² + s²)` is a quadratic form in the `u` and `v` of the vertices. At
//! least two vertices are pinned to remove the similarity freedom (a map is conformal up to a
//! rotation, scale and shift); the rest is the minimiser of the quadratic form, found from its
//! normal equations by a sparse Cholesky factorisation (`cholesky.rs`), or by preconditioned
//! conjugate gradients (`sparse.rs`) when the factor would be too large. The map is orientation
//! preserving by construction, and exact (zero energy) for developable surfaces.

use glam::{DVec2, DVec3, Vec2};
use modelcraft_mesh::PolyMesh;

use crate::UnwrapMethod;
use crate::abf::{AbfProblem, AbfReport};
use crate::charts::{Charts, Island, face_triangles};
use crate::cholesky::{Factor, nested_dissection};
use crate::sparse::{BlockCsr, solve_cg};

/// Largest Cholesky factor (entries) before the solver falls back to conjugate gradients.
const MAX_FACTOR_NNZ: usize = 24_000_000;

/// A vertex pinned to a UV position, named by one of its corners (the UV vertex of that corner).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pin {
    pub corner: u32,
    pub uv: Vec2,
}

/// How one island was solved.
#[derive(Clone, Debug, PartialEq)]
pub struct IslandReport {
    pub faces: usize,
    pub disc: bool,
    /// Conjugate-gradient iterations (0 for a direct solve).
    pub iterations: usize,
    /// The system was solved by a sparse Cholesky factorisation, not conjugate gradients.
    pub direct: bool,
    /// Relative residual of the linear solve.
    pub residual: f64,
    pub converged: bool,
    /// The solve failed (a degenerate island) and the island was projected onto a plane instead.
    pub fallback: bool,
    /// The island has user pins and keeps its place and scale.
    pub pinned: bool,
    /// The angle-based solve, when one was asked for. If it did not converge the island was
    /// flattened by LSCM instead (see [`IslandReport::abf_fell_back`]).
    pub abf: Option<AbfReport>,
}

impl IslandReport {
    /// ABF was asked for but did not converge, so the island was flattened by LSCM.
    pub fn abf_fell_back(&self) -> bool {
        self.abf.is_some_and(|a| !a.converged)
    }
}

pub(crate) struct Solved {
    /// UV of each island vertex, parallel to `Island::vertices`.
    pub uv: Vec<DVec2>,
    pub report: IslandReport,
}

struct Tri {
    l: [usize; 3],
    q: [DVec2; 3],
    area: f64,
}

const NO: usize = usize::MAX;

/// Local 2D coordinates of a 3D triangle in its own orthonormal frame, with the area; `None` for
/// a degenerate triangle.
pub(crate) fn local_frame(p: [DVec3; 3]) -> Option<([DVec2; 3], f64)> {
    let (e1, e2) = (p[1] - p[0], p[2] - p[0]);
    let n = e1.cross(e2);
    let area = 0.5 * n.length();
    let scale = e1.length_squared() + e2.length_squared() + (p[2] - p[1]).length_squared();
    if !(area.is_finite() && area > 1e-12 * scale && scale > 0.0) {
        return None;
    }
    let x = e1.normalize_or_zero();
    let y = n.normalize_or_zero().cross(x);
    Some(([DVec2::ZERO, DVec2::new(e1.dot(x), 0.0), DVec2::new(e2.dot(x), e2.dot(y))], area))
}

/// The three angles of a triangle given by its local 2D corners.
pub(crate) fn corner_angles(q: &[DVec2; 3]) -> [f64; 3] {
    let at = |i: usize| {
        let (u, v) = (q[(i + 1) % 3] - q[i], q[(i + 2) % 3] - q[i]);
        u.perp_dot(v).abs().atan2(u.dot(v))
    };
    [at(0), at(1), at(2)]
}

/// A triangle with the given angles, whose first edge has the length of the first edge of `q`:
/// the corners in a local frame and the area. `None` for angles that do not make a triangle.
fn shape_from_angles(q: &[DVec2; 3], a: [f64; 3]) -> Option<([DVec2; 3], f64)> {
    let l01 = q[0].distance(q[1]);
    // Law of sines: |p0 p2| / sin(angle at 1) = |p0 p1| / sin(angle at 2).
    let l02 = l01 * a[1].sin() / a[2].sin();
    let q2 = DVec2::new(a[0].cos(), a[0].sin()) * l02;
    let area = 0.5 * l01 * q2.y;
    (l01 > 0.0 && l02.is_finite() && area.is_finite() && area > 0.0).then_some(([DVec2::ZERO, DVec2::new(l01, 0.0), q2], area))
}

fn planar(p: &[DVec3], tris: &[Tri]) -> Vec<DVec2> {
    // Area-weighted normal of the island; project onto the plane it defines.
    let mut n = DVec3::ZERO;
    for t in tris {
        let (a, b, c) = (p[t.l[0]], p[t.l[1]], p[t.l[2]]);
        n += (b - a).cross(c - a);
    }
    let n = if n.length_squared() > 0.0 { n.normalize() } else { DVec3::Y };
    let helper = if n.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
    let u = n.cross(helper).normalize_or_zero();
    let v = n.cross(u);
    p.iter().map(|q| DVec2::new(q.dot(u), q.dot(v))).collect()
}

/// Solve one island. `user_pins` are `(UV vertex, uv)` pairs; the island keeps them.
pub(crate) fn solve_island(mesh: &PolyMesh, charts: &Charts, island: &Island, user_pins: &[(u32, DVec2)], method: UnwrapMethod) -> Solved {
    let nloc = island.vertices.len();
    let local = |uvv: u32| island.vertices.binary_search(&uvv).unwrap_or(NO);
    let pos3: Vec<DVec3> = island.vertices.iter().map(|v| mesh.position(charts.vertex_source.get(*v as usize).copied().unwrap_or(0)).as_dvec3()).collect();

    let mut tris: Vec<Tri> = Vec::with_capacity(island.faces.len() * 2);
    // Vertices of a triangle that was skipped: their wheel of triangles is not closed.
    let mut gap = vec![false; nloc];
    let mut buf = Vec::new();
    for &f in &island.faces {
        buf.clear();
        face_triangles(mesh, f, &mut buf);
        for t in &buf {
            let l = [0, 1, 2].map(|k| local(charts.corner_vertex.get(t[k] as usize).copied().unwrap_or(0)));
            let valid = !(l.contains(&NO) || l[0] == l[1] || l[1] == l[2] || l[0] == l[2]);
            let frame = if valid { local_frame([pos3[l[0]], pos3[l[1]], pos3[l[2]]]) } else { None };
            if let Some((q, area)) = frame {
                tris.push(Tri { l, q, area });
            } else {
                for v in l {
                    if let Some(g) = gap.get_mut(v) {
                        *g = true;
                    }
                }
            }
        }
    }
    let mut report = IslandReport {
        faces: island.faces.len(),
        disc: island.is_disc(),
        iterations: 0,
        direct: false,
        residual: 0.0,
        converged: true,
        fallback: false,
        pinned: false,
        abf: None,
    };
    let fallback = |report: IslandReport, tris: &[Tri]| {
        let mut r = report;
        r.fallback = true;
        r.converged = false;
        Solved { uv: planar(&pos3, tris), report: r }
    };
    if tris.is_empty() || nloc < 3 {
        return fallback(report, &tris);
    }

    // Boundary vertices (those on a cut edge) are the candidates for automatic pins.
    let mut on_boundary = vec![false; nloc];
    for &f in &island.faces {
        for h in mesh.face_half_edges(f) {
            if charts.edge_cut.get(mesh.edge_of(h) as usize).copied().unwrap_or(false) {
                for c in [h, mesh.next(h)] {
                    let l = local(charts.corner_vertex.get(c as usize).copied().unwrap_or(0));
                    if let Some(b) = on_boundary.get_mut(l) {
                        *b = true;
                    }
                }
            }
        }
    }
    let used: Vec<bool> = {
        let mut u = vec![false; nloc];
        for t in &tris {
            for l in t.l {
                u[l] = true;
            }
        }
        u
    };
    if method == UnwrapMethod::Abf {
        let interior: Vec<bool> = (0..nloc).map(|l| used[l] && !on_boundary[l] && !gap[l]).collect();
        let (angles, abf_report) = crate::abf::solve(&AbfProblem {
            tris: &tris.iter().map(|t| t.l).collect::<Vec<_>>(),
            angles: &tris.iter().map(|t| corner_angles(&t.q)).collect::<Vec<_>>(),
            interior: &interior,
            coords: &pos3.iter().map(|p| p.to_array()).collect::<Vec<_>>(),
        });
        report.abf = Some(abf_report);
        if let (Some(angles), true) = (angles, abf_report.converged) {
            for (t, a) in tris.iter_mut().zip(&angles) {
                if let Some((q, area)) = shape_from_angles(&t.q, *a) {
                    t.q = q;
                    t.area = area;
                }
            }
        }
    }
    let any_boundary = on_boundary.iter().zip(&used).any(|(b, u)| *b && *u);
    let candidate = |l: usize| used[l] && (!any_boundary || on_boundary[l]);

    let mut pinned: Vec<Option<DVec2>> = vec![None; nloc];
    for (v, uv) in user_pins {
        let l = local(*v);
        if l != NO && used[l] {
            pinned[l] = Some(*uv);
        }
    }
    let user: Vec<usize> = (0..nloc).filter(|l| pinned[*l].is_some()).collect();
    // Two user pins at the same UV would let the map collapse; keep the first only.
    if user.len() >= 2 && user.iter().all(|l| pinned[*l].is_some_and(|p| p.distance(pinned[user[0]].unwrap_or(DVec2::ZERO)) < 1e-9)) {
        for l in user.iter().skip(1) {
            pinned[*l] = None;
        }
    }
    let user: Vec<usize> = (0..nloc).filter(|l| pinned[*l].is_some()).collect();
    report.pinned = !user.is_empty();
    if user.len() < 2 {
        // Add automatic pins on the boundary: `a` far from the centre (or the user pin), `b` far from `a`.
        let a = match user.first() {
            Some(l) => *l,
            None => {
                let (mut sum, mut cnt) = (DVec3::ZERO, 0.0f64);
                for l in 0..nloc {
                    if used[l] {
                        sum += pos3[l];
                        cnt += 1.0;
                    }
                }
                let c = sum / cnt.max(1.0);
                let far = (0..nloc).filter(|l| candidate(*l)).max_by(|x, y| pos3[*x].distance_squared(c).total_cmp(&pos3[*y].distance_squared(c)));
                let Some(far) = far else { return fallback(report, &tris) };
                pinned[far] = Some(DVec2::ZERO);
                far
            }
        };
        let far =
            (0..nloc).filter(|l| candidate(*l) && *l != a).max_by(|x, y| pos3[*x].distance_squared(pos3[a]).total_cmp(&pos3[*y].distance_squared(pos3[a])));
        let Some(b) = far else { return fallback(report, &tris) };
        let d = pos3[a].distance(pos3[b]);
        if !(d.is_finite() && d > 1e-12) {
            return fallback(report, &tris);
        }
        pinned[b] = Some(pinned[a].unwrap_or(DVec2::ZERO) + DVec2::new(d, 0.0));
    }

    // Unknowns: the free (unpinned) vertices that some triangle uses.
    let mut free_of = vec![NO; nloc];
    let mut nfree = 0;
    for l in 0..nloc {
        if used[l] && pinned[l].is_none() {
            free_of[l] = nfree;
            nfree += 1;
        }
    }
    let mut start: Vec<DVec2> = planar(&pos3, &tris);
    for (l, p) in pinned.iter().enumerate() {
        if let Some(p) = p {
            start[l] = *p;
        }
    }
    if nfree == 0 {
        return Solved { uv: start, report };
    }
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); nfree];
    for t in &tris {
        for i in t.l {
            for j in t.l {
                if free_of[i] != NO && free_of[j] != NO {
                    rows[free_of[i]].push(free_of[j] as u32);
                }
            }
        }
    }
    for r in &mut rows {
        r.sort_unstable();
        r.dedup();
    }
    let Some(mut a) = BlockCsr::from_pattern(&rows) else { return fallback(report, &tris) };
    let mut rhs = vec![0.0; 2 * nfree];
    for t in &tris {
        let w = t.area.sqrt();
        let two_a = 2.0 * t.area;
        // Coefficients of u_j and v_j in the two residual rows.
        let mut c = [[[0.0f64; 2]; 3]; 2];
        for j in 0..3 {
            let (j1, j2) = ((j + 1) % 3, (j + 2) % 3);
            let (aj, bj) = ((t.q[j1].y - t.q[j2].y) / two_a, (t.q[j2].x - t.q[j1].x) / two_a);
            c[0][j] = [w * aj, -w * bj];
            c[1][j] = [w * bj, w * aj];
        }
        for (ti, i) in t.l.iter().enumerate() {
            let fi = free_of[*i];
            if fi == NO {
                continue;
            }
            for (tj, j) in t.l.iter().enumerate() {
                let fj = free_of[*j];
                if let Some(p) = pinned[*j] {
                    for row in &c {
                        let k = row[tj][0] * p.x + row[tj][1] * p.y;
                        rhs[2 * fi] -= row[ti][0] * k;
                        rhs[2 * fi + 1] -= row[ti][1] * k;
                    }
                } else if fj != NO {
                    let mut blk = [0.0; 4];
                    for row in &c {
                        blk[0] += row[ti][0] * row[tj][0];
                        blk[1] += row[ti][0] * row[tj][1];
                        blk[2] += row[ti][1] * row[tj][0];
                        blk[3] += row[ti][1] * row[tj][1];
                    }
                    a.add(fi, fj as u32, blk);
                }
            }
        }
    }
    let mut x = vec![0.0; 2 * nfree];
    for l in 0..nloc {
        if free_of[l] != NO {
            x[2 * free_of[l]] = start[l].x;
            x[2 * free_of[l] + 1] = start[l].y;
        }
    }
    let mut coords = vec![[0.0f64; 3]; nfree];
    for l in 0..nloc {
        if free_of[l] != NO {
            coords[free_of[l]] = pos3[l].to_array();
        }
    }
    let order = nested_dissection(&rows, &coords);
    drop(rows);
    let factor = Factor::new(&a, &order, MAX_FACTOR_NNZ, 0.0).or_else(|| Factor::new(&a, &order, MAX_FACTOR_NNZ, 1e-10));
    let cg = if let Some(f) = factor {
        // Direct solve, then two steps of iterative refinement against rounding in the factor.
        f.solve(&rhs, &mut x);
        let (mut r, mut dx) = (vec![0.0; 2 * nfree], vec![0.0; 2 * nfree]);
        for _ in 0..2 {
            a.mul(&x, &mut r);
            for (ri, bi) in r.iter_mut().zip(&rhs) {
                *ri = bi - *ri;
            }
            f.solve(&r, &mut dx);
            for (xi, di) in x.iter_mut().zip(&dx) {
                *xi += di;
            }
        }
        a.mul(&x, &mut r);
        let bn = rhs.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
        let res = r.iter().zip(&rhs).map(|(ri, bi)| (bi - ri).powi(2)).sum::<f64>().sqrt() / bn;
        report.direct = true;
        crate::sparse::CgReport { iterations: 0, residual: if x.iter().all(|v| v.is_finite()) { res } else { f64::INFINITY }, converged: res < 1e-8 }
    } else {
        solve_cg(&a, &rhs, &mut x, 1e-10, 8000)
    };
    report.iterations = cg.iterations;
    report.residual = cg.residual;
    report.converged = cg.converged;
    if !cg.residual.is_finite() {
        return fallback(report, &tris);
    }
    let mut uv = start;
    for l in 0..nloc {
        if free_of[l] != NO {
            uv[l] = DVec2::new(x[2 * free_of[l]], x[2 * free_of[l] + 1]);
        }
    }
    Solved { uv, report }
}
