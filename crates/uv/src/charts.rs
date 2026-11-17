//! Islands (charts): the pieces a mesh falls into when it is cut along its seams and its own
//! boundary, and the *UV vertices* they are made of.
//!
//! A UV vertex is a mesh vertex as seen from one side of the cuts: the corners around a vertex
//! that are connected through uncut edges share one UV vertex, so a vertex on a seam has two (or
//! more) of them and a "bow-tie" vertex that touches an island twice gets one per wedge.
//!
//! [`cut_to_disks`] adds the extra cuts an island needs to become a topological disc, which is
//! what an unwrap requires: a closed sphere gets a path, a ring gets a line joining its two
//! borders, a torus gets two loops. They come from the complement of a spanning tree of the
//! faces (the "tree–cotree" cut graph, Eppstein 2003) with its dangling branches pruned.

use std::collections::HashMap;

use glam::{DVec2, DVec3};
use modelcraft_mesh::{NONE, PolyMesh};

use crate::seams::SeamSet;
use crate::{MAX_FACES, UvError};

fn find(p: &mut [u32], mut x: u32) -> u32 {
    loop {
        let parent = p.get(x as usize).copied().unwrap_or(x);
        if parent == x {
            return x;
        }
        let grand = p.get(parent as usize).copied().unwrap_or(parent);
        if let Some(s) = p.get_mut(x as usize) {
            *s = grand;
        }
        x = grand;
    }
}

fn union(p: &mut [u32], a: u32, b: u32) {
    let (ra, rb) = (find(p, a), find(p, b));
    if ra != rb
        && let Some(s) = p.get_mut(ra.max(rb) as usize)
    {
        *s = ra.min(rb);
    }
}

/// One island: a set of faces connected through uncut edges.
#[derive(Clone, Debug, PartialEq)]
pub struct Island {
    /// Its faces, in increasing order.
    pub faces: Vec<u32>,
    /// Its UV vertices, in increasing order.
    pub vertices: Vec<u32>,
    /// Euler characteristic `V − E + F` of the island as a surface with boundary.
    pub euler: i64,
    /// Number of closed boundary loops (zero for a closed surface).
    pub boundary_loops: usize,
}

impl Island {
    /// A disc has Euler characteristic 1 and one boundary loop; only discs unwrap cleanly.
    pub fn is_disc(&self) -> bool {
        self.euler == 1 && self.boundary_loops == 1
    }
}

/// The islands of a mesh under a seam set, with the corner → UV vertex map.
#[derive(Clone, Debug, Default)]
pub struct Charts {
    /// UV vertex of every corner (indexed by corner id, see [`PolyMesh::corner_count`]).
    pub corner_vertex: Vec<u32>,
    /// The mesh vertex each UV vertex comes from.
    pub vertex_source: Vec<u32>,
    /// Island of every face.
    pub face_island: Vec<u32>,
    pub islands: Vec<Island>,
    /// Whether each edge is a cut (a seam or on the mesh boundary).
    pub edge_cut: Vec<bool>,
}

