//! A compressed 8-wide BVH after Ylitie, Karras and Laine, "Efficient Incoherent Ray Traversal on
//! GPUs Through Compressed Wide BVHs" (HPG 2017).
//!
//! A node holds up to eight children. Their boxes are stored as bytes relative to the node's own
//! box (the corner `p` and one power-of-two scale per axis), rounded outwards so that a quantised box
//! always contains the real one: a ray can only visit too much, never miss. One node is 112 bytes
//! against 32 for each of the seven binary nodes it replaces, and a ray tests eight boxes at once
//! (written as lane-parallel loops over arrays, which the compiler turns into vector code) and
//! visits the hit children nearest first.
//!
//! The tree is made from a binary [`Bvh`] by collapsing: starting from a node, the interior child
//! with the largest surface area is opened until there are eight children or only leaves. This is
//! simpler than the paper's dynamic-programming collapse (which picks the opening by expected
//! cost), so a node is not always the best eight-way split. Leaves keep the binary tree's
//! primitive ranges, so triangle and attribute order is shared with it.

use crate::{Aabb, Bvh, Ray};
use glam::Vec3A;

pub const WIDTH: usize = 8;
/// Largest leaf a child can describe (7 bits).
pub const MAX_WIDE_LEAF: u32 = 127;
/// Words in a [`WideNode`].
pub const NODE_WORDS: usize = 28;
/// Pending subtrees on the traversal stack: at most seven per level of a tree whose depth is bounded.
const STACK: usize = 512;
/// Deepest wide tree accepted: the GPU stack holds 128 pending subtrees and a level adds up to seven.
pub const MAX_WIDE_DEPTH: u32 = 18;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WideNode {
    /// Corner of the node's box.
    pub p: [f32; 3],
    /// Exponents (signed bytes in the low three) of the scale of each axis.
    pub exps: u32,
    /// Per child: 0 empty, 1 interior, `0x80 | count` a leaf.
    pub meta: [u8; WIDTH],
    /// Child boxes in steps of the scale from `p` (x, y, z rows).
    pub qlo: [[u8; WIDTH]; 3],
    pub qhi: [[u8; WIDTH]; 3],
    /// Interior: index of the child node. Leaf: first primitive.
    pub data: [u32; WIDTH],
    /// Pads the node to seven four-word rows (what a GPU loads in one go).
    pub pad: [u32; 2],
}

const _: () = assert!(std::mem::size_of::<WideNode>() == NODE_WORDS * 4);

#[inline]
fn pow2(e: i32) -> f32 {
    f32::from_bits(((e.clamp(-126, 127) + 127) as u32) << 23)
}

impl WideNode {
    #[inline]
    fn scale(&self) -> [f32; 3] {
        [pow2(i32::from(self.exps as u8 as i8)), pow2(i32::from((self.exps >> 8) as u8 as i8)), pow2(i32::from((self.exps >> 16) as u8 as i8))]
    }

