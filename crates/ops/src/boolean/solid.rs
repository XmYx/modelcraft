//! A closed triangle mesh prepared for a boolean: validated, oriented outward, with its
//! snapped integer copy for the exact predicates, and the generalised winding number.

use std::collections::HashMap;

use glam::{DVec3, Vec2, Vec3};

use super::exact::{MAX_COORD, Pt};
use super::{BoolError, Limits};

/// A triangle mesh as the boolean takes it: positions, triangles, per-corner UVs (all or none)
/// and one patch number per triangle.
#[derive(Clone, Debug, Default)]
pub struct TriMesh {
    pub positions: Vec<Vec3>,
    pub tris: Vec<[u32; 3]>,
    pub uvs: Option<Vec<[Vec2; 3]>>,
    pub patches: Vec<u32>,
}

/// A validated closed mesh.
pub(crate) struct Solid {
    pub pos: Vec<DVec3>,
    pub tris: Vec<[u32; 3]>,
    pub uvs: Option<Vec<[Vec2; 3]>>,
    pub patches: Vec<u32>,
    /// Edge ids of corner `k`'s edge (corner `k` to `k + 1`) of every triangle.
    pub tri_edges: Vec<[u32; 3]>,
    /// The two vertices of each edge, the smaller index first.
    pub edge_ends: Vec<[u32; 2]>,
    /// Unit outward normal of every triangle.
    pub normals: Vec<DVec3>,
    /// Integer copies of the positions (set by [`Solid::snap`]).
    pub ipts: Vec<Pt>,
    /// Integer bounding box of every triangle.
    pub boxes: Vec<([i64; 3], [i64; 3])>,
}

impl Solid {
    /// Check that `m` is a closed, edge-manifold, consistently wound mesh without degenerate
    /// triangles, and turn it outward if it is inside out.
    pub fn prepare(m: &TriMesh, limits: &Limits, name: &str) -> Result<Solid, BoolError> {
        if m.tris.is_empty() || m.positions.is_empty() {
            return Err(BoolError::Empty(name.to_string()));
        }
        if m.tris.len() > limits.max_triangles {
            return Err(BoolError::TooLarge(format!("{name} has {} triangles; the limit is {}", m.tris.len(), limits.max_triangles)));
        }
        if m.patches.len() != m.tris.len() || m.uvs.as_ref().is_some_and(|u| u.len() != m.tris.len()) {
            return Err(BoolError::Invalid(format!("{name}: triangle data has mismatched lengths")));
        }
        let n = m.positions.len() as u64;
        if m.tris.iter().any(|t| t.iter().any(|v| u64::from(*v) >= n) || t[0] == t[1] || t[1] == t[2] || t[0] == t[2]) {
            return Err(BoolError::Invalid(format!("{name} has a triangle with a missing or repeated vertex")));
        }
        let pos: Vec<DVec3> = m.positions.iter().map(|p| p.as_dvec3()).collect();
        if pos.iter().any(|p| !p.is_finite()) {
            return Err(BoolError::Invalid(format!("{name} has a position that is not a number")));
        }
        let mut tris = m.tris.clone();
        // Edge table: each undirected edge exactly twice, once in each direction.
        let build_edges = |tris: &[[u32; 3]]| -> Result<(Vec<[u32; 3]>, Vec<[u32; 2]>), BoolError> {
            let mut map: HashMap<(u32, u32), (u32, bool, u8)> = HashMap::with_capacity(tris.len() * 3 / 2);
            let mut tri_edges = vec![[0u32; 3]; tris.len()];
            let mut ends: Vec<[u32; 2]> = Vec::with_capacity(tris.len() * 3 / 2);
            for (t, tri) in tris.iter().enumerate() {
                for k in 0..3 {
                    let (a, b) = (tri[k], tri[(k + 1) % 3]);
                    let (key, forward) = if a < b { ((a, b), true) } else { ((b, a), false) };
                    let id = match map.get_mut(&key) {
                        None => {
                            let id = ends.len() as u32;
                            ends.push([key.0, key.1]);
                            map.insert(key, (id, forward, 1));
                            id
                        }
                        Some(e) => {
                            if e.2 >= 2 || e.1 == forward {
                                return Err(BoolError::Open(format!("{name} is not a closed, consistently wound surface (an edge is shared by the wrong faces)")));
                            }
                            e.2 += 1;
                            e.0
                        }
                    };
                    tri_edges[t][k] = id;
                }
            }
            if map.values().any(|e| e.2 != 2) {
                return Err(BoolError::Open(format!("{name} has open edges: fill its holes first")));
            }
            Ok((tri_edges, ends))
        };
        let (mut tri_edges, mut edge_ends) = build_edges(&tris)?;
        // Outward: a positive signed volume. An inside-out mesh is turned around.
        let volume: f64 = tris.iter().map(|t| pos[t[0] as usize].dot(pos[t[1] as usize].cross(pos[t[2] as usize]))).sum::<f64>() / 6.0;
        let mut uvs = m.uvs.clone();
        if volume < 0.0 {
            for t in &mut tris {
                t.swap(1, 2);
            }
            if let Some(u) = &mut uvs {
                for c in u {
                    c.swap(1, 2);
                }
            }
            (tri_edges, edge_ends) = build_edges(&tris)?;
        }
        let mut normals = Vec::with_capacity(tris.len());
        for t in &tris {
            let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
            let n = (b - a).cross(c - a);
            let len = n.length();
            if len <= 0.0 || !len.is_finite() {
                return Err(BoolError::Invalid(format!("{name} has a triangle with no area")));
            }
            normals.push(n / len);
        }
        Ok(Solid { pos, tris, uvs, patches: m.patches.clone(), tri_edges, edge_ends, normals, ipts: Vec::new(), boxes: Vec::new() })
    }

