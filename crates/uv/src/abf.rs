//! Angle-based flattening (ABF), after Sheffer and de Sturler, "Parameterization of Faceted
//! Surfaces for Meshing Using Angle-Based Flattening", Engineering with Computers 17 (2001), with
//! the reduced Newton system of Sheffer, Lévy, Mogilnitsky and Bogomyakov, "ABF++: Fast and Robust
//! Angle Based Flattening", ACM Transactions on Graphics 24 (2005).
//!
//! Instead of solving for positions, ABF solves for the three angles `α` of every triangle of the
//! flat result. A set of angles describes a valid planar triangulation exactly when
//!
//! 1. the angles of each triangle add up to `π`,
//! 2. the angles around each interior vertex add up to `2π`, and
//! 3. around each interior vertex the product of the sines of the angles on one side of its
//!    triangles equals the product of the sines opposite (the law of sines, applied around the
//!    wheel, says the edge lengths come back to where they started). Here that is
//!    `Σ ln sin α_next − Σ ln sin α_prev = 0`.
//!
//! Among the angle sets that satisfy these, ABF picks the one closest to the angles `β` of the
//! surface, `E = Σ (α − β)² / β²`, where the `β` around each interior vertex are first scaled to
//! add up to `2π` (a vertex on a curved surface has less or more than a full turn). It is a
//! quadratic objective with constraints, so the Lagrange conditions are solved by Newton's
//! method. Their matrix has a diagonal block for the angles (each angle appears in separate
//! terms), so — the observation of ABF++ — the angles and the triangle multipliers can be
//! eliminated, leaving a sparse symmetric positive-definite system of two unknowns per interior
//! vertex (the multipliers of constraints 2 and 3), which is the same shape as the LSCM system and
//! is solved with the same sparse Cholesky factorisation.
//!
//! The angles are turned into positions by a least-squares fit of triangles of the right shape,
//! the same energy as LSCM (see `lscm.rs`), which is exact when the angles are consistent.

use crate::cholesky::{Factor, nested_dissection};
use crate::sparse::{BlockCsr, solve_cg};

const NO: usize = usize::MAX;
/// Newton iterations before giving up.
const MAX_ITER: usize = 40;
/// Largest constraint violation (radians, or log of a ratio) accepted as solved.
const TOL: f64 = 1e-8;
/// Angles stay at least this far from 0 and π so the sine constraints stay finite.
const EPS_ANGLE: f64 = 1e-6;
/// Smallest angle of the target set, so one sliver cannot dominate the weights.
const MIN_TARGET: f64 = 1e-3;
const MAX_FACTOR_NNZ: usize = 24_000_000;

/// How an angle-based solve went.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AbfReport {
    /// Newton iterations taken.
    pub iterations: usize,
    /// The constraints hold to within the tolerance, so the angles are a valid flattening.
    pub converged: bool,
    /// Largest constraint violation left.
    pub residual: f64,
    /// `Σ (α − β)² / β²`, the quantity ABF minimises.
    pub energy: f64,
    /// Interior vertices (those with the sum and sine constraints).
    pub interior_vertices: usize,
    /// The reduced systems were solved by direct factorisation, not conjugate gradients.
    pub direct: bool,
}

/// An island's triangles, as ABF sees them.
pub(crate) struct AbfProblem<'a> {
    /// Triangles as island-local vertex indices, all wound the same way.
    pub tris: &'a [[usize; 3]],
    /// Angles of each triangle on the surface, per corner.
    pub angles: &'a [[f64; 3]],
    /// Which local vertices have a closed wheel of triangles.
    pub interior: &'a [bool],
    /// Positions of the local vertices, for the fill-reducing ordering.
    pub coords: &'a [[f64; 3]],
}

/// One entry of the constraint Jacobian for an angle: the row it is in (`2 k` for the sum
/// constraint of interior vertex `k`, `2 k + 1` for its sine constraint), its coefficient and the
/// sign of its second derivative.
#[derive(Clone, Copy)]
struct Entry {
    row: usize,
    coeff: f64,
    hess: f64,
}

