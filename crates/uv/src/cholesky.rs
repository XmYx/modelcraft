//! Sparse Cholesky factorisation `P A Pᵀ = L Lᵀ` of a symmetric positive-definite matrix, with a
//! geometric nested-dissection ordering, in pure Rust.
//!
//! The LSCM normal equations are badly conditioned (their smallest eigenvalue shrinks with the
//! mesh size), so conjugate gradients stall on meshes of a few thousand vertices; a direct solve
//! does not care. The method is the textbook up-looking sparse Cholesky: the elimination tree of
//! the matrix gives, for each row `k`, the pattern of row `k` of `L`, and row `k` is computed
//! from the rows before it (Davis, "Direct Methods for Sparse Linear Systems", SIAM 2006, ch. 4,
//! and Liu, "The role of elimination trees in sparse factorization", 1990).
//!
//! Fill-in depends on the elimination order. The vertices carry 3D positions, so the order is a
//! nested dissection by recursive coordinate bisection (George 1973): split the vertices at the
//! median along the longest axis, number each half first and the vertices that connect the halves
//! (the separator) last. On a surface mesh the separator is a curve of about `√n` vertices, which
//! keeps the factor near `n log n` entries.
//!
//! Memory is bounded: the size of `L` is counted before it is allocated and the factorisation
//! gives up (the caller falls back to conjugate gradients) above `max_nnz` entries.

use crate::sparse::BlockCsr;

const NONE: usize = usize::MAX;

/// A factorisation ready to solve with.
pub(crate) struct Factor {
    n: usize,
    /// `perm[new] = old` scalar index.
    perm: Vec<usize>,
    lp: Vec<usize>,
    li: Vec<u32>,
    lx: Vec<f64>,
}

/// Order the vertices of the graph `rows` (sorted neighbour lists, self included) by nested
/// dissection over `coords`. Returns `order[new] = old`.
pub(crate) fn nested_dissection(rows: &[Vec<u32>], coords: &[[f64; 3]]) -> Vec<usize> {
    let n = rows.len();
    let mut out_rev: Vec<usize> = Vec::with_capacity(n);
    let mut stamp = vec![0u32; n];
    let mut side = vec![0u8; n];
    let mut next_stamp = 1u32;
    let mut stack: Vec<Vec<usize>> = vec![(0..n).collect()];
    while let Some(mut set) = stack.pop() {
        if set.len() <= 24 {
            out_rev.extend(set);
            continue;
        }
        // Longest axis of the bounding box.
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for &v in &set {
            for a in 0..3 {
                let c = coords.get(v).map_or(0.0, |c| c[a]);
                lo[a] = lo[a].min(c);
                hi[a] = hi[a].max(c);
            }
        }
        let axis = (0..3).max_by(|a, b| (hi[*a] - lo[*a]).total_cmp(&(hi[*b] - lo[*b]))).unwrap_or(0);
        let key = |v: usize| coords.get(v).map_or(0.0, |c| c[axis]);
        set.sort_by(|a, b| key(*a).total_cmp(&key(*b)).then(a.cmp(b)));
        let half = set.len() / 2;
        let id = next_stamp;
        next_stamp += 1;
        for (i, &v) in set.iter().enumerate() {
            stamp[v] = id;
            side[v] = u8::from(i >= half);
        }
        // Separator: vertices of the second half that touch the first.
        let mut a_part = Vec::with_capacity(half);
        let mut b_part = Vec::with_capacity(set.len() - half);
        let mut sep = Vec::new();
        for &v in &set {
            if side[v] == 0 {
                a_part.push(v);
            } else if rows[v].iter().any(|w| stamp[*w as usize] == id && side[*w as usize] == 0) {
                sep.push(v);
            } else {
                b_part.push(v);
            }
        }
        // Reverse order: the separator first, then the second half, then the first.
        out_rev.extend(sep);
        stack.push(a_part);
        stack.push(b_part);
    }
    out_rev.reverse();
    out_rev
}

