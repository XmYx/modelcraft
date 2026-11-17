//! Ray picking against meshes (CPU, brute force over triangles). The sculpt engine has its own
//! accelerated picking for high-resolution meshes.

use glam::{Mat4, Vec3};

use crate::PolyMesh;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn transformed(&self, m: Mat4) -> Ray {
        Ray { origin: m.transform_point3(self.origin), dir: m.transform_vector3(self.dir) }
    }
}

/// Möller–Trumbore; returns the distance along the ray (in units of `dir`).
pub fn ray_triangle(r: &Ray, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = r.dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = r.origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = r.dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0 && t.is_finite()).then_some(t)
}

/// Slab test: does the ray (in units of `dir`, forwards only) cross the box `lo..hi`?
pub fn ray_aabb(r: &Ray, lo: Vec3, hi: Vec3) -> bool {
    let mut t0 = 0.0f32;
    let mut t1 = f32::INFINITY;
    for k in 0..3 {
        let (o, d, a, b) = (r.origin[k], r.dir[k], lo[k], hi[k]);
        if d.abs() < 1e-12 {
            if o < a || o > b {
                return false;
            }
        } else {
            let (mut n, mut f) = ((a - o) / d, (b - o) / d);
            if n > f {
                std::mem::swap(&mut n, &mut f);
            }
            t0 = t0.max(n);
            t1 = t1.min(f);
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f32,
    pub face: u32,
    pub point: Vec3,
}

/// Nearest face hit by a ray given in the mesh's space.
pub fn pick_face(m: &PolyMesh, r: &Ray) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for f in 0..m.face_count() as u32 {
        let mut it = m.face_vertices(f);
        let (Some(a), Some(mut b)) = (it.next(), it.next()) else { continue };
        let pa = m.position(a);
        for c in it {
            if let Some(t) = ray_triangle(r, pa, m.position(b), m.position(c))
                && best.is_none_or(|h| t < h.t)
            {
                best = Some(Hit { t, face: f, point: r.origin + r.dir * t });
            }
            b = c;
        }
    }
    best
}

/// Nearest vertex of the hit face to the hit point.
pub fn pick_vertex(m: &PolyMesh, r: &Ray) -> Option<u32> {
    let h = pick_face(m, r)?;
    m.face_vertices(h.face).min_by(|a, b| m.position(*a).distance(h.point).total_cmp(&m.position(*b).distance(h.point)))
}

/// Nearest edge of the hit face to the hit point.
pub fn pick_edge(m: &PolyMesh, r: &Ray) -> Option<u32> {
    let h = pick_face(m, r)?;
    let dist = |he: u32| {
        let (a, b) = (m.position(m.origin(he)), m.position(m.dest(he)));
        let ab = b - a;
        let t = ((h.point - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
        (a + ab * t).distance(h.point)
    };
    m.face_half_edges(h.face).min_by(|a, b| dist(*a).total_cmp(&dist(*b))).map(|he| m.edge_of(he))
}
