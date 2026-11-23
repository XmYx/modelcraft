//! The boolean itself: cut both meshes along where they cross, decide for each piece whether it
//! lies inside the other mesh, and keep the pieces the operation asks for.
//!
//! Inside and outside are decided from the combinatorics, not from probing points: crossing an
//! intersection segment flips the side, so the pieces next to a segment are labelled from the
//! other mesh's face normal there, and the label spreads across every edge that is not a cut.
//! Only a part of a mesh that no cut touches at all is labelled by a generalised winding number
//! (see [`Solid::winding`]).

use std::collections::HashMap;

use glam::{DVec3, Vec2, Vec3};

use super::arrange::{Arrangement, Side, arrange};
use super::faces::{DirEdge, Kind, Piece, cut_triangle};
use super::solid::{Solid, TriMesh};
use super::{BoolError, BoolOp, Limits};

/// Counters of one run, for tests and performance notes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub triangle_pairs: usize,
    pub crossings: usize,
    pub cut_triangles: usize,
    pub pieces: usize,
    pub winding_queries: usize,
    /// Which attempt (0 = as given; later ones nudge the second mesh by a hair) succeeded.
    pub attempt: usize,
}

pub struct Outcome {
    pub mesh: TriMesh,
    pub stats: Stats,
}

struct Rec {
    side: Side,
    tri: u32,
    piece: Piece,
    /// Inside the other mesh?
    inside: Option<bool>,
}

