//! Parametric primitives. Each [`Primitive`] is a generator: its parameters are kept in the scene
//! and the mesh is rebuilt when they change. All primitives carry UVs and sit centred on the
//! origin (the plane lies in XZ). Segment counts are clamped to [`MAX_SEGMENTS`].

use std::f32::consts::{PI, TAU};

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

pub use crate::primitives_more::Solid;
use crate::primitives_more::{self as more, Parts};
use crate::{OpResult, Polygon, rebuild};

pub const MAX_SEGMENTS: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Primitive {
    Cube {
        size: Vec3,
        segments: u32,
    },
    Plane {
        width: f32,
        depth: f32,
        segments: u32,
    },
    Sphere {
        radius: f32,
        segments: u32,
        rings: u32,
    },
    /// A subdivided cube pushed onto a sphere: even quads everywhere, the usual sculpting base.
    RoundCube {
        radius: f32,
        segments: u32,
    },
    Cylinder {
        radius: f32,
        height: f32,
        sides: u32,
        rings: u32,
        caps: bool,
    },
    Cone {
        radius: f32,
        height: f32,
        sides: u32,
        caps: bool,
    },
    Torus {
        radius: f32,
        tube: f32,
        segments: u32,
        sides: u32,
    },
    /// A flat disc facing +Y; `inner_radius > 0` cuts a hole (an annulus).
    Disc {
        radius: f32,
        inner_radius: f32,
        sides: u32,
        rings: u32,
    },
    /// A regular polygon: one n-gon face facing +Y.
    Polygon {
        radius: f32,
        sides: u32,
    },
    /// A hollow cylinder with an inner wall; closed with `caps`.
    Tube {
        radius: f32,
        inner_radius: f32,
        height: f32,
        sides: u32,
        rings: u32,
        caps: bool,
    },
    /// A cylinder of `height` between two hemispheres; `rings` rows per hemisphere.
    Capsule {
        radius: f32,
        height: f32,
        sides: u32,
        rings: u32,
    },
    /// A rectangular-base pyramid, apex up.
    Pyramid {
        width: f32,
        depth: f32,
        height: f32,
    },
    /// A tetrahedron, octahedron, dodecahedron or icosahedron of the given circumradius.
    Platonic {
        solid: Solid,
        radius: f32,
    },
    /// A seeded fractal-noise terrain in XZ. `scale` is the number of noise features across the
    /// width, `height` the total vertical range.
    Landscape {
        width: f32,
        depth: f32,
        height: f32,
        segments: u32,
        seed: u32,
        scale: f32,
        octaves: u32,
    },
}

/// The primitive kinds, in menu order.
pub const KINDS: &[&str] = &[
    "cube",
    "plane",
    "sphere",
    "round_cube",
    "cylinder",
    "cone",
    "torus",
    "disc",
    "polygon",
    "tube",
    "capsule",
    "pyramid",
    "tetrahedron",
    "octahedron",
    "dodecahedron",
    "icosahedron",
    "landscape",
];

