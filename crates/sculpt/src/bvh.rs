//! A bounding volume hierarchy over triangles: median split, leaves of up to 8 triangles,
//! children stored after their parent so a reverse sweep refits bottom-up. Traversal uses an
//! explicit stack (no recursion).

use glam::Vec3;

const LEAF: usize = 8;

#[derive(Clone, Copy, Debug)]
struct Node {
    lo: Vec3,
    hi: Vec3,
    /// Leaf: first index into `order`; inner: index of the left child (right = left + 1).
    first: u32,
    /// Leaf: triangle count; inner: 0.
    count: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
    /// The leaf node holding each triangle.
    tri_leaf: Vec<u32>,
    /// Triangles added to a leaf after the build (live detail), by leaf node.
    extra: std::collections::HashMap<u32, Vec<u32>>,
}

fn tri_bounds(p: &[Vec3], t: &[u32; 3]) -> (Vec3, Vec3) {
    let g = |i: u32| p.get(i as usize).copied().unwrap_or_default();
    let (a, b, c) = (g(t[0]), g(t[1]), g(t[2]));
    (a.min(b).min(c), a.max(b).max(c))
}

impl Bvh {
    pub fn build(p: &[Vec3], tris: &[[u32; 3]]) -> Bvh {
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let centroid = |t: u32| tris.get(t as usize).map(|tri| tri_bounds(p, tri)).map(|(a, b)| (a + b) * 0.5).unwrap_or_default();
        let mut nodes = vec![Node { lo: Vec3::ZERO, hi: Vec3::ZERO, first: 0, count: tris.len() as u32 }];
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let Some(node) = nodes.get(ni).copied() else { continue };
            let (s, n) = (node.first as usize, node.count as usize);
            let Some(slice) = order.get_mut(s..s + n) else { continue };
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let (mut clo, mut chi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for t in slice.iter() {
                if let Some(tri) = tris.get(*t as usize) {
                    let (a, b) = tri_bounds(p, tri);
                    lo = lo.min(a);
                    hi = hi.max(b);
                    let c = (a + b) * 0.5;
                    clo = clo.min(c);
                    chi = chi.max(c);
                }
            }
            if let Some(nd) = nodes.get_mut(ni) {
                nd.lo = lo;
                nd.hi = hi;
            }
            if n <= LEAF {
                continue;
            }
            let ext = chi - clo;
            let axis = if ext.x >= ext.y && ext.x >= ext.z {
                0
            } else if ext.y >= ext.z {
                1
            } else {
                2
            };
            let mid = n / 2;
            slice.select_nth_unstable_by(mid, |a, b| centroid(*a)[axis].total_cmp(&centroid(*b)[axis]));
            let left = nodes.len() as u32;
            nodes.push(Node { lo, hi, first: s as u32, count: mid as u32 });
            nodes.push(Node { lo, hi, first: (s + mid) as u32, count: (n - mid) as u32 });
            if let Some(nd) = nodes.get_mut(ni) {
                nd.first = left;
                nd.count = 0;
            }
            stack.push(left as usize);
            stack.push(left as usize + 1);
        }
        let mut tri_leaf = vec![0u32; tris.len()];
        for (i, n) in nodes.iter().enumerate() {
            if n.count > 0 {
                for t in order.get(n.first as usize..(n.first + n.count) as usize).unwrap_or_default() {
                    if let Some(x) = tri_leaf.get_mut(*t as usize) {
                        *x = i as u32;
                    }
                }
            }
        }
        Bvh { nodes, order, tri_leaf, extra: Default::default() }
    }

    /// Put the new triangle `child` into the leaf of `parent`. The caller guarantees the child
    /// lies inside the parent (it came from splitting it), so the leaf's box still holds.
    pub fn add_child(&mut self, parent: u32, child: u32) {
        let leaf = self.tri_leaf.get(parent as usize).copied().unwrap_or(0);
        if self.tri_leaf.len() <= child as usize {
            self.tri_leaf.resize(child as usize + 1, 0);
        }
        if let Some(x) = self.tri_leaf.get_mut(child as usize) {
            *x = leaf;
        }
        self.extra.entry(leaf).or_default().push(child);
    }

    /// Triangles added since the build.
    pub fn added(&self) -> usize {
        self.extra.values().map(Vec::len).sum()
    }

