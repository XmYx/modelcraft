//! A bounding volume hierarchy (BVH) for ray tracing.
//!
//! Built from the published literature only: the surface area heuristic (Goldsmith and Salmon
//! 1987; MacDonald and Booth 1990), binned construction (Wald, "On fast Construction of SAH-based
//! Bounding Volume Hierarchies", 2007), the slab test (Kay and Kajiya 1986) and the Möller–Trumbore
//! triangle test (1997).
//!
//! - [`Bvh`] is a flat array of 32-byte [`Node`]s in depth-first order: the left child of an
//!   interior node is the next node, the right child is `a` nodes further on. The layout is plain
//!   data (`bytemuck::Pod`), so the same bytes go to a GPU buffer.
//! - [`curves::CurveBvh`] holds hair and fibre segments (round cones) in the same kind of tree.
//! - [`TriangleBvh`] owns the triangles in leaf order and answers closest-hit and any-hit queries.
//!
//! Hostile input: non-finite primitives are left out of the tree, depth is capped (so the fixed
//! traversal stack cannot overflow) and nothing panics.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use glam::{Vec3, Vec3A};
use rayon::prelude::*;

pub mod curves;
pub mod ploc;
pub mod wide;
#[cfg(test)]
mod wide_tests;

/// Deepest tree the builder makes; the traversal stack holds one entry per level.
pub const MAX_DEPTH: usize = 60;
const STACK: usize = 64;
const BINS: usize = 16;
/// Largest leaf the heuristic may choose (larger leaves only appear when a split is impossible).
const MAX_LEAF: usize = 4;
/// Cost of visiting an interior node relative to one primitive test.
const TRAVERSAL_COST: f32 = 1.0;
/// Subtrees above this size are built on separate threads.
const PARALLEL_THRESHOLD: usize = 16_384;
/// Nodes above this size bin their primitives on several threads.
const PARALLEL_BINNING: usize = 262_144;

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3A,
    pub max: Vec3A,
}

impl Default for Aabb {
    fn default() -> Self {
        Aabb::EMPTY
    }
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { min: Vec3A::splat(f32::INFINITY), max: Vec3A::splat(f32::NEG_INFINITY) };

    pub fn from_points(points: &[Vec3]) -> Aabb {
        let mut b = Aabb::EMPTY;
        for p in points {
            b.grow(Vec3A::from(*p));
        }
        b
    }
    #[inline]
    pub fn grow(&mut self, p: Vec3A) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    #[inline]
    pub fn union(self, o: Aabb) -> Aabb {
        Aabb { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.min.cmpgt(self.max).any()
    }
    #[inline]
    pub fn is_finite(&self) -> bool {
        self.min.is_finite() && self.max.is_finite()
    }
    /// Half the surface area (zero for an empty box).
    #[inline]
    pub fn half_area(&self) -> f32 {
        if self.is_empty() {
            return 0.0;
        }
        let d = self.max - self.min;
        d.x * d.y + d.y * d.z + d.z * d.x
    }
    #[inline]
    pub fn centroid(&self) -> Vec3A {
        (self.min + self.max) * 0.5
    }
}

/// One node, 32 bytes. Interior (`count == 0`): the left child is the next node and the right
/// child is `a` nodes after this one. Leaf (`count > 0`): primitives `a .. a + count` in leaf order.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Node {
    pub min: [f32; 3],
    pub a: u32,
    pub max: [f32; 3],
    pub count: u32,
}

impl Node {
    fn new(b: &Aabb, a: u32, count: u32) -> Node {
        Node { min: b.min.to_array(), a, max: b.max.to_array(), count }
    }
    #[inline]
    pub fn is_leaf(&self) -> bool {
        self.count > 0
    }
}

/// A ray with a parametric interval.
#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3A,
    pub dir: Vec3A,
    pub tmin: f32,
    pub tmax: f32,
}

impl Ray {
    pub fn new(origin: Vec3, dir: Vec3) -> Ray {
        Ray { origin: origin.into(), dir: dir.into(), tmin: 0.0, tmax: f32::INFINITY }
    }
    pub fn segment(origin: Vec3, dir: Vec3, tmin: f32, tmax: f32) -> Ray {
        Ray { origin: origin.into(), dir: dir.into(), tmin, tmax }
    }
    #[inline]
    pub fn at(&self, t: f32) -> Vec3A {
        self.origin + self.dir * t
    }
}

