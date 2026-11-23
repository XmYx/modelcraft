//! Triangulating a flat polygon with holes, for the boolean's cut triangles and for text.
//!
//! Holes are joined to the outline by bridges to the nearest vertex they can see, and the joined
//! polygon is cut into triangles by ear clipping (D. Eberly, "Triangulation by Ear Clipping",
//! Geometric Tools, 2002). When the geometry cannot decide an ear, the corner closest to flat is
//! clipped, so the result always has one triangle fewer than the bridged polygon has corners and
//! the outline stays tiled, even for poor input.
//!
//! The work is counted as it is done (every segment test of the bridging and every point test of
//! the clipping) against [`MAX_WORK`]. A piece that would need more is refused with `None`, never
//! cut up crudely: a fan over a polygon with holes is not a triangulation of it.

use glam::DVec2;

/// Most segment and point tests one triangulation may make (about a second at most).
pub const MAX_WORK: u64 = 60_000_000;

pub fn cross(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    (b - a).perp_dot(c - a)
}

/// Triangulate a counter-clockwise outline with clockwise holes. `outline` and `holes` index
/// into `p`. Returns `corners - 2` triangles for the bridged outline, however poor the geometry,
/// or `None` for an empty outline, a hole with no corners or work past [`MAX_WORK`].
pub fn triangulate(p: &[DVec2], outline: &[usize], holes: &[Vec<usize>]) -> Option<Vec<[usize; 3]>> {
    if outline.len() < 3 || holes.iter().any(|h| h.is_empty()) || outline.iter().chain(holes.iter().flatten()).any(|i| *i >= p.len()) {
        return None;
    }
    let mut work: u64 = 0;
    let hole_corners: u64 = holes.iter().map(|h| h.len() as u64).sum();
    // The bridging tests every candidate against the whole outline and every hole, so its size
    // is known before it starts.
    let est = (holes.len() as u64).saturating_mul(outline.len() as u64 + hole_corners).saturating_mul(outline.len() as u64 + hole_corners);
    if est > MAX_WORK {
        return None;
    }
    let mut poly: Vec<usize> = outline.to_vec();
    // Bridge each hole, rightmost first, to the nearest vertex it can see.
    let mut order: Vec<usize> = (0..holes.len()).collect();
    let rightmost = |h: &Vec<usize>| h.iter().copied().enumerate().max_by(|a, b| p[a.1].x.total_cmp(&p[b.1].x)).map(|(i, _)| i);
    order.sort_by(|a, b| {
        let xa = rightmost(&holes[*a]).map_or(f64::MIN, |i| p[holes[*a][i]].x);
        let xb = rightmost(&holes[*b]).map_or(f64::MIN, |i| p[holes[*b][i]].x);
        xb.total_cmp(&xa)
    });
    for hi in order {
        let hole = &holes[hi];
        let Some(m_at) = rightmost(hole) else { continue };
        let m = hole[m_at];
        // Candidate vertices by distance; the first whose segment crosses no outline or hole edge.
        let mut cands: Vec<usize> = (0..poly.len()).collect();
        cands.sort_by(|a, b| (p[poly[*a]] - p[m]).length_squared().total_cmp(&(p[poly[*b]] - p[m]).length_squared()));
        let crosses = |a: DVec2, b: DVec2, c: DVec2, d: DVec2| -> bool {
            let (d1, d2, d3, d4) = (cross(a, b, c), cross(a, b, d), cross(c, d, a), cross(c, d, b));
            ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) && d1 != 0.0 && d2 != 0.0 && d3 != 0.0 && d4 != 0.0
        };
        let mut chosen = cands.first().copied().unwrap_or(0);
        'search: for c in &cands {
            let v = poly[*c];
            work += poly.len() as u64 + hole_corners;
            if work > MAX_WORK {
                return None;
            }
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                if a == v || b == v {
                    continue;
                }
                if crosses(p[m], p[v], p[a], p[b]) {
                    continue 'search;
                }
            }
            for h in holes.iter() {
                for i in 0..h.len() {
                    let (a, b) = (h[i], h[(i + 1) % h.len()]);
                    if a == m || b == m {
                        continue;
                    }
                    if crosses(p[m], p[v], p[a], p[b]) {
                        continue 'search;
                    }
                }
            }
            chosen = *c;
            break;
        }
        let mut merged: Vec<usize> = poly[..=chosen].to_vec();
        for k in 0..=hole.len() {
            merged.push(hole[(m_at + k) % hole.len()]);
        }
        merged.push(poly[chosen]);
        merged.extend_from_slice(&poly[chosen + 1..]);
        poly = merged;
    }
    let mut tris = ear_clip(p, poly, work)?;
    remove_slivers(p, &mut tris);
    Some(tris)
}

