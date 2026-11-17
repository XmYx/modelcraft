//! A symmetric positive-definite block-sparse matrix (2×2 blocks, one per pair of neighbouring
//! vertices) and a preconditioned conjugate-gradient solver, in pure Rust.
//!
//! The LSCM system has two unknowns per vertex and couples only neighbours, so it is stored as a
//! block CSR matrix: row `i` lists the columns `j` it touches (sorted, including `i` itself), and
//! each entry holds the 2×2 block `[[uu, uv], [vu, vv]]`. The solver is textbook conjugate
//! gradients (Hestenes and Stiefel 1952) with a block-Jacobi preconditioner. It is the fallback
//! when the direct solver (`cholesky.rs`) would need too much memory.

/// One 2×2 block, row-major.
pub(crate) type Block = [f64; 4];

pub(crate) struct BlockCsr {
    pub row_ptr: Vec<usize>,
    pub cols: Vec<u32>,
    pub vals: Vec<Block>,
}

impl BlockCsr {
    /// A matrix with the given sorted neighbour lists (each must contain its own row).
    pub fn from_pattern(rows: &[Vec<u32>]) -> Option<BlockCsr> {
        let total = rows.iter().try_fold(0usize, |a, r| a.checked_add(r.len()))?;
        let mut row_ptr = Vec::new();
        row_ptr.try_reserve_exact(rows.len() + 1).ok()?;
        let mut cols = Vec::new();
        cols.try_reserve_exact(total).ok()?;
        let mut vals: Vec<Block> = Vec::new();
        vals.try_reserve_exact(total).ok()?;
        row_ptr.push(0);
        for r in rows {
            cols.extend_from_slice(r);
            row_ptr.push(cols.len());
        }
        vals.resize(total, [0.0; 4]);
        Some(BlockCsr { row_ptr, cols, vals })
    }

    pub fn n(&self) -> usize {
        self.row_ptr.len().saturating_sub(1)
    }

    /// The entry index of block `(i, j)`.
    pub fn find(&self, i: usize, j: u32) -> Option<usize> {
        let (a, b) = (*self.row_ptr.get(i)?, *self.row_ptr.get(i + 1)?);
        let slice = self.cols.get(a..b)?;
        slice.binary_search(&j).ok().map(|k| a + k)
    }

    pub fn add(&mut self, i: usize, j: u32, b: Block) {
        if let Some(k) = self.find(i, j)
            && let Some(v) = self.vals.get_mut(k)
        {
            for (x, y) in v.iter_mut().zip(b) {
                *x += y;
            }
        }
    }

    /// `y = A x`, with `x` and `y` of length `2 n`.
    pub fn mul(&self, x: &[f64], y: &mut [f64]) {
        for i in 0..self.n() {
            let (a, b) = (self.row_ptr.get(i).copied().unwrap_or(0), self.row_ptr.get(i + 1).copied().unwrap_or(0));
            let (mut s0, mut s1) = (0.0, 0.0);
            for k in a..b {
                let (Some(&j), Some(m)) = (self.cols.get(k), self.vals.get(k)) else { continue };
                let j = j as usize;
                let (x0, x1) = (x.get(2 * j).copied().unwrap_or(0.0), x.get(2 * j + 1).copied().unwrap_or(0.0));
                s0 += m[0] * x0 + m[1] * x1;
                s1 += m[2] * x0 + m[3] * x1;
            }
            if let Some(o) = y.get_mut(2 * i) {
                *o = s0;
            }
            if let Some(o) = y.get_mut(2 * i + 1) {
                *o = s1;
            }
        }
    }

    /// The inverse of every diagonal block (a zero block where it is singular).
    pub fn inverse_diagonal(&self) -> Vec<Block> {
        (0..self.n())
            .map(|i| {
                let m = self.find(i, i as u32).and_then(|k| self.vals.get(k)).copied().unwrap_or([0.0; 4]);
                let det = m[0] * m[3] - m[1] * m[2];
                if det.abs() > 1e-300 && det.is_finite() { [m[3] / det, -m[1] / det, -m[2] / det, m[0] / det] } else { [0.0; 4] }
            })
            .collect()
    }
}

/// How a conjugate-gradient solve ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CgReport {
    pub iterations: usize,
    /// `‖b − A x‖ / ‖b‖` at the end (0 when `b` is zero).
    pub residual: f64,
    pub converged: bool,
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Solve `A x = b` from the starting guess in `x`. Stops at a relative residual of `tol` or after
/// `max_iter` iterations; `x` always holds the best iterate (finite values only).
pub(crate) fn solve_cg(a: &BlockCsr, b: &[f64], x: &mut [f64], tol: f64, max_iter: usize) -> CgReport {
    let n2 = b.len();
    let bnorm = dot(b, b).sqrt();
    if n2 == 0 || bnorm == 0.0 || x.len() != n2 {
        x.iter_mut().for_each(|v| *v = 0.0);
        return CgReport { iterations: 0, residual: 0.0, converged: true };
    }
    let pinv = a.inverse_diagonal();
    let precond = |r: &[f64], z: &mut [f64]| {
        for (i, p) in pinv.iter().enumerate() {
            let (r0, r1) = (r.get(2 * i).copied().unwrap_or(0.0), r.get(2 * i + 1).copied().unwrap_or(0.0));
            if let Some(o) = z.get_mut(2 * i) {
                *o = p[0] * r0 + p[1] * r1;
            }
            if let Some(o) = z.get_mut(2 * i + 1) {
                *o = p[2] * r0 + p[3] * r1;
            }
        }
    };
    let mut ax = vec![0.0; n2];
    a.mul(x, &mut ax);
    let mut r: Vec<f64> = b.iter().zip(&ax).map(|(bi, ai)| bi - ai).collect();
    let mut z = vec![0.0; n2];
    precond(&r, &mut z);
    let mut p = z.clone();
    let mut rz = dot(&r, &z);
    let mut it = 0;
    let mut res = dot(&r, &r).sqrt() / bnorm;
    while it < max_iter && res > tol {
        a.mul(&p, &mut ax);
        let pap = dot(&p, &ax);
        if !(pap.is_finite() && pap > 0.0) {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n2 {
            if let (Some(xi), Some(pi)) = (x.get_mut(i), p.get(i)) {
                *xi += alpha * pi;
            }
            if let (Some(ri), Some(ai)) = (r.get_mut(i), ax.get(i)) {
                *ri -= alpha * ai;
            }
        }
        res = dot(&r, &r).sqrt() / bnorm;
        precond(&r, &mut z);
        let rz_new = dot(&r, &z);
        let beta = if rz.abs() > 0.0 { rz_new / rz } else { 0.0 };
        rz = rz_new;
        for i in 0..n2 {
            if let (Some(pi), Some(zi)) = (p.get_mut(i), z.get(i)) {
                *pi = zi + beta * *pi;
            }
        }
        it += 1;
    }
    if x.iter().any(|v| !v.is_finite()) {
        x.iter_mut().for_each(|v| *v = 0.0);
        return CgReport { iterations: it, residual: f64::INFINITY, converged: false };
    }
    CgReport { iterations: it, residual: res, converged: res <= tol }
}