impl Charts {
    /// Split `mesh` along `seams` and its boundary.
    pub fn build(mesh: &PolyMesh, seams: &SeamSet) -> Result<Charts, UvError> {
        let nf = mesh.face_count();
        if nf > MAX_FACES {
            return Err(UvError::TooLarge(nf));
        }
        let nc = mesh.corner_count();
        let edge_cut: Vec<bool> = (0..mesh.edge_count() as u32).map(|e| mesh.is_boundary_edge(e) || seams.contains(mesh, e)).collect();
        let cut = |h: u32| edge_cut.get(mesh.edge_of(h) as usize).copied().unwrap_or(true);

        let mut cdsu: Vec<u32> = (0..nc as u32).collect();
        let mut fdsu: Vec<u32> = (0..nf as u32).collect();
        for h in 0..nc as u32 {
            let t = mesh.twin(h);
            if h >= t || mesh.face_of(t) == NONE || cut(h) {
                continue;
            }
            union(&mut cdsu, h, mesh.next(t));
            union(&mut cdsu, mesh.next(h), t);
            union(&mut fdsu, mesh.face_of(h), mesh.face_of(t));
        }

        // Number the roots in order of first appearance.
        let mut vid = vec![NONE; nc];
        let mut corner_vertex = vec![0u32; nc];
        let mut vertex_source = Vec::new();
        for c in 0..nc as u32 {
            let r = find(&mut cdsu, c);
            let id = match vid.get(r as usize).copied() {
                Some(id) if id != NONE => id,
                _ => {
                    let id = vertex_source.len() as u32;
                    if let Some(s) = vid.get_mut(r as usize) {
                        *s = id;
                    }
                    vertex_source.push(mesh.origin(c));
                    id
                }
            };
            if let Some(s) = corner_vertex.get_mut(c as usize) {
                *s = id;
            }
        }
        let mut iid = vec![NONE; nf];
        let mut face_island = vec![0u32; nf];
        let mut islands: Vec<Island> = Vec::new();
        for f in 0..nf as u32 {
            let r = find(&mut fdsu, f);
            let id = match iid.get(r as usize).copied() {
                Some(id) if id != NONE => id,
                _ => {
                    let id = islands.len() as u32;
                    if let Some(s) = iid.get_mut(r as usize) {
                        *s = id;
                    }
                    islands.push(Island { faces: Vec::new(), vertices: Vec::new(), euler: 0, boundary_loops: 0 });
                    id
                }
            };
            if let Some(s) = face_island.get_mut(f as usize) {
                *s = id;
            }
            if let Some(i) = islands.get_mut(id as usize) {
                i.faces.push(f);
            }
        }

        // Statistics per island.
        let mut out_cut: HashMap<u32, u32> = HashMap::new();
        for isl in islands.iter_mut() {
            let mut verts: Vec<u32> = Vec::new();
            let (mut corners, mut cut_corners) = (0i64, 0i64);
            out_cut.clear();
            for &f in &isl.faces {
                for h in mesh.face_half_edges(f) {
                    corners += 1;
                    verts.push(corner_vertex.get(h as usize).copied().unwrap_or(0));
                    if cut(h) {
                        cut_corners += 1;
                        out_cut.insert(corner_vertex.get(h as usize).copied().unwrap_or(0), h);
                    }
                }
            }
            verts.sort_unstable();
            verts.dedup();
            let edges = (corners - cut_corners) / 2 + cut_corners;
            isl.euler = verts.len() as i64 - edges + isl.faces.len() as i64;
            isl.vertices = verts;
            // Boundary loops: follow cut corners from head to tail.
            let mut seen: HashMap<u32, ()> = HashMap::new();
            let starts: Vec<u32> = out_cut.values().copied().collect();
            for s in starts {
                if seen.contains_key(&s) {
                    continue;
                }
                isl.boundary_loops += 1;
                let mut c = s;
                for _ in 0..=corners {
                    seen.insert(c, ());
                    let nv = corner_vertex.get(mesh.next(c) as usize).copied().unwrap_or(0);
                    match out_cut.get(&nv) {
                        Some(n) if *n != s => c = *n,
                        _ => break,
                    }
                }
            }
        }
        Ok(Charts { corner_vertex, vertex_source, face_island, islands, edge_cut })
    }

    /// Islands that are not discs.
    pub fn non_disc_islands(&self) -> usize {
        self.islands.iter().filter(|i| !i.is_disc()).count()
    }
}