/// What a build needs to know about one primitive.
#[derive(Clone, Copy, Debug)]
pub struct PrimInfo {
    pub bounds: Aabb,
    pub centroid: Vec3A,
}

/// Numbers describing a tree (for tests, logs and benchmarks).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub nodes: usize,
    pub leaves: usize,
    pub max_depth: usize,
    pub max_leaf: usize,
    /// Expected traversal cost under the surface area heuristic, relative to the root's area.
    pub sah_cost: f32,
}

/// The hierarchy: nodes plus the primitive order its leaves refer to.
#[derive(Clone, Debug, Default)]
pub struct Bvh {
    pub nodes: Vec<Node>,
    /// Leaf slot → index of the primitive in the build input.
    pub prim_order: Vec<u32>,
}

#[derive(Clone, Copy)]
struct Bin {
    bounds: Aabb,
    count: u32,
}

const EMPTY_BIN: Bin = Bin { bounds: Aabb::EMPTY, count: 0 };
type Bins = [[Bin; BINS]; 3];

#[inline]
fn bin_of(c: f32, lo: f32, scale: f32) -> usize {
    (((c - lo) * scale) as usize).min(BINS - 1)
}

fn bin_prims(prims: &[PrimInfo], idx: &[u32], lo: Vec3A, scale: Vec3A) -> Bins {
    let add = |mut bins: Bins, i: &u32| {
        if let Some(p) = prims.get(*i as usize) {
            let c = p.centroid;
            for (axis, row) in bins.iter_mut().enumerate() {
                let b = &mut row[bin_of(c[axis], lo[axis], scale[axis])];
                b.bounds = b.bounds.union(p.bounds);
                b.count += 1;
            }
        }
        bins
    };
    if idx.len() >= PARALLEL_BINNING {
        idx.par_chunks(PARALLEL_BINNING / 8).map(|chunk| chunk.iter().fold([[EMPTY_BIN; BINS]; 3], add)).reduce(
            || [[EMPTY_BIN; BINS]; 3],
            |mut a, b| {
                for axis in 0..3 {
                    for k in 0..BINS {
                        a[axis][k].bounds = a[axis][k].bounds.union(b[axis][k].bounds);
                        a[axis][k].count += b[axis][k].count;
                    }
                }
                a
            },
        )
    } else {
        idx.iter().fold([[EMPTY_BIN; BINS]; 3], add)
    }
}

enum Split {
    Leaf,
    At(usize),
}

