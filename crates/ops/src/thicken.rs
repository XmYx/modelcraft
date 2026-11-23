//! Thicken: give a surface a thickness. Every point moves along its vertex normal, once outward
//! and once inward, and the two copies are joined: an open sheet gets a rim strip along every
//! border so the result is a closed solid; a closed mesh becomes a hollow shell with an inner
//! wall facing inward.

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::{MAX_RESULT_FACES, OpError, OpResult, PolyMesh, Polygon, rebuild};

/// Largest thickness accepted.
pub const MAX_AMOUNT: f32 = 100_000.0;
/// Smallest thickness used (a thickness of nothing would fold the two walls onto each other).
pub const MIN_AMOUNT: f32 = 1e-5;

/// Which way the thickness grows from the surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Half out, half in: the surface stays in the middle of the wall.
    #[default]
    Both,
    /// All of it on the outside; the surface is the inner wall.
    Outward,
    /// All of it on the inside; the surface is the outer wall.
    Inward,
}

pub const DIRECTIONS: &[&str] = &["both", "outward", "inward"];

impl Direction {
    pub fn name(self) -> &'static str {
        match self {
            Direction::Both => "both",
            Direction::Outward => "outward",
            Direction::Inward => "inward",
        }
    }
    pub fn from_name(n: &str) -> Option<Direction> {
        [Direction::Both, Direction::Outward, Direction::Inward].into_iter().find(|d| d.name() == n)
    }
}