impl Primitive {
    /// The default primitive of a kind (`cube`, `plane`, …), `None` for an unknown kind.
    pub fn default_of(kind: &str) -> Option<Primitive> {
        Some(match kind {
            "cube" => Primitive::Cube { size: Vec3::splat(2.0), segments: 1 },
            "plane" => Primitive::Plane { width: 4.0, depth: 4.0, segments: 4 },
            "sphere" => Primitive::Sphere { radius: 1.0, segments: 32, rings: 16 },
            "round_cube" => Primitive::RoundCube { radius: 1.0, segments: 16 },
            "cylinder" => Primitive::Cylinder { radius: 1.0, height: 2.0, sides: 24, rings: 1, caps: true },
            "cone" => Primitive::Cone { radius: 1.0, height: 2.0, sides: 24, caps: true },
            "torus" => Primitive::Torus { radius: 1.0, tube: 0.35, segments: 32, sides: 16 },
            "disc" => Primitive::Disc { radius: 1.0, inner_radius: 0.0, sides: 32, rings: 4 },
            "polygon" => Primitive::Polygon { radius: 1.0, sides: 6 },
            "tube" => Primitive::Tube { radius: 1.0, inner_radius: 0.6, height: 2.0, sides: 32, rings: 1, caps: true },
            "capsule" => Primitive::Capsule { radius: 0.6, height: 1.6, sides: 24, rings: 8 },
            "pyramid" => Primitive::Pyramid { width: 2.0, depth: 2.0, height: 2.0 },
            "landscape" => Primitive::Landscape { width: 8.0, depth: 8.0, height: 2.0, segments: 64, seed: 1, scale: 4.0, octaves: 4 },
            other => Primitive::Platonic { solid: Solid::from_kind(other)?, radius: 1.0 },
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Primitive::Cube { .. } => "cube",
            Primitive::Plane { .. } => "plane",
            Primitive::Sphere { .. } => "sphere",
            Primitive::RoundCube { .. } => "round_cube",
            Primitive::Cylinder { .. } => "cylinder",
            Primitive::Cone { .. } => "cone",
            Primitive::Torus { .. } => "torus",
            Primitive::Disc { .. } => "disc",
            Primitive::Polygon { .. } => "polygon",
            Primitive::Tube { .. } => "tube",
            Primitive::Capsule { .. } => "capsule",
            Primitive::Pyramid { .. } => "pyramid",
            Primitive::Platonic { solid, .. } => solid.kind(),
            Primitive::Landscape { .. } => "landscape",
        }
    }

    /// Build the mesh.
    pub fn build(&self) -> OpResult {
        let seg = |n: u32, min: u32| n.clamp(min, MAX_SEGMENTS);
        let len = |x: f32| if x.is_finite() { x.abs().max(1e-4) } else { 1.0 };
        match *self {
            Primitive::Cube { size, segments } => box_mesh(Vec3::new(len(size.x), len(size.y), len(size.z)), seg(segments, 1).min(256), false),
            Primitive::RoundCube { radius, segments } => box_mesh(Vec3::splat(len(radius) * 2.0), seg(segments, 1), true),
            Primitive::Plane { width, depth, segments } => plane(len(width), len(depth), seg(segments, 1)),
            Primitive::Sphere { radius, segments, rings } => sphere(len(radius), seg(segments, 3), seg(rings, 2)),
            Primitive::Cylinder { radius, height, sides, rings, caps } => lathe(len(radius), len(radius), len(height), seg(sides, 3), seg(rings, 1), caps),
            Primitive::Cone { radius, height, sides, caps } => lathe(len(radius), 0.0, len(height), seg(sides, 3), 1, caps),
            Primitive::Torus { radius, tube, segments, sides } => torus(len(radius), len(tube), seg(segments, 3), seg(sides, 3)),
            _ => match self.parts() {
                Some(parts) => rebuild(parts.0, parts.1),
                None => Err(crate::OpError::Invalid(format!("{} has no mesh builder", self.kind()))),
            },
        }
    }

    /// The raw positions and polygons of the second set of primitives, before they are rebuilt
    /// (tests check that `PolyMesh::build` had nothing to repair). Sizes are clamped here.
    pub(crate) fn parts(&self) -> Option<Parts> {
        let seg = |n: u32, min: u32| n.clamp(min, MAX_SEGMENTS);
        let len = |x: f32| if x.is_finite() { x.abs().max(1e-4) } else { 1.0 };
        Some(match *self {
            Primitive::Disc { radius, inner_radius, sides, rings } => more::disc(len(radius), inner_radius, seg(sides, 3), seg(rings, 1)),
            Primitive::Polygon { radius, sides } => more::polygon(len(radius), seg(sides, 3)),
            Primitive::Tube { radius, inner_radius, height, sides, rings, caps } => {
                more::tube(len(radius), inner_radius, len(height), seg(sides, 3), seg(rings, 1), caps)
            }
            Primitive::Capsule { radius, height, sides, rings } => more::capsule(len(radius), len(height), seg(sides, 3), seg(rings, 1)),
            Primitive::Pyramid { width, depth, height } => more::pyramid(len(width), len(depth), len(height)),
            Primitive::Platonic { solid, radius } => more::platonic(solid, len(radius)),
            Primitive::Landscape { width, depth, height, segments, seed, scale, octaves } => {
                let height = if height.is_finite() { height.clamp(-1e4, 1e4) } else { 0.0 };
                let scale = if scale.is_finite() { scale.clamp(0.01, 256.0) } else { 4.0 };
                more::landscape(len(width), len(depth), height, seg(segments, 1), seed, scale, octaves.clamp(1, 8))
            }
            _ => return None,
        })
    }
}

fn plane(w: f32, d: f32, n: u32) -> OpResult {
    let mut pos = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            pos.push(Vec3::new((i as f32 / n as f32 - 0.5) * w, 0.0, (j as f32 / n as f32 - 0.5) * d));
        }
    }
    let uv = |i: u32, j: u32| Vec2::new(i as f32 / n as f32, j as f32 / n as f32);
    let mut polys = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            polys.push(Polygon::with_uvs(vec![a, a + n + 1, a + n + 2, a + 1], vec![uv(i, j), uv(i, j + 1), uv(i + 1, j + 1), uv(i + 1, j)]));
        }
    }
    rebuild(pos, polys)
}

