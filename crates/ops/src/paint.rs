//! Vertex colour painting: stamps along a stroke and fills. Colours live on the corners of the
//! faces ([`PolyMesh::corner_colors`]), so a stamp paints every corner at the vertices it reaches.
//!
//! A stroke is a list of points in the mesh's space; stamping them in order gives the same result
//! every time, which is what lets a stroke be one history step that replays exactly.

use std::collections::HashMap;

use glam::Vec3;
use modelcraft_mesh::{PolyMesh, WHITE};
use serde::{Deserialize, Serialize};

use crate::{OpError, OpResult, clean_selection};

/// Most stamps in one stroke.
pub const MAX_STAMPS: usize = 100_000;
/// Most corner updates one stroke may do (a bound on the work, not only on the input).
pub const MAX_UPDATES: usize = 50_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Blend towards the brush colour.
    #[default]
    Paint,
    /// Blend back towards white (an unpainted corner).
    Erase,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Brush {
    /// Reach of one stamp, in the mesh's units.
    pub radius: f32,
    /// How far a stamp at its centre moves a corner towards the colour, 0 to 1.
    pub strength: f32,
    /// RGBA, linear 0 to 1.
    pub color: [f32; 4],
    #[serde(default)]
    pub mode: Mode,
}

fn clean_color(c: [f32; 4]) -> Option<[f32; 4]> {
    c.iter().all(|x| x.is_finite()).then(|| c.map(|x| x.clamp(0.0, 1.0)))
}

/// Corners (as half-edge ids) of each vertex.
fn corners_by_vertex(m: &PolyMesh) -> Vec<Vec<u32>> {
    let mut out = vec![Vec::new(); m.vertex_count()];
    for h in 0..m.corner_count() as u32 {
        if let Some(l) = out.get_mut(m.origin(h) as usize) {
            l.push(h);
        }
    }
    out
}

/// The colours of every corner (white where the mesh has none yet).
fn start(m: &PolyMesh) -> Vec<[f32; 4]> {
    if m.has_colors() { m.corner_colors().to_vec() } else { vec![WHITE; m.corner_count()] }
}

fn with_colors(m: &PolyMesh, colors: &[[f32; 4]]) -> OpResult {
    let mut out = m.clone();
    out.set_colors(colors)?;
    Ok(out)
}

/// Stamp the brush at each point in turn. `falloff` maps the distance from the point as a share
/// of the radius (0 at the centre, 1 at the rim) to a weight (1 at the centre).
pub fn stroke(m: &PolyMesh, points: &[Vec3], brush: &Brush, falloff: &dyn Fn(f32) -> f32) -> OpResult {
    let Some(color) = clean_color(brush.color) else { return Err(OpError::Invalid("the colour is not a number".into())) };
    if !(brush.radius.is_finite() && brush.radius > 1e-9) || !brush.strength.is_finite() {
        return Err(OpError::Invalid("the brush needs a positive radius and a strength".into()));
    }
    if points.len() > MAX_STAMPS {
        return Err(OpError::Invalid(format!("a stroke can have at most {MAX_STAMPS} stamps")));
    }
    let strength = brush.strength.clamp(0.0, 1.0);
    let target = if brush.mode == Mode::Erase { WHITE } else { color };
    let (r, n) = (brush.radius, m.vertex_count());
    if n == 0 || points.is_empty() || m.corner_count() == 0 {
        return Ok(m.clone());
    }
    // A grid of vertices, cells one radius wide.
    let key = |p: Vec3| ((p.x / r).floor().clamp(-1e9, 1e9) as i32, (p.y / r).floor().clamp(-1e9, 1e9) as i32, (p.z / r).floor().clamp(-1e9, 1e9) as i32);
    let mut grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    for v in 0..n as u32 {
        grid.entry(key(m.position(v))).or_default().push(v);
    }
    let by_vertex = corners_by_vertex(m);
    let mut colors = start(m);
    let mut updates = 0usize;
    for p in points.iter().filter(|p| p.is_finite()) {
        let (cx, cy, cz) = key(*p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &v in grid.get(&(cx + dx, cy + dy, cz + dz)).into_iter().flatten() {
                        let d = m.position(v).distance(*p);
                        if d >= r {
                            continue;
                        }
                        let w = (strength * falloff(d / r)).clamp(0.0, 1.0);
                        if w <= 0.0 {
                            continue;
                        }
                        for &h in by_vertex.get(v as usize).into_iter().flatten() {
                            updates += 1;
                            if let Some(c) = colors.get_mut(h as usize) {
                                for (x, t) in c.iter_mut().zip(target) {
                                    *x += (t - *x) * w;
                                }
                            }
                        }
                    }
                }
            }
        }
        if updates > MAX_UPDATES {
            return Err(OpError::TooLarge(updates));
        }
    }
    with_colors(m, &colors)
}

