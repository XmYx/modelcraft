//! Triangle meshes for scene building and a few generated shapes (used by the built-in test
//! and benchmark scenes).

use glam::{Vec2, Vec3};

/// A triangle mesh in its own space. Normals and texture coordinates are per triangle corner.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshData {
    pub positions: Vec<Vec3>,
    pub indices: Vec<[u32; 3]>,
    /// One normal per corner of every triangle; `None` shades with the face normal.
    pub normals: Option<Vec<[Vec3; 3]>>,
    pub uvs: Option<Vec<[Vec2; 3]>>,
}

impl MeshData {
    pub fn triangle_count(&self) -> usize {
        self.indices.len()
    }

    /// Append another mesh (missing normals or UVs on either side drop that attribute).
    pub fn append(&mut self, other: &MeshData) {
        let base = self.positions.len() as u32;
        let had = self.indices.len();
        self.positions.extend_from_slice(&other.positions);
        self.indices.extend(other.indices.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
        self.normals = match (self.normals.take(), &other.normals) {
            (Some(mut a), Some(b)) if a.len() == had => {
                a.extend_from_slice(b);
                Some(a)
            }
            (None, Some(b)) if had == 0 => Some(b.clone()),
            _ => None,
        };
        self.uvs = match (self.uvs.take(), &other.uvs) {
            (Some(mut a), Some(b)) if a.len() == had => {
                a.extend_from_slice(b);
                Some(a)
            }
            (None, Some(b)) if had == 0 => Some(b.clone()),
            _ => None,
        };
    }
}

/// A parallelogram `p0, p0+e1, p0+e1+e2, p0+e2`; its front face looks along `e1 × e2`.
pub fn quad(p0: Vec3, e1: Vec3, e2: Vec3) -> MeshData {
    let uv = [Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y];
    MeshData {
        positions: vec![p0, p0 + e1, p0 + e1 + e2, p0 + e2],
        indices: vec![[0, 1, 2], [0, 2, 3]],
        normals: None,
        uvs: Some(vec![[uv[0], uv[1], uv[2]], [uv[0], uv[2], uv[3]]]),
    }
}

/// An axis-aligned box with outward faces.
pub fn cuboid(min: Vec3, max: Vec3) -> MeshData {
    let d = max - min;
    let (x, y, z) = (Vec3::X * d.x, Vec3::Y * d.y, Vec3::Z * d.z);
    let mut m = MeshData::default();
    for q in [
        quad(min, y, x),     // -z
        quad(min + z, x, y), // +z
        quad(min, z, y),     // -x
        quad(min + x, y, z), // +x
        quad(min, x, z),     // -y
        quad(min + y, z, x), // +y
    ] {
        m.append(&q);
    }
    m
}

/// A latitude–longitude sphere with smooth normals. Counts are clamped to 3..=2048.
pub fn uv_sphere(center: Vec3, radius: f32, segments: u32, rings: u32) -> MeshData {
    let (segs, rings) = (segments.clamp(3, 2048), rings.clamp(2, 2048));
    let mut positions = Vec::with_capacity(((segs + 1) * (rings + 1)) as usize);
    let mut dirs = Vec::with_capacity(positions.capacity());
    let mut tex = Vec::with_capacity(positions.capacity());
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let theta = v * std::f32::consts::PI;
        for s in 0..=segs {
            let u = s as f32 / segs as f32;
            let phi = u * std::f32::consts::TAU;
            let d = Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            dirs.push(d);
            positions.push(center + d * radius);
            tex.push(Vec2::new(u, v));
        }
    }
    let mut indices = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut push = |a: u32, b: u32, c: u32| {
        indices.push([a, b, c]);
        normals.push([dirs[a as usize], dirs[b as usize], dirs[c as usize]]);
        uvs.push([tex[a as usize], tex[b as usize], tex[c as usize]]);
    };
    for r in 0..rings {
        for s in 0..segs {
            let a = r * (segs + 1) + s;
            let b = a + segs + 1;
            if r > 0 {
                push(a, a + 1, b);
            }
            if r + 1 < rings {
                push(a + 1, b + 1, b);
            }
        }
    }
    MeshData { positions, indices, normals: Some(normals), uvs: Some(uvs) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outward(m: &MeshData, center: Vec3) -> bool {
        m.indices.iter().all(|t| {
            let [a, b, c] = t.map(|i| m.positions[i as usize]);
            (b - a).cross(c - a).dot((a + b + c) / 3.0 - center) > 0.0
        })
    }

    #[test]
    fn generated_shapes_face_outward() {
        let c = cuboid(Vec3::new(-1.0, 0.0, 2.0), Vec3::new(1.0, 3.0, 4.0));
        assert_eq!(c.triangle_count(), 12);
        assert!(outward(&c, Vec3::new(0.0, 1.5, 3.0)));
        assert!(c.uvs.as_ref().is_some_and(|u| u.len() == 12));
        let s = uv_sphere(Vec3::ONE, 2.0, 16, 8);
        assert!(outward(&s, Vec3::ONE));
        assert_eq!(s.triangle_count(), 16 * (8 - 1) * 2);
        assert!(s.positions.iter().all(|p| ((*p - Vec3::ONE).length() - 2.0).abs() < 1e-5));
        // Hostile counts are clamped.
        assert!(uv_sphere(Vec3::ZERO, 1.0, 0, u32::MAX).triangle_count() < 20_000);
    }
}
