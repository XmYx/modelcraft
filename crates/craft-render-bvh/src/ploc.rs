//! Parallel locally-ordered clustering (PLOC), after Meister and Bittner, "Parallel Locally-Ordered
//! Clustering for Bounding Volume Hierarchy Construction" (IEEE TVCG 2018).
//!
//! Primitives are sorted along a Morton curve through their centroids. Each round, every cluster
//! looks for the neighbour that gives the smallest merged box among the `RADIUS` clusters on each
//! side of it in the sorted order, and two clusters that chose each other are merged; the rounds
//! repeat until one cluster is left. Because every step is local and independent, a round is one
//! parallel loop (on a GPU, one dispatch), which is what makes the build suited to trees that are
//! rebuilt every frame. Here it runs on all cores through rayon.
//!
//! The result is a binary [`Bvh`] like the binned build's, with subtrees of up to `MAX_LEAF`
//! primitives collapsed into leaves where the surface area heuristic says so. A tree deeper than
//! the traversal stack allows (which clustering does not rule out) is replaced by the binned
//! build's.

use glam::Vec3A;
use rayon::prelude::*;

use crate::{Aabb, Bvh, MAX_DEPTH, MAX_LEAF, Node, PrimInfo, TRAVERSAL_COST};

/// Clusters searched on each side of a cluster.
pub const RADIUS: usize = 25;

/// 30-bit Morton code of a point in the unit cube.
#[inline]
fn morton3(p: Vec3A) -> u32 {
    fn spread(v: u32) -> u32 {
        let mut v = v & 0x3ff;
        v = (v | (v << 16)) & 0x030000ff;
        v = (v | (v << 8)) & 0x0300f00f;
        v = (v | (v << 4)) & 0x030c30c3;
        (v | (v << 2)) & 0x09249249
    }
    let q = |c: f32| ((c.clamp(0.0, 1.0) * 1023.0) as u32).min(1023);
    spread(q(p.x)) | (spread(q(p.y)) << 1) | (spread(q(p.z)) << 2)
}

#[derive(Clone, Copy)]
struct Tmp {
    bounds: Aabb,
    left: u32,
    right: u32,
    /// Primitives below (1 for a leaf, whose `left` is its sorted position).
    count: u32,
    /// Expected cost of the subtree if kept (SAH) and its height.
    cost: f32,
    height: u32,
}

const LEAF: u32 = u32::MAX;