/// Flat triangles (three corners on one line, which ear clipping leaves along straight stretches
/// of the outline) are removed by flipping their long edge: the triangle on the other side of that
/// edge is split at the middle corner instead. The tiling and the closedness do not change; a
/// flat triangle left in a surface would have no normal for the next operation to use.
fn remove_slivers(p: &[DVec2], tris: &mut [[usize; 3]]) {
    let scale = tris.iter().flatten().fold(0.0f64, |m, v| m.max(p[*v].x.abs()).max(p[*v].y.abs())).max(1e-300);
    let eps = scale * scale * 1e-13;
    let flat = |t: &[usize; 3]| cross(p[t[0]], p[t[1]], p[t[2]]).abs() <= eps;
    let len = |a: usize, b: usize| (p[a] - p[b]).length_squared();
    let mut left_alone: Vec<usize> = Vec::new();
    for _ in 0..tris.len().saturating_mul(2) {
        let Some(s) = (0..tris.len()).find(|i| flat(&tris[*i]) && !left_alone.contains(i)) else { return };
        let t = tris[s];
        // Try each edge, longest first: the triangle across it may be split at the corner
        // opposite, which takes this flat triangle away.
        let mut edges: Vec<usize> = (0..3).collect();
        edges.sort_by(|a, b| len(t[(*b + 1) % 3], t[(*b + 2) % 3]).total_cmp(&len(t[(*a + 1) % 3], t[(*a + 2) % 3])));
        let mut done = false;
        for k in edges {
            let (m, u, v) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
            // The triangle across the edge covers it the other way round: (v, u, d).
            let found = tris.iter().enumerate().find_map(|(i, n)| {
                if i == s {
                    return None;
                }
                (0..3).find(|j| n[*j] == v && n[(*j + 1) % 3] == u).map(|j| (i, n[(j + 2) % 3]))
            });
            let Some((ni, d)) = found else { continue };
            if cross(p[u], p[v], p[d]).abs() <= eps {
                continue;
            }
            // That triangle covered v→u; it is now covered as v→m→u, so the tiling is the same.
            tris[s] = [v, m, d];
            tris[ni] = [m, u, d];
            done = true;
            break;
        }
        if !done {
            left_alone.push(s);
        }
    }
}

