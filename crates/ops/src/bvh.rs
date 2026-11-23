//! A bounding-volume hierarchy over triangles, for closest-point and ray queries (the shrink-wrap
//! deformer). Built by splitting at the median of the longest axis; queries walk it with a stack,
//! so they never recurse. Triangles with a point that is not finite are left out.

use glam::Vec3;

use crate::PolyMesh;

/// Most triangles one hierarchy holds.
pub const MAX_TRIANGLES: usize = 8_000_000;
const LEAF: usize = 4;

#[derive(Clone, Copy, Debug)]
struct Node {
    lo: Vec3,
    hi: Vec3,
    /// A leaf holds `count` triangles from `first` in the order list; an inner node's children
    /// are the nodes `first` and `first + 1`.
    first: u32,
    count: u32,
}

/// A point on the surface found by [`TriBvh::closest`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub tri: u32,
    pub point: Vec3,
    pub distance: f32,
}

#[derive(Clone, Debug, Default)]
pub struct TriBvh {
    tris: Vec<[Vec3; 3]>,
    order: Vec<u32>,
    nodes: Vec<Node>,
}

/// The point of the triangle `a`, `b`, `c` nearest to `p`: the projection when it falls inside,
/// otherwise the nearest point of an edge.
pub fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let n = (b - a).cross(c - a);
    let area2 = n.length_squared();
    if area2 > 0.0 {
        let w0 = (c - b).cross(p - b).dot(n) / area2;
        let w1 = (a - c).cross(p - c).dot(n) / area2;
        let w2 = (b - a).cross(p - a).dot(n) / area2;
        if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
            return a * w0 + b * w1 + c * w2;
        }
    }
    let edge = |u: Vec3, v: Vec3| {
        let d = v - u;
        let len2 = d.length_squared();
        let t = if len2 > 0.0 { ((p - u).dot(d) / len2).clamp(0.0, 1.0) } else { 0.0 };
        u + d * t
    };
    [edge(a, b), edge(b, c), edge(c, a)].into_iter().min_by(|x, y| (*x - p).length_squared().total_cmp(&(*y - p).length_squared())).unwrap_or(a)
}

impl TriBvh {
    /// The hierarchy of a polygon mesh (fan-triangulated), after moving its points by `to`.
    pub fn from_mesh(m: &PolyMesh, to: glam::Mat4) -> TriBvh {
        let (tris, _) = m.triangulate();
        let tris: Vec<[Vec3; 3]> = tris
            .into_iter()
            .take(MAX_TRIANGLES)
            .map(|t| [to.transform_point3(m.position(t[0])), to.transform_point3(m.position(t[1])), to.transform_point3(m.position(t[2]))])
            .collect();
        TriBvh::build(tris)
    }