/// Combine two closed meshes. The first attempt works on the meshes as given; if the result is
/// not a closed surface (a configuration the exact predicates resolved in a way the floating
/// point geometry could not follow) the second mesh is nudged by a tiny, fixed amount and the run
/// repeated, twice at most.
pub fn boolean(a: &TriMesh, b: &TriMesh, op: BoolOp, limits: &Limits) -> Result<Outcome, BoolError> {
    let mut last = BoolError::Inconsistent("no attempt ran".into());
    for (attempt, nudge) in [0.0f64, 3e-6, 4.1e-5].into_iter().enumerate() {
        match run(a, b, op, limits, nudge, attempt) {
            Ok(o) => return Ok(o),
            Err(e @ (BoolError::Inconsistent(_) | BoolError::NotClosed(_))) => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

fn run(a: &TriMesh, b: &TriMesh, op: BoolOp, limits: &Limits, nudge: f64, attempt: usize) -> Result<Outcome, BoolError> {
    let mut b = b.clone();
    let mut sa = Solid::prepare(a, limits, "the first mesh")?;
    if nudge != 0.0 {
        // Faces that lie in one plane (boxes that share a face, say) have no geometric answer: the
        // second mesh is moved a hair, towards the first when the result should merge or cut
        // (union, subtraction) and away from it when touching should give nothing (intersection),
        // with a sideways part so that no pair of faces stays in one plane.
        let (lo, hi) = bounds(&sa.pos);
        let d = (hi - lo).max_element().max(1e-12) * nudge;
        let centre = |p: &[Vec3]| p.iter().map(|v| v.as_dvec3()).sum::<DVec3>() / p.len().max(1) as f64;
        let toward = (centre(&a.positions) - centre(&b.positions)).normalize_or_zero();
        let toward = if op == BoolOp::Intersect { -toward } else { toward };
        let dir = (toward * 0.8 + DVec3::new(0.577_350_3, 0.707_106_8, 0.408_248_3) * 0.2).normalize_or_zero() * d;
        for p in &mut b.positions {
            *p += dir.as_vec3();
        }
    }
    let mut sb = Solid::prepare(&b, limits, "the second mesh")?;
    let (na, nb) = (sa.pos.len() as u32, sb.pos.len() as u32);
    let (lo_a, hi_a) = bounds(&sa.pos);
    let (lo_b, hi_b) = bounds(&sb.pos);
    let (lo, hi) = (lo_a.min(lo_b), hi_a.max(hi_b));
    let extent = (hi - lo).max_element();
    if !(extent > 0.0 && extent.is_finite()) {
        return Err(BoolError::Invalid("the meshes have no size".into()));
    }
    let cell = extent / f64::from(1u32 << 26);
    sa.snap(lo, cell, 0)?;
    sb.snap(lo, cell, na)?;
    let base = na + nb;
    let arr = arrange(&sa, &sb, base, limits)?;
    let solids = [&sa, &sb];
    let pos = |id: u32| -> DVec3 {
        if id < na {
            sa.pos[id as usize]
        } else if id < base {
            sb.pos[(id - na) as usize]
        } else {
            arr.verts.get((id - base) as usize).map_or(DVec3::ZERO, |v| v.pos)
        }
    };
    let mut stats = Stats { triangle_pairs: arr.pairs_tested, crossings: arr.verts.len(), ..Stats::default() };
    stats.attempt = attempt;

    // Cut every triangle.
    let mut recs: Vec<Rec> = Vec::new();
    for side in [Side::A, Side::B] {
        let solid = solids[side.index()];
        let offset = if side == Side::A { 0 } else { na };
        for t in 0..solid.tris.len() as u32 {
            let tri = solid.tris[t as usize];
            let corners = [tri[0] + offset, tri[1] + offset, tri[2] + offset];
            let seg_ids = arr.tri_segments[side.index()].get(&t);
            let Some(seg_ids) = seg_ids else {
                let edges = (0..3).map(|k| DirEdge { from: corners[k], to: corners[(k + 1) % 3], kind: Kind::Boundary }).collect();
                recs.push(Rec { side, tri: t, piece: Piece { edges, tris: vec![corners] }, inside: None });
                continue;
            };
            stats.cut_triangles += 1;
            let chains = edge_chains(solid, side, &arr, t);
            let segments: Vec<(u32, u32, u32)> = seg_ids
                .iter()
                .map(|s| {
                    let seg = arr.segments[*s as usize];
                    (seg.ends[0], seg.ends[1], seg.tri[1 - side.index()])
                })
                .collect();
            for piece in cut_triangle(corners, solid.normals[t as usize], &chains, &segments, &pos)? {
                recs.push(Rec { side, tri: t, piece, inside: None });
            }
        }
    }
    stats.pieces = recs.len();

    // Which piece lies across each uncut edge.
    let mut across: HashMap<(u32, u32), usize> = HashMap::new();
    for (i, r) in recs.iter().enumerate() {
        for e in &r.piece.edges {
            if e.kind == Kind::Boundary {
                across.insert((e.from, e.to), i);
            }
        }
    }
    // Labels next to a cut: the other mesh's normal says which side is inside it.
    for r in &mut recs {
        let n_t = solids[r.side.index()].normals[r.tri as usize];
        let other = solids[r.side.other().index()];
        for e in &r.piece.edges {
            let Kind::Cut { other: u } = e.kind else { continue };
            // Faces in one plane cut along a line the floats cannot tell; try again nudged.
            if n_t.cross(other.normals[u as usize]).length() < 1e-6 {
                return Err(BoolError::Inconsistent("two faces lie in one plane".into()));
            }
            let dir = pos(e.to) - pos(e.from);
            let left = n_t.cross(dir);
            let inside = left.dot(other.normals[u as usize]) < 0.0;
            match r.inside {
                None => r.inside = Some(inside),
                Some(prev) if prev == inside => {}
                Some(_) => return Err(BoolError::Inconsistent("a piece is on both sides of the other mesh".into())),
            }
        }
    }
    // Spread across the uncut edges.
    let links = |i: usize, recs: &[Rec]| -> Result<Vec<usize>, BoolError> {
        let mut v = Vec::new();
        for e in &recs[i].piece.edges {
            if e.kind == Kind::Boundary {
                match across.get(&(e.to, e.from)) {
                    Some(j) => v.push(*j),
                    None => return Err(BoolError::Inconsistent("a piece's edge has no neighbour".into())),
                }
            }
        }
        Ok(v)
    };
    let mut queue: Vec<usize> = (0..recs.len()).filter(|i| recs[*i].inside.is_some()).collect();
    while let Some(i) = queue.pop() {
        let Some(label) = recs[i].inside else { continue };
        for j in links(i, &recs)? {
            match recs[j].inside {
                None => {
                    recs[j].inside = Some(label);
                    queue.push(j);
                }
                Some(l) if l == label => {}
                Some(_) => return Err(BoolError::Inconsistent("labels disagree across an uncut edge".into())),
            }
        }
    }
    // Whatever no cut reaches is wholly inside or wholly outside: ask the winding number.
    let mut work: u64 = 0;
    let mut visited = vec![false; recs.len()];
    for start in 0..recs.len() {
        if recs[start].inside.is_some() || visited[start] {
            continue;
        }
        let mut stack = vec![start];
        let mut members = vec![start];
        visited[start] = true;
        while let Some(i) = stack.pop() {
            for j in links(i, &recs)? {
                if !visited[j] {
                    visited[j] = true;
                    members.push(j);
                    stack.push(j);
                }
            }
        }
        let side = recs[start].side;
        let other = solids[side.other().index()];
        // The corner point farthest from the other surface says it most reliably.
        let mut best: Option<(f64, bool)> = None;
        for i in members.iter().take(16) {
            let c = recs[*i].piece.tris.first().map_or(0, |t| t[0]);
            work += other.tris.len() as u64 * 2;
            if work > limits.max_winding_work {
                return Err(BoolError::TooLarge("too many separate parts to classify against the other mesh".into()));
            }
            stats.winding_queries += 1;
            let (w, away) = other.probe(pos(c));
            if best.is_none_or(|(d, _)| away > d) {
                best = Some((away, w > 0.5));
            }
        }
        let label = best.is_some_and(|(_, l)| l);
        for i in members {
            recs[i].inside = Some(label);
        }
    }

    // Keep what the operation wants.
    let uvs = solids[0].uvs.is_some() && solids[1].uvs.is_some();
    let patch_offset = sa.patches.iter().copied().max().map_or(0, |m| m.saturating_add(1));
    let mut out = TriMesh { uvs: uvs.then(Vec::new), ..TriMesh::default() };
    let mut remap: HashMap<u32, u32> = HashMap::new();
    for r in &recs {
        let inside = r.inside.unwrap_or(false);
        let (keep, flip) = match (op, r.side, inside) {
            (BoolOp::Union, _, false) => (true, false),
            (BoolOp::Intersect, _, true) => (true, false),
            (BoolOp::Subtract, Side::A, false) => (true, false),
            (BoolOp::Subtract, Side::B, true) => (true, true),
            _ => (false, false),
        };
        if !keep {
            continue;
        }
        let solid = solids[r.side.index()];
        let patch = solid.patches[r.tri as usize].saturating_add(if r.side == Side::B { patch_offset } else { 0 });
        let parent = solid.tris[r.tri as usize];
        let offset = if r.side == Side::A { 0 } else { na };
        let parent_ids = [parent[0] + offset, parent[1] + offset, parent[2] + offset];
        for t in &r.piece.tris {
            let mut ids = *t;
            let mut corner_uv = [Vec2::ZERO; 3];
            if let Some(u) = &solid.uvs {
                for (k, v) in ids.iter().enumerate() {
                    corner_uv[k] = uv_at(solid, r.tri as usize, &parent_ids, *v, pos(*v), u);
                }
            }
            if flip {
                ids.swap(1, 2);
                corner_uv.swap(1, 2);
            }
            let mut mapped = [0u32; 3];
            for (k, v) in ids.iter().enumerate() {
                mapped[k] = *remap.entry(*v).or_insert_with(|| {
                    out.positions.push(pos(*v).as_vec3());
                    (out.positions.len() - 1) as u32
                });
            }
            out.tris.push(mapped);
            out.patches.push(patch);
            if let Some(u) = &mut out.uvs {
                u.push(corner_uv);
            }
        }
    }
    check_closed(&out)?;
    Ok(Outcome { mesh: out, stats })
}

fn bounds(p: &[DVec3]) -> (DVec3, DVec3) {
    p.iter().fold((DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)), |(lo, hi), q| (lo.min(*q), hi.max(*q)))
}

/// The crossing vertices along each edge of triangle `t`, in the direction of its winding.
fn edge_chains(solid: &Solid, side: Side, arr: &Arrangement, t: u32) -> [Vec<u32>; 3] {
    let tri = solid.tris[t as usize];
    let mut chains: [Vec<u32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (k, chain) in chains.iter_mut().enumerate() {
        let edge = solid.tri_edges[t as usize][k];
        let Some(points) = arr.edge_points[side.index()].get(&edge) else { continue };
        let forward = tri[k] == solid.edge_ends[edge as usize][0];
        let ids = points.iter().map(|p| p.1);
        if forward {
            chain.extend(ids);
        } else {
            chain.extend(ids.rev());
        }
    }
    chains
}

/// The UV of vertex `v` (at `p`) in triangle `t`: an original corner keeps its own, anything else
/// is interpolated across the triangle.
fn uv_at(solid: &Solid, t: usize, parent: &[u32; 3], v: u32, p: DVec3, uvs: &[[Vec2; 3]]) -> Vec2 {
    let c = uvs[t];
    if let Some(k) = parent.iter().position(|x| *x == v) {
        return c[k];
    }
    let tri = solid.tris[t];
    let (a, b, d) = (solid.pos[tri[0] as usize], solid.pos[tri[1] as usize], solid.pos[tri[2] as usize]);
    let n = solid.normals[t];
    let total = (b - a).cross(d - a).dot(n);
    if total.abs() < 1e-300 {
        return c[0];
    }
    let w0 = ((b - p).cross(d - p).dot(n) / total).clamp(0.0, 1.0);
    let w1 = ((d - p).cross(a - p).dot(n) / total).clamp(0.0, 1.0);
    let w2 = ((a - p).cross(b - p).dot(n) / total).clamp(0.0, 1.0);
    let s = (w0 + w1 + w2).max(1e-300);
    let (w0, w1, w2) = ((w0 / s) as f32, (w1 / s) as f32, (w2 / s) as f32);
    c[0] * w0 + c[1] * w1 + c[2] * w2
}

/// Every directed edge of the result must meet its opposite exactly once.
fn check_closed(m: &TriMesh) -> Result<(), BoolError> {
    let mut count: HashMap<(u32, u32), i32> = HashMap::with_capacity(m.tris.len() * 3);
    for t in &m.tris {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if a == b {
                return Err(BoolError::NotClosed("a result triangle has a repeated vertex".into()));
            }
            *count.entry((a, b)).or_insert(0) += 1;
        }
    }
    for ((a, b), n) in &count {
        if *n != 1 || count.get(&(*b, *a)) != Some(&1) {
            return Err(BoolError::NotClosed("the result is not a closed, manifold surface".into()));
        }
    }
    Ok(())
}

/// Signed volume of a triangle mesh (positive when it is outward), for tests and callers.
pub fn volume(m: &TriMesh) -> f64 {
    m.tris
        .iter()
        .map(|t| {
            let p = |i: u32| m.positions.get(i as usize).map_or(DVec3::ZERO, |v| Vec3::as_dvec3(v));
            p(t[0]).dot(p(t[1]).cross(p(t[2])))
        })
        .sum::<f64>()
        / 6.0
}
