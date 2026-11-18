//! Hair and fibre curves in the BVH.
//!
//! A curve is a polyline of points with a radius at each point. Each segment is a round cone: the
//! frustum between the two end circles plus a sphere at each end, so a bend has no crack on its
//! outside. A ray meets it by solving the quadratic of the cone about the segment's axis, and by the
//! two sphere quadratics; the nearest root inside the segment wins. (Tubes and swept spheres are
//! the usual primitives for fibre rendering, see Reshetov and Luebke, "Phantom Ray-Hair
//! Intersector", HPG 2018, for the survey; this is the plain analytic version for the CPU.)
//!
//! The segments go into the same BVH as triangles ([`crate::Bvh`]), with boxes grown by the radii.

use glam::{Vec3, Vec3A};
use rayon::prelude::*;

use crate::{Aabb, Bvh, PrimInfo, Ray};

/// Most segments one curve set may hold.
pub const MAX_SEGMENTS: usize = 30_000_000;

/// One segment: a round cone from `p0` (radius `r0`) to `p1` (radius `r1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub p0: Vec3A,
    pub p1: Vec3A,
    pub r0: f32,
    pub r1: f32,
}

/// A ray–segment intersection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CurveHit {
    pub t: f32,
    /// Index of the segment in the build input.
    pub segment: u32,
    /// Index of the segment in leaf order (into [`CurveBvh::segments`]).
    pub slot: u32,
}

/// What a shader needs about a hit: where on the axis the hit is, the axis direction, the
/// outward normal (perpendicular to the axis) and the radius there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitFrame {
    pub axis_point: Vec3A,
    pub tangent: Vec3A,
    pub normal: Vec3A,
    pub radius: f32,
}

impl Segment {
    pub fn bounds(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        for (p, r) in [(self.p0, self.r0), (self.p1, self.r1)] {
            b.grow(p - Vec3A::splat(r));
            b.grow(p + Vec3A::splat(r));
        }
        b
    }

    fn is_valid(&self) -> bool {
        self.p0.is_finite() && self.p1.is_finite() && self.r0.is_finite() && self.r1.is_finite() && self.r0 >= 0.0 && self.r1 >= 0.0
    }

    /// The nearest hit of the ray inside `(ray.tmin, tmax)`.
    #[inline]
    pub fn intersect(&self, ray: &Ray, tmax: f32) -> Option<f32> {
        let (o, d) = (ray.origin.as_dvec3(), ray.dir.as_dvec3());
        let (p0, p1) = (self.p0.as_dvec3(), self.p1.as_dvec3());
        let (r0, r1) = (f64::from(self.r0), f64::from(self.r1));
        let (tmin, tmax) = (f64::from(ray.tmin), f64::from(tmax));
        let mut best = f64::INFINITY;
        let mut take = |t: f64| {
            if t > tmin && t < tmax && t < best {
                best = t;
            }
        };
        // The end spheres.
        for (c, r) in [(p0, r0), (p1, r1)] {
            if r <= 0.0 {
                continue;
            }
            let w = o - c;
            let (b, cc) = (w.dot(d), w.dot(w) - r * r);
            let a = d.dot(d);
            let disc = b * b - a * cc;
            if disc >= 0.0 && a > 0.0 {
                let s = disc.sqrt();
                take((-b - s) / a);
                take((-b + s) / a);
            }
        }
        // The cone between them.
        let ba = p1 - p0;
        let l2 = ba.dot(ba);
        if l2 > 1e-24 {
            let w = o - p0;
            let (s0, s1) = (w.dot(ba) / l2, d.dot(ba) / l2);
            let dr = r1 - r0;
            let (a_vec, b_vec) = (w - ba * s0, d - ba * s1);
            let (ra, rb) = (r0 + dr * s0, dr * s1);
            let qa = b_vec.dot(b_vec) - rb * rb;
            let qb = a_vec.dot(b_vec) - ra * rb;
            let qc = a_vec.dot(a_vec) - ra * ra;
            let disc = qb * qb - qa * qc;
            if qa.abs() > 1e-18 && disc >= 0.0 {
                let s = disc.sqrt();
                for t in [(-qb - s) / qa, (-qb + s) / qa] {
                    let along = s0 + t * s1;
                    if (0.0..=1.0).contains(&along) && ra + t * rb >= 0.0 {
                        take(t);
                    }
                }
            }
        }
        best.is_finite().then_some(best as f32)
    }

