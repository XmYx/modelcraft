//! Cutting one triangle along the segments where the other mesh crosses it.
//!
//! The crossing vertices on the triangle's edges and inside it, and the segments between them,
//! make a planar graph; its faces are the pieces the triangle falls into, some with closed loops
//! inside them as holes. Each piece is triangulated by ear clipping (after bridging its holes to
//! its outline; D. Eberly, "Triangulation by Ear Clipping", Geometric Tools, 2002, within a work
//! budget: a piece that needs more is an error). The graph's faces are found by sorting edges by
//! angle in floating point, so the shared crossing vertices make the result closed only as long as
//! that sort agrees with the exact decisions; `check_closed` in the caller is what guarantees the
//! result is watertight, and a result that is not is an error or a retry, never returned.

use std::collections::HashMap;

use glam::{DVec2, DVec3};

use super::BoolError;
use crate::triangulate2d::triangulate;

/// What a directed edge of a piece borders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A piece of one of the triangle's own edges: the next triangle over continues the surface.
    Boundary,
    /// Part of the intersection curve, where triangle `other` of the other mesh crosses.
    Cut { other: u32 },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DirEdge {
    pub from: u32,
    pub to: u32,
    pub kind: Kind,
}

/// One piece of a cut triangle.
#[derive(Debug, Default)]
pub(crate) struct Piece {
    /// The directed edges of its outline and holes, the piece on their left.
    pub edges: Vec<DirEdge>,
    /// The piece as triangles, wound like the original triangle.
    pub tris: Vec<[u32; 3]>,
}

struct He {
    from: usize,
    to: usize,
    kind: Kind,
    twin: usize,
    /// The half-edge runs along the triangle's outline with the triangle on its left.
    inner: bool,
}

struct Cycle {
    hes: Vec<usize>,
    area: f64,
    comp: usize,
}

fn find(p: &mut [usize], mut x: usize) -> usize {
    while p[x] != x {
        p[x] = p[p[x]];
        x = p[x];
    }
    x
}

