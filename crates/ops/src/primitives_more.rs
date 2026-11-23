//! The second set of parametric primitives: disc, polygon, tube, capsule, pyramid, the four
//! platonic solids that are not a cube, and a seeded noise landscape. Each builder returns the
//! raw `(positions, polygons)` so tests can check that `PolyMesh::build` had nothing to repair;
//! [`crate::primitives::Primitive::build`] rebuilds them through [`crate::rebuild`].
//!
//! Conventions match the first set: centred on the origin, Y up, faces wound so normals point
//! out (flat shapes face +Y), every corner has a UV in 0..1 and sizes are already clamped by the
//! caller.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::Polygon;

pub(crate) type Parts = (Vec<Vec3>, Vec<Polygon>);

/// The four platonic solids a [`crate::primitives::Primitive::Platonic`] can be (the cube is its
/// own primitive).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Solid {
    Tetrahedron,
    Octahedron,
    Dodecahedron,
    Icosahedron,
}

impl Solid {
    pub const ALL: [Solid; 4] = [Solid::Tetrahedron, Solid::Octahedron, Solid::Dodecahedron, Solid::Icosahedron];

    pub fn kind(self) -> &'static str {
        match self {
            Solid::Tetrahedron => "tetrahedron",
            Solid::Octahedron => "octahedron",
            Solid::Dodecahedron => "dodecahedron",
            Solid::Icosahedron => "icosahedron",
        }
    }

    pub fn from_kind(kind: &str) -> Option<Solid> {
        Solid::ALL.into_iter().find(|s| s.kind() == kind)
    }

    /// Vertex count, edge count and face count (the Euler characteristic is always 2).
    pub fn counts(self) -> (usize, usize, usize) {
        match self {
            Solid::Tetrahedron => (4, 6, 4),
            Solid::Octahedron => (6, 12, 8),
            Solid::Dodecahedron => (20, 30, 12),
            Solid::Icosahedron => (12, 30, 20),
        }
    }
}

/// Position on a circle of radius `r` in the XZ plane (the angle runs from +X towards -Z, the
/// same way the first set's lathe does).
fn ring_point(th: f32, r: f32, y: f32) -> Vec3 {
    Vec3::new(th.cos() * r, y, -th.sin() * r)
}

/// A planar polar UV: `frac` of the way from the cell centre to its rim.
fn polar_uv(center: Vec2, half: f32, th: f32, frac: f32) -> Vec2 {
    center + Vec2::new(th.cos(), th.sin()) * (half * frac)
}

/// A flat disc (or annulus when `inner > 0`) facing +Y. `rings` radial bands, `sides` around.
pub(crate) fn disc(radius: f32, inner: f32, sides: u32, rings: u32) -> Parts {
    let inner = if inner.is_finite() { inner.clamp(0.0, radius * 0.999) } else { 0.0 };
    let hole = inner > 1e-6;
    let mut pos = Vec::new();
    let center = if hole {
        None
    } else {
        pos.push(Vec3::ZERO);
        Some(0u32)
    };
    // Ring k runs from the inner radius (k = 0, or the first ring out of the centre) to the rim.
    let first = if hole { 0 } else { 1 };
    let ring_r = |k: u32| inner + (radius - inner) * k as f32 / rings as f32;
    for k in first..=rings {
        for i in 0..sides {
            pos.push(ring_point(TAU * i as f32 / sides as f32, ring_r(k), 0.0));
        }
    }
    let base = u32::from(center.is_some());
    let at = |k: u32, i: u32| base + (k - first) * sides + i % sides;
    let uv = |k: u32, i: u32| polar_uv(Vec2::splat(0.5), 0.5, TAU * i as f32 / sides as f32, ring_r(k) / radius);
    let mut polys = Vec::new();
    for i in 0..sides {
        if let Some(c) = center {
            polys.push(Polygon::with_uvs(vec![c, at(1, i), at(1, i + 1)], vec![Vec2::splat(0.5), uv(1, i), uv(1, i + 1)]));
        }
        let start = if hole { 0 } else { 1 };
        for k in start..rings {
            polys.push(Polygon::with_uvs(
                vec![at(k, i), at(k + 1, i), at(k + 1, i + 1), at(k, i + 1)],
                vec![uv(k, i), uv(k + 1, i), uv(k + 1, i + 1), uv(k, i + 1)],
            ));
        }
    }
    (pos, polys)
}

