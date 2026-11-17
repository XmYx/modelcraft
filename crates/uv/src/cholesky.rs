//! Sparse Cholesky factorisation `P A Pᵀ = L Lᵀ` of a symmetric positive-definite matrix, with a
//! geometric nested-dissection ordering, in pure Rust.
//!
//! The LSCM normal equations are badly conditioned (their smallest eigenvalue shrinks with the
//! mesh size), so conjugate gradients stall on meshes of a few thousand vertices; a direct solve
//! does not care.
//!
//! The factorisation is the left-looking column method of George and Liu, "Computer Solution of
//! Large Sparse Positive Definite Systems" (Prentice-Hall, 1981), written from its mathematical
//! description:
//!
//! - *Pattern.* Column `j` of `L` has a nonzero in row `r > j` exactly when `A` has one there or
//!   some earlier column `c` whose first sub-diagonal row is `j` (a child of `j` in the elimination
//!   tree) has one there. So a column's pattern is the union of its own pattern in `A` and its
//!   children's patterns, less row `j`, and no tree needs to be built beforehand.
//! - *Values.* Column `j` is `A`'s column minus the multiples of earlier columns `k` with
//!   `L[j][k] ≠ 0`, then scaled by the square root of its diagonal. Each finished column waits on
//!   the row of its next unused entry, so column `j` finds exactly the columns that update it
//!   in a linked list, without searching.
//!
//! Fill-in depends on the elimination order. The vertices carry 3D positions, so the order is a
//! nested dissection by recursive coordinate bisection (George 1973): split the vertices at the
//! median along the longest axis, number each half first and the vertices that connect the halves
//! (the separator) last. On a surface mesh the separator is a curve of about `√n` vertices, which
//! keeps the factor near `n log n` entries.
//!
//! Memory is bounded: the factorisation gives up (the caller falls back to conjugate gradients)
//! as soon as `L` would need more than `max_nnz` entries, before it grows the arrays.

use crate::sparse::BlockCsr;

const NONE: usize = usize::MAX;

/// A factorisation ready to solve with.
pub(crate) struct Factor {
    n: usize,
    /// `perm[new] = old` scalar index.
    perm: Vec<usize>,
    /// Start of each column in `rows` and `vals` (`n + 1` entries); the diagonal comes first.
    col_start: Vec<usize>,
    rows: Vec<u32>,
    vals: Vec<f64>,
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
        let mut position = vec![0usize; n];
        for (new, &old) in perm.iter().enumerate() {
            *position.get_mut(old)? = new;
        }

        // The lower triangle of the permuted matrix by columns: `low_rows[low_start[c]..low_start[c + 1]]`
        // are the rows `r >= c` of column `c`.
        let mut low_start = vec![0usize; n + 1];
        let blocks = |f: &mut dyn FnMut(usize, usize, f64)| {
            for i in 0..nb {
                let (s, e) = (*a.row_ptr.get(i)?, *a.row_ptr.get(i + 1)?);
                for k in s..e {
                    let j = *a.cols.get(k)? as usize;
                    let blk = a.vals.get(k)?;
                    for (c, val) in blk.iter().enumerate() {
                        f(2 * i + c / 2, 2 * j + c % 2, *val);
                    }
                }
            }
            Some(())
        };
        blocks(&mut |r, c, _| {
            let (pr, pc) = (position[r], position[c]);
            if pr >= pc {
                low_start[pc + 1] += 1;
            }
        })?;
        for c in 0..n {
            low_start[c + 1] += low_start[c];
        }
        let low_total = *low_start.last()?;
        let mut low_rows = vec![0u32; low_total];
        let mut low_vals = vec![0.0f64; low_total];
        let mut next_slot = low_start.clone();
        blocks(&mut |r, c, v| {
            let (pr, pc) = (position[r], position[c]);
            if pr >= pc {
                low_rows[next_slot[pc]] = pr as u32;
                low_vals[next_slot[pc]] = v;
                next_slot[pc] += 1;
            }
        })?;
        drop(next_slot);
        let diag_scale =
            (0..n).map(|c| (low_start[c]..low_start[c + 1]).filter(|p| low_rows[*p] as usize == c).map(|p| low_vals[p]).sum::<f64>()).fold(0.0f64, f64::max);