/// Thicken `m` by `amount` (its absolute value, clamped to [`MIN_AMOUNT`], [`MAX_AMOUNT`]).
pub fn thicken(m: &PolyMesh, amount: f32, direction: Direction) -> OpResult {
    if m.face_count() == 0 {
        return Err(OpError::Invalid("there is no surface to thicken".into()));
    }
    let amount = if amount.is_finite() { amount.abs().clamp(MIN_AMOUNT, MAX_AMOUNT) } else { 1.0 };
    let (out_by, in_by) = match direction {
        Direction::Both => (amount * 0.5, amount * 0.5),
        Direction::Outward => (amount, 0.0),
        Direction::Inward => (0.0, amount),
    };
    let nv = m.vertex_count() as u32;
    let normals = m.vertex_normals();
    let mut positions: Vec<Vec3> = Vec::with_capacity(m.vertex_count() * 2);
    positions.extend(m.positions().iter().zip(&normals).map(|(p, n)| *p + *n * out_by));
    positions.extend(m.positions().iter().zip(&normals).map(|(p, n)| *p - *n * in_by));
    let has_uvs = m.has_uvs();
    let mut polys: Vec<Polygon> = Vec::with_capacity(m.face_count() * 2);
    let mut rim: Vec<(u32, u32, Vec2, Vec2, u32)> = Vec::new();
    for f in 0..m.face_count() as u32 {
        let hes: Vec<u32> = m.face_half_edges(f).collect();
        let verts: Vec<u32> = hes.iter().map(|h| m.origin(*h)).collect();
        let uvs: Vec<Vec2> = if has_uvs { hes.iter().map(|h| m.uv(*h)).collect() } else { Vec::new() };
        let patch = m.patch(f);
        let make = |verts: Vec<u32>, uvs: Vec<Vec2>| {
            let mut p = if has_uvs { Polygon::with_uvs(verts, uvs) } else { Polygon::new(verts) };
            p.patch = patch;
            p
        };
        polys.push(make(verts.clone(), uvs.clone()));
        // The inner wall faces the other way: the same corners, in reverse, on the inner copy.
        polys.push(make(verts.iter().rev().map(|v| v + nv).collect(), uvs.iter().rev().copied().collect()));
        for (k, h) in hes.iter().enumerate() {
            if m.is_boundary_edge(m.edge_of(*h)) {
                let next = (k + 1) % hes.len();
                let (ua, ub) = if has_uvs { (uvs[k], uvs[next]) } else { (Vec2::ZERO, Vec2::ZERO) };
                rim.push((m.origin(*h), m.dest(*h), ua, ub, patch));
            }
        }
    }
    // Rim strips: outer copy to inner copy along every border edge, facing away from the sheet.
    for (a, b, ua, ub, patch) in rim {
        let verts = vec![a, a + nv, b + nv, b];
        let mut p = if has_uvs { Polygon::with_uvs(verts, vec![ua, ua, ub, ub]) } else { Polygon::new(verts) };
        p.patch = patch;
        polys.push(p);
    }
    if polys.len() > MAX_RESULT_FACES {
        return Err(OpError::TooLarge(polys.len()));
    }
    rebuild(positions, polys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;
    use crate::testutil::{closed, euler};
    use proptest::prelude::*;

    fn area_and_volume(m: &PolyMesh) -> (f64, f64) {
        let mut area = 0.0f64;
        let mut vol = 0.0f64;
        let (tris, _) = m.triangulate();
        for t in tris {
            let (a, b, c) = (m.position(t[0]).as_dvec3(), m.position(t[1]).as_dvec3(), m.position(t[2]).as_dvec3());
            area += (b - a).cross(c - a).length() * 0.5;
            vol += a.dot(b.cross(c)) / 6.0;
        }
        (area, vol)
    }

    #[test]
    fn an_open_sheet_becomes_a_closed_solid_with_the_right_volume() {
        let plane = Primitive::Plane { width: 4.0, depth: 2.0, segments: 4 }.build().unwrap();
        assert!(!closed(&plane));
        for dir in [Direction::Both, Direction::Outward, Direction::Inward] {
            let t = thicken(&plane, 0.5, dir).unwrap();
            t.validate().unwrap();
            assert!(closed(&t), "{dir:?}");
            assert_eq!(euler(&t), 2, "a sheet with a rim is a ball: {dir:?}");
            let (_, v) = area_and_volume(&t);
            assert!((v.abs() - 4.0).abs() < 1e-3, "{dir:?}: area 8 × 0.5 = 4, got {v}");
            assert!(v > 0.0, "outward-facing: {dir:?}");
        }
        let both = thicken(&plane, 1.0, Direction::Both).unwrap();
        let (lo, hi) = both.bounds();
        assert!((lo.y + 0.5).abs() < 1e-5 && (hi.y - 0.5).abs() < 1e-5, "centred on the sheet");
        let up = thicken(&plane, 1.0, Direction::Outward).unwrap();
        let (lo, hi) = up.bounds();
        assert!(lo.y.abs() < 1e-5 && (hi.y - 1.0).abs() < 1e-5, "the sheet is the inner wall");
    }

    #[test]
    fn a_closed_mesh_becomes_a_hollow_shell() {
        let cube = Primitive::Cube { size: Vec3::splat(2.0), segments: 2 }.build().unwrap();
        let t = thicken(&cube, 0.2, Direction::Inward).unwrap();
        assert!(closed(&t));
        t.validate().unwrap();
        assert_eq!(euler(&t), 4, "two shells");
        let (_, v) = area_and_volume(&t);
        // Wall of 0.2 inside a 2-cube: 8 - 1.6³ (corner normals are not exact, so roughly).
        assert!(v > 2.5 && v < 4.5, "{v}");
        assert!(t.has_uvs());
    }

    #[test]
    fn nothing_to_thicken_and_hostile_amounts_are_handled() {
        assert!(thicken(&PolyMesh::default(), 1.0, Direction::Both).is_err());
        let sphere = Primitive::Sphere { radius: 1.0, segments: 8, rings: 6 }.build().unwrap();
        for amount in [f32::NAN, f32::INFINITY, -2.0, 0.0, 1e30] {
            let t = thicken(&sphere, amount, Direction::Both).unwrap();
            t.validate().unwrap();
            assert!(t.positions().iter().all(|p| p.is_finite()));
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(40))]
        #[test]
        fn any_primitive_thickens_to_a_valid_closed_mesh(kind in 0usize..17, amount in 0.01f32..0.5, dir in 0usize..3) {
            let kinds = crate::primitives::KINDS;
            let p = Primitive::default_of(kinds[kind % kinds.len()]).unwrap();
            let m = p.build().unwrap();
            let t = thicken(&m, amount, [Direction::Both, Direction::Outward, Direction::Inward][dir]).unwrap();
            prop_assert!(t.validate().is_ok());
            prop_assert!(closed(&t), "{}", p.kind());
            prop_assert!(t.face_count() >= m.face_count() * 2);
        }
    }
}
