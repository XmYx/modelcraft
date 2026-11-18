//! Snapping to geometry: the vertex, edge point or surface point of other meshes that is nearest
//! the cursor. The work is in screen space through a projection the caller supplies (so this
//! module knows nothing of cameras), bounded by a budget of elements visited, and the answer is a
//! world-space point the caller moves something to.

use glam::{Mat4, Vec2, Vec3};
use modelcraft_mesh::PolyMesh;

use crate::pick::{self, Ray};

/// Most vertices plus edges looked at in one query; a bigger scene is sampled evenly, so a snap
/// stays interactive and the cost is the same for a huge mesh as for a large one.
pub const BUDGET: usize = 400_000;

/// What can be snapped to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SnapTargets {
    pub vertex: bool,
    pub edge: bool,
    pub surface: bool,
}

impl SnapTargets {
    pub fn any(self) -> bool {
        self.vertex || self.edge || self.surface
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SnapKind {
    Vertex,
    Edge,
    Surface,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapHit {
    pub point: Vec3,
    pub kind: SnapKind,
    /// Screen distance to the cursor (0 for a surface hit under it).
    pub pixels: f32,
}

/// A mesh with its world matrix.
pub type Placed<'a> = (Mat4, &'a PolyMesh);

/// The best snap point for `cursor` among `meshes`, within `radius` pixels for vertices and edges
/// (the nearest wins, vertices before edges before surfaces when equally near). `project` takes a
/// world point to pixels (`None` behind the camera); `ray` is the cursor's ray into the scene for
/// the surface.
pub fn snap(meshes: &[Placed<'_>], targets: SnapTargets, project: &dyn Fn(Vec3) -> Option<Vec2>, cursor: Vec2, radius: f32, ray: &Ray) -> Option<SnapHit> {
    let mut best: Option<SnapHit> = None;
    let mut consider = |h: SnapHit| {
        // Prefer the more specific kind when the distances are within a pixel.
        let rank = |k: SnapKind| match k {
            SnapKind::Vertex => 0,
            SnapKind::Edge => 1,
            SnapKind::Surface => 2,
        };
        if best.is_none_or(|b| h.pixels + 1.0 < b.pixels || (h.pixels < b.pixels + 1.0 && rank(h.kind) < rank(b.kind))) {
            best = Some(h);
        }
    };
    let total: usize = meshes.iter().map(|(_, m)| m.vertex_count() * usize::from(targets.vertex) + m.edge_count() * usize::from(targets.edge)).sum();
    // Visit every `stride`-th element when over the budget.
    let stride = total.div_ceil(BUDGET).max(1);
    for (w, m) in meshes {
        if targets.vertex {
            for v in (0..m.vertex_count() as u32).step_by(stride) {
                let p = w.transform_point3(m.position(v));
                if let Some(s) = project(p) {
                    let d = s.distance(cursor);
                    if d <= radius {
                        consider(SnapHit { point: p, kind: SnapKind::Vertex, pixels: d });
                    }
                }
            }
        }
        if targets.edge {
            for e in (0..m.edge_count() as u32).step_by(stride) {
                let (a, b) = m.edge_vertices(e);
                let (pa, pb) = (w.transform_point3(m.position(a)), w.transform_point3(m.position(b)));
                let (Some(sa), Some(sb)) = (project(pa), project(pb)) else { continue };
                let ab = sb - sa;
                let t = ((cursor - sa).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
                let d = (sa + ab * t).distance(cursor);
                if d <= radius {
                    consider(SnapHit { point: pa.lerp(pb, t), kind: SnapKind::Edge, pixels: d });
                }
            }
        }
        if targets.surface
            && let Some((point, _)) = surface_in(*w, m, ray)
        {
            consider(SnapHit { point, kind: SnapKind::Surface, pixels: radius });
        }
    }
    // A surface hit is only the answer when no vertex or edge is near: it is as far as the radius.
    best
}

fn surface_in(w: Mat4, m: &PolyMesh, ray: &Ray) -> Option<(Vec3, f32)> {
    let det = w.determinant();
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let local = ray.transformed(w.inverse());
    let h = pick::pick_face(m, &local)?;
    let p = w.transform_point3(h.point);
    Some((p, (p - ray.origin).length()))
}

/// The nearest surface point along a world ray, among `meshes`.
pub fn surface_point(meshes: &[Placed<'_>], ray: &Ray) -> Option<Vec3> {
    meshes.iter().filter_map(|(w, m)| surface_in(*w, m, ray)).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use modelcraft_ops::primitives::Primitive;

    use super::*;

    fn cube() -> PolyMesh {
        Primitive::Cube { size: Vec3::splat(2.0), segments: 1 }.build().unwrap()
    }

    /// Orthographic top-down "camera": x, z to pixels at 100 px per unit.
    fn top(p: Vec3) -> Option<Vec2> {
        Some(Vec2::new(p.x * 100.0, p.z * 100.0))
    }

    fn down_ray(x: f32, z: f32) -> Ray {
        Ray { origin: Vec3::new(x, 10.0, z), dir: -Vec3::Y }
    }

    const ALL: SnapTargets = SnapTargets { vertex: true, edge: true, surface: true };

    #[test]
    fn a_vertex_wins_near_a_corner_an_edge_near_a_side_and_the_surface_elsewhere() {
        let m = cube();
        let meshes = [(Mat4::IDENTITY, &m)];
        let hit = |x: f32, z: f32, t: SnapTargets| snap(&meshes, t, &top, Vec2::new(x * 100.0, z * 100.0), 12.0, &down_ray(x, z));
        // Near the corner (1, 1): the vertex, though the edge and surface are there too.
        let h = hit(0.95, 1.0, ALL).unwrap();
        assert_eq!(h.kind, SnapKind::Vertex);
        assert!((h.point.x - 1.0).abs() < 1e-5 && (h.point.z - 1.0).abs() < 1e-5);
        // Along the middle of the +x side: the edge, at the cursor's height along it.
        let h = hit(1.05, 0.3, SnapTargets { vertex: true, edge: true, surface: false }).unwrap();
        assert_eq!(h.kind, SnapKind::Edge);
        assert!((h.point.x - 1.0).abs() < 1e-4 && (h.point.z - 0.3).abs() < 0.02, "{:?}", h.point);
        // Over the middle of the top: only the surface answers, at height 1.
        let h = hit(0.2, 0.2, ALL).unwrap();
        assert_eq!(h.kind, SnapKind::Surface);
        assert!((h.point.y - 1.0).abs() < 1e-4);
        // Off the cube altogether, or with the targets off, nothing.
        assert!(hit(5.0, 5.0, ALL).is_none());
        assert!(hit(0.95, 1.0, SnapTargets::default()).is_none());
    }

    #[test]
    fn the_world_matrix_places_the_candidates_and_the_nearest_object_wins_the_surface() {
        let m = cube();
        let near = Mat4::from_translation(Vec3::new(0.0, 3.0, 0.0));
        let far = Mat4::from_translation(Vec3::new(0.0, -3.0, 0.0));
        let meshes = [(far, &m), (near, &m)];
        let p = surface_point(&meshes, &down_ray(0.0, 0.0)).unwrap();
        assert!((p.y - 4.0).abs() < 1e-4, "{p:?}: the top of the nearer cube");
        let h = snap(&meshes, SnapTargets { vertex: true, ..Default::default() }, &top, Vec2::new(100.0, 100.0), 5.0, &down_ray(1.0, 1.0)).unwrap();
        assert!((h.point.x - 1.0).abs() < 1e-4);
        // A zero-scale object cannot be hit.
        assert!(surface_point(&[(Mat4::from_scale(Vec3::ZERO), &m)], &down_ray(0.0, 0.0)).is_none());
    }

    #[test]
    fn a_huge_scene_is_sampled_not_scanned() {
        // 600 x 600 plane: 361 k vertices and 721 k edges. Over the budget, so it is strided.
        let big = Primitive::Plane { width: 6.0, depth: 6.0, segments: 600 }.build().unwrap();
        let counted = std::cell::Cell::new(0usize);
        let project = |p: Vec3| {
            counted.set(counted.get() + 1);
            top(p)
        };
        let meshes = [(Mat4::IDENTITY, &big)];
        let t = SnapTargets { vertex: true, edge: true, surface: false };
        let _ = snap(&meshes, t, &project, Vec2::ZERO, 10.0, &down_ray(0.0, 0.0));
        let elements = big.vertex_count() + big.edge_count();
        assert!(elements > BUDGET * 2);
        // Each visited vertex projects once and each visited edge twice.
        assert!(counted.get() <= 3 * BUDGET, "{} projections for {elements} elements", counted.get());
        assert!(counted.get() > 0);
    }
}