/// Cut one triangle. `corners` are the global vertex ids of its corners in winding order,
/// `chains[k]` the crossing vertices along the edge from corner `k` to corner `k + 1` in that
/// order, and `segments` the cut segments `(end, end, other triangle)` inside it.
pub(crate) fn cut_triangle(
    corners: [u32; 3],
    normal: DVec3,
    chains: &[Vec<u32>; 3],
    segments: &[(u32, u32, u32)],
    pos: &dyn Fn(u32) -> DVec3,
) -> Result<Vec<Piece>, BoolError> {
    // The outline, in order.
    let mut ring: Vec<u32> = Vec::new();
    for k in 0..3 {
        ring.push(corners[k]);
        ring.extend(chains[k].iter().copied());
    }
    let mut nodes = ring.clone();
    let mut index: HashMap<u32, usize> = nodes.iter().enumerate().map(|(i, v)| (*v, i)).collect();
    if index.len() != nodes.len() {
        return Err(BoolError::Inconsistent("a triangle edge has the same crossing twice".into()));
    }
    for (a, b, _) in segments {
        for v in [a, b] {
            if !index.contains_key(v) {
                index.insert(*v, nodes.len());
                nodes.push(*v);
            }
        }
    }
    // Planar coordinates with the normal pointing at the viewer, so counter-clockwise is the
    // triangle's winding.
    let helper = if normal.x.abs() < 0.6 { DVec3::X } else { DVec3::Y };
    let e1 = normal.cross(helper).normalize_or_zero();
    let e2 = normal.cross(e1);
    let p3: Vec<DVec3> = nodes.iter().map(|v| pos(*v)).collect();
    let p2: Vec<DVec2> = p3.iter().map(|p| DVec2::new(p.dot(e1), p.dot(e2))).collect();

    let mut hes: Vec<He> = Vec::new();
    let add_pair = |hes: &mut Vec<He>, a: usize, b: usize, kind: Kind, inner_forward: bool| {
        let f = hes.len();
        hes.push(He { from: a, to: b, kind, twin: f + 1, inner: inner_forward });
        hes.push(He { from: b, to: a, kind, twin: f, inner: false });
    };
    let n_ring = ring.len();
    for i in 0..n_ring {
        add_pair(&mut hes, i, (i + 1) % n_ring, Kind::Boundary, true);
    }
    let mut seen_cut: HashMap<(usize, usize), ()> = HashMap::new();
    for (a, b, other) in segments {
        let (ia, ib) = (index[a], index[b]);
        if ia == ib || seen_cut.insert((ia.min(ib), ia.max(ib)), ()).is_some() {
            return Err(BoolError::Inconsistent("a cut segment is empty or repeated".into()));
        }
        add_pair(&mut hes, ia, ib, Kind::Cut { other: *other }, false);
        // A cut has the piece on both sides: both directions belong to pieces.
        let last = hes.len() - 1;
        hes[last].inner = false;
    }
    // Adjacency around each node, counter-clockwise by direction.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    for (h, e) in hes.iter().enumerate() {
        adj[e.from].push(h);
    }
    let angle = |h: usize| {
        let d = p2[hes[h].to] - p2[hes[h].from];
        d.y.atan2(d.x)
    };
    for list in &mut adj {
        list.sort_by(|a, b| angle(*a).total_cmp(&angle(*b)).then(a.cmp(b)));
    }
    let mut slot = vec![0usize; hes.len()];
    for list in &adj {
        for (i, h) in list.iter().enumerate() {
            slot[*h] = i;
        }
    }
    // Trace the faces: the next half-edge of u→v turns at v to the neighbour just before u in
    // counter-clockwise order.
    let next = |h: usize| -> usize {
        let t = hes[h].twin;
        let list = &adj[hes[h].to];
        let j = slot[t];
        list[(j + list.len() - 1) % list.len()]
    };
    let mut cycle_of = vec![usize::MAX; hes.len()];
    let mut cycles: Vec<Cycle> = Vec::new();
    for start in 0..hes.len() {
        if cycle_of[start] != usize::MAX {
            continue;
        }
        let mut list = Vec::new();
        let mut h = start;
        loop {
            cycle_of[h] = cycles.len();
            list.push(h);
            h = next(h);
            if h == start {
                break;
            }
            if list.len() > hes.len() {
                return Err(BoolError::Inconsistent("a cut triangle's faces do not close".into()));
            }
        }
        let area = list
            .iter()
            .map(|h| {
                let (a, b) = (p2[hes[*h].from], p2[hes[*h].to]);
                a.x * b.y - b.x * a.y
            })
            .sum::<f64>()
            * 0.5;
        cycles.push(Cycle { hes: list, area, comp: 0 });
    }
    // Connected components of the graph.
    let mut uf: Vec<usize> = (0..nodes.len()).collect();
    for e in &hes {
        let (a, b) = (find(&mut uf, e.from), find(&mut uf, e.to));
        uf[a] = b;
    }
    for c in &mut cycles {
        c.comp = find(&mut uf, hes[c.hes[0]].from);
    }
    let outer_comp = find(&mut uf, 0);
    // The triangle's own region is the cycle made of inner half-edges; every other cycle with a
    // negative area is the outside of a closed loop and so a hole in whatever contains the loop.
    let mut faces: Vec<usize> = Vec::new();
    let mut holes: Vec<usize> = Vec::new();
    for (i, c) in cycles.iter().enumerate() {
        let is_outside = c.hes.iter().all(|h| !hes[*h].inner && matches!(hes[*h].kind, Kind::Boundary));
        if is_outside && c.comp == outer_comp {
            continue; // the unbounded face
        }
        if c.area > 0.0 || (c.area == 0.0 && c.comp == outer_comp) {
            faces.push(i);
        } else if c.comp != outer_comp {
            holes.push(i);
        } else {
            return Err(BoolError::Inconsistent("a cut triangle has a face turned inside out".into()));
        }
    }
    // Each hole goes to the smallest face of another component that contains one of its points.
    let inside = |face: usize, p: DVec2| -> bool {
        let mut hit = false;
        let hs = &cycles[face].hes;
        for h in hs {
            let (a, b) = (p2[hes[*h].from], p2[hes[*h].to]);
            if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                hit = !hit;
            }
        }
        hit
    };
    let mut face_holes: Vec<Vec<usize>> = vec![Vec::new(); cycles.len()];
    for h in &holes {
        let probe = p2[hes[cycles[*h].hes[0]].from];
        let best =
            faces.iter().filter(|f| cycles[**f].comp != cycles[*h].comp && inside(**f, probe)).min_by(|a, b| cycles[**a].area.total_cmp(&cycles[**b].area));
        match best {
            Some(f) => face_holes[*f].push(*h),
            None => return Err(BoolError::Inconsistent("a loop on a triangle lies in no face".into())),
        }
    }
    let mut pieces = Vec::new();
    for f in faces {
        let mut piece = Piece::default();
        let outline: Vec<usize> = cycles[f].hes.iter().map(|h| hes[*h].from).collect();
        let hole_rings: Vec<Vec<usize>> = face_holes[f].iter().map(|h| cycles[*h].hes.iter().map(|x| hes[*x].from).collect()).collect();
        for h in cycles[f].hes.iter().chain(face_holes[f].iter().flat_map(|h| cycles[*h].hes.iter())) {
            piece.edges.push(DirEdge { from: nodes[hes[*h].from], to: nodes[hes[*h].to], kind: hes[*h].kind });
        }
        let Some(tris) = triangulate(&p2, &outline, &hole_rings) else {
            return Err(BoolError::TooLarge("a cut triangle has too many holes or corners to retriangulate within the limit".into()));
        };
        for t in tris {
            piece.tris.push([nodes[t[0]], nodes[t[1]], nodes[t[2]]]);
        }
        pieces.push(piece);
    }
    Ok(pieces)
}
