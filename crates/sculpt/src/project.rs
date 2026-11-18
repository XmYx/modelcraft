//! Project detail: move the vertices of one mesh onto the surface of another, so a clean mesh
//! (a retopology, a lower detail level stepped up again) takes the shape of a detailed one.
//!
//! Each vertex first looks along its own normal, both ways, for the other surface; that keeps
//! vertices from sliding sideways across it. A vertex that finds nothing that way within the
//! distance takes the closest point of the other surface instead, and one with nothing in reach
//! stays where it is.

use glam::Vec3;

use crate::{SculptMesh, StrokeUndo};

/// Closest point to `p` on the triangle `a b c` (Ericson, Real-Time Collision Detection, 5.1.5).
fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

impl SculptMesh {
    /// The closest point of the surface to `p` within `reach`, if there is one.
    pub fn closest_point(&self, p: Vec3, reach: f32) -> Option<Vec3> {
        if !(p.is_finite() && reach.is_finite() && reach > 0.0) {
            return None;
        }
        let mut best: Option<(f32, Vec3)> = None;
        self.bvh.visit_sphere(p, reach + self.drift, |t| {
            if self.hidden.get(t as usize).copied().unwrap_or(false) {
                return;
            }
            let Some(tri) = self.tris.get(t as usize) else { return };
            let g = |i: u32| self.positions.get(i as usize).copied().unwrap_or_default();
            let q = closest_on_triangle(p, g(tri[0]), g(tri[1]), g(tri[2]));
            let d = q.distance_squared(p);
            if d <= reach * reach && best.is_none_or(|(b, _)| d < b) {
                best = Some((d, q));
            }
        });
        best.map(|(_, q)| q)
    }

    /// Move every visible vertex onto `source` (already in this mesh's space), no further than
    /// `reach`, as far as its mask allows. See the module documentation for how a vertex finds
    /// its place.
    pub fn project_onto(&mut self, source: &SculptMesh, reach: f32) -> StrokeUndo {
        let mut u = StrokeUndo::default();
        if self.stroke.is_some() || !(reach.is_finite() && reach > 0.0) {
            return u;
        }
        for v in 0..self.positions.len() {
            let (Some(p), Some(m)) = (self.positions.get(v).copied(), self.mask.get(v).copied()) else { continue };
            let w = 1.0 - m.clamp(0.0, 1.0);
            if w <= 0.0 || self.vertex_hidden(v as u32) {
                continue;
            }
            let n = self.normals.get(v).copied().unwrap_or(Vec3::Y);
            let along = [n, -n]
                .into_iter()
                .filter_map(|d| source.raycast(p, d).filter(|h| h.0 <= reach).map(|h| (h.0, h.1)))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|h| h.1);
            let Some(target) = along.or_else(|| source.closest_point(p, reach)) else { continue };
            let q = p + (target - p) * w;
            if !q.is_finite() || q == p {
                continue;
            }
            if let Some(x) = self.positions.get_mut(v) {
                *x = q;
            }
            u.indices.push(v as u32);
            u.before.push(p);
            u.after.push(q);
            u.mask_before.push(m);
            u.mask_after.push(m);
        }
        self.update_normals(&u.indices);
        self.refit_around(&u.indices);
        u
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use modelcraft_mesh::PolyMesh;
    use modelcraft_ops::primitives::Primitive;

    fn ball(seg: u32, radius: f32) -> PolyMesh {
        Primitive::RoundCube { radius, segments: seg }.build().unwrap()
    }

    /// A ball with a bump field: the radius at a direction.
    fn bumpy(d: Vec3) -> f32 {
        1.0 + 0.06 * (d.x * 9.0).sin() * (d.y * 7.0).cos() + 0.04 * (d.z * 11.0).sin()
    }