/// The triangles of a face as triples of corner ids, in the face's winding order.
///
/// A triangle is itself. A quad is split along its shorter diagonal, unless that diagonal lies
/// outside a concave quad, and a larger polygon is cut by ear clipping (Meisters 1975: every
/// simple polygon with more than three corners has an ear, a convex corner whose triangle holds no
/// other corner). Polygons of more than [`EAR_CLIP_LIMIT`] corners, and polygons that are not
/// simple, fall back to a fan from the first corner.
pub fn face_triangles(mesh: &PolyMesh, f: u32, out: &mut Vec<[u32; 3]>) {
    let c: Vec<u32> = mesh.face_half_edges(f).collect();
    let n = c.len();
    let at = |i: usize| c.get(i).copied().unwrap_or(NONE);
    let p = |i: usize| mesh.position(mesh.origin(at(i))).as_dvec3();
    match n {
        0..=2 => {}
        3 => out.push([at(0), at(1), at(2)]),
        _ => {
            // Work in the plane of the polygon: project onto its Newell normal.
            let mut normal = DVec3::ZERO;
            for i in 0..n {
                let (a, b) = (p(i), p((i + 1) % n));
                normal += DVec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
            }
            let normal = normal.normalize_or_zero();
            let fan = |out: &mut Vec<[u32; 3]>| {
                for i in 1..n - 1 {
                    out.push([at(0), at(i), at(i + 1)]);
                }
            };
            if normal == DVec3::ZERO {
                return fan(out);
            }
            let helper = if normal.x.abs() < 0.9 { DVec3::X } else { DVec3::Y };
            let u = normal.cross(helper).normalize_or_zero();
            let w = normal.cross(u);
            // 2D points, counter-clockwise when seen along the normal: u, w, normal are right-handed.
            let q: Vec<DVec2> = (0..n).map(|i| DVec2::new(p(i).dot(u), p(i).dot(w))).collect();
            let convex =
                |a: usize, b: usize, d: usize| (q[b] - q[a]).perp_dot(q[d] - q[a]) > 1e-12 * (q[b] - q[a]).length_squared().max((q[d] - q[a]).length_squared());
            if n == 4 {
                // Shorter diagonal first; both triangles must wind with the polygon.
                let (d02, d13) = (q[0].distance_squared(q[2]), q[1].distance_squared(q[3]));
                let options: [[[usize; 3]; 2]; 2] = [[[0, 1, 2], [0, 2, 3]], [[1, 2, 3], [1, 3, 0]]];
                let order = if d02 <= d13 { [0, 1] } else { [1, 0] };
                for o in order {
                    if options[o].iter().all(|t| convex(t[0], t[1], t[2])) {
                        for t in options[o] {
                            out.push(t.map(at));
                        }
                        return;
                    }
                }
                return fan(out);
            }
            if n > EAR_CLIP_LIMIT {
                return fan(out);
            }
            let mut ring: Vec<usize> = (0..n).collect();
            let inside = |a: usize, b: usize, d: usize, x: usize| {
                let (e1, e2, e3) = ((q[b] - q[a]).perp_dot(q[x] - q[a]), (q[d] - q[b]).perp_dot(q[x] - q[b]), (q[a] - q[d]).perp_dot(q[x] - q[d]));
                e1 >= 0.0 && e2 >= 0.0 && e3 >= 0.0
            };
            let first = out.len();
            while ring.len() > 3 {
                let m = ring.len();
                let ear = (0..m).find(|&i| {
                    let (a, b, d) = (ring[(i + m - 1) % m], ring[i], ring[(i + 1) % m]);
                    convex(a, b, d) && !ring.iter().any(|&x| x != a && x != b && x != d && inside(a, b, d, x))
                });
                let Some(i) = ear else {
                    // Not a simple polygon: give up on clipping.
                    out.truncate(first);
                    return fan(out);
                };
                out.push([at(ring[(i + m - 1) % m]), at(ring[i]), at(ring[(i + 1) % m])]);
                ring.remove(i);
            }
            if let [a, b, d] = ring[..] {
                out.push([at(a), at(b), at(d)]);
            }
        }
    }
}

/// Largest polygon (in corners) that is ear-clipped; bigger ones are fanned.
const EAR_CLIP_LIMIT: usize = 64;

/// What [`cut_to_disks`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CutReport {
    /// Islands that were not discs.
    pub islands_cut: usize,
    /// Edges added to the seam set.
    pub edges_added: usize,
}

fn pos(mesh: &PolyMesh, v: u32) -> DVec3 {
    mesh.position(v).as_dvec3()
}

