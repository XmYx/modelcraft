//! Finding where two closed meshes cross: a grid to find candidate triangle pairs, the exact
//! edge-through-triangle tests, and the shared vertices those crossings make.
//!
//! Every crossing is named by an edge of one mesh and a triangle of the other, so the same
//! crossing found from the two triangles on either side of the edge is one vertex: the result
//! is watertight by construction, whatever the arithmetic does to the vertex's position.

use std::collections::HashMap;

use glam::DVec3;

use super::exact::segment_crosses_triangle;
use super::solid::Solid;
use super::{BoolError, Limits};

/// Which mesh a thing belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Side {
    A,
    B,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::A => Side::B,
            Side::B => Side::A,
        }
    }
    pub fn index(self) -> usize {
        match self {
            Side::A => 0,
            Side::B => 1,
        }
    }
}

/// A crossing vertex: an edge of one mesh through a triangle of the other (named by that pair
/// in `vertex_of`; only where it lies is kept).
#[derive(Clone, Copy, Debug)]
pub(crate) struct CrossVertex {
    pub pos: DVec3,
}

/// One piece of the intersection curve: the segment where triangle `tri[0]` of A meets triangle
/// `tri[1]` of B, between two crossing vertices.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Segment {
    pub tri: [u32; 2],
    pub ends: [u32; 2],
}

pub(crate) struct Arrangement {
    /// Crossing vertices; their global id is `base + index`.
    pub verts: Vec<CrossVertex>,
    pub segments: Vec<Segment>,
    /// Per side, per edge: crossing vertices on it with their parameter from the edge's first end.
    pub edge_points: [HashMap<u32, Vec<(f64, u32)>>; 2],
    /// Per side, per triangle: the indices (into `segments`) of the segments in it.
    pub tri_segments: [HashMap<u32, Vec<u32>>; 2],
    pub pairs_tested: usize,
}

/// A uniform grid over triangle boxes.
struct Grid {
    origin: [i64; 3],
    cell: i64,
    dims: [i64; 3],
    cells: HashMap<i64, Vec<u32>>,
}

impl Grid {
    fn key(&self, x: i64, y: i64, z: i64) -> i64 {
        (x * self.dims[1] + y) * self.dims[2] + z
    }
    fn range(&self, lo: [i64; 3], hi: [i64; 3]) -> [(i64, i64); 3] {
        let mut r = [(0, 0); 3];
        for k in 0..3 {
            let a = ((lo[k] - self.origin[k]) / self.cell).clamp(0, self.dims[k] - 1);
            let b = ((hi[k] - self.origin[k]) / self.cell).clamp(0, self.dims[k] - 1);
            r[k] = (a, b);
        }
        r
    }
}

pub(crate) fn arrange(a: &Solid, b: &Solid, base: u32, limits: &Limits) -> Result<Arrangement, BoolError> {
    // The region where both meshes' boxes overlap bounds the grid.
    let bounds = |s: &Solid| {
        s.boxes.iter().fold(([i64::MAX; 3], [i64::MIN; 3]), |(lo, hi), (l, h)| {
            ([lo[0].min(l[0]), lo[1].min(l[1]), lo[2].min(l[2])], [hi[0].max(h[0]), hi[1].max(h[1]), hi[2].max(h[2])])
        })
    };
    let (alo, ahi) = bounds(a);
    let (blo, bhi) = bounds(b);
    let lo = [alo[0].max(blo[0]), alo[1].max(blo[1]), alo[2].max(blo[2])];
    let hi = [ahi[0].min(bhi[0]), ahi[1].min(bhi[1]), ahi[2].min(bhi[2])];
    let mut out = Arrangement {
        verts: Vec::new(),
        segments: Vec::new(),
        edge_points: [HashMap::new(), HashMap::new()],
        tri_segments: [HashMap::new(), HashMap::new()],
        pairs_tested: 0,
    };
    if lo.iter().zip(&hi).any(|(l, h)| l > h) {
        return Ok(out);
    }
    // Cell size: about the average size of B's triangles in the overlap, but never so small that
    // the grid has more than about 2^20 cells.
    let overlaps = |bx: &([i64; 3], [i64; 3])| (0..3).all(|k| bx.0[k] <= hi[k] && bx.1[k] >= lo[k]);
    let b_tris: Vec<u32> = (0..b.tris.len() as u32).filter(|t| overlaps(&b.boxes[*t as usize])).collect();
    if b_tris.is_empty() {
        return Ok(out);
    }
    let avg = b_tris
        .iter()
        .map(|t| {
            let bx = &b.boxes[*t as usize];
            (0..3).map(|k| bx.1[k] - bx.0[k]).max().unwrap_or(1)
        })
        .sum::<i64>()
        / b_tris.len() as i64;
    let span = (0..3).map(|k| hi[k] - lo[k] + 1).max().unwrap_or(1);
    let cell = avg.max(span / 100).max(1);
    let dims = [(hi[0] - lo[0]) / cell + 1, (hi[1] - lo[1]) / cell + 1, (hi[2] - lo[2]) / cell + 1];
    let mut grid = Grid { origin: lo, cell, dims, cells: HashMap::new() };
    let mut budget = limits.max_grid_entries;
    for t in &b_tris {
        let bx = &b.boxes[*t as usize];
        let r = grid.range(bx.0, bx.1);
        let n = ((r[0].1 - r[0].0 + 1) * (r[1].1 - r[1].0 + 1) * (r[2].1 - r[2].0 + 1)) as usize;
        if n > budget {
            return Err(BoolError::TooLarge("the triangles are too unevenly sized to find their overlaps".into()));
        }
        budget -= n;
        for x in r[0].0..=r[0].1 {
            for y in r[1].0..=r[1].1 {
                for z in r[2].0..=r[2].1 {
                    let key = grid.key(x, y, z);
                    grid.cells.entry(key).or_default().push(*t);
                }
            }
        }
    }
    let mut stamp = vec![u32::MAX; b.tris.len()];
    let mut visits: u64 = 0;
    let mut vertex_of: HashMap<(Side, u32, u32), u32> = HashMap::new();
    for ta in 0..a.tris.len() as u32 {
        let abox = &a.boxes[ta as usize];
        if !overlaps(abox) {
            continue;
        }
        let r = grid.range(abox.0, abox.1);
        // The cells a box covers are known before they are walked: charge them first.
        let cells = ((r[0].1 - r[0].0 + 1) * (r[1].1 - r[1].0 + 1) * (r[2].1 - r[2].0 + 1)) as u64;
        visits = visits.saturating_add(cells);
        if visits > limits.max_grid_visits {
            return Err(BoolError::TooLarge("the triangles are too long and thin to find their overlaps within the limit".into()));
        }
        for x in r[0].0..=r[0].1 {
            for y in r[1].0..=r[1].1 {
                for z in r[2].0..=r[2].1 {
                    let Some(list) = grid.cells.get(&grid.key(x, y, z)) else { continue };
                    visits = visits.saturating_add(list.len() as u64);
                    if visits > limits.max_grid_visits {
                        return Err(BoolError::TooLarge("the triangles are too long and thin to find their overlaps within the limit".into()));
                    }
                    for &tb in list {
                        if stamp[tb as usize] == ta {
                            continue;
                        }
                        stamp[tb as usize] = ta;
                        let bbox = &b.boxes[tb as usize];
                        if (0..3).any(|k| abox.0[k] > bbox.1[k] || abox.1[k] < bbox.0[k]) {
                            continue;
                        }
                        out.pairs_tested += 1;
                        if out.pairs_tested > limits.max_pairs {
                            return Err(BoolError::TooLarge("too many triangle pairs overlap".into()));
                        }
                        test_pair(a, b, ta, tb, base, &mut out, &mut vertex_of)?;
                    }
                }
            }
        }
    }
    // Sort each edge's crossing points along the edge.
    for side in &mut out.edge_points {
        for pts in side.values_mut() {
            pts.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.cmp(&q.1)));
        }
    }
    Ok(out)
}