/// A box with `n` segments per side; `round` pushes every vertex onto the inscribed sphere
/// (with an even spread, so the quads stay close to square).
fn box_mesh(size: Vec3, n: u32, round: bool) -> OpResult {
    let mut pos: Vec<Vec3> = Vec::new();
    let mut index = std::collections::HashMap::new();
    let mut polys = Vec::new();
    // Each face: origin corner, u axis, v axis, normal (as unit-cube vectors in -1..1).
    let faces = [
        (Vec3::new(1., -1., 1.), Vec3::new(0., 0., -2.), Vec3::new(0., 2., 0.)),
        (Vec3::new(-1., -1., -1.), Vec3::new(0., 0., 2.), Vec3::new(0., 2., 0.)),
        (Vec3::new(-1., 1., 1.), Vec3::new(2., 0., 0.), Vec3::new(0., 0., -2.)),
        (Vec3::new(-1., -1., -1.), Vec3::new(2., 0., 0.), Vec3::new(0., 0., 2.)),
        (Vec3::new(-1., -1., 1.), Vec3::new(2., 0., 0.), Vec3::new(0., 2., 0.)),
        (Vec3::new(1., -1., -1.), Vec3::new(-2., 0., 0.), Vec3::new(0., 2., 0.)),
    ];
    let mut vert = |p: Vec3, pos: &mut Vec<Vec3>| -> u32 {
        let key = ((p.x * 4096.0).round() as i64, (p.y * 4096.0).round() as i64, (p.z * 4096.0).round() as i64);
        *index.entry(key).or_insert_with(|| {
            pos.push(p);
            (pos.len() - 1) as u32
        })
    };
    for (fi, (o, u, v)) in faces.iter().enumerate() {
        let mut id = |i: u32, j: u32, pos: &mut Vec<Vec3>| vert(*o + *u * (i as f32 / n as f32) + *v * (j as f32 / n as f32), pos);
        // UV atlas: a 3×2 grid of faces.
        let cell = Vec2::new((fi % 3) as f32 / 3.0, (fi / 3) as f32 / 2.0);
        let uv = |i: u32, j: u32| cell + Vec2::new(i as f32 / n as f32 / 3.0, 0.5 - j as f32 / n as f32 / 2.0);
        for j in 0..n {
            for i in 0..n {
                let ids = vec![id(i, j, &mut pos), id(i + 1, j, &mut pos), id(i + 1, j + 1, &mut pos), id(i, j + 1, &mut pos)];
                let mut p = Polygon::with_uvs(ids, vec![uv(i, j), uv(i + 1, j), uv(i + 1, j + 1), uv(i, j + 1)]);
                p.patch = fi as u32;
                polys.push(p);
            }
        }
    }
    for p in &mut pos {
        if round {
            // Even cube-to-sphere mapping.
            let (x, y, z) = (p.x, p.y, p.z);
            let q = Vec3::new(
                x * (1.0 - y * y / 2.0 - z * z / 2.0 + y * y * z * z / 3.0).max(0.0).sqrt(),
                y * (1.0 - z * z / 2.0 - x * x / 2.0 + z * z * x * x / 3.0).max(0.0).sqrt(),
                z * (1.0 - x * x / 2.0 - y * y / 2.0 + x * x * y * y / 3.0).max(0.0).sqrt(),
            );
            *p = q * size * 0.5;
        } else {
            *p *= size * 0.5;
        }
    }
    rebuild(pos, polys)
}