/// A regular polygon: one n-gon face facing +Y.
pub(crate) fn polygon(radius: f32, sides: u32) -> Parts {
    let pos: Vec<Vec3> = (0..sides).map(|i| ring_point(TAU * i as f32 / sides as f32, radius, 0.0)).collect();
    let verts: Vec<u32> = (0..sides).collect();
    let uvs: Vec<Vec2> = (0..sides).map(|i| polar_uv(Vec2::splat(0.5), 0.5, TAU * i as f32 / sides as f32, 1.0)).collect();
    (pos, vec![Polygon::with_uvs(verts, uvs)])
}

/// A hollow cylinder: outer wall, inner wall and (with `caps`) two flat rims. Closed with caps.
pub(crate) fn tube(radius: f32, inner: f32, height: f32, sides: u32, rings: u32, caps: bool) -> Parts {
    let inner = if inner.is_finite() { inner.clamp(radius * 0.001, radius * 0.999) } else { radius * 0.5 };
    let mut pos = Vec::new();
    for wall_r in [radius, inner] {
        for j in 0..=rings {
            let y = -height / 2.0 + height * j as f32 / rings as f32;
            for i in 0..sides {
                pos.push(ring_point(TAU * i as f32 / sides as f32, wall_r, y));
            }
        }
    }
    let per_wall = (rings + 1) * sides;
    let outer_at = |j: u32, i: u32| j * sides + i % sides;
    let inner_at = move |j: u32, i: u32| per_wall + j * sides + i % sides;
    let u = |i: u32| i as f32 / sides as f32;
    let mut polys = Vec::new();
    // Wall UVs: the outer wall in the top band of the sheet, the inner wall below it.
    let wall_v = |band: f32, j: u32| 0.3 + 0.3 * band + 0.3 * j as f32 / rings as f32;
    for j in 0..rings {
        for i in 0..sides {
            polys.push(Polygon::with_uvs(
                vec![outer_at(j, i), outer_at(j, i + 1), outer_at(j + 1, i + 1), outer_at(j + 1, i)],
                vec![
                    Vec2::new(u(i), wall_v(1.0, j)),
                    Vec2::new(u(i + 1), wall_v(1.0, j)),
                    Vec2::new(u(i + 1), wall_v(1.0, j + 1)),
                    Vec2::new(u(i), wall_v(1.0, j + 1)),
                ],
            ));
            polys.push(Polygon::with_uvs(
                vec![inner_at(j + 1, i), inner_at(j + 1, i + 1), inner_at(j, i + 1), inner_at(j, i)],
                vec![
                    Vec2::new(u(i), wall_v(0.0, j + 1)),
                    Vec2::new(u(i + 1), wall_v(0.0, j + 1)),
                    Vec2::new(u(i + 1), wall_v(0.0, j)),
                    Vec2::new(u(i), wall_v(0.0, j)),
                ],
            ));
        }
    }
    if caps {
        let rim = |cx: f32, i: u32, wall_r: f32| polar_uv(Vec2::new(cx, 0.15), 0.14, TAU * i as f32 / sides as f32, wall_r / radius);
        for i in 0..sides {
            // Top rim faces up, bottom rim faces down.
            polys.push(Polygon::with_uvs(
                vec![inner_at(rings, i), outer_at(rings, i), outer_at(rings, i + 1), inner_at(rings, i + 1)],
                vec![rim(0.75, i, inner), rim(0.75, i, radius), rim(0.75, i + 1, radius), rim(0.75, i + 1, inner)],
            ));
            polys.push(Polygon::with_uvs(
                vec![inner_at(0, i + 1), outer_at(0, i + 1), outer_at(0, i), inner_at(0, i)],
                vec![rim(0.25, i + 1, inner), rim(0.25, i + 1, radius), rim(0.25, i, radius), rim(0.25, i, inner)],
            ));
        }
    }
    (pos, polys)
}