/// Choose and apply a split of `idx`; primitives left of the returned position go left.
fn split(prims: &[PrimInfo], idx: &mut [u32], bounds: &Aabb, depth: usize) -> Split {
    let n = idx.len();
    if n <= 1 || depth >= MAX_DEPTH {
        return Split::Leaf;
    }
    let mut cb = Aabb::EMPTY;
    for i in idx.iter() {
        if let Some(p) = prims.get(*i as usize) {
            cb.grow(p.centroid);
        }
    }
    let extent = cb.max - cb.min;
    let scale = Vec3A::new(
        if extent.x > 0.0 { BINS as f32 / extent.x } else { 0.0 },
        if extent.y > 0.0 { BINS as f32 / extent.y } else { 0.0 },
        if extent.z > 0.0 { BINS as f32 / extent.z } else { 0.0 },
    );
    let mut best: Option<(f32, usize, usize)> = None;
    if extent.max_element() > 0.0 && scale.is_finite() {
        let bins = bin_prims(prims, idx, cb.min, scale);
        for (axis, row) in bins.iter().enumerate() {
            if extent[axis] <= 0.0 {
                continue;
            }
            // Sweep from the right, then from the left.
            let mut right_area = [0.0f32; BINS];
            let mut right_count = [0u32; BINS];
            let mut acc = Aabb::EMPTY;
            let mut cnt = 0;
            for k in (1..BINS).rev() {
                acc = acc.union(row[k].bounds);
                cnt += row[k].count;
                right_area[k] = acc.half_area();
                right_count[k] = cnt;
            }
            let mut acc = Aabb::EMPTY;
            let mut cnt = 0;
            for k in 1..BINS {
                acc = acc.union(row[k - 1].bounds);
                cnt += row[k - 1].count;
                if cnt == 0 || right_count[k] == 0 {
                    continue;
                }
                let cost = acc.half_area() * cnt as f32 + right_area[k] * right_count[k] as f32;
                if cost.is_finite() && best.is_none_or(|(c, _, _)| cost < c) {
                    best = Some((cost, axis, k));
                }
            }
        }
    }
    let area = bounds.half_area();
    if let Some((cost, axis, k)) = best {
        let split_cost = if area > 0.0 { TRAVERSAL_COST + cost / area } else { f32::INFINITY };
        if n <= MAX_LEAF && split_cost >= n as f32 {
            return Split::Leaf;
        }
        let (lo, sc) = (cb.min[axis], scale[axis]);
        let mut left = 0;
        for j in 0..n {
            let goes_left = prims.get(idx[j] as usize).is_some_and(|p| bin_of(p.centroid[axis], lo, sc) < k);
            if goes_left {
                idx.swap(left, j);
                left += 1;
            }
        }
        if left > 0 && left < n {
            return Split::At(left);
        }
    }
    if n <= MAX_LEAF {
        return Split::Leaf;
    }
    // Centroids coincide (or the numbers are degenerate): halve by count so depth stays logarithmic.
    Split::At(n / 2)
}

fn bounds_of(prims: &[PrimInfo], idx: &[u32]) -> Aabb {
    let mut b = Aabb::EMPTY;
    for i in idx {
        if let Some(p) = prims.get(*i as usize) {
            b = b.union(p.bounds);
        }
    }
    b
}

fn build_serial(nodes: &mut Vec<Node>, prims: &[PrimInfo], idx: &mut [u32], first: u32, depth: usize) {
    let bounds = bounds_of(prims, idx);
    let me = nodes.len();
    match split(prims, idx, &bounds, depth) {
        Split::Leaf => nodes.push(Node::new(&bounds, first, idx.len() as u32)),
        Split::At(mid) => {
            nodes.push(Node::new(&bounds, 0, 0));
            let (l, r) = idx.split_at_mut(mid);
            build_serial(nodes, prims, l, first, depth + 1);
            let right = nodes.len();
            build_serial(nodes, prims, r, first + mid as u32, depth + 1);
            nodes[me].a = (right - me) as u32;
        }
    }
}

fn build_parallel(prims: &[PrimInfo], idx: &mut [u32], first: u32, depth: usize) -> Vec<Node> {
    if idx.len() <= PARALLEL_THRESHOLD {
        let mut nodes = Vec::with_capacity(idx.len());
        build_serial(&mut nodes, prims, idx, first, depth);
        return nodes;
    }
    let bounds = bounds_of(prims, idx);
    match split(prims, idx, &bounds, depth) {
        Split::Leaf => vec![Node::new(&bounds, first, idx.len() as u32)],
        Split::At(mid) => {
            let (l, r) = idx.split_at_mut(mid);
            let (ln, rn) = rayon::join(|| build_parallel(prims, l, first, depth + 1), || build_parallel(prims, r, first + mid as u32, depth + 1));
            let mut nodes = Vec::with_capacity(1 + ln.len() + rn.len());
            nodes.push(Node::new(&bounds, 1 + ln.len() as u32, 0));
            nodes.extend(ln);
            nodes.extend(rn);
            nodes
        }
    }
}