/// Add the seams needed to make every island a disc (see the module docs). Existing seams are
/// kept; an island that is already a disc is left alone.
pub fn cut_to_disks(mesh: &PolyMesh, seams: &SeamSet) -> Result<(SeamSet, CutReport), UvError> {
    let charts = Charts::build(mesh, seams)?;
    let mut out = seams.clone();
    let mut report = CutReport::default();
    let mut deg = vec![0u32; mesh.vertex_count()];
    for (e, c) in charts.edge_cut.iter().enumerate() {
        if *c {
            let (a, b) = mesh.edge_vertices(e as u32);
            for v in [a, b] {
                if let Some(d) = deg.get_mut(v as usize) {
                    *d += 1;
                }
            }
        }
    }
    let mut visited = vec![false; mesh.face_count()];
    let mut tree = vec![false; mesh.edge_count()];
    for isl in charts.islands.iter().filter(|i| !i.is_disc()) {
        report.islands_cut += 1;
        // Spanning tree of the faces through uncut edges.
        let Some(&root) = isl.faces.first() else { continue };
        let mut stack = vec![root];
        if let Some(v) = visited.get_mut(root as usize) {
            *v = true;
        }
        while let Some(f) = stack.pop() {
            for h in mesh.face_half_edges(f) {
                let e = mesh.edge_of(h) as usize;
                let g = mesh.face_of(mesh.twin(h));
                if g == NONE || charts.edge_cut.get(e).copied().unwrap_or(true) || visited.get(g as usize).copied().unwrap_or(true) {
                    continue;
                }
                if let Some(v) = visited.get_mut(g as usize) {
                    *v = true;
                }
                if let Some(t) = tree.get_mut(e) {
                    *t = true;
                }
                stack.push(g);
            }
        }
        // The complement: uncut edges of the island that the tree does not cross.
        let mut cand: Vec<u32> = Vec::new();
        for &f in &isl.faces {
            for h in mesh.face_half_edges(f) {
                let e = mesh.edge_of(h);
                if !charts.edge_cut.get(e as usize).copied().unwrap_or(true) && !tree.get(e as usize).copied().unwrap_or(true) && mesh.edge_half_edge(e) == h {
                    cand.push(e);
                }
            }
        }
        let all_candidates = cand.clone();
        let mut inc: HashMap<u32, Vec<u32>> = HashMap::new();
        for &e in &cand {
            let (a, b) = mesh.edge_vertices(e);
            for v in [a, b] {
                inc.entry(v).or_default().push(e);
                if let Some(d) = deg.get_mut(v as usize) {
                    *d += 1;
                }
            }
        }
        let mut alive: HashMap<u32, ()> = cand.iter().map(|e| (*e, ())).collect();
        let mut work: Vec<u32> = inc.keys().copied().filter(|v| deg.get(*v as usize) == Some(&1)).collect();
        while let Some(v) = work.pop() {
            if deg.get(v as usize) != Some(&1) {
                continue;
            }
            let Some(e) = inc.get(&v).and_then(|l| l.iter().copied().find(|e| alive.contains_key(e))) else { continue };
            alive.remove(&e);
            let (a, b) = mesh.edge_vertices(e);
            for w in [a, b] {
                if let Some(d) = deg.get_mut(w as usize) {
                    *d = d.saturating_sub(1);
                }
                if w != v && deg.get(w as usize) == Some(&1) {
                    work.push(w);
                }
            }
        }
        cand.retain(|e| alive.contains_key(e));
        if cand.is_empty() && isl.boundary_loops == 0 && isl.euler == 2 {
            // A closed sphere: cut along the longest path of the spanning-tree complement.
            cand = tree_diameter(mesh, &all_candidates);
            for &e in &cand {
                let (a, b) = mesh.edge_vertices(e);
                for v in [a, b] {
                    if let Some(d) = deg.get_mut(v as usize) {
                        *d += 1;
                    }
                }
            }
        }
        for e in cand {
            if out.mark(mesh, e) {
                report.edges_added += 1;
            }
        }
    }
    Ok((out, report))
}

/// The edges of the longest path (by length) in a forest of edges: two sweeps, from any vertex to
/// the farthest one, then from there to the farthest one again.
fn tree_diameter(mesh: &PolyMesh, edges: &[u32]) -> Vec<u32> {
    let mut adj: HashMap<u32, Vec<(u32, u32, f64)>> = HashMap::new();
    for &e in edges {
        let (a, b) = mesh.edge_vertices(e);
        let l = pos(mesh, a).distance(pos(mesh, b));
        adj.entry(a).or_default().push((b, e, l));
        adj.entry(b).or_default().push((a, e, l));
    }
    let Some(&start) = adj.keys().min() else { return Vec::new() };
    let sweep = |from: u32| -> (u32, HashMap<u32, (u32, u32)>) {
        let mut dist: HashMap<u32, f64> = HashMap::new();
        let mut parent: HashMap<u32, (u32, u32)> = HashMap::new();
        dist.insert(from, 0.0);
        let mut stack = vec![from];
        let (mut far, mut far_d) = (from, 0.0);
        while let Some(v) = stack.pop() {
            let dv = dist.get(&v).copied().unwrap_or(0.0);
            for (w, e, l) in adj.get(&v).into_iter().flatten() {
                if dist.contains_key(w) {
                    continue;
                }
                dist.insert(*w, dv + l);
                parent.insert(*w, (v, *e));
                if dv + l > far_d {
                    far_d = dv + l;
                    far = *w;
                }
                stack.push(*w);
            }
        }
        (far, parent)
    };
    let (a, _) = sweep(start);
    let (b, parent) = sweep(a);
    let mut path = Vec::new();
    let mut c = b;
    while let Some((p, e)) = parent.get(&c) {
        path.push(*e);
        c = *p;
        if path.len() > edges.len() {
            break;
        }
    }
    path
}
