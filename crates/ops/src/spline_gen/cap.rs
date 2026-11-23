//! Filling closed outlines with triangles (for the caps of extrusions, revolutions, lofts and
//! sweeps): nested outlines (holes), concave shapes and bad input all give a result or an error.
//!
//! The outlines are projected onto the plane facing `normal`. Outlines nested an odd number of
//! levels deep are holes. Each hole is joined to the outline around it by a bridge from its
//! right-most point to a point that can see it, and the single outline that results is cut into
//! triangles by ear clipping. The work is counted and capped, so a hostile outline gives
//! [`OpError::Invalid`] rather than a stall.

use glam::{Vec2, Vec3};

use crate::OpError;

/// Most points in all the outlines of one cap.
pub const MAX_CAP_POINTS: usize = 30_000;
/// Most outlines in one cap.
pub const MAX_CAP_CONTOURS: usize = 1_024;
/// The work (point tests) one cap may use.
const MAX_WORK: u64 = 120_000_000;
/// The work (edge tests) the nesting of a set of outlines may use.
pub const MAX_NEST_WORK: u64 = 20_000_000;

fn area2(pts: &[Vec2], idx: &[usize]) -> f32 {
    let n = idx.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts[idx[i]], pts[idx[(i + 1) % n]]);
            a.x * b.y - b.x * a.y
        })
        .sum()
}

fn inside_polygon(p: Vec2, pts: &[Vec2], idx: &[usize]) -> bool {
    let n = idx.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (pts[idx[i]], pts[idx[j]]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// A frame for the plane facing `normal`: `u × v = normal`.
pub fn plane_basis(normal: Vec3) -> (Vec3, Vec3) {
    let n = normal.try_normalize().unwrap_or(Vec3::Z);
    let u = n.any_orthonormal_vector();
    (u, n.cross(u))
}

/// The nesting of closed outlines in the plane facing `normal`: for each, whether it is a hole
/// (nested an odd number of levels deep) and its parent outline. Outlines whose box does not hold
/// the probe point are skipped without testing their edges, and the edge tests that remain are
/// counted: more than [`MAX_NEST_WORK`] is an error, so many outlines cannot stall an evaluation.
pub fn nesting(contours: &[&[Vec3]], normal: Vec3) -> Result<Vec<(bool, Option<usize>)>, OpError> {
    nesting_counted(contours, normal).map(|(n, _)| n)
}

/// [`nesting`], also returning the edge tests it made (for tests of the work bound).
pub fn nesting_counted(contours: &[&[Vec3]], normal: Vec3) -> Result<(Vec<(bool, Option<usize>)>, u64), OpError> {
    if contours.len() > MAX_CAP_CONTOURS {
        return Err(OpError::Invalid(format!("{} closed curves are too many to tell holes from outlines; at most {MAX_CAP_CONTOURS}", contours.len())));
    }
    let (u, v) = plane_basis(normal);
    let pts2: Vec<Vec<Vec2>> = contours.iter().map(|c| c.iter().map(|p| Vec2::new(p.dot(u), p.dot(v))).collect()).collect();
    let flat: Vec<Vec2> = pts2.iter().flatten().copied().collect();
    let mut offs = Vec::with_capacity(contours.len());
    let mut o = 0;
    for c in &pts2 {
        offs.push(o);
        o += c.len();
    }
    let idx: Vec<Vec<usize>> = pts2.iter().enumerate().map(|(i, c)| (offs[i]..offs[i] + c.len()).collect()).collect();
    let areas: Vec<f32> = idx.iter().map(|ix| area2(&flat, ix).abs()).collect();
    let boxes: Vec<(Vec2, Vec2)> =
        pts2.iter().map(|c| c.iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)))).collect();
    let mut out = Vec::with_capacity(contours.len());
    let mut work = 0u64;
    for i in 0..contours.len() {
        let probe = idx[i].first().map(|k| flat[*k]);
        let mut depth = 0;
        let mut parent: Option<usize> = None;
        if let Some(p) = probe {
            for j in 0..contours.len() {
                work += 1;
                if j == i || idx[j].len() < 3 || areas[j] <= areas[i] || p.cmplt(boxes[j].0).any() || p.cmpgt(boxes[j].1).any() {
                    continue;
                }
                work += idx[j].len() as u64;
                if work > MAX_NEST_WORK {
                    return Err(OpError::Invalid("these curves are nested too intricately to tell the holes from the outlines".into()));
                }
                if inside_polygon(p, &flat, &idx[j]) {
                    depth += 1;
                    if parent.is_none_or(|q| areas[j] < areas[q]) {
                        parent = Some(j);
                    }
                }
            }
        }
        out.push((depth % 2 == 1, parent));
    }
    Ok((out, work))
}