    /// The box of child `k` as the traversal sees it.
    #[inline]
    pub fn child_box(&self, k: usize) -> Aabb {
        let s = self.scale();
        let at = |q: &[[u8; WIDTH]; 3]| {
            Vec3A::new(self.p[0] + f32::from(q[0][k]) * s[0], self.p[1] + f32::from(q[1][k]) * s[1], self.p[2] + f32::from(q[2][k]) * s[2])
        };
        Aabb { min: at(&self.qlo), max: at(&self.qhi) }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WideBvh {
    pub nodes: Vec<WideNode>,
}

/// The smallest scale exponent for which 255 steps cover `extent`.
fn exponent(extent: f32) -> i32 {
    if extent.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) || !extent.is_finite() {
        return -126;
    }
    let mut e = ((extent / 255.0).log2().ceil() as i32).clamp(-126, 127);
    while e < 127 && 255.0 * pow2(e) < extent {
        e += 1;
    }
    e
}

impl WideBvh {
    /// Collapse a binary tree. `None` when a leaf is larger than a child can describe.
    pub fn from_binary(bvh: &Bvh) -> Option<WideBvh> {
        if bvh.nodes.is_empty() {
            return Some(WideBvh::default());
        }
        let mut out = WideBvh { nodes: Vec::with_capacity(bvh.nodes.len() / 4 + 1) };
        // Pending (binary node, parent wide node and child slot, depth) whose wide node is not made yet.
        let mut work: Vec<(usize, Option<(usize, usize)>, u32)> = vec![(0, None, 1)];
        while let Some((at, parent, depth)) = work.pop() {
            if depth > MAX_WIDE_DEPTH {
                return None;
            }
            let me = out.nodes.len();
            if let Some((pi, slot)) = parent {
                out.nodes[pi].data[slot] = me as u32;
            }
            let root = *bvh.nodes.get(at)?;
            let area = |i: usize| bvh.nodes.get(i).map_or(0.0, |n| Aabb { min: Vec3A::from_array(n.min), max: Vec3A::from_array(n.max) }.half_area());
            let mut kids: Vec<usize> = if root.count > 0 { vec![at] } else { vec![at + 1, at + root.a as usize] };
            while kids.len() < WIDTH {
                let open = kids
                    .iter()
                    .enumerate()
                    .filter(|(_, k)| bvh.nodes.get(**k).is_some_and(|n| n.count == 0))
                    .max_by(|a, b| area(*a.1).total_cmp(&area(*b.1)))
                    .map(|(i, _)| i);
                let Some(i) = open else { break };
                let k = kids[i];
                let n = *bvh.nodes.get(k)?;
                kids[i] = k + 1;
                kids.push(k + n.a as usize);
            }
            let (lo, hi) = (Vec3A::from_array(root.min), Vec3A::from_array(root.max));
            let exps = [exponent(hi.x - lo.x), exponent(hi.y - lo.y), exponent(hi.z - lo.z)];
            let mut node = WideNode {
                p: lo.to_array(),
                exps: (exps[0] as i8 as u8 as u32) | ((exps[1] as i8 as u8 as u32) << 8) | ((exps[2] as i8 as u8 as u32) << 16),
                meta: [0; WIDTH],
                qlo: [[255; WIDTH]; 3],
                qhi: [[0; WIDTH]; 3],
                data: [0; WIDTH],
                pad: [0; 2],
            };
            let scale = [pow2(exps[0]), pow2(exps[1]), pow2(exps[2])];
            for (slot, k) in kids.iter().enumerate() {
                let n = *bvh.nodes.get(*k)?;
                for a in 0..3 {
                    let (nlo, nhi) = (n.min[a], n.max[a]);
                    let mut q = (((nlo - node.p[a]) / scale[a]).floor().max(0.0) as u32).min(255) as u8;
                    while q > 0 && node.p[a] + f32::from(q) * scale[a] > nlo {
                        q -= 1;
                    }
                    node.qlo[a][slot] = q;
                    let mut q = (((nhi - node.p[a]) / scale[a]).ceil().max(0.0) as u32).min(255) as u8;
                    while q < 255 && node.p[a] + f32::from(q) * scale[a] < nhi {
                        q += 1;
                    }
                    node.qhi[a][slot] = q;
                }
                if n.count > 0 {
                    if n.count > MAX_WIDE_LEAF {
                        return None;
                    }
                    node.meta[slot] = 0x80 | n.count as u8;
                    node.data[slot] = n.a;
                } else {
                    node.meta[slot] = 1;
                }
            }
            out.nodes.push(node);
            // Children are made after their parent, last slot first so the first slot is on top.
            for (slot, k) in kids.iter().enumerate().rev() {
                if bvh.nodes.get(*k).is_some_and(|n| n.count == 0) {
                    work.push((*k, Some((me, slot)), depth + 1));
                }
            }
        }
        Some(out)
    }

    /// Visit the leaves a ray reaches, the nearest boxes first, with the same contract as
    /// [`Bvh::traverse`]: `leaf(first, count, tmax)` may shorten `tmax` and returns `true` to stop.
    #[inline]
    pub fn traverse(&self, ray: &Ray, mut leaf: impl FnMut(u32, u32, &mut f32) -> bool) {
        if self.nodes.is_empty() {
            return;
        }
        let inv = ray.dir.recip();
        let (o, tmin) = (ray.origin, ray.tmin);
        let mut tmax = ray.tmax;
        let mut stack = [(0u32, 0f32); STACK];
        let mut sp = 0usize;
        let mut at = 0usize;
        loop {
            let Some(n) = self.nodes.get(at) else { return };
            let s = n.scale();
            // All eight boxes at once.
            let mut near = [0f32; WIDTH];
            let mut far = [0f32; WIDTH];
            for k in 0..WIDTH {
                let mut tn = tmin;
                let mut tf = tmax;
                for a in 0..3 {
                    let lo = n.p[a] + f32::from(n.qlo[a][k]) * s[a];
                    let hi = n.p[a] + f32::from(n.qhi[a][k]) * s[a];
                    let (t0, t1) = ((lo - o[a]) * inv[a], (hi - o[a]) * inv[a]);
                    tn = tn.max(t0.min(t1));
                    tf = tf.min(t0.max(t1));
                }
                near[k] = tn;
                far[k] = tf;
            }
            // Hit children by distance, nearest last (so it is popped first).
            let mut order = [0usize; WIDTH];
            let mut count = 0usize;
            for k in 0..WIDTH {
                if n.meta[k] != 0 && near[k] <= far[k] {
                    let mut j = count;
                    while j > 0 && near[order[j - 1]] < near[k] {
                        order[j] = order[j - 1];
                        j -= 1;
                    }
                    order[j] = k;
                    count += 1;
                }
            }
            // Walk from the nearest (the end of `order`): leaves are tested at once, interior
            // children wait on the stack.
            for j in (0..count).rev() {
                let k = order[j];
                if near[k] > tmax {
                    continue;
                }
                let m = n.meta[k];
                if m & 0x80 != 0 && leaf(n.data[k], u32::from(m & 0x7f), &mut tmax) {
                    return;
                }
            }
            // Interior children go on the stack farthest first, so the nearest is popped first.
            for j in 0..count {
                let k = order[j];
                if n.meta[k] == 1 && near[k] <= tmax && sp < STACK {
                    stack[sp] = (n.data[k], near[k]);
                    sp += 1;
                }
            }
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
}
