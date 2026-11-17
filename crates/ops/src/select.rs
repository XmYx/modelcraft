//! Selection helpers: edge loops and rings, boundary loops, grow and shrink.

use std::collections::BTreeSet;

use modelcraft_mesh::{NONE, PolyMesh};

/// The edge loop through `edge`: from each end, continue straight through vertices with four
/// edges (across quads). Stops at other valences, at the boundary or when the loop closes.
pub fn edge_loop(m: &PolyMesh, edge: u32) -> Vec<u32> {
    if edge as usize >= m.edge_count() {
        return Vec::new();
    }
    let mut out: BTreeSet<u32> = BTreeSet::from([edge]);
    let start = m.edge_half_edge(edge);
    for first in [start, m.twin(start)] {
        let mut h = first;
        for _ in 0..m.edge_count() {
            let v = m.dest(h);
            if m.vertex_out(v).count() != 4 || m.is_boundary_vertex(v) {
                break;
            }
            // Straight on: skip one edge around the vertex.
            let n = m.next(m.twin(m.next(h)));
            if n == NONE || m.face_of(m.twin(m.next(h))) == NONE {
                break;
            }
            let e = m.edge_of(n);
            if !out.insert(e) {
                break;
            }
            h = n;
        }
    }
    out.into_iter().collect()
}

/// Edge loops through every given edge.
pub fn loops(m: &PolyMesh, edges: &[u32]) -> Vec<u32> {
    let mut out = BTreeSet::new();
    for e in edges {
        out.extend(edge_loop(m, *e));
    }
    out.into_iter().collect()
}

/// Edge rings through every given edge.
pub fn rings(m: &PolyMesh, edges: &[u32]) -> Vec<u32> {
    let mut out = BTreeSet::new();
    for e in edges {
        out.extend(crate::edit::edge_ring(m, *e));
    }
    out.into_iter().collect()
}

/// The boundary loop (vertices in order) that contains a boundary edge.
pub fn boundary_loop(m: &PolyMesh, edge: u32) -> Option<Vec<u32>> {
    if edge as usize >= m.edge_count() || !m.is_boundary_edge(edge) {
        return None;
    }
    let h0 = m.edge_half_edge(edge);
    let start = if m.face_of(h0) == NONE { h0 } else { m.twin(h0) };
    let mut out = Vec::new();
    let mut h = start;
    for _ in 0..m.half_edge_count() {
        out.push(m.origin(h));
        h = m.next(h);
        if h == start {
            return Some(out);
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Vertex,
    Edge,
    Face,
}

fn vertices_of(m: &PolyMesh, kind: Kind, sel: &BTreeSet<u32>) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for c in sel {
        match kind {
            Kind::Vertex => {
                out.insert(*c);
            }
            Kind::Edge => {
                let (a, b) = m.edge_vertices(*c);
                out.insert(a);
                out.insert(b);
            }
            Kind::Face => out.extend(m.face_vertices(*c)),
        }
    }
    out
}

/// Add every element touching the selection.
pub fn grow(m: &PolyMesh, kind: Kind, sel: &BTreeSet<u32>) -> BTreeSet<u32> {
    let verts = vertices_of(m, kind, sel);
    let mut out = sel.clone();
    for v in verts {
        match kind {
            Kind::Vertex => out.extend(m.vertex_neighbors(v)),
            Kind::Edge => out.extend(m.vertex_out(v).map(|h| m.edge_of(h))),
            Kind::Face => out.extend(m.vertex_faces(v)),
        }
    }
    out
}

/// Remove every element on the border of the selection.
pub fn shrink(m: &PolyMesh, kind: Kind, sel: &BTreeSet<u32>) -> BTreeSet<u32> {
    // A vertex is inside when everything around it is selected.
    let inside = |v: u32| match kind {
        Kind::Vertex => sel.contains(&v) && m.vertex_neighbors(v).all(|w| sel.contains(&w)) && !m.is_boundary_vertex(v),
        Kind::Edge => m.vertex_out(v).all(|h| sel.contains(&m.edge_of(h))),
        Kind::Face => !m.is_boundary_vertex(v) && m.vertex_faces(v).all(|f| sel.contains(&f)),
    };
    sel.iter()
        .copied()
        .filter(|c| match kind {
            Kind::Vertex => inside(*c),
            Kind::Edge => {
                let (a, b) = m.edge_vertices(*c);
                inside(a) && inside(b)
            }
            Kind::Face => m.face_vertices(*c).all(inside),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::Primitive;

    #[test]
    fn loops_rings_and_borders() {
        let s = Primitive::Torus { radius: 1.0, tube: 0.3, segments: 12, sides: 8 }.build().unwrap();
        // Every torus edge loop closes: 12 edges around the ring or 8 around the tube.
        for e in 0..s.edge_count() as u32 {
            let l = edge_loop(&s, e).len();
            assert!(l == 12 || l == 8, "loop of {l}");
        }
        let p = Primitive::Plane { width: 1.0, depth: 1.0, segments: 4 }.build().unwrap();
        let border = (0..p.edge_count() as u32).find(|e| p.is_boundary_edge(*e)).unwrap();
        assert_eq!(boundary_loop(&p, border).unwrap().len(), 16);
        assert!(boundary_loop(&s, 0).is_none());
        // On a torus the ring through an edge runs the other way round from its loop.
        assert_eq!(rings(&s, &[0]).len() + edge_loop(&s, 0).len(), 20);
    }

    #[test]
    fn grow_and_shrink_faces() {
        let p = Primitive::Plane { width: 1.0, depth: 1.0, segments: 5 }.build().unwrap();
        let centre: BTreeSet<u32> = [12].into_iter().collect();
        let g = grow(&p, Kind::Face, &centre);
        assert_eq!(g.len(), 9);
        assert_eq!(shrink(&p, Kind::Face, &g), centre);
        let all: BTreeSet<u32> = (0..25).collect();
        assert_eq!(shrink(&p, Kind::Face, &all).len(), 9);
    }
}
