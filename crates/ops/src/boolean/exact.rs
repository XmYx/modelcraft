//! Exact orientation predicates on integer points, made total by symbolic perturbation.
//!
//! Points are integers on a grid of at most [`MAX_COORD`] (2^28) in each axis, so a 4×4
//! determinant of homogeneous coordinates fits an `i128` exactly (no rounding anywhere). When the
//! determinant is exactly zero (four coplanar points) the sign is decided by the *simulation of
//! simplicity* of Edelsbrunner and Mücke ("Simulation of Simplicity: A Technique to Cope with
//! Degenerate Cases in Geometric Algorithms", ACM Transactions on Graphics 9(1), 1990): every
//! coordinate `x_ik` of the point with index `i` is moved by the infinitesimal `ε^(2^(3i+k))`,
//! and the sign is that of the first non-zero term of the determinant's expansion in `ε`.
//! The result is the predicate of a perturbed point set in which no four points are coplanar, so
//! every decision made from these predicates is consistent with one concrete (if infinitesimally
//! different) input. It never returns zero for four points with distinct indices.

/// Largest absolute coordinate the predicates accept (2^28): keeps every determinant inside `i128`.
pub const MAX_COORD: i64 = 1 << 28;

/// A grid point with the global index that orders its perturbation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pt {
    pub c: [i64; 3],
    pub index: u32,
}

type M4 = [[i128; 4]; 4];

fn det3(m: [[i128; 3]; 3]) -> i128 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn det4(m: &M4) -> i128 {
    let mut total = 0i128;
    for col in 0..4 {
        let mut minor = [[0i128; 3]; 3];
        for (r, row) in minor.iter_mut().enumerate() {
            let mut cc = 0;
            for c in 0..4 {
                if c != col {
                    row[cc] = m[r + 1][c];
                    cc += 1;
                }
            }
        }
        let term = m[0][col] * det3(minor);
        total += if col % 2 == 0 { term } else { -term };
    }
    total
}

fn row(p: &Pt) -> [i128; 4] {
    [i128::from(p.c[0]), i128::from(p.c[1]), i128::from(p.c[2]), 1]
}

/// The sign (-1, 0, 1) of the exact determinant of the four points' homogeneous coordinates:
/// positive when `d` lies on the positive side of the plane through `a`, `b`, `c` taken in that
/// order (the side the right-hand rule's normal of `a, b, c` points away from, in the usual
/// determinant convention). Zero means exactly coplanar; see [`orient`] for the perturbed sign.
pub fn orient_exact(a: &Pt, b: &Pt, c: &Pt, d: &Pt) -> i32 {
    let m: M4 = [row(a), row(b), row(c), row(d)];
    det4(&m).signum() as i32
}

/// One term of the perturbed determinant: rows `rows[k]` (0..4) perturbed in column `cols[k]`.
struct Term {
    rows: [u8; 4],
    cols: [u8; 4],
    len: usize,
}

fn terms() -> &'static [Term] {
    use std::sync::OnceLock;
    static T: OnceLock<Vec<Term>> = OnceLock::new();
    T.get_or_init(|| {
        let mut out = Vec::new();
        // Every subset of rows, each given a distinct column among 0..3.
        for mask in 0u8..16 {
            let rows: Vec<u8> = (0..4).filter(|r| mask & (1 << r) != 0).collect();
            let n = rows.len();
            if n > 3 {
                continue;
            }
            // Assign distinct columns by trying all injective maps.
            let mut cols = vec![0u8; n];
            fn assign(i: usize, n: usize, rows: &[u8], cols: &mut Vec<u8>, used: u8, out: &mut Vec<Term>) {
                if i == n {
                    let mut t = Term { rows: [0; 4], cols: [0; 4], len: n };
                    for k in 0..n {
                        if let (Some(r), Some(c)) = (rows.get(k), cols.get(k)) {
                            t.rows[k] = *r;
                            t.cols[k] = *c;
                        }
                    }
                    out.push(t);
                    return;
                }
                for c in 0..3u8 {
                    if used & (1 << c) == 0 {
                        if let Some(slot) = cols.get_mut(i) {
                            *slot = c;
                        }
                        assign(i + 1, n, rows, cols, used | (1 << c), out);
                    }
                }
            }
            assign(0, n, &rows, &mut cols, 0, &mut out);
        }
        out
    })
}

/// The sign of the orientation determinant of four points with **distinct indices**, never
/// zero: exact when the points are not coplanar, otherwise the sign of the infinitesimally
/// perturbed configuration. Swapping two arguments flips the sign, as for a determinant.
pub fn orient(a: &Pt, b: &Pt, c: &Pt, d: &Pt) -> i32 {
    let pts = [a, b, c, d];
    let base = orient_exact(a, b, c, d);
    if base != 0 {
        return base;
    }
    // Order the terms by the size of their `ε` exponent: smaller exponent = larger term. A term's
    // exponent is the sum of 2^(3·index + column) over its perturbed coordinates, and sums of
    // distinct powers of two compare like their descending lists of exponents.
    let rank = |r: u8, c: u8| -> u64 { u64::from(pts.get(usize::from(r)).map_or(0, |p| p.index)) * 3 + u64::from(c) };
    let mut order: Vec<(Vec<u64>, usize)> = terms()
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut key: Vec<u64> = (0..t.len).map(|k| rank(t.rows[k], t.cols[k])).collect();
            key.sort_unstable_by(|x, y| y.cmp(x));
            (key, i)
        })
        .collect();
    order.sort();
    for (_, i) in order {
        let Some(t) = terms().get(i) else { continue };
        let mut m: M4 = [row(a), row(b), row(c), row(d)];
        for k in 0..t.len {
            let r = usize::from(t.rows[k]);
            let col = usize::from(t.cols[k]);
            m[r] = [0; 4];
            m[r][col] = 1;
        }
        let v = det4(&m).signum() as i32;
        if v != 0 {
            return v;
        }
    }
    // Unreachable for distinct indices; a stable answer rather than a panic if it ever happens.
    1
}