    pub fn build(tris: Vec<[Vec3; 3]>) -> TriBvh {
        let mut tris = tris;
        tris.truncate(MAX_TRIANGLES);
        let mut order: Vec<u32> = (0..tris.len() as u32).filter(|i| tris[*i as usize].iter().all(|p| p.is_finite())).collect();
        let mut nodes = Vec::new();
        if !order.is_empty() {
            let len = order.len();
            let centroid = |i: u32| (tris[i as usize][0] + tris[i as usize][1] + tris[i as usize][2]) / 3.0;
            // Iterative build: (node index, range start, range end).
            nodes.push(Node { lo: Vec3::ZERO, hi: Vec3::ZERO, first: 0, count: 0 });
            let mut work = vec![(0usize, 0usize, len)];
            while let Some((ni, s, e)) = work.pop() {
                let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                let (mut clo, mut chi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                for i in &order[s..e] {
                    for p in &tris[*i as usize] {
                        lo = lo.min(*p);
                        hi = hi.max(*p);
                    }
                    let c = centroid(*i);
                    clo = clo.min(c);
                    chi = chi.max(c);
                }
                if e - s <= LEAF {
                    nodes[ni] = Node { lo, hi, first: s as u32, count: (e - s) as u32 };
                    continue;
                }
                let ext = chi - clo;
                let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
                order[s..e].sort_by(|a, b| centroid(*a)[axis].total_cmp(&centroid(*b)[axis]));
                let mid = (s + e) / 2;
                let left = nodes.len();
                nodes.push(Node { lo: Vec3::ZERO, hi: Vec3::ZERO, first: 0, count: 0 });
                nodes.push(Node { lo: Vec3::ZERO, hi: Vec3::ZERO, first: 0, count: 0 });
                nodes[ni] = Node { lo, hi, first: left as u32, count: 0 };
                work.push((left, s, mid));
                work.push((left + 1, mid, e));
            }
        }
        TriBvh { tris, order, nodes }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn triangle(&self, i: u32) -> Option<[Vec3; 3]> {
        self.tris.get(i as usize).copied()
    }

    /// The unit normal of triangle `i` (zero for a missing or flat triangle).
    pub fn normal(&self, i: u32) -> Vec3 {
        self.triangle(i).map_or(Vec3::ZERO, |[a, b, c]| (b - a).cross(c - a).normalize_or_zero())
    }

    fn box_distance2(n: &Node, p: Vec3) -> f32 {
        let d = (n.lo - p).max(p - n.hi).max(Vec3::ZERO);
        d.length_squared()
    }

    /// The nearest point of the surface to `p`.
    pub fn closest(&self, p: Vec3) -> Option<Hit> {
        if self.nodes.is_empty() || !p.is_finite() {
            return None;
        }
        let mut best: Option<(f32, u32, Vec3)> = None;
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let n = self.nodes[ni];
            if best.is_some_and(|(d2, _, _)| Self::box_distance2(&n, p) > d2) {
                continue;
            }
            if n.count > 0 {
                for i in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    let [a, b, c] = self.tris[*i as usize];
                    let q = closest_on_triangle(p, a, b, c);
                    let d2 = (q - p).length_squared();
                    if best.is_none_or(|(b2, _, _)| d2 < b2) {
                        best = Some((d2, *i, q));
                    }
                }
            } else {
                let (l, r) = (n.first as usize, n.first as usize + 1);
                // Nearer child last, so it is popped first.
                let (dl, dr) = (Self::box_distance2(&self.nodes[l], p), Self::box_distance2(&self.nodes[r], p));
                if dl < dr {
                    stack.push(r);
                    stack.push(l);
                } else {
                    stack.push(l);
                    stack.push(r);
                }
            }
        }
        best.map(|(d2, tri, point)| Hit { tri, point, distance: d2.sqrt() })
    }