struct Layout<'a> {
    tris: &'a [[usize; 3]],
    vk: Vec<usize>,
}

impl Layout<'_> {
    /// The vertex constraints angle `m` of triangle `t` takes part in, at the angle `a`.
    ///
    /// Corner `m` is at vertex `m`. It is the "next" angle of the vertex before it (`m + 2`) and
    /// the "previous" angle of the vertex after it (`m + 1`), going round the triangle.
    fn entries(&self, t: usize, m: usize, a: f64) -> [Option<Entry>; 3] {
        let tri = self.tris.get(t).copied().unwrap_or([0; 3]);
        let k_of = |pos: usize| self.vk.get(tri[pos % 3]).copied().unwrap_or(NO);
        let (s, c) = a.sin_cos();
        let cot = c / s;
        let csc2 = 1.0 / (s * s);
        let own = k_of(m);
        let next = k_of(m + 2);
        let prev = k_of(m + 1);
        [
            (own != NO).then(|| Entry { row: 2 * own, coeff: 1.0, hess: 0.0 }),
            (next != NO).then(|| Entry { row: 2 * next + 1, coeff: cot, hess: -csc2 }),
            (prev != NO).then(|| Entry { row: 2 * prev + 1, coeff: -cot, hess: csc2 }),
        ]
    }
}

