//! A static 3-d tree over points, for nearest-k queries. Built by median splits (depth at most
//! `log2 n + 1`), queried recursively to that depth.

use glam::Vec3;

/// Most neighbours one query returns.
pub const MAX_K: usize = 8;

pub(crate) struct KdTree {
    points: Vec<Vec3>,
    /// Point indices arranged so the median of every range is the node of that range.
    order: Vec<u32>,
    /// Split axis of the node at each position of `order`.
    axis: Vec<u8>,
}

/// Up to [`MAX_K`] neighbours, nearest first: squared distance and point index.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Neighbors {
    pub items: [(f32, u32); MAX_K],
    pub len: usize,
}

impl Neighbors {
    fn new() -> Neighbors {
        Neighbors { items: [(f32::INFINITY, u32::MAX); MAX_K], len: 0 }
    }
    fn worst(&self, k: usize) -> f32 {
        if self.len < k { f32::INFINITY } else { self.items.get(k - 1).map_or(f32::INFINITY, |i| i.0) }
    }
    fn push(&mut self, k: usize, d2: f32, idx: u32) {
        if d2 >= self.worst(k) {
            return;
        }
        let mut i = self.len.min(k - 1);
        // `i` is the slot being filled; shift larger entries down.
        while i > 0 && self.items.get(i - 1).is_some_and(|p| p.0 > d2) {
            if let Some(prev) = self.items.get(i - 1).copied()
                && let Some(slot) = self.items.get_mut(i)
            {
                *slot = prev;
            }
            i -= 1;
        }
        if let Some(slot) = self.items.get_mut(i) {
            *slot = (d2, idx);
        }
        self.len = (self.len + 1).min(k);
    }
    pub(crate) fn as_slice(&self) -> &[(f32, u32)] {
        self.items.get(..self.len).unwrap_or_default()
    }
}

impl KdTree {
    pub(crate) fn new(points: Vec<Vec3>) -> KdTree {
        let n = points.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut axis = vec![0u8; n];
        // Explicit stack: ranges still to split.
        let mut stack = vec![(0usize, n)];
        while let Some((lo, hi)) = stack.pop() {
            if hi - lo <= 1 {
                continue;
            }
            let Some(slice) = order.get(lo..hi) else { continue };
            let (mut mn, mut mx) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
            for &i in slice {
                let p = points.get(i as usize).copied().unwrap_or(Vec3::ZERO);
                mn = mn.min(p);
                mx = mx.max(p);
            }
            let e = mx - mn;
            let ax = if e.x >= e.y && e.x >= e.z {
                0
            } else if e.y >= e.z {
                1
            } else {
                2
            };
            let mid = lo + (hi - lo) / 2;
            if let Some(slice) = order.get_mut(lo..hi) {
                slice.select_nth_unstable_by(mid - lo, |a, b| {
                    let pa = points.get(*a as usize).map_or(0.0, |p| p[ax]);
                    let pb = points.get(*b as usize).map_or(0.0, |p| p[ax]);
                    pa.total_cmp(&pb)
                });
            }
            if let Some(a) = axis.get_mut(mid) {
                *a = ax as u8;
            }
            stack.push((lo, mid));
            stack.push((mid + 1, hi));
        }
        KdTree { points, order, axis }
    }

    /// The `k` (1..=[`MAX_K`]) points nearest `q`, nearest first.
    pub(crate) fn nearest(&self, q: Vec3, k: usize) -> Neighbors {
        let mut out = Neighbors::new();
        let k = k.clamp(1, MAX_K);
        self.search(0, self.order.len(), q, k, &mut out);
        out
    }

    fn search(&self, lo: usize, hi: usize, q: Vec3, k: usize, out: &mut Neighbors) {
        if lo >= hi {
            return;
        }
        let mid = lo + (hi - lo) / 2;
        let Some(&idx) = self.order.get(mid) else { return };
        let p = self.points.get(idx as usize).copied().unwrap_or(Vec3::ZERO);
        out.push(k, (p - q).length_squared(), idx);
        if hi - lo == 1 {
            return;
        }
        let ax = self.axis.get(mid).copied().unwrap_or(0) as usize;
        let diff = q[ax] - p[ax];
        let (near, far) = if diff < 0.0 { ((lo, mid), (mid + 1, hi)) } else { ((mid + 1, hi), (lo, mid)) };
        self.search(near.0, near.1, q, k, out);
        if diff * diff < out.worst(k) {
            self.search(far.0, far.1, q, k, out);
        }
    }
}