    /// The nearest crossing of the ray `o + t d` with `0 <= t <= max_t`: `(t, triangle)`.
    /// Triangle test of Möller and Trumbore, "Fast, Minimum Storage Ray-Triangle Intersection",
    /// Journal of Graphics Tools 2(1), 1997.
    pub fn ray(&self, o: Vec3, d: Vec3, max_t: f32) -> Option<(f32, u32)> {
        if self.nodes.is_empty() || !o.is_finite() || !d.is_finite() || d == Vec3::ZERO {
            return None;
        }
        let inv = Vec3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut best: Option<(f32, u32)> = None;
        let mut limit = max_t;
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let n = self.nodes[ni];
            // Slab test.
            let (t0, t1) = ((n.lo - o) * inv, (n.hi - o) * inv);
            let (tn, tf) = (t0.min(t1), t0.max(t1));
            let (enter, exit) = (tn.max_element().max(0.0), tf.min_element().min(limit));
            if enter > exit && !(enter.is_nan() || exit.is_nan()) {
                continue;
            }
            if n.count > 0 {
                for i in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    let [a, b, c] = self.tris[*i as usize];
                    let (e1, e2) = (b - a, c - a);
                    let pv = d.cross(e2);
                    let det = e1.dot(pv);
                    if det.abs() < 1e-12 {
                        continue;
                    }
                    let s = o - a;
                    let u = s.dot(pv) / det;
                    let q = s.cross(e1);
                    let v = d.dot(q) / det;
                    let t = e2.dot(q) / det;
                    if u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t >= 0.0 && t <= limit {
                        limit = t;
                        best = Some((t, *i));
                    }
                }
            } else {
                stack.push(n.first as usize);
                stack.push(n.first as usize + 1);
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use proptest::prelude::*;

    fn sphere_bvh() -> (PolyMesh, TriBvh) {
        let m = Primitive::Sphere { radius: 1.0, segments: 24, rings: 12 }.build().unwrap();
        let b = TriBvh::from_mesh(&m, glam::Mat4::IDENTITY);
        (m, b)
    }

    #[test]
    fn closest_point_on_a_triangle_covers_face_edge_and_corner() {
        let (a, b, c) = (Vec3::ZERO, Vec3::X, Vec3::Y);
        assert_eq!(closest_on_triangle(Vec3::new(0.2, 0.2, 3.0), a, b, c), Vec3::new(0.2, 0.2, 0.0));
        assert_eq!(closest_on_triangle(Vec3::new(0.5, -2.0, 1.0), a, b, c), Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(closest_on_triangle(Vec3::new(-3.0, -3.0, 0.0), a, b, c), Vec3::ZERO);
        // A flat (zero-area) triangle still answers.
        assert!(closest_on_triangle(Vec3::ONE, a, a, a).is_finite());
    }

    #[test]
    fn closest_matches_brute_force() {
        let (m, bvh) = sphere_bvh();
        let (tris, _) = m.triangulate();
        for i in 0..200 {
            let p = Vec3::new((i as f32 * 0.37).sin() * 2.5, (i as f32 * 0.91).cos() * 2.5, (i as f32 * 0.13).sin() * 2.0);
            let hit = bvh.closest(p).unwrap();
            let brute = tris
                .iter()
                .map(|t| (closest_on_triangle(p, m.position(t[0]), m.position(t[1]), m.position(t[2])) - p).length())
                .fold(f32::MAX, f32::min);
            assert!((hit.distance - brute).abs() < 1e-5, "{} vs {brute}", hit.distance);
        }
    }

    #[test]
    fn rays_find_the_surface_and_miss_outside() {
        let (_, bvh) = sphere_bvh();
        let (t, _) = bvh.ray(Vec3::new(0.0, 5.0, 0.0), -Vec3::Y, 100.0).unwrap();
        assert!((t - 4.0).abs() < 0.02, "{t}");
        assert!(bvh.ray(Vec3::new(5.0, 5.0, 0.0), -Vec3::Y, 100.0).is_none());
        assert!(bvh.ray(Vec3::new(0.0, 5.0, 0.0), -Vec3::Y, 1.0).is_none(), "too short");
        assert!(bvh.ray(Vec3::ZERO, Vec3::ZERO, 10.0).is_none());
    }

    #[test]
    fn empty_and_hostile_input_answer_none() {
        let empty = TriBvh::build(vec![]);
        assert!(empty.is_empty() && empty.closest(Vec3::ZERO).is_none() && empty.ray(Vec3::ZERO, Vec3::X, 1.0).is_none());
        let bad = TriBvh::build(vec![[Vec3::NAN, Vec3::X, Vec3::Y]]);
        assert!(bad.closest(Vec3::ZERO).is_none());
        let (_, b) = sphere_bvh();
        assert!(b.closest(Vec3::NAN).is_none());
    }

    proptest! {
        #[test]
        fn closest_never_beats_a_brute_force_search(px in -4.0f32..4.0, py in -4.0f32..4.0, pz in -4.0f32..4.0) {
            let (m, bvh) = sphere_bvh();
            let p = Vec3::new(px, py, pz);
            let (tris, _) = m.triangulate();
            let brute = tris.iter().map(|t| (closest_on_triangle(p, m.position(t[0]), m.position(t[1]), m.position(t[2])) - p).length()).fold(f32::MAX, f32::min);
            let hit = bvh.closest(p).unwrap();
            prop_assert!((hit.distance - brute).abs() < 1e-4);
        }
    }
}