fn test_pair(
    a: &Solid,
    b: &Solid,
    ta: u32,
    tb: u32,
    base: u32,
    out: &mut Arrangement,
    vertex_of: &mut HashMap<(Side, u32, u32), u32>,
) -> Result<(), BoolError> {
    let pa = a.tri_pts(ta as usize);
    let pb = b.tri_pts(tb as usize);
    let mut found: Vec<u32> = Vec::with_capacity(2);
    // Edges of A through triangle tb, then edges of B through triangle ta.
    for (side, solid, tri, other, other_pts, other_tri) in [(Side::A, a, ta, b, pb, tb), (Side::B, b, tb, a, pa, ta)] {
        for k in 0..3 {
            let edge = solid.tri_edges[tri as usize][k];
            let [e0, e1] = solid.edge_ends[edge as usize];
            let (p, q) = (&solid.ipts[e0 as usize], &solid.ipts[e1 as usize]);
            if !segment_crosses_triangle(p, q, other_pts[0], other_pts[1], other_pts[2]) {
                continue;
            }
            let key = (side, edge, other_tri);
            let id = match vertex_of.get(&key) {
                Some(id) => *id,
                None => {
                    let (pos, s) = crossing_position(solid, other, e0, e1, other_tri);
                    let id = base + out.verts.len() as u32;
                    out.verts.push(CrossVertex { pos });
                    out.edge_points[side.index()].entry(edge).or_default().push((s, id));
                    vertex_of.insert(key, id);
                    id
                }
            };
            found.push(id);
        }
    }
    match found.len() {
        0 => Ok(()),
        2 => {
            let seg = out.segments.len() as u32;
            out.segments.push(Segment { tri: [ta, tb], ends: [found[0], found[1]] });
            out.tri_segments[0].entry(ta).or_default().push(seg);
            out.tri_segments[1].entry(tb).or_default().push(seg);
            Ok(())
        }
        n => Err(BoolError::Inconsistent(format!("two triangles crossed in {n} places"))),
    }
}

/// Where the edge `e0`–`e1` of `solid` meets the plane of triangle `tri` of `other`, and how far
/// along the edge that is (0..1). Edges nearly in the plane fall back to the middle.
fn crossing_position(solid: &Solid, other: &Solid, e0: u32, e1: u32, tri: u32) -> (DVec3, f64) {
    let (p, q) = (solid.pos[e0 as usize], solid.pos[e1 as usize]);
    let t = other.tris[tri as usize];
    let a = other.pos[t[0] as usize];
    let n = other.normals[tri as usize];
    let den = n.dot(q - p);
    let s = if den.abs() > 1e-300 { (n.dot(a - p) / den).clamp(0.0, 1.0) } else { 0.5 };
    let s = if s.is_finite() { s } else { 0.5 };
    (p + (q - p) * s, s)
}