fn sphere(r: f32, segs: u32, rings: u32) -> OpResult {
    let mut pos = vec![Vec3::new(0.0, r, 0.0)];
    for j in 1..rings {
        let phi = PI * j as f32 / rings as f32;
        for i in 0..segs {
            let th = TAU * i as f32 / segs as f32;
            pos.push(Vec3::new(phi.sin() * th.cos(), phi.cos(), -phi.sin() * th.sin()) * r);
        }
    }
    pos.push(Vec3::new(0.0, -r, 0.0));
    let bottom = (pos.len() - 1) as u32;
    let ring = |j: u32, i: u32| 1 + (j - 1) * segs + (i % segs);
    let uv = |i: u32, j: u32| Vec2::new(i as f32 / segs as f32, j as f32 / rings as f32);
    let mut polys = Vec::new();
    for i in 0..segs {
        let mid = Vec2::new((i as f32 + 0.5) / segs as f32, 0.0);
        polys.push(Polygon::with_uvs(vec![0, ring(1, i), ring(1, i + 1)], vec![mid, uv(i, 1), uv(i + 1, 1)]));
        for j in 1..rings - 1 {
            polys.push(Polygon::with_uvs(
                vec![ring(j, i), ring(j + 1, i), ring(j + 1, i + 1), ring(j, i + 1)],
                vec![uv(i, j), uv(i, j + 1), uv(i + 1, j + 1), uv(i + 1, j)],
            ));
        }
        let midb = Vec2::new((i as f32 + 0.5) / segs as f32, 1.0);
        polys.push(Polygon::with_uvs(vec![ring(rings - 1, i), bottom, ring(rings - 1, i + 1)], vec![uv(i, rings - 1), midb, uv(i + 1, rings - 1)]));
    }
    rebuild(pos, polys)
}

/// Cylinder (`r_top == r_bottom`) or cone (`r_top == 0`) around Y.
fn lathe(r_bottom: f32, r_top: f32, h: f32, sides: u32, rings: u32, caps: bool) -> OpResult {
    let cone = r_top <= 0.0;
    let rows = if cone { rings } else { rings + 1 };
    let mut pos = Vec::new();
    for j in 0..rows {
        let t = j as f32 / rings as f32;
        let r = r_bottom + (r_top - r_bottom) * t;
        for i in 0..sides {
            let th = TAU * i as f32 / sides as f32;
            pos.push(Vec3::new(th.cos() * r, -h / 2.0 + h * t, -th.sin() * r));
        }
    }
    let at = |j: u32, i: u32| j * sides + i % sides;
    let uv = |i: u32, j: u32| Vec2::new(i as f32 / sides as f32, 1.0 - 0.5 * j as f32 / rings as f32);
    let mut polys = Vec::new();
    for j in 0..rows - 1 {
        for i in 0..sides {
            polys.push(Polygon::with_uvs(
                vec![at(j, i), at(j, i + 1), at(j + 1, i + 1), at(j + 1, i)],
                vec![uv(i, j), uv(i + 1, j), uv(i + 1, j + 1), uv(i, j + 1)],
            ));
        }
    }
    if cone {
        pos.push(Vec3::new(0.0, h / 2.0, 0.0));
        let apex = (pos.len() - 1) as u32;
        for i in 0..sides {
            let mid = Vec2::new((i as f32 + 0.5) / sides as f32, 0.5);
            polys.push(Polygon::with_uvs(vec![at(rows - 1, i), at(rows - 1, i + 1), apex], vec![uv(i, rows - 1), uv(i + 1, rows - 1), mid]));
        }
    }
    let cap_uv = |i: u32, cx: f32| {
        let th = TAU * i as f32 / sides as f32;
        Vec2::new(cx + 0.24 * th.cos(), 0.75 + 0.24 * th.sin())
    };
    if caps {
        let bottom: Vec<u32> = (0..sides).rev().map(|i| at(0, i)).collect();
        polys.push(Polygon::with_uvs(bottom, (0..sides).rev().map(|i| cap_uv(i, 0.25)).collect()));
        if !cone {
            let top: Vec<u32> = (0..sides).map(|i| at(rows - 1, i)).collect();
            polys.push(Polygon::with_uvs(top, (0..sides).map(|i| cap_uv(i, 0.75)).collect()));
        }
    }
    if !polys.iter().all(|p| p.uvs.is_some()) {
        polys.iter_mut().for_each(|p| p.uvs = None);
    }
    rebuild(pos, polys)
}