        let mut col_start: Vec<usize> = Vec::new();
        col_start.try_reserve_exact(n + 1).ok()?;
        let mut rows: Vec<u32> = Vec::new();
        let mut vals: Vec<f64> = Vec::new();
        // Linked lists: `child_head[j]` chains the finished columns whose first sub-diagonal row is
        // `j`; `wait_head[j]` chains those whose next unused entry is in row `j`.
        let mut child_head = vec![NONE; n];
        let mut child_next = vec![NONE; n];
        let mut wait_head = vec![NONE; n];
        let mut wait_next = vec![NONE; n];
        // Position of the next unused entry of each finished column.
        let mut cursor = vec![0usize; n];
        let mut seen = vec![NONE; n];
        let mut work = vec![0.0f64; n];
        let mut pattern: Vec<u32> = Vec::new();
        for j in 0..n {
            // Column j starts where the last one ended, which is also where that one ends.
            let start = rows.len();
            col_start.push(start);
            // Pattern of column j below the diagonal: its own entries in A and its children's.
            pattern.clear();
            for p in low_start[j]..low_start[j + 1] {
                let r = low_rows[p];
                if r as usize > j && seen[r as usize] != j {
                    seen[r as usize] = j;
                    pattern.push(r);
                }
            }
            let mut c = child_head[j];
            while c != NONE {
                for p in col_start[c] + 1..col_start[c + 1] {
                    let r = rows[p];
                    if r as usize > j && seen[r as usize] != j {
                        seen[r as usize] = j;
                        pattern.push(r);
                    }
                }
                c = child_next[c];
            }
            pattern.sort_unstable();
            if start.checked_add(1 + pattern.len())? > max_nnz {
                return None;
            }
            rows.try_reserve(1 + pattern.len()).ok()?;
            vals.try_reserve(1 + pattern.len()).ok()?;
            rows.push(j as u32);
            rows.extend_from_slice(&pattern);
            vals.resize(rows.len(), 0.0);

            // Values: A's column, less the updates of the earlier columns that reach row j.
            for p in low_start[j]..low_start[j + 1] {
                work[low_rows[p] as usize] += low_vals[p];
            }
            work[j] += shift * diag_scale;
            let mut k = wait_head[j];
            wait_head[j] = NONE;
            while k != NONE {
                let after = wait_next[k];
                let (from, to) = (cursor[k], col_start[k + 1]);
                let ljk = vals[from];
                for q in from..to {
                    work[rows[q] as usize] -= vals[q] * ljk;
                }
                cursor[k] = from + 1;
                if from + 1 < to {
                    let r = rows[from + 1] as usize;
                    wait_next[k] = wait_head[r];
                    wait_head[r] = k;
                }
                k = after;
            }
            let d = work[j];
            work[j] = 0.0;
            if !(d.is_finite() && d > 1e-14 * diag_scale.max(1e-300)) {
                return None;
            }
            let root = d.sqrt();
            vals[start] = root;
            for (i, &r) in pattern.iter().enumerate() {
                vals[start + 1 + i] = work[r as usize] / root;
                work[r as usize] = 0.0;
            }
            // Register the finished column with its parent.
            cursor[j] = start + 1;
            if let Some(&parent) = pattern.first() {
                let parent = parent as usize;
                child_next[j] = child_head[parent];
                child_head[parent] = j;
                wait_next[j] = wait_head[parent];
                wait_head[parent] = j;
            }
        }
        col_start.push(rows.len());
        Some(Factor { n, perm, col_start, rows, vals })
    }

    /// Solve `A x = b` (`b` and `x` in the original, unpermuted numbering).
    pub fn solve(&self, b: &[f64], x: &mut [f64]) {
        if b.len() != self.n || x.len() != self.n {
            return;
        }
        let mut y: Vec<f64> = self.perm.iter().map(|&old| b[old]).collect();
        for j in 0..self.n {
            let (s, e) = (self.col_start[j], self.col_start[j + 1]);
            y[j] /= self.vals[s];
            let yj = y[j];
            for p in s + 1..e {
                y[self.rows[p] as usize] -= self.vals[p] * yj;
            }
        }
        for j in (0..self.n).rev() {
            let (s, e) = (self.col_start[j], self.col_start[j + 1]);
            let mut acc = y[j];
            for p in s + 1..e {
                acc -= self.vals[p] * y[self.rows[p] as usize];
            }
            y[j] = acc / self.vals[s];
        }
        for (new, &old) in self.perm.iter().enumerate() {
            x[old] = y[new];
        }
    }
}