/// A surface of revolution around Y from a profile of `(radius, y)` rows, bottom to top. A row
/// with radius 0 at either end is a pole (a triangle fan). `v` is the profile arc length.
fn revolve(profile: &[(f32, f32)], sides: u32) -> Parts {
    let rows = profile.len();
    let mut arc = vec![0.0f32; rows];
    for k in 1..rows {
        let (a, b) = (profile[k - 1], profile[k]);
        arc[k] = arc[k - 1] + ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    }
    let total = arc.last().copied().unwrap_or(1.0).max(1e-9);
    let v = |k: usize| arc[k] / total;
    let pole_lo = profile.first().is_some_and(|p| p.0 <= 0.0);
    let pole_hi = profile.last().is_some_and(|p| p.0 <= 0.0);
    let mut pos = Vec::new();
    let mut row_start = vec![0u32; rows];
    for (k, &(r, y)) in profile.iter().enumerate() {
        row_start[k] = pos.len() as u32;
        if r <= 0.0 && (k == 0 || k + 1 == rows) {
            pos.push(Vec3::new(0.0, y, 0.0));
        } else {
            for i in 0..sides {
                pos.push(ring_point(TAU * i as f32 / sides as f32, r, y));
            }
        }
    }
    let at = |k: usize, i: u32| -> u32 {
        let is_pole = (k == 0 && pole_lo) || (k + 1 == rows && pole_hi);
        if is_pole { row_start[k] } else { row_start[k] + i % sides }
    };
    let u = |i: u32| i as f32 / sides as f32;
    let mid = |i: u32| (i as f32 + 0.5) / sides as f32;
    let mut polys = Vec::new();
    for k in 0..rows - 1 {
        let lo_pole = k == 0 && pole_lo;
        let hi_pole = k + 2 == rows && pole_hi;
        for i in 0..sides {
            let (a, b) = (at(k, i), at(k, i + 1));
            let (c, d) = (at(k + 1, i + 1), at(k + 1, i));
            if lo_pole {
                polys.push(Polygon::with_uvs(vec![a, c, d], vec![Vec2::new(mid(i), v(k)), Vec2::new(u(i + 1), v(k + 1)), Vec2::new(u(i), v(k + 1))]));
            } else if hi_pole {
                polys.push(Polygon::with_uvs(vec![a, b, c], vec![Vec2::new(u(i), v(k)), Vec2::new(u(i + 1), v(k)), Vec2::new(mid(i), v(k + 1))]));
            } else {
                polys.push(Polygon::with_uvs(
                    vec![a, b, c, d],
                    vec![Vec2::new(u(i), v(k)), Vec2::new(u(i + 1), v(k)), Vec2::new(u(i + 1), v(k + 1)), Vec2::new(u(i), v(k + 1))],
                ));
            }
        }
    }
    (pos, polys)
}

/// A capsule: a cylinder of `height` between two hemispheres of `radius`, `rings` rows per
/// hemisphere.
pub(crate) fn capsule(radius: f32, height: f32, sides: u32, rings: u32) -> Parts {
    let mut profile = Vec::new();
    for k in 0..=rings {
        let phi = FRAC_PI_2 * k as f32 / rings as f32;
        // From the bottom pole up to the lower equator.
        profile.push((if k == 0 { 0.0 } else { radius * phi.sin() }, -height / 2.0 - radius * phi.cos()));
    }
    for k in 0..=rings {
        let phi = FRAC_PI_2 - FRAC_PI_2 * k as f32 / rings as f32;
        profile.push((if k == rings { 0.0 } else { radius * phi.sin() }, height / 2.0 + radius * phi.cos()));
    }
    revolve(&profile, sides)
}

/// A pyramid with a rectangular base: five faces, apex up.
pub(crate) fn pyramid(width: f32, depth: f32, height: f32) -> Parts {
    let (w, d, h) = (width / 2.0, depth / 2.0, height / 2.0);
    let pos = vec![Vec3::new(-w, -h, -d), Vec3::new(w, -h, -d), Vec3::new(w, -h, d), Vec3::new(-w, -h, d), Vec3::new(0.0, h, 0.0)];
    let mut polys = vec![Polygon::with_uvs(vec![0, 1, 2, 3], vec![Vec2::new(0.0, 0.5), Vec2::new(0.5, 0.5), Vec2::new(0.5, 1.0), Vec2::new(0.0, 1.0)])];
    // Four side triangles in the other three cells of a 2×2 atlas (the last one shares the base's
    // row); each is a right-leaning triangle inside its cell.
    let cells = [Vec2::new(0.5, 0.5), Vec2::new(0.0, 0.0), Vec2::new(0.5, 0.0), Vec2::new(0.25, 0.25)];
    for (s, cell) in cells.into_iter().enumerate() {
        let (a, b) = (s as u32, (s as u32 + 1) % 4);
        polys.push(Polygon::with_uvs(vec![b, a, 4], vec![cell + Vec2::new(0.5, 0.0) * 0.98, cell, cell + Vec2::new(0.25, 0.5) * 0.98]));
    }
    (pos, polys)
}