    #[test]
    fn closest_points_on_a_triangle_and_a_ball() {
        let (a, b, c) = (Vec3::ZERO, Vec3::X, Vec3::Y);
        assert_eq!(closest_on_triangle(Vec3::new(0.25, 0.25, 3.0), a, b, c), Vec3::new(0.25, 0.25, 0.0));
        assert_eq!(closest_on_triangle(Vec3::new(-1.0, -1.0, 0.0), a, b, c), a);
        assert_eq!(closest_on_triangle(Vec3::new(2.0, -1.0, 0.0), a, b, c), b);
        assert_eq!(closest_on_triangle(Vec3::new(0.5, -1.0, 0.0), a, b, c), Vec3::new(0.5, 0.0, 0.0));
        assert!(closest_on_triangle(Vec3::new(1.0, 1.0, 0.0), a, b, c).distance(Vec3::new(0.5, 0.5, 0.0)) < 1e-6);
        let s = SculptMesh::new(&ball(24, 1.0), None, None);
        let q = s.closest_point(Vec3::new(0.0, 3.0, 0.0), 5.0).unwrap();
        assert!((q.length() - 1.0).abs() < 0.01 && q.y > 0.99);
        assert!(s.closest_point(Vec3::new(0.0, 3.0, 0.0), 1.0).is_none(), "out of reach");
        assert!(s.closest_point(Vec3::splat(f32::NAN), 1.0).is_none());
    }

    #[test]
    fn a_plain_ball_takes_the_shape_of_a_detailed_one() {
        let mut detailed = ball(48, 1.0);
        detailed.map_positions(|_, p| p.normalize() * bumpy(p.normalize()));
        let source = SculptMesh::new(&detailed, None, None);
        // The target has other vertices than the source: a different segment count.
        let mut target = SculptMesh::new(&ball(31, 1.0), None, None);
        let before = target.positions.clone();
        let u = target.project_onto(&source, 0.5);
        assert_eq!(u.indices.len(), target.vertex_count(), "every vertex found the surface");
        let worst = target.positions.iter().map(|p| (p.length() - bumpy(p.normalize())).abs()).fold(0.0f32, f32::max);
        // The source is flat between its own vertices, so the match is as good as its facets.
        assert!(worst < 0.004, "{worst}");
        // Vertices went along their normals: no sideways sliding to speak of.
        let slide = target.positions.iter().zip(&before).map(|(p, q)| p.normalize().distance(q.normalize())).fold(0.0f32, f32::max);
        assert!(slide < 0.02, "{slide}");
        target.apply_undo(&u, false);
        assert_eq!(target.positions, before);
    }

    #[test]
    fn reach_mask_and_hidden_faces_limit_the_projection() {
        let m = ball(12, 1.0);
        let source = SculptMesh::new(&ball(12, 1.3), None, None);
        let mut target = SculptMesh::new(&m, None, None);
        assert!(target.project_onto(&source, 0.2).is_empty(), "the other surface is 0.3 away");
        assert!(target.project_onto(&source, f32::NAN).is_empty());
        // Half masked, a quarter hidden.
        target.mask_edit(|_, p, _| if p.x > 0.0 { 1.0 } else { 0.0 });
        let hide: Vec<bool> = (0..m.face_count() as u32).map(|f| m.face_center(f).y > 0.0 && m.face_center(f).x < -0.2).collect();
        assert!(target.set_face_hidden(&hide));
        let before = target.positions.clone();
        let u = target.project_onto(&source, 0.4);
        assert!(!u.indices.is_empty());
        for (v, p) in target.positions.iter().enumerate() {
            let moved = *p != before[v];
            if before[v].x > 0.0 {
                assert!(!moved, "masked");
            }
            if moved {
                assert!((p.length() - 1.3).abs() < 0.02);
            }
        }
        let hidden_only = (0..target.vertex_count() as u32).filter(|v| target.vertex_hidden(*v)).count();
        assert!(hidden_only > 0);
        assert!((0..target.vertex_count() as u32).filter(|v| target.vertex_hidden(*v)).all(|v| target.positions[v as usize] == before[v as usize]));
    }
}