/// Triangles filling the closed `contours` as seen from the side `normal` points to: indices into
/// the points of all the contours one after the other, each triangle counter-clockwise about
/// `normal`. Holes and the outlines around them are told apart by nesting, whichever way each was
/// drawn.
pub fn fill(contours: &[&[Vec3]], normal: Vec3) -> Result<Vec<[u32; 3]>, OpError> {
    let total: usize = contours.iter().map(|c| c.len()).sum();
    if total > MAX_CAP_POINTS || contours.len() > MAX_CAP_CONTOURS {
        return Err(OpError::Invalid(format!("a cap of {total} points in {} outlines is too much; use fewer points", contours.len())));
    }
    let (u, v) = plane_basis(normal);
    let pts: Vec<Vec2> = contours.iter().flat_map(|c| c.iter().map(|p| Vec2::new(p.dot(u), p.dot(v)))).collect();
    let mut idx: Vec<Vec<usize>> = Vec::with_capacity(contours.len());
    let mut o = 0;
    for c in contours {
        idx.push((o..o + c.len()).collect());
        o += c.len();
    }
    let nest = nesting(contours, normal)?;
    // Outer outlines counter-clockwise, holes clockwise.
    for (i, ix) in idx.iter_mut().enumerate() {
        let hole = nest.get(i).is_some_and(|n| n.0);
        let a = area2(&pts, ix);
        if (a < 0.0) != hole {
            ix.reverse();
        }
    }
    let mut work = 0u64;
    let mut tris = Vec::new();
    for outer in 0..contours.len() {
        if nest[outer].0 || idx[outer].len() < 3 {
            continue;
        }
        // An outline with no area (flat on edge to the cap's plane) has nothing to fill.
        let (lo, hi) = idx[outer].iter().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(lo, hi), k| (lo.min(pts[*k]), hi.max(pts[*k])));
        let size = (hi - lo).max_element();
        if !size.is_finite() || area2(&pts, &idx[outer]).abs() <= 1e-9 * size * size {
            continue;
        }
        let mut ring = idx[outer].clone();
        let mut holes: Vec<usize> = (0..contours.len()).filter(|h| nest[*h].1 == Some(outer) && idx[*h].len() >= 3).collect();
        // Right-most hole first: bridges then never cross a hole still to be joined.
        holes.sort_by(|a, b| {
            let mx = |h: &usize| idx[*h].iter().map(|k| pts[*k].x).fold(f32::MIN, f32::max);
            mx(b).total_cmp(&mx(a))
        });
        for h in holes {
            bridge(&mut ring, &idx[h], &pts, &mut work)?;
        }
        ear_clip(&ring, &pts, &mut tris, &mut work)?;
    }
    Ok(tris)
}

fn cross(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b - a).perp_dot(c - a)
}

fn in_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
    d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0
}