fn unit_vertices(solid: Solid) -> Vec<Vec3> {
    let phi = (1.0 + 5f32.sqrt()) / 2.0;
    let raw: Vec<Vec3> = match solid {
        Solid::Tetrahedron => vec![Vec3::new(1.0, 1.0, 1.0), Vec3::new(1.0, -1.0, -1.0), Vec3::new(-1.0, 1.0, -1.0), Vec3::new(-1.0, -1.0, 1.0)],
        Solid::Octahedron => vec![Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z],
        Solid::Icosahedron => {
            let mut v = Vec::new();
            for (a, b) in [(1.0, phi), (1.0, -phi), (-1.0, phi), (-1.0, -phi)] {
                v.push(Vec3::new(0.0, a, b));
                v.push(Vec3::new(a, b, 0.0));
                v.push(Vec3::new(b, 0.0, a));
            }
            v
        }
        Solid::Dodecahedron => {
            let mut v = Vec::new();
            for x in [-1.0, 1.0] {
                for y in [-1.0, 1.0] {
                    for z in [-1.0, 1.0] {
                        v.push(Vec3::new(x, y, z));
                    }
                }
            }
            let inv = 1.0 / phi;
            for (a, b) in [(inv, phi), (inv, -phi), (-inv, phi), (-inv, -phi)] {
                v.push(Vec3::new(0.0, a, b));
                v.push(Vec3::new(a, b, 0.0));
                v.push(Vec3::new(b, 0.0, a));
            }
            v
        }
    };
    raw.into_iter().map(Vec3::normalize).collect()
}

/// The outward unit normal of each face, found by testing every vertex triple: a triple is a
/// face plane when no vertex lies beyond it. A solid has at most 20 vertices, so this is a
/// fixed 1140 triples.
fn face_directions(verts: &[Vec3]) -> Vec<Vec3> {
    let mut dirs: Vec<Vec3> = Vec::new();
    for a in 0..verts.len() {
        for b in a + 1..verts.len() {
            for c in b + 1..verts.len() {
                let (Some(&pa), Some(&pb), Some(&pc)) = (verts.get(a), verts.get(b), verts.get(c)) else { continue };
                let n = (pb - pa).cross(pc - pa).normalize_or_zero();
                if n == Vec3::ZERO {
                    continue;
                }
                let n = if n.dot(pa) < 0.0 { -n } else { n };
                let d = n.dot(pa);
                if verts.iter().all(|v| n.dot(*v) <= d + 1e-4) && !dirs.iter().any(|e| e.dot(n) > 0.999) {
                    dirs.push(n);
                }
            }
        }
    }
    dirs
}

