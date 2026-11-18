//! A spatial hash over points or boxes, for neighbour queries in the solvers.
//!
//! Cells are a regular grid of `cell` metres. Items are stored as `(cell key, index)` pairs
//! sorted by key and then index, so queries visit candidates in the same order on every run
//! (a `HashMap` would not): bakes stay repeatable. Boxes that span more than [`MAX_CELLS`] cells
//! (a ground plane) go in a short list every query also returns.

use glam::Vec3;

/// Most cells one box is entered into; larger boxes are listed as `big`.
pub const MAX_CELLS: usize = 64;
/// Largest cell coordinate; positions beyond it share the outermost cells.
const LIMIT: i64 = 1 << 19;

#[derive(Clone, Debug, Default)]
pub struct SpatialHash {
    cell: f32,
    inv: f32,
    items: Vec<(u64, u32)>,
    big: Vec<u32>,
}

fn coord(v: f32, inv: f32) -> i64 {
    ((v * inv).floor() as i64).clamp(-LIMIT, LIMIT)
}

fn key(x: i64, y: i64, z: i64) -> u64 {
    let m = (1u64 << 21) - 1;
    (((x + LIMIT) as u64 & m) << 42) | (((y + LIMIT) as u64 & m) << 21) | ((z + LIMIT) as u64 & m)
}

impl SpatialHash {
    pub fn new(cell: f32) -> SpatialHash {
        let cell = if cell.is_finite() && cell > 1e-6 { cell } else { 1.0 };
        SpatialHash { cell, inv: 1.0 / cell, items: Vec::new(), big: Vec::new() }
    }

    pub fn cell(&self) -> f32 {
        self.cell
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.big.clear();
    }

    /// Insert point `i` at `p` (call [`SpatialHash::finish`] after the last insert).
    pub fn insert_point(&mut self, i: u32, p: Vec3) {
        self.items.push((key(coord(p.x, self.inv), coord(p.y, self.inv), coord(p.z, self.inv)), i));
    }

    /// Insert item `i` covering the box `lo..hi`.
    pub fn insert_box(&mut self, i: u32, lo: Vec3, hi: Vec3) {
        let (a, b) =
            ([coord(lo.x, self.inv), coord(lo.y, self.inv), coord(lo.z, self.inv)], [coord(hi.x, self.inv), coord(hi.y, self.inv), coord(hi.z, self.inv)]);
        let n = (b[0] - a[0] + 1).saturating_mul(b[1] - a[1] + 1).saturating_mul(b[2] - a[2] + 1);
        if n < 1 || n as usize > MAX_CELLS {
            self.big.push(i);
            return;
        }
        for x in a[0]..=b[0] {
            for y in a[1]..=b[1] {
                for z in a[2]..=b[2] {
                    self.items.push((key(x, y, z), i));
                }
            }
        }
    }

    /// Sort the inserted items; queries are valid after this.
    pub fn finish(&mut self) {
        self.items.sort_unstable();
    }

    fn visit_cell(&self, k: u64, f: &mut impl FnMut(u32)) {
        let start = self.items.partition_point(|(c, _)| *c < k);
        for (c, i) in self.items.get(start..).unwrap_or_default() {
            if *c != k {
                break;
            }
            f(*i);
        }
    }

    /// Visit every item in the cells a sphere of `radius` around `p` touches (at most 5×5×5
    /// cells), then the big boxes. An item entered into several cells can be visited more than
    /// once when it is a box; points never are.
    pub fn query(&self, p: Vec3, radius: f32, mut f: impl FnMut(u32)) {
        let r = radius.max(0.0);
        let (lo, hi) = (p - Vec3::splat(r), p + Vec3::splat(r));
        let a = [coord(lo.x, self.inv), coord(lo.y, self.inv), coord(lo.z, self.inv)];
        let b = [coord(hi.x, self.inv).min(a[0] + 4), coord(hi.y, self.inv).min(a[1] + 4), coord(hi.z, self.inv).min(a[2] + 4)];
        for x in a[0]..=b[0] {
            for y in a[1]..=b[1] {
                for z in a[2]..=b[2] {
                    self.visit_cell(key(x, y, z), &mut f);
                }
            }
        }
        for i in &self.big {
            f(*i);
        }
    }

    /// Items in the number of entries (for tests and memory estimates).
    pub fn len(&self) -> usize {
        self.items.len() + self.big.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;

    #[test]
    fn a_point_query_finds_every_point_within_the_radius() {
        let mut r = Rng::new(4);
        let pts: Vec<Vec3> = (0..500).map(|_| Vec3::new(r.range(-3.0, 3.0), r.range(-3.0, 3.0), r.range(-3.0, 3.0))).collect();
        let mut h = SpatialHash::new(0.5);
        for (i, p) in pts.iter().enumerate() {
            h.insert_point(i as u32, *p);
        }
        h.finish();
        for q in pts.iter().take(40) {
            let mut seen = Vec::new();
            h.query(*q, 0.5, |i| seen.push(i));
            let want: Vec<u32> = (0..pts.len() as u32).filter(|i| pts[*i as usize].distance(*q) <= 0.5).collect();
            assert!(want.iter().all(|i| seen.contains(i)), "a point within the radius was missed");
            let mut dedup = seen.clone();
            dedup.sort_unstable();
            dedup.dedup();
            assert_eq!(dedup.len(), seen.len(), "points are visited once");
        }
    }

    #[test]
    fn boxes_are_found_where_they_are_and_big_ones_always() {
        let mut h = SpatialHash::new(1.0);
        h.insert_box(0, Vec3::new(0.1, 0.1, 0.1), Vec3::new(0.9, 0.9, 0.9));
        h.insert_box(1, Vec3::new(5.1, 0.1, 0.1), Vec3::new(5.9, 0.9, 0.9));
        h.insert_box(2, Vec3::splat(-1000.0), Vec3::splat(1000.0));
        h.finish();
        let mut seen = Vec::new();
        h.query(Vec3::splat(0.5), 0.1, |i| seen.push(i));
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 2], "the near box and the big one, not the far box");
        assert_eq!(h.len(), 3);
        h.clear();
        assert!(h.is_empty());
    }

    #[test]
    fn hostile_coordinates_and_cells_do_not_panic() {
        let mut h = SpatialHash::new(f32::NAN);
        assert_eq!(h.cell(), 1.0);
        for p in [Vec3::splat(f32::MAX), Vec3::NAN, Vec3::splat(f32::NEG_INFINITY), Vec3::splat(1e30)] {
            h.insert_point(0, p);
            h.insert_box(1, p, p);
            h.query(p, f32::NAN, |_| {});
            h.query(p, 1e30, |_| {});
        }
        h.finish();
    }
}