fn ear_clip(p: &[DVec2], mut poly: Vec<usize>, mut work: u64) -> Option<Vec<[usize; 3]>> {
    let mut out = Vec::with_capacity(poly.len().saturating_sub(2));
    let scale = poly.iter().fold(0.0f64, |m, v| m.max(p[*v].x.abs()).max(p[*v].y.abs())).max(1e-300);
    let eps = scale * scale * 1e-14;
    while poly.len() > 3 {
        let n = poly.len();
        let mut pick: Option<usize> = None;
        let mut fallback: Option<(usize, f64)> = None;
        for i in 0..n {
            let (a, b, c) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
            let area = cross(p[a], p[b], p[c]);
            work += 1;
            if area > eps {
                work += n as u64;
                if work > MAX_WORK {
                    return None;
                }
                let blocked = poly.iter().any(|v| {
                    if *v == a || *v == b || *v == c || p[*v] == p[a] || p[*v] == p[b] || p[*v] == p[c] {
                        return false;
                    }
                    cross(p[a], p[b], p[*v]) >= 0.0 && cross(p[b], p[c], p[*v]) >= 0.0 && cross(p[c], p[a], p[*v]) >= 0.0
                });
                if !blocked {
                    pick = Some(i);
                    break;
                }
            }
            // The corner closest to flat is the least harmful one to cut when no ear exists.
            let flat = area.abs();
            if fallback.is_none_or(|(_, f)| flat < f) {
                fallback = Some((i, flat));
            }
        }
        let i = pick.or(fallback.map(|f| f.0)).unwrap_or(0);
        let (a, b, c) = (poly[(i + n - 1) % n], poly[i], poly[(i + 1) % n]);
        out.push([a, b, c]);
        poly.remove(i);
        work += n as u64;
    }
    if poly.len() == 3 {
        out.push([poly[0], poly[1], poly[2]]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn area(p: &[DVec2], t: &[usize; 3]) -> f64 {
        cross(p[t[0]], p[t[1]], p[t[2]]) * 0.5
    }

    #[test]
    fn a_square_with_a_square_hole_tiles_the_ring() {
        let p = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(4.0, 0.0),
            DVec2::new(4.0, 4.0),
            DVec2::new(0.0, 4.0),
            // The hole, clockwise.
            DVec2::new(1.0, 1.0),
            DVec2::new(1.0, 3.0),
            DVec2::new(3.0, 3.0),
            DVec2::new(3.0, 1.0),
        ];
        let tris = triangulate(&p, &[0, 1, 2, 3], &[vec![4, 5, 6, 7]]).unwrap();
        assert_eq!(tris.len(), 4 + 4 + 2 - 2, "outline 4, hole 4, plus the bridge's two corners, minus 2");
        let total: f64 = tris.iter().map(|t| area(&p, t)).sum();
        assert!((total - 12.0).abs() < 1e-9, "area of the ring: {total}");
        assert!(tris.iter().all(|t| area(&p, t) > -1e-12), "all counter-clockwise");
    }

    #[test]
    fn a_concave_outline_and_junk_input_still_triangulate() {
        // An L shape.
        let p = vec![DVec2::new(0.0, 0.0), DVec2::new(2.0, 0.0), DVec2::new(2.0, 1.0), DVec2::new(1.0, 1.0), DVec2::new(1.0, 2.0), DVec2::new(0.0, 2.0)];
        let tris = triangulate(&p, &[0, 1, 2, 3, 4, 5], &[]).unwrap();
        assert_eq!(tris.len(), 4);
        assert!((tris.iter().map(|t| area(&p, t)).sum::<f64>() - 3.0).abs() < 1e-9);
        // Collinear points and repeats: a triangle for every corner but two, none lost.
        let flat = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 0.0), DVec2::new(2.0, 0.0), DVec2::new(3.0, 0.0), DVec2::new(1.5, 0.0)];
        assert_eq!(triangulate(&flat, &[0, 1, 2, 3, 4], &[]).unwrap().len(), 3);
        assert!(triangulate(&flat, &[], &[]).is_none());
        assert!(triangulate(&flat, &[0, 1], &[]).is_none());
        // Holes with no outline, an empty hole and a corner out of range are refused, not indexed.
        assert!(triangulate(&flat, &[], &[vec![0, 1, 2]]).is_none());
        assert!(triangulate(&flat, &[0, 1, 2], &[vec![]]).is_none());
        assert!(triangulate(&flat, &[0, 1, 99], &[]).is_none());
    }

    proptest! {
        /// A star-shaped outline of any size, with or without a hole, is tiled exactly.
        #[test]
        fn star_polygons_are_tiled_exactly(n in 3usize..40, spikes in 0.1f64..0.9, hole in any::<bool>()) {
            let mut p: Vec<DVec2> = (0..n).map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                let r = if i % 2 == 0 { 5.0 } else { 5.0 * spikes + 0.5 };
                DVec2::new(a.cos() * r, a.sin() * r)
            }).collect();
            let outline: Vec<usize> = (0..n).collect();
            let mut holes = Vec::new();
            let mut want = (0..n).map(|i| cross(DVec2::ZERO, p[i], p[(i + 1) % n]) * 0.5).sum::<f64>();
            if hole {
                let base = p.len();
                // A small clockwise square at the centre (inside every spiky outline).
                for q in [DVec2::new(-0.2, -0.2), DVec2::new(-0.2, 0.2), DVec2::new(0.2, 0.2), DVec2::new(0.2, -0.2)] {
                    p.push(q);
                }
                holes.push((base..base + 4).collect::<Vec<_>>());
                want -= 0.16;
            }
            let tris = triangulate(&p, &outline, &holes).unwrap();
            let got: f64 = tris.iter().map(|t| area(&p, t)).sum();
            prop_assert!((got - want).abs() < 1e-6 * want.abs().max(1.0), "{got} vs {want}");
        }
    }

    #[test]
    fn a_plate_with_many_holes_is_tiled_exactly_and_too_much_work_is_refused() {
        // A square plate with a 12 × 12 grid of square holes.
        let mut p = vec![DVec2::new(0.0, 0.0), DVec2::new(100.0, 0.0), DVec2::new(100.0, 100.0), DVec2::new(0.0, 100.0)];
        let mut holes = Vec::new();
        for i in 0..12 {
            for j in 0..12 {
                let (x, y) = (4.0 + i as f64 * 8.0, 4.0 + j as f64 * 8.0);
                let base = p.len();
                for q in [DVec2::new(x, y), DVec2::new(x, y + 3.0), DVec2::new(x + 3.0, y + 3.0), DVec2::new(x + 3.0, y)] {
                    p.push(q);
                }
                holes.push((base..base + 4).collect::<Vec<_>>());
            }
        }
        let tris = triangulate(&p, &[0, 1, 2, 3], &holes).unwrap();
        let area: f64 = tris.iter().map(|t| cross(p[t[0]], p[t[1]], p[t[2]]) * 0.5).sum();
        assert!((area - (10_000.0 - 144.0 * 9.0)).abs() < 1e-6, "{area}");
        assert!(tris.iter().all(|t| cross(p[t[0]], p[t[1]], p[t[2]]) > -1e-9), "no triangle is turned over");
        // Far too many holes for the budget: refused, not fanned over a non-convex polygon.
        let many: Vec<Vec<usize>> = (0..3000).map(|_| vec![4, 5, 6, 7]).collect();
        assert!(triangulate(&p, &[0, 1, 2, 3], &many).is_none());
        // A convex ring of 20 thousand corners costs more than the budget too.
        let big: Vec<DVec2> = (0..20_000).map(|i| DVec2::from_angle(std::f64::consts::TAU * i as f64 / 20_000.0) * 100.0).collect();
        let outline: Vec<usize> = (0..big.len()).collect();
        assert!(triangulate(&big, &outline, &[]).is_none());
        let ok: Vec<DVec2> = big.iter().step_by(10).copied().collect();
        let ok_ids: Vec<usize> = (0..ok.len()).collect();
        assert_eq!(triangulate(&ok, &ok_ids, &[]).unwrap().len(), 2_000 - 2, "a convex ring of two thousand corners fits");
    }
}