    /// The shading frame at a point on the surface.
    pub fn frame_at(&self, x: Vec3A) -> HitFrame {
        let ba = self.p1 - self.p0;
        let l2 = ba.length_squared();
        let s = if l2 > 1e-24 { ((x - self.p0).dot(ba) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let axis_point = self.p0 + ba * s;
        let tangent = ba.normalize_or(Vec3A::Y);
        let radial = x - axis_point;
        // Perpendicular to the axis, so a hit on an end sphere still has a normal in the plane the
        // fibre model works in.
        let perp = radial - tangent * radial.dot(tangent);
        let normal = perp.normalize_or(tangent.any_orthonormal_vector());
        HitFrame { axis_point, tangent, normal, radius: self.r0 + (self.r1 - self.r0) * s }
    }
}

/// A BVH over curve segments, with the segments stored in leaf order.
#[derive(Clone, Debug, Default)]
pub struct CurveBvh {
    pub bvh: Bvh,
    pub segments: Vec<Segment>,
}

impl CurveBvh {
    /// Build over segments. Segments with non-finite numbers or negative radii are left out.
    pub fn build(input: &[Segment]) -> CurveBvh {
        let limit = input.len().min(MAX_SEGMENTS);
        let prims: Vec<PrimInfo> = input
            .get(..limit)
            .unwrap_or_default()
            .par_iter()
            .map(|s| {
                if s.is_valid() {
                    let b = s.bounds();
                    PrimInfo { bounds: b, centroid: b.centroid() }
                } else {
                    PrimInfo { bounds: Aabb::EMPTY, centroid: Vec3A::ZERO }
                }
            })
            .collect();
        let bvh = Bvh::build(&prims);
        let segments =
            bvh.prim_order.iter().map(|i| input.get(*i as usize).copied().unwrap_or(Segment { p0: Vec3A::ZERO, p1: Vec3A::ZERO, r0: 0.0, r1: 0.0 })).collect();
        CurveBvh { bvh, segments }
    }

    /// Build from strands: `points` holds `points_per_strand` points per strand back to back and
    /// `radii` one radius per point.
    pub fn from_strands(points: &[Vec3], radii: &[f32], points_per_strand: usize) -> CurveBvh {
        if points_per_strand < 2 || radii.len() != points.len() {
            return CurveBvh::default();
        }
        let mut segs = Vec::with_capacity(points.len().saturating_sub(points.len() / points_per_strand));
        for (strand, rs) in points.chunks_exact(points_per_strand).zip(radii.chunks_exact(points_per_strand)) {
            for (p, r) in strand.windows(2).zip(rs.windows(2)) {
                segs.push(Segment { p0: p[0].into(), p1: p[1].into(), r0: r[0], r1: r[1] });
            }
        }
        CurveBvh::build(&segs)
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub fn bounds(&self) -> Aabb {
        self.bvh.bounds()
    }

    /// The nearest hit along the ray.
    #[inline]
    pub fn intersect(&self, ray: &Ray) -> Option<CurveHit> {
        let mut best: Option<CurveHit> = None;
        self.bvh.traverse(ray, |first, count, tmax| {
            for slot in first as usize..(first + count) as usize {
                let Some(seg) = self.segments.get(slot) else { break };
                if let Some(t) = seg.intersect(ray, *tmax) {
                    *tmax = t;
                    best = Some(CurveHit { t, segment: self.bvh.prim_order.get(slot).copied().unwrap_or(0), slot: slot as u32 });
                }
            }
            false
        });
        best
    }

    /// Whether anything lies inside the ray's interval.
    #[inline]
    pub fn occluded(&self, ray: &Ray) -> bool {
        let mut found = false;
        self.bvh.traverse(ray, |first, count, tmax| {
            for slot in first as usize..(first + count) as usize {
                let Some(seg) = self.segments.get(slot) else { break };
                if seg.intersect(ray, *tmax).is_some() {
                    found = true;
                    return true;
                }
            }
            false
        });
        found
    }

    /// The segment a hit refers to.
    pub fn segment_of(&self, hit: &CurveHit) -> Option<&Segment> {
        self.segments.get(hit.slot as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn seg(p0: Vec3, p1: Vec3, r0: f32, r1: f32) -> Segment {
        Segment { p0: p0.into(), p1: p1.into(), r0, r1 }
    }

    #[test]
    fn a_ray_meets_a_cylinder_where_geometry_says() {
        // A tube of radius 0.1 along z from -1 to 1; a ray along x at height 0.05 hits at the circle.
        let s = seg(Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, 0.0, 1.0), 0.1, 0.1);
        let ray = Ray::new(Vec3::new(-2.0, 0.05, 0.2), Vec3::X);
        let t = s.intersect(&ray, f32::INFINITY).unwrap();
        assert!((t - (2.0 - (0.01f32 - 0.0025).sqrt())).abs() < 1e-5, "{t}");
        let f = s.frame_at(ray.at(t));
        assert!((f.normal - Vec3A::new(-(0.0075f32.sqrt()), 0.05, 0.0) / 0.1).length() < 1e-4);
        assert!((f.tangent - Vec3A::Z).length() < 1e-6 && (f.radius - 0.1).abs() < 1e-7);
        // Missing by a hair, and pointing away.
        assert!(s.intersect(&Ray::new(Vec3::new(-2.0, 0.11, 0.0), Vec3::X), f32::INFINITY).is_none());
        assert!(s.intersect(&Ray::new(Vec3::new(2.0, 0.0, 0.0), Vec3::X), f32::INFINITY).is_none());
        // A ray along the axis meets the end sphere.
        let end = s.intersect(&Ray::new(Vec3::new(0.0, 0.0, -3.0), Vec3::Z), f32::INFINITY).unwrap();
        assert!((end - 1.9).abs() < 1e-5, "{end}");
        // tmax cuts it off.
        assert!(s.intersect(&ray, 1.5).is_none());
    }

    #[test]
    fn a_taper_narrows_the_tube() {
        let s = seg(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0), 0.2, 0.0);
        // Halfway along the radius is 0.1: a ray at height 0.15 misses there, at 0.05 it hits.
        assert!(s.intersect(&Ray::new(Vec3::new(-1.0, 0.15, 0.5), Vec3::X), f32::INFINITY).is_none());
        let t = s.intersect(&Ray::new(Vec3::new(-1.0, 0.05, 0.5), Vec3::X), f32::INFINITY).unwrap();
        assert!(t > 0.8 && t < 1.0);
    }

    #[test]
    fn a_bend_has_no_crack() {
        // Two segments meeting at a right angle: rays through the outside of the corner must hit.
        let curves = CurveBvh::from_strands(&[Vec3::new(-1.0, 0.0, 0.0), Vec3::ZERO, Vec3::new(0.0, 1.0, 0.0)], &[0.1; 3], 3);
        assert_eq!(curves.segment_count(), 2);
        let mut misses = 0;
        for k in 0..400 {
            let a = k as f32 / 400.0 * std::f32::consts::TAU;
            // Aimed at points on the sphere of radius 0.09 around the corner, from far away.
            let target = Vec3::new(a.cos(), a.sin(), 0.0) * 0.09;
            let o = target + Vec3::new(a.cos(), a.sin(), 0.7).normalize() * 3.0;
            let ray = Ray::new(o, (target - o).normalize());
            if curves.intersect(&ray).is_none() {
                misses += 1;
            }
        }
        assert_eq!(misses, 0);
    }

    fn v3() -> impl Strategy<Value = Vec3> {
        (-5.0f32..5.0, -5.0f32..5.0, -5.0f32..5.0).prop_map(|(x, y, z)| Vec3::new(x, y, z))
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn the_bvh_matches_brute_force(
            segs in prop::collection::vec((v3(), v3(), 0.0f32..0.3, 0.0f32..0.3), 1..120),
            rays in prop::collection::vec((v3(), v3()), 1..30),
        ) {
            let segs: Vec<Segment> = segs.into_iter().map(|(a, d, r0, r1)| seg(a, a + d * 0.3, r0, r1)).collect();
            let bvh = CurveBvh::build(&segs);
            prop_assert_eq!(bvh.segment_count(), segs.len());
            for (o, d) in rays {
                if d.length() < 1e-3 { continue; }
                let ray = Ray::new(o, d.normalize());
                let brute = segs.iter().filter_map(|s| s.intersect(&ray, f32::INFINITY)).fold(f32::INFINITY, f32::min);
                let got = bvh.intersect(&ray).map(|h| h.t);
                prop_assert_eq!(got.unwrap_or(f32::INFINITY), brute);
                prop_assert_eq!(bvh.occluded(&ray), brute.is_finite());
            }
        }
    }

    #[test]
    fn hostile_input_is_left_out() {
        assert!(CurveBvh::build(&[]).intersect(&Ray::new(Vec3::ZERO, Vec3::X)).is_none());
        let bad = [
            seg(Vec3::NAN, Vec3::X, 0.1, 0.1),
            seg(Vec3::ZERO, Vec3::X, -1.0, 0.1),
            seg(Vec3::ZERO, Vec3::splat(f32::INFINITY), 0.1, 0.1),
            seg(Vec3::ZERO, Vec3::ZERO, 0.0, 0.0),
            seg(Vec3::new(0.0, 0.0, 5.0), Vec3::new(1.0, 0.0, 5.0), 0.2, 0.2),
        ];
        let c = CurveBvh::build(&bad);
        assert_eq!(c.intersect(&Ray::new(Vec3::new(0.5, -1.0, 5.0), Vec3::Y)).map(|h| h.segment), Some(4));
        assert!(CurveBvh::from_strands(&[Vec3::ZERO; 4], &[0.1; 3], 2).segment_count() == 0, "radii and points must agree");
        assert_eq!(CurveBvh::from_strands(&[Vec3::ZERO; 4], &[0.1; 4], 1).segment_count(), 0);
        // A point-like ray direction does not panic.
        let _ = c.intersect(&Ray::new(Vec3::ZERO, Vec3::ZERO));
        let _ = c.intersect(&Ray::new(Vec3::NAN, Vec3::X));
    }
}