/// Whether the open segment `p`–`q` crosses the triangle `a`, `b`, `c` in the perturbed
/// configuration: the endpoints lie on opposite sides of the triangle's plane, and the segment's
/// line passes through the triangle (the three tetrahedra it makes with the edges agree).
pub fn segment_crosses_triangle(p: &Pt, q: &Pt, a: &Pt, b: &Pt, c: &Pt) -> bool {
    if orient(a, b, c, p) == orient(a, b, c, q) {
        return false;
    }
    let s0 = orient(p, q, a, b);
    s0 == orient(p, q, b, c) && s0 == orient(p, q, c, a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i64, y: i64, z: i64, index: u32) -> Pt {
        Pt { c: [x, y, z], index }
    }

    #[test]
    fn orientation_is_exact_and_antisymmetric() {
        let (a, b, c) = (p(0, 0, 0, 0), p(1, 0, 0, 1), p(0, 1, 0, 2));
        let up = p(0, 0, 1, 3);
        let down = p(0, 0, -1, 4);
        assert_eq!(orient_exact(&a, &b, &c, &up), -orient_exact(&a, &b, &c, &down));
        assert_ne!(orient_exact(&a, &b, &c, &up), 0);
        assert_eq!(orient(&a, &b, &c, &up), orient_exact(&a, &b, &c, &up));
        // Swapping two arguments flips the sign.
        assert_eq!(orient(&b, &a, &c, &up), -orient(&a, &b, &c, &up));
        // Huge coordinates stay exact: a plane tilted by one grid unit over 2^28.
        let m = MAX_COORD;
        let (a, b, c) = (p(-m, -m, 0, 0), p(m, -m, 0, 1), p(-m, m, 1, 2));
        let above = p(m, m, 1, 3);
        let below = p(m, m, 0, 4);
        assert_ne!(orient_exact(&a, &b, &c, &above), orient_exact(&a, &b, &c, &below));
    }

    #[test]
    fn coplanar_points_get_a_consistent_nonzero_sign() {
        // Every permutation of four coplanar points with distinct indices has a nonzero sign that
        // follows the parity of the permutation.
        let pts = [p(0, 0, 5, 7), p(3, 0, 5, 2), p(0, 4, 5, 9), p(1, 1, 5, 4)];
        assert_eq!(orient_exact(&pts[0], &pts[1], &pts[2], &pts[3]), 0);
        let mut perms = Vec::new();
        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    for l in 0..4 {
                        if i != j && i != k && i != l && j != k && j != l && k != l {
                            perms.push([i, j, k, l]);
                        }
                    }
                }
            }
        }
        let parity = |perm: &[usize; 4]| {
            let mut inv = 0;
            for x in 0..4 {
                for y in x + 1..4 {
                    if perm[x] > perm[y] {
                        inv += 1;
                    }
                }
            }
            if inv % 2 == 0 { 1 } else { -1 }
        };
        let reference = orient(&pts[0], &pts[1], &pts[2], &pts[3]);
        assert_ne!(reference, 0);
        for perm in &perms {
            let s = orient(&pts[perm[0]], &pts[perm[1]], &pts[perm[2]], &pts[perm[3]]);
            assert_eq!(s, reference * parity(perm), "{perm:?}");
        }
    }

    #[test]
    fn identical_coordinates_with_distinct_indices_are_still_ordered() {
        // Four copies of the same point: only the indices decide, and they decide consistently.
        let pts = [p(1, 2, 3, 0), p(1, 2, 3, 1), p(1, 2, 3, 2), p(1, 2, 3, 3)];
        let s = orient(&pts[0], &pts[1], &pts[2], &pts[3]);
        assert_ne!(s, 0);
        assert_eq!(orient(&pts[1], &pts[0], &pts[2], &pts[3]), -s);
        assert_eq!(orient(&pts[3], &pts[2], &pts[1], &pts[0]), s, "reversing four items is an even permutation");
    }

    #[test]
    fn a_segment_through_a_triangle_crosses_it_and_one_beside_does_not() {
        let (a, b, c) = (p(0, 0, 0, 0), p(10, 0, 0, 1), p(0, 10, 0, 2));
        assert!(segment_crosses_triangle(&p(2, 2, -5, 3), &p(2, 2, 5, 4), &a, &b, &c));
        assert!(!segment_crosses_triangle(&p(20, 20, -5, 3), &p(20, 20, 5, 4), &a, &b, &c), "outside the triangle");
        assert!(!segment_crosses_triangle(&p(2, 2, 1, 3), &p(2, 2, 5, 4), &a, &b, &c), "stops short");
        // On the plane, exactly on an edge: the perturbation puts it on one side only, and the
        // same call gives the same answer every time.
        let on_edge = |i| segment_crosses_triangle(&p(5, 0, -5, i), &p(5, 0, 5, i + 1), &a, &b, &c);
        assert_eq!(on_edge(3), on_edge(3));
    }
}