/// Solve for the angles. `None` when the solve failed outright (non-finite numbers, a failed
/// factorisation); the report says why when it did not converge.
pub(crate) fn solve(p: &AbfProblem) -> (Option<Vec<[f64; 3]>>, AbfReport) {
    let nt = p.tris.len();
    let nv = p.interior.len();
    let mut report = AbfReport::default();
    let mut vk = vec![NO; nv];
    let mut ni = 0usize;
    let mut used = vec![false; nv];
    for t in p.tris {
        for &v in t {
            if let Some(u) = used.get_mut(v) {
                *u = true;
            }
        }
    }
    for v in 0..nv {
        if p.interior.get(v).copied().unwrap_or(false) && used.get(v).copied().unwrap_or(false) {
            vk[v] = ni;
            ni += 1;
        }
    }
    report.interior_vertices = ni;
    if nt == 0 || p.angles.len() != nt {
        return (None, report);
    }
    let layout = Layout { tris: p.tris, vk };

    // Target angles: the surface's, scaled to a full turn around interior vertices.
    let mut beta: Vec<f64> = p.angles.iter().flatten().copied().collect();
    let mut total = vec![0.0f64; nv];
    for (t, tri) in p.tris.iter().enumerate() {
        for m in 0..3 {
            if let (Some(s), Some(b)) = (total.get_mut(tri[m]), beta.get(3 * t + m)) {
                *s += *b;
            }
        }
    }
    for (t, tri) in p.tris.iter().enumerate() {
        for m in 0..3 {
            let v = tri[m];
            let scale = if layout.vk.get(v).copied().unwrap_or(NO) != NO && total.get(v).copied().unwrap_or(0.0) > 0.0 {
                2.0 * std::f64::consts::PI / total[v]
            } else {
                1.0
            };
            if let Some(b) = beta.get_mut(3 * t + m) {
                *b = (*b * scale).clamp(MIN_TARGET, std::f64::consts::PI - MIN_TARGET);
            }
        }
    }
    let two_w: Vec<f64> = beta.iter().map(|b| 2.0 / (b * b)).collect();
    let mut alpha = beta.clone();
    let mut lam_tri = vec![0.0f64; nt];
    let mut lam_v = vec![0.0f64; 2 * ni];

    // The sparsity of the reduced system: interior vertices that share a triangle.
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); ni];
    for tri in p.tris {
        for &a in tri {
            for &b in tri {
                let (ka, kb) = (layout.vk[a], layout.vk[b]);
                if ka != NO && kb != NO {
                    rows[ka].push(kb as u32);
                }
            }
        }
    }
    for r in &mut rows {
        r.sort_unstable();
        r.dedup();
    }
    let mut coords = vec![[0.0f64; 3]; ni];
    for v in 0..nv {
        if layout.vk[v] != NO {
            coords[layout.vk[v]] = p.coords.get(v).copied().unwrap_or([0.0; 3]);
        }
    }
    let order = if ni > 0 { nested_dissection(&rows, &coords) } else { Vec::new() };

    let pi = std::f64::consts::PI;
    let mut cons_v = vec![0.0f64; 2 * ni];
    let mut cons_t = vec![0.0f64; nt];
    let mut residual;
    let mut iteration = 0;
    loop {
        // Constraint values.
        cons_v.iter_mut().for_each(|v| *v = 0.0);
        for (t, c) in cons_t.iter_mut().enumerate() {
            *c = alpha[3 * t] + alpha[3 * t + 1] + alpha[3 * t + 2] - pi;
        }
        for c in cons_v.chunks_mut(2) {
            c[0] = -2.0 * pi;
        }
        for t in 0..nt {
            for m in 0..3 {
                let a = alpha[3 * t + m];
                let tri = p.tris[t];
                let k_own = layout.vk[tri[m]];
                if k_own != NO {
                    cons_v[2 * k_own] += a;
                }
                let ln = a.sin().ln();
                let k_next = layout.vk[tri[(m + 2) % 3]];
                if k_next != NO {
                    cons_v[2 * k_next + 1] += ln;
                }
                let k_prev = layout.vk[tri[(m + 1) % 3]];
                if k_prev != NO {
                    cons_v[2 * k_prev + 1] -= ln;
                }
            }
        }
        residual = cons_t.iter().chain(cons_v.iter()).fold(0.0f64, |m, c| m.max(c.abs()));
        if !residual.is_finite() {
            report.residual = f64::INFINITY;
            return (None, report);
        }
        if residual < TOL || iteration >= MAX_ITER {
            break;
        }
        iteration += 1;

        // Gradient of the Lagrangian and the diagonal of its Hessian.
        let mut g = vec![0.0f64; 3 * nt];
        let mut dinv = vec![0.0f64; 3 * nt];
        for t in 0..nt {
            for m in 0..3 {
                let i = 3 * t + m;
                let mut gi = two_w[i] * (alpha[i] - beta[i]) + lam_tri[t];
                let mut lam_ii = two_w[i];
                for e in layout.entries(t, m, alpha[i]).into_iter().flatten() {
                    gi += e.coeff * lam_v[e.row];
                    if e.row % 2 == 1 {
                        lam_ii += e.hess * lam_v[e.row];
                    }
                }
                g[i] = gi;
                // Far from the solution the Hessian can lose definiteness; keep it positive.
                dinv[i] = 1.0 / lam_ii.max(0.05 * two_w[i]);
            }
        }

        // Right-hand sides b = C − J D g, per triangle row and per vertex row; the per-triangle
        // couplings E to the vertex rows; and the reduced matrix S = M_vv − M_vt M_tt⁻¹ M_tv.
        let mut bt = vec![0.0f64; nt];
        let mut bv = cons_v.clone();
        let mut a_t = vec![0.0f64; nt];
        let mut e_t = vec![[0.0f64; 6]; nt];
        let Some(mut s) = BlockCsr::from_pattern(&rows) else { return (None, report) };
        for t in 0..nt {
            bt[t] = cons_t[t];
            for m in 0..3 {
                let i = 3 * t + m;
                bt[t] -= dinv[i] * g[i];
                a_t[t] += dinv[i];
                let tri = p.tris[t];
                for e in layout.entries(t, m, alpha[i]).into_iter().flatten() {
                    bv[e.row] -= e.coeff * dinv[i] * g[i];
                    // Which vertex of the triangle this row belongs to, for the coupling table.
                    let k = e.row / 2;
                    if let Some(pos) = (0..3).find(|q| layout.vk[tri[*q]] == k) {
                        e_t[t][2 * pos + e.row % 2] += dinv[i] * e.coeff;
                    }
                }
                // M_vv part: this angle couples the rows it appears in with each other.
                let ents = layout.entries(t, m, alpha[i]);
                for ep in ents.iter().flatten() {
                    for eq in ents.iter().flatten() {
                        let blk_idx = (ep.row % 2) * 2 + eq.row % 2;
                        let mut blk = [0.0; 4];
                        blk[blk_idx] = dinv[i] * ep.coeff * eq.coeff;
                        s.add(ep.row / 2, (eq.row / 2) as u32, blk);
                    }
                }
            }
            // Eliminate the triangle row: subtract E_p E_q / A.
            let tri = p.tris[t];
            for pp in 0..6 {
                for qq in 0..6 {
                    let (kp, kq) = (layout.vk[tri[pp / 2]], layout.vk[tri[qq / 2]]);
                    if kp == NO || kq == NO {
                        continue;
                    }
                    let mut blk = [0.0; 4];
                    blk[(pp % 2) * 2 + qq % 2] = -e_t[t][pp] * e_t[t][qq] / a_t[t];
                    s.add(kp, kq as u32, blk);
                }
            }
        }
        let mut rhs = bv;
        for t in 0..nt {
            let tri = p.tris[t];
            for pp in 0..6 {
                let kp = layout.vk[tri[pp / 2]];
                if kp != NO {
                    rhs[2 * kp + pp % 2] -= e_t[t][pp] * bt[t] / a_t[t];
                }
            }
        }

        // Solve for the vertex multipliers.
        let mut dlam_v = vec![0.0f64; 2 * ni];
        if ni > 0 {
            let factor = Factor::new(&s, &order, MAX_FACTOR_NNZ, 0.0)
                .or_else(|| Factor::new(&s, &order, MAX_FACTOR_NNZ, 1e-10))
                .or_else(|| Factor::new(&s, &order, MAX_FACTOR_NNZ, 1e-7));
            if let Some(f) = factor {
                f.solve(&rhs, &mut dlam_v);
                report.direct = true;
            } else {
                report.direct = false;
                let cg = solve_cg(&s, &rhs, &mut dlam_v, 1e-12, 6000);
                if !cg.residual.is_finite() {
                    return (None, report);
                }
            }
        } else {
            report.direct = true;
        }
        if dlam_v.iter().any(|v| !v.is_finite()) {
            return (None, report);
        }

        // Back-substitute for the triangle multipliers and the angles.
        let mut dlam_t = vec![0.0f64; nt];
        for t in 0..nt {
            let tri = p.tris[t];
            let mut v = bt[t];
            for pp in 0..6 {
                let kp = layout.vk[tri[pp / 2]];
                if kp != NO {
                    v -= e_t[t][pp] * dlam_v[2 * kp + pp % 2];
                }
            }
            dlam_t[t] = v / a_t[t];
        }
        let mut dalpha = vec![0.0f64; 3 * nt];
        let mut tau = 1.0f64;
        for t in 0..nt {
            for m in 0..3 {
                let i = 3 * t + m;
                let mut v = g[i] + dlam_t[t];
                for e in layout.entries(t, m, alpha[i]).into_iter().flatten() {
                    v += e.coeff * dlam_v[e.row];
                }
                let d = -dinv[i] * v;
                dalpha[i] = d;
                if d < 0.0 {
                    tau = tau.min(0.9 * (alpha[i] - EPS_ANGLE) / -d);
                } else if d > 0.0 {
                    tau = tau.min(0.9 * (pi - EPS_ANGLE - alpha[i]) / d);
                }
            }
        }
        if !tau.is_finite() || tau <= 0.0 || dalpha.iter().any(|v| !v.is_finite()) {
            return (None, report);
        }
        for (a, d) in alpha.iter_mut().zip(&dalpha) {
            *a = (*a + tau * d).clamp(EPS_ANGLE, pi - EPS_ANGLE);
        }
        for (l, d) in lam_tri.iter_mut().zip(&dlam_t) {
            *l += tau * d;
        }
        for (l, d) in lam_v.iter_mut().zip(&dlam_v) {
            *l += tau * d;
        }
    }

    report.iterations = iteration;
    report.residual = residual;
    report.converged = residual < TOL;
    report.energy = alpha.iter().zip(&beta).map(|(a, b)| ((a - b) / b).powi(2)).sum();
    let out: Vec<[f64; 3]> = alpha.chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
    (Some(out), report)
}