impl Bvh {
    /// Build over any primitives. Primitives whose bounds are empty or not finite are left out.
    pub fn build(prims: &[PrimInfo]) -> Bvh {
        let limit = prims.len().min(u32::MAX as usize / 2);
        let mut idx: Vec<u32> = (0..limit as u32)
            .filter(|i| prims.get(*i as usize).is_some_and(|p| p.bounds.is_finite() && !p.bounds.is_empty() && p.centroid.is_finite()))
            .collect();
        if idx.is_empty() {
            return Bvh::default();
        }
        let nodes = build_parallel(prims, &mut idx, 0, 0);
        Bvh { nodes, prim_order: idx }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn bounds(&self) -> Aabb {
        self.nodes.first().map_or(Aabb::EMPTY, |n| Aabb { min: Vec3A::from_array(n.min), max: Vec3A::from_array(n.max) })
    }

    /// Visit the leaves a ray reaches, nearest box first. `leaf(first, count, tmax)` tests its
    /// primitives, may shorten `tmax`, and returns `true` to stop the traversal.
    #[inline]
    pub fn traverse(&self, ray: &Ray, mut leaf: impl FnMut(u32, u32, &mut f32) -> bool) {
        let Some(root) = self.nodes.first() else { return };
        let inv = ray.dir.recip();
        let mut tmax = ray.tmax;
        let entry = |n: &Node, tmax: f32| -> f32 {
            let t0 = (Vec3A::from_array(n.min) - ray.origin) * inv;
            let t1 = (Vec3A::from_array(n.max) - ray.origin) * inv;
            let near = t0.min(t1).max_element().max(ray.tmin);
            let far = t0.max(t1).min_element().min(tmax);
            if near <= far { near } else { f32::INFINITY }
        };
        if entry(root, tmax) == f32::INFINITY {
            return;
        }
        let mut stack = [(0u32, 0f32); STACK];
        let mut sp = 0usize;
        let mut at = 0usize;
        loop {
            let Some(n) = self.nodes.get(at) else { return };
            if n.count > 0 {
                if leaf(n.a, n.count, &mut tmax) {
                    return;
                }
            } else {
                let (l, r) = (at + 1, at + n.a as usize);
                let (Some(ln), Some(rn)) = (self.nodes.get(l), self.nodes.get(r)) else { return };
                let (tl, tr) = (entry(ln, tmax), entry(rn, tmax));
                let (near, far, tfar) = if tl <= tr { (l, r, tr) } else { (r, l, tl) };
                if tl.min(tr) < f32::INFINITY {
                    if tfar < f32::INFINITY && sp < STACK {
                        stack[sp] = (far as u32, tfar);
                        sp += 1;
                    }
                    at = near;
                    continue;
                }
            }
            // Pop the next subtree that is still in range.
            loop {
                if sp == 0 {
                    return;
                }
                sp -= 1;
                let (node, t) = stack[sp];
                if t <= tmax {
                    at = node as usize;
                    break;
                }
            }
        }
    }

    pub fn stats(&self) -> Stats {
        let mut s = Stats { nodes: self.nodes.len(), ..Stats::default() };
        let root_area = self.bounds().half_area();
        let mut stack = vec![(0usize, 1usize)];
        while let Some((at, depth)) = stack.pop() {
            let Some(n) = self.nodes.get(at) else { continue };
            let area = Aabb { min: Vec3A::from_array(n.min), max: Vec3A::from_array(n.max) }.half_area();
            let rel = if root_area > 0.0 { area / root_area } else { 0.0 };
            s.max_depth = s.max_depth.max(depth);
            if n.count > 0 {
                s.leaves += 1;
                s.max_leaf = s.max_leaf.max(n.count as usize);
                s.sah_cost += rel * n.count as f32;
            } else {
                s.sah_cost += rel * TRAVERSAL_COST;
                if depth <= MAX_DEPTH + 1 {
                    stack.push((at + 1, depth + 1));
                    stack.push((at + n.a as usize, depth + 1));
                }
            }
        }
        s
    }
}

/// A triangle prepared for the Möller–Trumbore test: one corner and two edges.
#[derive(Clone, Copy, Debug)]
pub struct TriAccel {
    pub p0: Vec3A,
    pub e1: Vec3A,
    pub e2: Vec3A,
}

/// A ray–triangle intersection. `u` and `v` are the barycentric weights of corners 1 and 2.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f32,
    pub u: f32,
    pub v: f32,
    /// Index of the triangle in the build input.
    pub prim: u32,
}

/// How the binary tree is built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Builder {
    /// Top-down, splitting by the surface area heuristic over 16 bins (see [`Bvh::build`]).
    #[default]
    BinnedSah,
    /// Bottom-up parallel locally-ordered clustering (see [`ploc`]).
    Ploc,
}