/// Set the colour of every corner at the listed vertices (all of them when the list is empty).
pub fn fill(m: &PolyMesh, verts: &[u32], color: [f32; 4]) -> OpResult {
    let Some(color) = clean_color(color) else { return Err(OpError::Invalid("the colour is not a number".into())) };
    let mut colors = start(m);
    if verts.is_empty() {
        colors.iter_mut().for_each(|c| *c = color);
    } else {
        let sel = clean_selection(verts, m.vertex_count());
        let by_vertex = corners_by_vertex(m);
        for v in sel {
            for &h in by_vertex.get(v as usize).into_iter().flatten() {
                if let Some(c) = colors.get_mut(h as usize) {
                    *c = color;
                }
            }
        }
    }
    with_colors(m, &colors)
}

/// The average colour at each vertex (white for a mesh without colours), for formats and
/// displays that keep one colour per vertex.
pub fn vertex_colors(m: &PolyMesh) -> Vec<[f32; 4]> {
    let mut sum = vec![[0.0f32; 4]; m.vertex_count()];
    let mut count = vec![0u32; m.vertex_count()];
    for h in 0..m.corner_count() as u32 {
        let (v, c) = (m.origin(h) as usize, m.color(h));
        if let (Some(s), Some(n)) = (sum.get_mut(v), count.get_mut(v)) {
            for (a, b) in s.iter_mut().zip(c) {
                *a += b;
            }
            *n += 1;
        }
    }
    sum.iter().zip(&count).map(|(s, n)| if *n == 0 { WHITE } else { s.map(|x| x / *n as f32) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;

    fn grid() -> PolyMesh {
        Primitive::Plane { width: 4.0, depth: 4.0, segments: 4 }.build().unwrap()
    }
    fn red(radius: f32, strength: f32) -> Brush {
        Brush { radius, strength, color: [1.0, 0.0, 0.0, 1.0], mode: Mode::Paint }
    }
    fn flat(_: f32) -> f32 {
        1.0
    }
    fn linear(x: f32) -> f32 {
        1.0 - x
    }

    #[test]
    fn a_stamp_colours_the_corners_it_reaches_by_the_falloff() {
        let g = grid();
        let centre = (0..g.vertex_count() as u32).find(|v| g.position(*v).length() < 1e-5).unwrap();
        let m = stroke(&g, &[Vec3::ZERO], &red(1.5, 1.0), &linear).unwrap();
        m.validate().unwrap();
        assert!(m.has_colors());
        for h in 0..m.corner_count() as u32 {
            let d = m.position(m.origin(h)).length();
            let want = if d < 1.5 { 1.0 - d / 1.5 } else { 0.0 };
            let c = m.color(h);
            // Paint towards red: green and blue drop by the weight, red stays 1.
            assert!((c[0] - 1.0).abs() < 1e-6);
            assert!((c[1] - (1.0 - want)).abs() < 1e-5, "corner at {d}: {c:?} wanted weight {want}");
        }
        let h = m.vertex_out(centre).find(|h| m.face_of(*h) != modelcraft_mesh::NONE).unwrap();
        assert_eq!(m.color(h), [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn strokes_replay_exactly_and_stamps_accumulate() {
        let g = grid();
        let pts: Vec<Vec3> = (0..8).map(|i| Vec3::new(-2.0 + i as f32 * 0.5, 0.0, 0.0)).collect();
        let a = stroke(&g, &pts, &red(1.0, 0.4), &linear).unwrap();
        let b = stroke(&g, &pts, &red(1.0, 0.4), &linear).unwrap();
        assert_eq!(a, b);
        // Two half strokes equal one whole stroke, so a stroke in several steps is the same.
        let first = stroke(&g, &pts[..4], &red(1.0, 0.4), &linear).unwrap();
        let both = stroke(&first, &pts[4..], &red(1.0, 0.4), &linear).unwrap();
        assert!(a.corner_colors().iter().zip(both.corner_colors()).all(|(x, y)| x.iter().zip(y).all(|(p, q)| (p - q).abs() < 1e-6)));
        // A second pass paints further towards the colour.
        let again = stroke(&a, &pts, &red(1.0, 0.4), &linear).unwrap();
        let sum = |m: &PolyMesh| m.corner_colors().iter().map(|c| c[1]).sum::<f32>();
        assert!(sum(&again) < sum(&a));
    }

    #[test]
    fn erase_goes_back_to_white() {
        let g = grid();
        let painted = stroke(&g, &[Vec3::ZERO], &red(1.0, 1.0), &flat).unwrap();
        let erased = stroke(&painted, &[Vec3::ZERO], &Brush { mode: Mode::Erase, ..red(1.0, 1.0) }, &flat).unwrap();
        assert!(erased.corner_colors().iter().all(|c| *c == WHITE));
    }

    #[test]
    fn fills_set_vertices_or_everything() {
        let g = grid();
        let all = fill(&g, &[], [0.0, 1.0, 0.0, 1.0]).unwrap();
        assert!(all.corner_colors().iter().all(|c| *c == [0.0, 1.0, 0.0, 1.0]));
        let one = fill(&g, &[7], [0.0, 0.0, 1.0, 1.0]).unwrap();
        let blue = (0..one.corner_count() as u32).filter(|h| one.color(*h) == [0.0, 0.0, 1.0, 1.0]).count();
        assert_eq!(blue, one.vertex_faces(7).count());
        assert_eq!(fill(&g, &[999], [0.0; 4]).unwrap().corner_colors().iter().filter(|c| **c != WHITE).count(), 0);
        assert!(fill(&g, &[], [f32::NAN, 0.0, 0.0, 1.0]).is_err());
        // Out-of-range values clamp.
        assert!(fill(&g, &[], [2.0, -1.0, 0.5, 1.0]).unwrap().corner_colors().iter().all(|c| *c == [1.0, 0.0, 0.5, 1.0]));
    }

    #[test]
    fn hostile_brushes_are_errors_not_panics() {
        let g = grid();
        for b in [red(0.0, 1.0), red(-1.0, 1.0), red(f32::NAN, 1.0), red(1.0, f32::NAN), Brush { color: [f32::NAN; 4], ..red(1.0, 1.0) }] {
            assert!(stroke(&g, &[Vec3::ZERO], &b, &flat).is_err());
        }
        let many = vec![Vec3::ZERO; MAX_STAMPS + 1];
        assert!(stroke(&g, &many, &red(1.0, 1.0), &flat).is_err());
        // Non-finite points are skipped.
        assert!(stroke(&g, &[Vec3::NAN], &red(1.0, 1.0), &flat).unwrap().corner_colors().iter().all(|c| *c == WHITE));
    }

    #[test]
    fn colours_survive_the_edits_that_keep_faces_and_get_white_on_new_ones() {
        let g = grid();
        let painted = fill(&g, &[], [1.0, 0.0, 0.0, 1.0]).unwrap();
        // Extrude one face: the old corners keep red, the new wall faces are white.
        let m = crate::edit::extrude_faces(&painted, &[0], 0.5).unwrap();
        assert!(m.has_colors());
        m.validate().unwrap();
        let red_corners = m.corner_colors().iter().filter(|c| **c == [1.0, 0.0, 0.0, 1.0]).count();
        let white = m.corner_colors().iter().filter(|c| **c == WHITE).count();
        assert!(red_corners >= painted.corner_count() && white == 16, "{red_corners} {white}");
        // Through a polygon list and back.
        let again = PolyMesh::build(painted.positions().to_vec(), painted.to_polygons()).unwrap().0;
        assert_eq!(again.corner_colors(), painted.corner_colors());
        // Per-vertex average of a uniform mesh.
        assert!(vertex_colors(&painted).iter().all(|c| *c == [1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn decimate_keeps_the_colours_it_had() {
        let ball = Primitive::Sphere { radius: 1.0, segments: 24, rings: 16 }.build().unwrap();
        let painted = stroke(&ball, &[Vec3::Y], &red(1.2, 1.0), &linear).unwrap();
        let known: Vec<[f32; 4]> = painted.corner_colors().to_vec();
        let out = crate::decimate::decimate(&painted, 0.3).unwrap();
        assert!(out.has_colors());
        out.validate().unwrap();
        assert!(out.triangle_count() < painted.triangle_count());
        // Every colour on the result was on the input (corners are kept, not invented).
        assert!(out.corner_colors().iter().all(|c| known.contains(c)));
        // The top is still red, the bottom still white.
        let at = |m: &PolyMesh, y: f32| (0..m.corner_count() as u32).filter(|h| (m.position(m.origin(*h)).y - y).abs() < 0.05).map(|h| m.color(h)).next();
        assert!(at(&out, 1.0).is_none_or(|c| c[1] < 0.5));
        assert!(at(&out, -1.0).is_none_or(|c| c == WHITE));
    }

    #[test]
    fn a_polygon_list_with_some_colours_gets_white_for_the_rest() {
        use modelcraft_mesh::Polygon;
        let pos = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::new(1.0, 1.0, 0.0)];
        let mut a = Polygon::new(vec![0, 1, 2]);
        a.colors = Some(vec![[1.0, 0.0, 0.0, 1.0]; 3]);
        let b = Polygon::new(vec![1, 3, 2]);
        let m = PolyMesh::build(pos, vec![a, b]).unwrap().0;
        m.validate().unwrap();
        assert_eq!(m.corner_colors().iter().filter(|c| **c == WHITE).count(), 3);
        assert_eq!(m.corner_colors().iter().filter(|c| **c == [1.0, 0.0, 0.0, 1.0]).count(), 3);
        // Through JSON (the project file) and back.
        let json = serde_json::to_string(&m).unwrap();
        let back: PolyMesh = serde_json::from_str(&json).unwrap();
        assert_eq!(back.corner_colors(), m.corner_colors());
        // A mesh with no colours writes none.
        let plain = Primitive::Plane { width: 1.0, depth: 1.0, segments: 1 }.build().unwrap();
        assert!(!serde_json::to_string(&plain).unwrap().contains("colors"));
    }
}