/// A regular solid of circumradius `radius`. Faces are found as the vertices on the plane of each
/// face direction, wound counter-clockwise seen from outside, each in its own UV cell.
pub(crate) fn platonic(solid: Solid, radius: f32) -> Parts {
    let verts = unit_vertices(solid);
    let dirs = face_directions(&verts);
    let (_, _, nfaces) = solid.counts();
    let cols = (nfaces as f32).sqrt().ceil().max(1.0) as usize;
    let rows = nfaces.div_ceil(cols);
    let mut polys = Vec::new();
    for (fi, n) in dirs.iter().enumerate() {
        let best = verts.iter().map(|v| v.dot(*n)).fold(f32::MIN, f32::max);
        let mut on: Vec<u32> = (0..verts.len() as u32).filter(|i| verts.get(*i as usize).is_some_and(|v| v.dot(*n) > best - 1e-4)).collect();
        // Sort counter-clockwise about the face direction.
        let helper = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let e1 = n.cross(helper).normalize_or_zero();
        let e2 = n.cross(e1);
        let angle = |i: u32| {
            let p = verts.get(i as usize).copied().unwrap_or(Vec3::ZERO);
            p.dot(e2).atan2(p.dot(e1))
        };
        on.sort_by(|a, b| angle(*a).total_cmp(&angle(*b)));
        let cell = Vec2::new((fi % cols) as f32 / cols as f32, (fi / cols) as f32 / rows as f32);
        let size = Vec2::new(1.0 / cols as f32, 1.0 / rows as f32);
        let k = on.len().max(1);
        let uvs: Vec<Vec2> = (0..on.len())
            .map(|c| {
                let th = TAU * c as f32 / k as f32 + PI / 2.0;
                cell + size * (Vec2::splat(0.5) + Vec2::new(th.cos(), th.sin()) * 0.5)
            })
            .collect();
        polys.push(Polygon::with_uvs(on, uvs));
    }
    (verts.into_iter().map(|v| v * radius).collect(), polys)
}