    /// Fill in the integer copies: positions snapped to the grid with origin `origin` and cell
    /// `cell`, indices starting at `first_index`.
    pub fn snap(&mut self, origin: DVec3, cell: f64, first_index: u32) -> Result<(), BoolError> {
        self.ipts = self
            .pos
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let q = (*p - origin) / cell;
                Pt { c: [q.x.round() as i64, q.y.round() as i64, q.z.round() as i64], index: first_index + i as u32 }
            })
            .collect();
        if self.ipts.iter().any(|p| p.c.iter().any(|c| c.abs() > MAX_COORD)) {
            return Err(BoolError::Invalid("the meshes are too large to snap to the exact grid".into()));
        }
        self.boxes = self
            .tris
            .iter()
            .map(|t| {
                let (a, b, c) = (self.ipts[t[0] as usize].c, self.ipts[t[1] as usize].c, self.ipts[t[2] as usize].c);
                ([a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1]), a[2].min(b[2]).min(c[2])], [a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1]), a[2].max(b[2]).max(c[2])])
            })
            .collect();
        Ok(())
    }

    pub fn tri_pts(&self, t: usize) -> [&Pt; 3] {
        let tri = self.tris[t];
        [&self.ipts[tri[0] as usize], &self.ipts[tri[1] as usize], &self.ipts[tri[2] as usize]]
    }

    /// The winding number of `p` and its distance to the surface. A point on or very near the
    /// surface has an unreliable winding number, so callers prefer the point farthest from it.
    pub fn probe(&self, p: DVec3) -> (f64, f64) {
        let mut near = f64::MAX;
        for t in &self.tris {
            let (a, b, c) = (self.pos[t[0] as usize], self.pos[t[1] as usize], self.pos[t[2] as usize]);
            near = near.min(distance_to_triangle(p, a, b, c));
        }
        (self.winding(p), near)
    }

    /// The generalised winding number of `p`: the solid angle the surface subtends, over 4π
    /// (Van Oosterom and Strackee, "The Solid Angle of a Plane Triangle", IEEE Trans. Biomed.
    /// Eng. 30(2), 1983). About 1 inside a closed outward mesh, about 0 outside.
    pub fn winding(&self, p: DVec3) -> f64 {
        let mut total = 0.0;
        for t in &self.tris {
            let a = self.pos[t[0] as usize] - p;
            let b = self.pos[t[1] as usize] - p;
            let c = self.pos[t[2] as usize] - p;
            let (la, lb, lc) = (a.length(), b.length(), c.length());
            let num = a.dot(b.cross(c));
            let den = la * lb * lc + a.dot(b) * lc + a.dot(c) * lb + b.dot(c) * la;
            total += 2.0 * num.atan2(den);
        }
        total / (4.0 * std::f64::consts::PI)
    }
}

/// Distance from `p` to the triangle `a`, `b`, `c`: the plane distance when `p` projects inside,
/// otherwise the least distance to an edge.
pub(crate) fn distance_to_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> f64 {
    let n = (b - a).cross(c - a);
    let area2 = n.length_squared();
    if area2 > 0.0 {
        // Barycentric coordinates of the projection of p.
        let w0 = (c - b).cross(p - b).dot(n) / area2;
        let w1 = (a - c).cross(p - c).dot(n) / area2;
        let w2 = (b - a).cross(p - a).dot(n) / area2;
        if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
            return (p - a).dot(n).abs() / area2.sqrt();
        }
    }
    let edge = |u: DVec3, v: DVec3| {
        let d = v - u;
        let len2 = d.length_squared();
        let t = if len2 > 0.0 { ((p - u).dot(d) / len2).clamp(0.0, 1.0) } else { 0.0 };
        (p - (u + d * t)).length()
    };
    edge(a, b).min(edge(b, c)).min(edge(c, a))
}