    fn leaf_tris(&self, i: usize, n: &Node) -> impl Iterator<Item = u32> + '_ {
        let own = self.order.get(n.first as usize..(n.first + n.count) as usize).unwrap_or_default();
        own.iter().copied().chain(self.extra.get(&(i as u32)).map(Vec::as_slice).unwrap_or_default().iter().copied())
    }

    pub fn heap_bytes(&self) -> usize {
        self.nodes.len() * std::mem::size_of::<Node>() + self.order.len() * 4 + self.tri_leaf.len() * 4 + self.added() * 4
    }

    pub fn refit(&mut self, p: &[Vec3], tris: &[[u32; 3]]) {
        for i in (0..self.nodes.len()).rev() {
            let Some(node) = self.nodes.get(i).copied() else { continue };
            let (lo, hi) = if node.count > 0 {
                self.leaf_tris(i, &node)
                    .filter_map(|t| tris.get(t as usize))
                    .map(|t| tri_bounds(p, t))
                    .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), (a, b)| (lo.min(a), hi.max(b)))
            } else {
                let l = self.nodes.get(node.first as usize).copied();
                let r = self.nodes.get(node.first as usize + 1).copied();
                match (l, r) {
                    (Some(l), Some(r)) => (l.lo.min(r.lo), l.hi.max(r.hi)),
                    _ => (node.lo, node.hi),
                }
            };
            if let Some(nd) = self.nodes.get_mut(i) {
                nd.lo = lo;
                nd.hi = hi;
            }
        }
    }

    /// Refit only the leaves holding the triangles `moved`, and the boxes above them. The cost
    /// is one cheap pass over the nodes plus the real work for the moved part, instead of a
    /// box computation for every triangle of the mesh.
    pub fn refit_moved(&mut self, p: &[Vec3], tris: &[[u32; 3]], moved: impl Iterator<Item = u32>) {
        let mut dirty = vec![false; self.nodes.len()];
        for t in moved {
            if let Some(d) = self.tri_leaf.get(t as usize).and_then(|leaf| dirty.get_mut(*leaf as usize)) {
                *d = true;
            }
        }
        for i in (0..self.nodes.len()).rev() {
            let Some(node) = self.nodes.get(i).copied() else { continue };
            let (lo, hi) = if node.count > 0 {
                if !dirty.get(i).copied().unwrap_or(false) {
                    continue;
                }
                self.leaf_tris(i, &node)
                    .filter_map(|t| tris.get(t as usize))
                    .map(|t| tri_bounds(p, t))
                    .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), (a, b)| (lo.min(a), hi.max(b)))
            } else {
                let (li, ri) = (node.first as usize, node.first as usize + 1);
                if !(dirty.get(li).copied().unwrap_or(false) || dirty.get(ri).copied().unwrap_or(false)) {
                    continue;
                }
                if let Some(d) = dirty.get_mut(i) {
                    *d = true;
                }
                match (self.nodes.get(li).copied(), self.nodes.get(ri).copied()) {
                    (Some(l), Some(r)) => (l.lo.min(r.lo), l.hi.max(r.hi)),
                    _ => (node.lo, node.hi),
                }
            };
            if let Some(nd) = self.nodes.get_mut(i) {
                nd.lo = lo;
                nd.hi = hi;
            }
        }
    }

    /// Calls `f` for every triangle in leaves whose box (grown by `pad`) meets the sphere.
    pub fn visit_sphere(&self, c: Vec3, r: f32, mut f: impl FnMut(u32)) {
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let Some(n) = self.nodes.get(i) else { continue };
            let q = c.clamp(n.lo, n.hi);
            if q.distance_squared(c) > r * r {
                continue;
            }
            if n.count > 0 {
                for t in self.leaf_tris(i, n) {
                    f(t);
                }
            } else {
                stack.push(n.first as usize);
                stack.push(n.first as usize + 1);
            }
        }
    }

    /// Does a ray hit anything at all? (`skip`: triangles to ignore, or empty.)
    pub fn any_hit(&self, p: &[Vec3], tris: &[[u32; 3]], o: Vec3, d: Vec3, pad: f32, skip: &[bool]) -> bool {
        if !(o.is_finite() && d.is_finite()) || d.length_squared() == 0.0 {
            return false;
        }
        let inv = d.recip();
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let Some(n) = self.nodes.get(i) else { continue };
            let (lo, hi) = (n.lo - Vec3::splat(pad), n.hi + Vec3::splat(pad));
            let t1 = (lo - o) * inv;
            let t2 = (hi - o) * inv;
            if t1.min(t2).max_element().max(0.0) > t1.max(t2).min_element() {
                continue;
            }
            if n.count > 0 {
                for t in self.leaf_tris(i, n) {
                    let Some(tri) = tris.get(t as usize) else { continue };
                    if skip.get(t as usize).copied().unwrap_or(false) {
                        continue;
                    }
                    let g = |i: u32| p.get(i as usize).copied().unwrap_or_default();
                    if ray_tri(o, d, g(tri[0]), g(tri[1]), g(tri[2])).is_some() {
                        return true;
                    }
                }
            } else {
                stack.push(n.first as usize);
                stack.push(n.first as usize + 1);
            }
        }
        false
    }

    /// Nearest hit of a ray (`skip`: triangles to ignore, or empty).
    pub fn raycast(&self, p: &[Vec3], tris: &[[u32; 3]], o: Vec3, d: Vec3, pad: f32, skip: &[bool]) -> Option<(f32, Vec3, u32)> {
        if !(o.is_finite() && d.is_finite()) || d.length_squared() == 0.0 {
            return None;
        }
        let inv = d.recip();
        let mut best: Option<(f32, u32)> = None;
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let Some(n) = self.nodes.get(i) else { continue };
            let (lo, hi) = (n.lo - Vec3::splat(pad), n.hi + Vec3::splat(pad));
            let t1 = (lo - o) * inv;
            let t2 = (hi - o) * inv;
            let tmin = t1.min(t2).max_element().max(0.0);
            let tmax = t1.max(t2).min_element();
            if tmin > tmax || best.is_some_and(|(b, _)| tmin > b) {
                continue;
            }
            if n.count > 0 {
                for t in self.leaf_tris(i, n) {
                    let Some(tri) = tris.get(t as usize) else { continue };
                    if skip.get(t as usize).copied().unwrap_or(false) {
                        continue;
                    }
                    let g = |i: u32| p.get(i as usize).copied().unwrap_or_default();
                    if let Some(h) = ray_tri(o, d, g(tri[0]), g(tri[1]), g(tri[2]))
                        && best.is_none_or(|(b, _)| h < b)
                    {
                        best = Some((h, t));
                    }
                }
            } else {
                stack.push(n.first as usize);
                stack.push(n.first as usize + 1);
            }
        }
        best.map(|(t, tri)| (t, o + d * t, tri))
    }
}

fn ray_tri(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-14 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}