/// A hash of an integer lattice point and a seed, as a float in 0..1 (an integer avalanche mix).
fn lattice(seed: u32, x: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (z as u32).wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// Smooth value noise at a point, 0..1.
fn value_noise(seed: u32, x: f32, z: f32) -> f32 {
    let (xf, zf) = (x.floor(), z.floor());
    let (tx, tz) = (x - xf, z - zf);
    let (xi, zi) = (xf as i32, zf as i32);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sz) = (s(tx), s(tz));
    let a = lattice(seed, xi, zi);
    let b = lattice(seed, xi.wrapping_add(1), zi);
    let c = lattice(seed, xi, zi.wrapping_add(1));
    let d = lattice(seed, xi.wrapping_add(1), zi.wrapping_add(1));
    let top = a + (b - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sz
}

/// Fractal value noise, normalised to 0..1.
pub(crate) fn fractal(seed: u32, x: f32, z: f32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut freq) = (0.0, 1.0, 0.0, 1.0);
    for o in 0..octaves {
        sum += amp * value_noise(seed.wrapping_add(o.wrapping_mul(0x1000_0001)), x * freq, z * freq);
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    if norm > 0.0 { (sum / norm).clamp(0.0, 1.0) } else { 0.5 }
}

/// A square-grid terrain in XZ: `segments` quads per side, heights from seeded fractal noise
/// with `scale` features across the width, spanning `-height/2 ..= height/2`.
pub(crate) fn landscape(width: f32, depth: f32, height: f32, segments: u32, seed: u32, scale: f32, octaves: u32) -> Parts {
    let n = segments;
    let mut pos = Vec::with_capacity(((n + 1) * (n + 1)) as usize);
    for j in 0..=n {
        for i in 0..=n {
            let (fx, fz) = (i as f32 / n as f32, j as f32 / n as f32);
            let y = (fractal(seed, fx * scale, fz * scale, octaves) - 0.5) * height;
            pos.push(Vec3::new((fx - 0.5) * width, y, (fz - 0.5) * depth));
        }
    }
    let uv = |i: u32, j: u32| Vec2::new(i as f32 / n as f32, j as f32 / n as f32);
    let mut polys = Vec::with_capacity((n * n) as usize);
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            polys.push(Polygon::with_uvs(vec![a, a + n + 1, a + n + 2, a + 1], vec![uv(i, j), uv(i, j + 1), uv(i + 1, j + 1), uv(i + 1, j)]));
        }
    }
    (pos, polys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{MAX_SEGMENTS, Primitive};
    use crate::testutil::{closed, euler};
    use modelcraft_mesh::PolyMesh;
    use proptest::prelude::*;

    fn kinds() -> Vec<Primitive> {
        ["disc", "polygon", "tube", "capsule", "pyramid", "tetrahedron", "octahedron", "dodecahedron", "icosahedron", "landscape"]
            .into_iter()
            .filter_map(Primitive::default_of)
            .collect()
    }

    /// Every corner UV lies in 0..1 and every position is finite.
    fn assert_sane(name: &str, m: &PolyMesh) {
        m.validate().unwrap();
        assert!(m.has_uvs(), "{name} lost its UVs");
        for uv in m.corner_uvs() {
            assert!((-1e-4..=1.0001).contains(&uv.x) && (-1e-4..=1.0001).contains(&uv.y), "{name}: uv {uv:?} outside 0..1");
        }
        assert!(m.positions().iter().all(|p| p.is_finite()));
    }

    #[test]
    fn nothing_needs_repairing_and_uvs_are_in_range() {
        for p in kinds() {
            let (pos, polys) = p.parts().unwrap_or_default();
            if pos.is_empty() {
                continue; // landscape: checked below
            }
            let (m, report) = PolyMesh::build(pos, polys).unwrap();
            assert!(report.is_clean(), "{}: build repaired {report:?}", p.kind());
            assert_sane(p.kind(), &m);
        }
        for p in kinds() {
            assert_sane(p.kind(), &p.build().unwrap());
        }
    }

    #[test]
    fn closed_shapes_are_closed_with_the_right_euler_number_and_face_out() {
        // (kind, Euler characteristic): spheres 2, the tube is a torus-like solid 0.
        let expected = [("capsule", 2), ("pyramid", 2), ("tetrahedron", 2), ("octahedron", 2), ("dodecahedron", 2), ("icosahedron", 2), ("tube", 0)];
        for (kind, chi) in expected {
            let m = Primitive::default_of(kind).unwrap().build().unwrap();
            assert!(closed(&m), "{kind} is open");
            assert_eq!(euler(&m), chi, "{kind}");
            let (lo, hi) = m.bounds();
            let c = (lo + hi) / 2.0;
            // Convex solids: every face points away from the centre; the tube's inner wall is
            // allowed to face the axis instead.
            let out = (0..m.face_count() as u32).filter(|f| m.face_normal(*f).dot(m.face_center(*f) - c) > 0.0).count();
            let want = if kind == "tube" { m.face_count() / 2 } else { m.face_count() };
            assert!(out >= want, "{kind}: only {out}/{} faces point out", m.face_count());
        }
    }

    #[test]
    fn open_shapes_face_up_and_have_one_boundary_loop_per_rim() {
        for kind in ["disc", "polygon", "landscape"] {
            let m = Primitive::default_of(kind).unwrap().build().unwrap();
            assert!(!closed(&m), "{kind} should be open");
            assert!((0..m.face_count() as u32).all(|f| m.face_normal(f).y > 0.0), "{kind} has a face pointing down");
        }
        let mut annulus = Primitive::default_of("disc").unwrap();
        if let Primitive::Disc { inner_radius, .. } = &mut annulus {
            *inner_radius = 0.5;
        }
        let m = annulus.build().unwrap();
        // A hole turns the disc into a ring: Euler characteristic 0.
        assert_eq!(euler(&m), 0);
        assert!((0..m.face_count() as u32).all(|f| m.face_normal(f).y > 0.0));
        assert!(m.positions().iter().all(|p| p.length() >= 0.5 - 1e-4));
        // Without caps a tube is open.
        let open = Primitive::Tube { radius: 1.0, inner_radius: 0.5, height: 1.0, sides: 8, rings: 2, caps: false }.build().unwrap();
        assert!(!closed(&open));
    }

    #[test]
    fn platonic_solids_have_the_textbook_counts_and_equal_edges() {
        for solid in Solid::ALL {
            let m = Primitive::Platonic { solid, radius: 2.0 }.build().unwrap();
            let (v, e, f) = solid.counts();
            assert_eq!((m.vertex_count(), m.edge_count(), m.face_count()), (v, e, f), "{solid:?}");
            for p in m.positions() {
                assert!((p.length() - 2.0).abs() < 1e-4, "{solid:?} vertex off the circumsphere");
            }
            let lens: Vec<f32> = (0..m.edge_count() as u32)
                .map(|e| {
                    let (a, b) = m.edge_vertices(e);
                    (m.position(a) - m.position(b)).length()
                })
                .collect();
            let (lo, hi) = lens.iter().fold((f32::MAX, f32::MIN), |(lo, hi), l| (lo.min(*l), hi.max(*l)));
            assert!(hi - lo < 1e-3, "{solid:?} edges differ: {lo}..{hi}");
        }
    }

    #[test]
    fn the_capsule_is_as_tall_as_its_parts() {
        let m = Primitive::Capsule { radius: 0.5, height: 2.0, sides: 16, rings: 6 }.build().unwrap();
        let (lo, hi) = m.bounds();
        assert!((hi.y - lo.y - 3.0).abs() < 1e-4, "height {}", hi.y - lo.y);
        assert!((hi.x - 0.5).abs() < 1e-3);
    }

    #[test]
    fn the_landscape_is_seeded_bounded_and_varies() {
        let make = |seed: u32| Primitive::Landscape { width: 4.0, depth: 4.0, height: 2.0, segments: 16, seed, scale: 3.0, octaves: 4 }.build().unwrap();
        let (a, b, a2) = (make(1), make(2), make(1));
        assert_eq!(a, a2, "the same seed gives the same terrain");
        assert_ne!(a, b, "another seed gives another terrain");
        let (lo, hi) = a.bounds();
        assert!(lo.y >= -1.0 - 1e-4 && hi.y <= 1.0 + 1e-4);
        assert!(hi.y - lo.y > 0.05, "the terrain is flat");
        assert_eq!(a.face_count(), 256);
    }

    #[test]
    fn hostile_parameters_never_panic_and_stay_bounded() {
        let nasty = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 0.0, 1e30, -1e30];
        for &x in &nasty {
            for &n in &[0u32, 1, 2, 3, u32::MAX] {
                let prims = [
                    Primitive::Disc { radius: x, inner_radius: x, sides: n, rings: n },
                    Primitive::Polygon { radius: x, sides: n },
                    Primitive::Tube { radius: x, inner_radius: x, height: x, sides: n.min(64), rings: n.min(8), caps: true },
                    Primitive::Capsule { radius: x, height: x, sides: n.min(64), rings: n.min(8) },
                    Primitive::Pyramid { width: x, depth: x, height: x },
                    Primitive::Platonic { solid: Solid::Dodecahedron, radius: x },
                    Primitive::Landscape { width: x, depth: x, height: x, segments: n.min(64), seed: n, scale: x, octaves: n },
                ];
                for p in prims {
                    let m = p.build().unwrap();
                    m.validate().unwrap();
                    assert!(m.face_count() <= (MAX_SEGMENTS * MAX_SEGMENTS * 2) as usize);
                }
            }
        }
        // The largest allowed grid stays well below the element cap.
        let big = Primitive::Landscape { width: 1.0, depth: 1.0, height: 1.0, segments: u32::MAX, seed: 0, scale: 1.0, octaves: 8 }.parts().unwrap();
        assert!(big.0.len() <= ((MAX_SEGMENTS + 1) * (MAX_SEGMENTS + 1)) as usize);
        assert!(big.0.len() < modelcraft_mesh::MAX_ELEMENTS);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn random_parameters_give_valid_meshes_with_uvs_in_range(
            kind in 0usize..10,
            r in 0.001f32..50.0, r2 in 0.0f32..60.0, h in 0.001f32..50.0,
            sides in 0u32..40, rings in 0u32..12, seed in any::<u32>(), caps in any::<bool>(),
        ) {
            let p = match kind {
                0 => Primitive::Disc { radius: r, inner_radius: r2, sides, rings },
                1 => Primitive::Polygon { radius: r, sides },
                2 => Primitive::Tube { radius: r, inner_radius: r2, height: h, sides, rings, caps },
                3 => Primitive::Capsule { radius: r, height: h, sides, rings },
                4 => Primitive::Pyramid { width: r, depth: r2.max(0.01), height: h },
                5 => Primitive::Platonic { solid: Solid::ALL[(seed % 4) as usize], radius: r },
                _ => Primitive::Landscape { width: r, depth: r2.max(0.01), height: h, segments: sides, seed, scale: h, octaves: rings },
            };
            let (pos, polys) = p.parts().unwrap_or_default();
            let (m, report) = PolyMesh::build(pos, polys).unwrap();
            prop_assert!(report.is_clean(), "{p:?}: {report:?}");
            prop_assert!(m.has_uvs());
            for uv in m.corner_uvs() {
                prop_assert!((-1e-4..=1.0001).contains(&uv.x) && (-1e-4..=1.0001).contains(&uv.y));
            }
            prop_assert!(m.validate().is_ok());
        }
    }
}