/// A BVH over triangles, with the triangles stored in leaf order.
#[derive(Clone, Debug, Default)]
pub struct TriangleBvh {
    pub bvh: Bvh,
    /// Triangles in leaf order (`bvh.prim_order` maps a slot back to the input index).
    pub tris: Vec<TriAccel>,
    /// The same hierarchy collapsed to 8-wide compressed nodes, when [`TriangleBvh::enable_wide`]
    /// made it; queries use it in place of `bvh`. Its leaves refer to the same triangle slots.
    pub wide: Option<wide::WideBvh>,
}

#[inline]
fn hit_triangle(tri: &TriAccel, ray: &Ray, tmax: f32) -> Option<(f32, f32, f32)> {
    let p = ray.dir.cross(tri.e2);
    let det = tri.e1.dot(p);
    if det == 0.0 {
        return None;
    }
    let inv = 1.0 / det;
    let s = ray.origin - tri.p0;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(tri.e1);
    let v = ray.dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = tri.e2.dot(q) * inv;
    (t > ray.tmin && t < tmax).then_some((t, u, v))
}

impl TriangleBvh {
    /// Build from triangles given as three corners each.
    pub fn build(triangles: &[[Vec3; 3]]) -> TriangleBvh {
        TriangleBvh::build_with(triangles, Builder::BinnedSah)
    }

    /// Build with the chosen tree builder.
    pub fn build_with(triangles: &[[Vec3; 3]], builder: Builder) -> TriangleBvh {
        let prims: Vec<PrimInfo> = triangles
            .par_iter()
            .map(|t| {
                let b = Aabb::from_points(t);
                PrimInfo { bounds: b, centroid: b.centroid() }
            })
            .collect();
        let bvh = match builder {
            Builder::BinnedSah => Bvh::build(&prims),
            Builder::Ploc => ploc::build(&prims),
        };
        let tris = bvh
            .prim_order
            .iter()
            .map(|i| {
                let t = triangles.get(*i as usize).copied().unwrap_or([Vec3::ZERO; 3]);
                let p0 = Vec3A::from(t[0]);
                TriAccel { p0, e1: Vec3A::from(t[1]) - p0, e2: Vec3A::from(t[2]) - p0 }
            })
            .collect();
        TriangleBvh { bvh, tris, wide: None }
    }

    /// Collapse the tree to 8-wide compressed nodes and answer queries with them from now on.
    /// `false` (and nothing changes) when the tree is too deep or has a leaf too large for it.
    pub fn enable_wide(&mut self) -> bool {
        match wide::WideBvh::from_binary(&self.bvh) {
            Some(w) => {
                self.wide = Some(w);
                true
            }
            None => false,
        }
    }