fn torus(r: f32, tube: f32, segs: u32, sides: u32) -> OpResult {
    let mut pos = Vec::new();
    for i in 0..segs {
        let th = TAU * i as f32 / segs as f32;
        for j in 0..sides {
            let ph = TAU * j as f32 / sides as f32;
            let rr = r + tube * ph.cos();
            pos.push(Vec3::new(th.cos() * rr, tube * ph.sin(), -th.sin() * rr));
        }
    }
    let at = |i: u32, j: u32| (i % segs) * sides + j % sides;
    let uv = |i: u32, j: u32| Vec2::new(i as f32 / segs as f32, j as f32 / sides as f32);
    let mut polys = Vec::new();
    for i in 0..segs {
        for j in 0..sides {
            polys.push(Polygon::with_uvs(
                vec![at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)],
                vec![uv(i, j), uv(i + 1, j), uv(i + 1, j + 1), uv(i, j + 1)],
            ));
        }
    }
    rebuild(pos, polys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn every_primitive_is_valid_and_outward() {
        let torus = Primitive::default_of("torus").unwrap().build().unwrap();
        assert!(closed(&torus));
        assert_eq!(euler(&torus), 0);
        for kind in KINDS {
            let p = Primitive::default_of(kind).unwrap();
            let m = p.build().unwrap();
            m.validate().unwrap();
            assert!(m.has_uvs(), "{kind} lost its UVs");
            if !matches!(*kind, "plane" | "torus" | "disc" | "polygon" | "tube" | "landscape") {
                assert!(closed(&m), "{kind} is open");
                assert_eq!(euler(&m), 2, "{kind}");
                let (lo, hi) = m.bounds();
                let c = (lo + hi) / 2.0;
                let out = (0..m.face_count() as u32).filter(|f| m.face_normal(*f).dot(m.face_center(*f) - c) > 0.0).count();
                assert!(out * 10 >= m.face_count() * 9, "{kind}: {out}/{} faces point out", m.face_count());
            }
        }
    }

    #[test]
    fn cube_segments_and_round_cube() {
        let m = Primitive::Cube { size: Vec3::ONE, segments: 3 }.build().unwrap();
        assert_eq!(m.face_count(), 54);
        assert_eq!(m.vertex_count(), 56);
        let r = Primitive::RoundCube { radius: 1.0, segments: 8 }.build().unwrap();
        for p in r.positions() {
            assert!((p.length() - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn hostile_parameters_are_clamped() {
        let m = Primitive::Sphere { radius: f32::NAN, segments: 0, rings: u32::MAX }.build().unwrap();
        m.validate().unwrap();
        assert!(m.face_count() <= (MAX_SEGMENTS * MAX_SEGMENTS) as usize);
    }
}