/// Build over any primitives (those with empty or non-finite bounds are left out).
pub fn build(prims: &[PrimInfo]) -> Bvh {
    let limit = prims.len().min(u32::MAX as usize / 4);
    let mut idx: Vec<u32> =
        (0..limit as u32).filter(|i| prims.get(*i as usize).is_some_and(|p| p.bounds.is_finite() && !p.bounds.is_empty() && p.centroid.is_finite())).collect();
    if idx.len() < 2 {
        return Bvh::build(prims);
    }
    let mut cb = Aabb::EMPTY;
    for i in &idx {
        if let Some(p) = prims.get(*i as usize) {
            cb.grow(p.centroid);
        }
    }
    let extent = (cb.max - cb.min).max(Vec3A::splat(1e-20));
    let code = |i: u32| prims.get(i as usize).map_or(0, |p| morton3((p.centroid - cb.min) / extent));
    idx.par_sort_unstable_by_key(|i| (code(*i), *i));

    let n = idx.len();
    let mut tmp: Vec<Tmp> = Vec::with_capacity(2 * n);
    for (pos, i) in idx.iter().enumerate() {
        let b = prims.get(*i as usize).map_or(Aabb::EMPTY, |p| p.bounds);
        tmp.push(Tmp { bounds: b, left: pos as u32, right: LEAF, count: 1, cost: 1.0, height: 1 });
    }
    // The clusters in sorted order, as indices into `tmp`.
    let mut clusters: Vec<u32> = (0..n as u32).collect();
    let mut rounds = 0;
    while clusters.len() > 1 {
        rounds += 1;
        let m = clusters.len();
        let boxes: Vec<Aabb> = clusters.iter().map(|c| tmp[*c as usize].bounds).collect();
        // The best neighbour of every cluster.
        let nearest: Vec<u32> = (0..m)
            .into_par_iter()
            .map(|i| {
                // Outwards from the cluster, so that among equal merged areas the closest in the
                // sorted order wins (a regular grid has many ties).
                let mut best = (f32::INFINITY, if i > 0 { i - 1 } else { 1 });
                for d in 1..=RADIUS {
                    for j in [i.wrapping_sub(d), i + d] {
                        if j < m {
                            let a = boxes[i].union(boxes[j]).half_area();
                            if a < best.0 {
                                best = (a, j);
                            }
                        }
                    }
                }
                best.1 as u32
            })
            .collect();
        let mut next: Vec<u32> = Vec::with_capacity(m / 2 + 1);
        let mut merged = 0;
        for i in 0..m {
            let j = nearest[i] as usize;
            let mutual = nearest[j] as usize == i;
            if mutual && i > j {
                continue; // consumed by the lower of the pair
            }
            if mutual {
                let (a, b) = (clusters[i], clusters[j]);
                let (ta, tb) = (tmp[a as usize], tmp[b as usize]);
                let bounds = ta.bounds.union(tb.bounds);
                let area = bounds.half_area();
                let rel = |t: &Tmp| if area > 0.0 { t.bounds.half_area() / area } else { 0.5 };
                tmp.push(Tmp {
                    bounds,
                    left: a,
                    right: b,
                    count: ta.count + tb.count,
                    cost: TRAVERSAL_COST + rel(&ta) * ta.cost + rel(&tb) * tb.cost,
                    height: 1 + ta.height.max(tb.height),
                });
                next.push((tmp.len() - 1) as u32);
                merged += 1;
            } else {
                next.push(clusters[i]);
            }
        }
        if merged == 0 || rounds > 10_000 {
            // Cannot happen (the closest pair is always mutual), but never loop forever.
            return Bvh::build(prims);
        }
        clusters = next;
    }
    let root = clusters[0] as usize;
    if tmp[root].height as usize > MAX_DEPTH {
        return Bvh::build(prims);
    }
    // A subtree becomes a leaf when it is small and a leaf is cheaper than its tree.
    let collapse = |t: &Tmp| t.right == LEAF || (t.count as usize <= MAX_LEAF && t.count as f32 <= t.cost);

    // Emit in depth-first order: the left child follows its parent, the right child is `a` nodes on.
    let mut nodes: Vec<Node> = Vec::with_capacity(2 * n);
    let mut order: Vec<u32> = Vec::with_capacity(n);
    enum Step {
        Visit(u32, Option<usize>),
    }
    let mut stack = vec![Step::Visit(root as u32, None)];
    while let Some(Step::Visit(t, patch)) = stack.pop() {
        let me = nodes.len();
        if let Some(parent) = patch {
            nodes[parent].a = (me - parent) as u32;
        }
        let node = tmp[t as usize];
        if collapse(&node) {
            let first = order.len() as u32;
            let mut sub = vec![t];
            while let Some(s) = sub.pop() {
                let x = tmp[s as usize];
                if x.right == LEAF {
                    order.push(idx[x.left as usize]);
                } else {
                    sub.push(x.right);
                    sub.push(x.left);
                }
            }
            nodes.push(Node { min: node.bounds.min.to_array(), a: first, max: node.bounds.max.to_array(), count: order.len() as u32 - first });
        } else {
            nodes.push(Node { min: node.bounds.min.to_array(), a: 0, max: node.bounds.max.to_array(), count: 0 });
            stack.push(Step::Visit(node.right, Some(me)));
            stack.push(Step::Visit(node.left, None));
        }
    }
    Bvh { nodes, prim_order: order }
}