impl Factor {
    /// Factor `a` (blocks of 2×2, symmetric) with the given block order. `None` when the matrix
    /// is not positive definite, or `L` would need more than `max_nnz` entries.
    pub fn new(a: &BlockCsr, block_order: &[usize], max_nnz: usize, shift: f64) -> Option<Factor> {
        let nb = a.n();
        let n = nb.checked_mul(2)?;
        if block_order.len() != nb {
            return None;
        }
        let mut perm = Vec::new();
        perm.try_reserve_exact(n).ok()?;
        for &b in block_order {
            perm.push(2 * b);
            perm.push(2 * b + 1);
        }
        let mut inv = vec![0usize; n];
        for (new, &old) in perm.iter().enumerate() {
            *inv.get_mut(old)? = new;
        }
        // Upper triangle (row <= col) of the permuted matrix, column by column.
        let mut count = vec![0usize; n + 1];
        let entries = |f: &mut dyn FnMut(usize, usize, f64)| {
            for i in 0..nb {
                let (s, e) = (*a.row_ptr.get(i)?, *a.row_ptr.get(i + 1)?);
                for k in s..e {
                    let j = *a.cols.get(k)? as usize;
                    let b = a.vals.get(k)?;
                    for (c, val) in b.iter().enumerate() {
                        f(2 * i + c / 2, 2 * j + c % 2, *val);
                    }
                }
            }
            Some(())
        };
        entries(&mut |r, c, _| {
            let (pr, pc) = (inv[r], inv[c]);
            if pr <= pc {
                count[pc + 1] += 1;
            }
        })?;
        for k in 0..n {
            count[k + 1] += count[k];
        }
        let nnz_a = *count.last()?;
        let mut ai = vec![0u32; nnz_a];
        let mut ax = vec![0.0f64; nnz_a];
        let mut fill = count.clone();
        entries(&mut |r, c, v| {
            let (pr, pc) = (inv[r], inv[c]);
            if pr <= pc {
                ai[fill[pc]] = pr as u32;
                ax[fill[pc]] = v;
                fill[pc] += 1;
            }
        })?;
        let ap = count;
        let diag_scale = (0..n).map(|k| (ap[k]..ap[k + 1]).filter(|p| ai[*p] as usize == k).map(|p| ax[p]).sum::<f64>()).fold(0.0f64, f64::max);

        // Elimination tree.
        let mut parent = vec![NONE; n];
        let mut ancestor = vec![NONE; n];
        for k in 0..n {
            for p in ap[k]..ap[k + 1] {
                let mut i = ai[p] as usize;
                while i != NONE && i < k {
                    let next = ancestor[i];
                    ancestor[i] = k;
                    if next == NONE {
                        parent[i] = k;
                    }
                    i = next;
                }
            }
        }
        drop(ancestor);

        // Pattern of row k of L: the nodes reached from the entries of column k of A up the tree.
        let mut mark = vec![NONE; n];
        let mut stack = vec![0usize; n];
        let mut path = vec![0usize; n];
        let ereach = |k: usize, mark: &mut [usize], stack: &mut [usize], path: &mut [usize]| -> usize {
            let mut top = n;
            mark[k] = k;
            for p in ap[k]..ap[k + 1] {
                let mut i = ai[p] as usize;
                if i > k {
                    continue;
                }
                let mut len = 0;
                while mark[i] != k {
                    path[len] = i;
                    len += 1;
                    mark[i] = k;
                    i = parent[i];
                }
                while len > 0 {
                    len -= 1;
                    top -= 1;
                    stack[top] = path[len];
                }
            }
            top
        };

        // Column counts of L (the diagonal included), to size the factor before allocating it.
        let mut lp = vec![0usize; n + 1];
        for k in 0..n {
            lp[k + 1] += 1;
            let top = ereach(k, &mut mark, &mut stack, &mut path);
            for t in top..n {
                lp[stack[t] + 1] += 1;
            }
        }
        for k in 0..n {
            lp[k + 1] += lp[k];
        }
        let nnz = *lp.last()?;
        if nnz > max_nnz {
            return None;
        }
        let mut li = Vec::new();
        li.try_reserve_exact(nnz).ok()?;
        li.resize(nnz, 0u32);
        let mut lx = Vec::new();
        lx.try_reserve_exact(nnz).ok()?;
        lx.resize(nnz, 0.0f64);

        // Numeric factorisation, one row of L at a time.
        let mut next = lp.clone();
        let mut x = vec![0.0f64; n];
        mark.iter_mut().for_each(|m| *m = NONE);
        for k in 0..n {
            let top = ereach(k, &mut mark, &mut stack, &mut path);
            let mut d = 0.0;
            for p in ap[k]..ap[k + 1] {
                let i = ai[p] as usize;
                if i < k {
                    x[i] = ax[p];
                } else if i == k {
                    d = ax[p] + shift * diag_scale;
                }
            }
            for t in top..n {
                let i = stack[t];
                let lii = lx[lp[i]];
                let lki = x[i] / lii;
                x[i] = 0.0;
                for p in lp[i] + 1..next[i] {
                    x[li[p] as usize] -= lx[p] * lki;
                }
                d -= lki * lki;
                let p = next[i];
                next[i] += 1;
                li[p] = k as u32;
                lx[p] = lki;
            }
            if !(d.is_finite() && d > 1e-14 * diag_scale.max(1e-300)) {
                return None;
            }
            let p = next[k];
            next[k] += 1;
            li[p] = k as u32;
            lx[p] = d.sqrt();
        }
        Some(Factor { n, perm, lp, li, lx })
    }

    /// Solve `A x = b` (`b` and `x` in the original, unpermuted numbering).
    pub fn solve(&self, b: &[f64], x: &mut [f64]) {
        if b.len() != self.n || x.len() != self.n {
            return;
        }
        let mut y: Vec<f64> = self.perm.iter().map(|&old| b[old]).collect();
        for j in 0..self.n {
            y[j] /= self.lx[self.lp[j]];
            let yj = y[j];
            for p in self.lp[j] + 1..self.lp[j + 1] {
                y[self.li[p] as usize] -= self.lx[p] * yj;
            }
        }
        for j in (0..self.n).rev() {
            let mut s = y[j];
            for p in self.lp[j] + 1..self.lp[j + 1] {
                s -= self.lx[p] * y[self.li[p] as usize];
            }
            y[j] = s / self.lx[self.lp[j]];
        }
        for (new, &old) in self.perm.iter().enumerate() {
            x[old] = y[new];
        }
    }
}