/// Join a hole (clockwise) to the outline `ring` (counter-clockwise) by a cut from the hole's
/// right-most point to a point of the outline that can see it.
fn bridge(ring: &mut Vec<usize>, hole: &[usize], pts: &[Vec2], work: &mut u64) -> Result<(), OpError> {
    let Some((hi, &m_idx)) = hole.iter().enumerate().max_by(|a, b| pts[*a.1].x.total_cmp(&pts[*b.1].x)) else { return Ok(()) };
    let m = pts[m_idx];
    // The outline edge the ray from `m` to +x hits first.
    let n = ring.len();
    let mut best: Option<(f32, usize)> = None; // (x of the hit, index in ring of the edge's start)
    for i in 0..n {
        let (a, b) = (pts[ring[i]], pts[ring[(i + 1) % n]]);
        if (a.y <= m.y && b.y >= m.y || b.y <= m.y && a.y >= m.y) && (a.y - b.y).abs() > 1e-12 {
            let t = (m.y - a.y) / (b.y - a.y);
            let x = a.x + t * (b.x - a.x);
            if x >= m.x && best.is_none_or(|(bx, _)| x < bx) {
                best = Some((x, i));
            }
        }
    }
    *work += n as u64;
    let Some((hit_x, edge)) = best else { return Err(OpError::Invalid("a hole lies outside its outline".into())) };
    let (ea, eb) = (edge, (edge + 1) % n);
    // The end of the edge that is further right is the first candidate to bridge to.
    let mut p_pos = if pts[ring[ea]].x > pts[ring[eb]].x { ea } else { eb };
    let hit = Vec2::new(hit_x, m.y);
    // A reflex outline point inside the triangle (m, hit, p) would block the view: bridge to the one
    // closest in direction to the ray instead.
    let (pa, pb, pc) = if pts[ring[p_pos]].y < m.y { (m, pts[ring[p_pos]], hit) } else { (m, hit, pts[ring[p_pos]]) };
    let mut best_angle = f32::MAX;
    for k in 0..n {
        if k == p_pos {
            continue;
        }
        let q = pts[ring[k]];
        if in_triangle(q, pa, pb, pc) && q != pts[ring[p_pos]] && q != hit {
            let d = q - m;
            let ang = (d.y.abs() / d.x.max(1e-12)).atan();
            if ang < best_angle || (ang == best_angle && d.length_squared() < (pts[ring[p_pos]] - m).length_squared()) {
                best_angle = ang;
                p_pos = k;
            }
        }
    }
    *work += n as u64;
    if *work > MAX_WORK {
        return Err(OpError::Invalid("the cap is too detailed to fill".into()));
    }
    // ... P, M, (hole around), M, P ...
    let h = hole.len();
    let mut out = Vec::with_capacity(n + h + 2);
    out.extend_from_slice(&ring[..=p_pos]);
    for k in 0..=h {
        out.push(hole[(hi + k) % h]);
    }
    out.push(ring[p_pos]);
    out.extend_from_slice(&ring[p_pos + 1..]);
    *ring = out;
    Ok(())
}

/// Cut the counter-clockwise outline `ring` into triangles.
fn ear_clip(ring: &[usize], pts: &[Vec2], tris: &mut Vec<[u32; 3]>, work: &mut u64) -> Result<(), OpError> {
    let n = ring.len();
    let mut prev: Vec<usize> = (0..n).map(|i| (i + n - 1) % n).collect();
    let mut next: Vec<usize> = (0..n).map(|i| (i + 1) % n).collect();
    let mut alive = n;
    let mut cur = 0usize;
    let mut since_clip = 0usize;
    let p = |i: usize| pts[ring[i]];
    while alive > 3 {
        // Every pass counts, ears or not: a degenerate outline that clips nothing for a round at a
        // time must still run out of budget.
        *work += 1;
        let (a, b, c) = (prev[cur], cur, next[cur]);
        let convex = cross(p(a), p(b), p(c)) > 1e-12;
        let mut ear = convex;
        if ear {
            // No other remaining point may lie in the triangle (coincident bridge points do not count).
            let mut k = next[c];
            while k != a {
                *work += 1;
                let q = p(k);
                if q != p(a) && q != p(b) && q != p(c) && in_triangle(q, p(a), p(b), p(c)) {
                    ear = false;
                    break;
                }
                k = next[k];
            }
        }
        if *work > MAX_WORK {
            return Err(OpError::Invalid("the cap is too detailed to fill".into()));
        }
        if ear || since_clip > alive {
            // After a full round without an ear (a degenerate outline) the corner is cut anyway,
            // so that the loop always ends.
            if convex || ear {
                tris.push([ring[a] as u32, ring[b] as u32, ring[c] as u32]);
            }
            next[a] = c;
            prev[c] = a;
            alive -= 1;
            cur = c;
            since_clip = 0;
        } else {
            cur = c;
            since_clip += 1;
        }
    }
    let (a, b, c) = (cur, next[cur], next[next[cur]]);
    if alive == 3 && cross(p(a), p(b), p(c)) > 1e-12 {
        tris.push([ring[a] as u32, ring[b] as u32, ring[c] as u32]);
    }
    Ok(())
}