    /// Visit the leaves a ray reaches (see [`Bvh::traverse`]) through whichever tree is active.
    #[inline]
    fn traverse(&self, ray: &Ray, leaf: impl FnMut(u32, u32, &mut f32) -> bool) {
        match &self.wide {
            Some(w) => w.traverse(ray, leaf),
            None => self.bvh.traverse(ray, leaf),
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// The closest hit accepted by `accept` (use it for cut-outs; it sees every candidate that is
    /// nearer than the current closest one).
    #[inline]
    pub fn intersect_with(&self, ray: &Ray, mut accept: impl FnMut(&Hit) -> bool) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        self.traverse(ray, |first, count, tmax| {
            let (first, count) = (first as usize, count as usize);
            for slot in first..first + count {
                let Some(tri) = self.tris.get(slot) else { break };
                if let Some((t, u, v)) = hit_triangle(tri, ray, *tmax) {
                    let hit = Hit { t, u, v, prim: self.bvh.prim_order.get(slot).copied().unwrap_or(0) };
                    if accept(&hit) {
                        *tmax = t;
                        best = Some(hit);
                    }
                }
            }
            false
        });
        best
    }

    /// The closest hit along the ray.
    #[inline]
    pub fn intersect(&self, ray: &Ray) -> Option<Hit> {
        self.intersect_with(ray, |_| true)
    }

    /// Whether anything accepted by `blocks` lies inside the ray's interval.
    #[inline]
    pub fn occluded_with(&self, ray: &Ray, mut blocks: impl FnMut(&Hit) -> bool) -> bool {
        let mut found = false;
        self.traverse(ray, |first, count, tmax| {
            let (first, count) = (first as usize, count as usize);
            for slot in first..first + count {
                let Some(tri) = self.tris.get(slot) else { break };
                if let Some((t, u, v)) = hit_triangle(tri, ray, *tmax) {
                    let hit = Hit { t, u, v, prim: self.bvh.prim_order.get(slot).copied().unwrap_or(0) };
                    if blocks(&hit) {
                        found = true;
                        return true;
                    }
                }
            }
            false
        });
        found
    }

    #[inline]
    pub fn occluded(&self, ray: &Ray) -> bool {
        self.occluded_with(ray, |_| true)
    }

    /// The triangles in leaf order as `[p0, e1, e2]` rows of four floats (for a GPU buffer).
    pub fn gpu_triangles(&self) -> Vec<[[f32; 4]; 3]> {
        self.tris.iter().map(|t| [t.p0.extend(0.0).to_array(), t.e1.extend(0.0).to_array(), t.e2.extend(0.0).to_array()]).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn brute(tris: &[[Vec3; 3]], ray: &Ray) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        for (i, t) in tris.iter().enumerate() {
            let p0 = Vec3A::from(t[0]);
            let acc = TriAccel { p0, e1: Vec3A::from(t[1]) - p0, e2: Vec3A::from(t[2]) - p0 };
            if let Some((t, u, v)) = hit_triangle(&acc, ray, best.map_or(ray.tmax, |h| h.t)) {
                best = Some(Hit { t, u, v, prim: i as u32 });
            }
        }
        best
    }

    fn v3() -> impl Strategy<Value = Vec3> {
        (-10.0f32..10.0, -10.0f32..10.0, -10.0f32..10.0).prop_map(|(x, y, z)| Vec3::new(x, y, z))
    }

    fn check_structure(bvh: &Bvh, n: usize) {
        // Every primitive appears in exactly one leaf and children lie inside their parents.
        let mut seen = vec![0u32; n];
        let mut stack = vec![0usize];
        while let Some(at) = stack.pop() {
            let node = bvh.nodes[at];
            if node.count > 0 {
                for s in node.a..node.a + node.count {
                    seen[bvh.prim_order[s as usize] as usize] += 1;
                }
            } else {
                for c in [at + 1, at + node.a as usize] {
                    let ch = bvh.nodes[c];
                    for k in 0..3 {
                        assert!(ch.min[k] >= node.min[k] && ch.max[k] <= node.max[k]);
                    }
                    stack.push(c);
                }
            }
        }
        assert!(seen.iter().all(|c| *c == 1));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn matches_brute_force(
            tris in prop::collection::vec((v3(), v3(), v3()), 1..200),
            rays in prop::collection::vec((v3(), v3()), 1..40),
        ) {
            let tris: Vec<[Vec3; 3]> = tris.into_iter().map(|(a, b, c)| [a, a + b * 0.2, a + c * 0.2]).collect();
            let bvh = TriangleBvh::build(&tris);
            check_structure(&bvh.bvh, tris.len());
            for (o, d) in rays {
                if d.length() < 1e-3 { continue; }
                let ray = Ray::new(o, d.normalize());
                let (a, b) = (bvh.intersect(&ray), brute(&tris, &ray));
                prop_assert_eq!(a.map(|h| h.t), b.map(|h| h.t));
                prop_assert_eq!(bvh.occluded(&ray), b.is_some());
            }
        }
    }

    #[test]
    fn empty_and_hostile_input() {
        let empty = TriangleBvh::build(&[]);
        assert!(empty.intersect(&Ray::new(Vec3::ZERO, Vec3::X)).is_none());
        assert!(!empty.occluded(&Ray::new(Vec3::ZERO, Vec3::X)));
        let bad = [
            [Vec3::splat(f32::NAN); 3],
            [Vec3::ZERO, Vec3::splat(f32::INFINITY), Vec3::X],
            [Vec3::new(1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, -1.0), Vec3::new(1.0, 0.0, 1.0)],
        ];
        let bvh = TriangleBvh::build(&bad);
        assert_eq!(bvh.triangle_count(), 1);
        let hit = bvh.intersect(&Ray::new(Vec3::ZERO, Vec3::X)).unwrap();
        assert_eq!(hit.prim, 2);
        assert!((hit.t - 1.0).abs() < 1e-6);
        // A ray with a NaN direction finds nothing and does not hang.
        assert!(bvh.intersect(&Ray::new(Vec3::ZERO, Vec3::splat(f32::NAN))).is_none());
    }

    #[test]
    fn coincident_primitives_stay_shallow() {
        // 10 000 identical triangles cannot be separated by any plane.
        let tris = vec![[Vec3::ZERO, Vec3::X, Vec3::Y]; 10_000];
        let bvh = TriangleBvh::build(&tris);
        let s = bvh.bvh.stats();
        assert!(s.max_depth <= 16, "{s:?}");
        assert!(s.max_leaf <= MAX_LEAF);
        assert!(bvh.intersect(&Ray::new(Vec3::new(0.2, 0.2, 1.0), -Vec3::Z)).is_some());
    }

    #[test]
    fn large_grid_builds_in_parallel_and_is_well_formed() {
        let n = 200;
        let mut tris = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let p = Vec3::new(x as f32, y as f32, ((x * 7 + y * 13) % 5) as f32 * 0.1);
                tris.push([p, p + Vec3::X, p + Vec3::Y]);
                tris.push([p + Vec3::X, p + Vec3::X + Vec3::Y, p + Vec3::Y]);
            }
        }
        let bvh = TriangleBvh::build(&tris);
        check_structure(&bvh.bvh, tris.len());
        let s = bvh.bvh.stats();
        assert!(s.max_depth <= MAX_DEPTH + 1 && s.sah_cost < 120.0, "{s:?}");
        let ray = Ray::new(Vec3::new(50.3, 70.2, 10.0), -Vec3::Z);
        assert_eq!(bvh.intersect(&ray).map(|h| h.t), brute(&tris, &ray).map(|h| h.t));
        // An accept filter that rejects everything finds nothing; one that rejects the first
        // surface finds none behind it here.
        assert!(bvh.intersect_with(&ray, |_| false).is_none());
    }

    /// A timing, not a check: `cargo test --release -p craft-render-bvh -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark"]
    fn bench_build_two_million_triangles() {
        let n = 1000;
        let mut tris = Vec::with_capacity(n * n * 2);
        for y in 0..n {
            for x in 0..n {
                let h = |x: usize, y: usize| ((x as f32 * 0.05).sin() + (y as f32 * 0.07).cos()) * 3.0;
                let p = |x: usize, y: usize| Vec3::new(x as f32 * 0.1, h(x, y), y as f32 * 0.1);
                tris.push([p(x, y), p(x + 1, y), p(x, y + 1)]);
                tris.push([p(x + 1, y), p(x + 1, y + 1), p(x, y + 1)]);
            }
        }
        let t = std::time::Instant::now();
        let bvh = TriangleBvh::build(&tris);
        let build = t.elapsed();
        let s = bvh.bvh.stats();
        let t = std::time::Instant::now();
        let rays = 2_000_000;
        let mut hits = 0;
        for i in 0..rays {
            let (a, b) = ((i % 1000) as f32 * 0.1 + 0.03, (i / 1000 % 1000) as f32 * 0.1 + 0.05);
            let ray = Ray::new(Vec3::new(a, 20.0, b), Vec3::new(0.3, -1.0, 0.2).normalize());
            hits += usize::from(bvh.intersect(&ray).is_some());
        }
        let trace = t.elapsed();
        eprintln!(
            "{} triangles: build {:.0} ms, {} nodes, depth {}, SAH cost {:.1}; {} rays on one thread in {:.0} ms ({:.2} M rays/s), {} hits",
            tris.len(),
            build.as_secs_f64() * 1e3,
            s.nodes,
            s.max_depth,
            s.sah_cost,
            rays,
            trace.as_secs_f64() * 1e3,
            rays as f64 / trace.as_secs_f64() / 1e6,
            hits
        );
    }
}
