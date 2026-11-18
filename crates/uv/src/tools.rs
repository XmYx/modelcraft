//! Hand tools for a finished layout: relax, straighten and stitch islands together.
//!
//! - [`relax_corners`]: a few iterations of the local/global flattening of Liu, Zhang, Xu, Liu and
//!   Gortler, "A Local/Global Approach to Mesh Parameterization", Eurographics SGP 2008. The *local*
//!   step fits every triangle's current map to the closest rotation (as-rigid-as-possible, which
//!   keeps lengths and angles) or the closest similarity (as-similar-as-possible, which keeps
//!   angles only); the *global* step moves the free vertices to minimise the distance to those
//!   fits, a sparse symmetric positive-definite system with cotangent weights that is the same
//!   for every iteration, so it is factored once (`cholesky.rs`) and solved a few times.
//!   Vertices outside the selection are held in place; an island with nothing held is anchored at
//!   its centre and turned back to where it was so it does not drift.
//! - [`straighten_corners`]: selected chains of UV edges are laid on the line between their ends,
//!   keeping the ratios of the edge lengths.
//! - [`stitch_islands`]: sew islands along shared mesh edges by moving one island onto the other
//!   (a least-squares similarity of the edge ends: Umeyama 1991, in the plane), then weld.

use std::collections::{HashMap, HashSet};

use glam::{DVec2, DVec3, Vec2};
use modelcraft_mesh::{NONE, PolyMesh};

use crate::UvError;
use crate::charts::{Charts, face_triangles};
use crate::cholesky::{Factor, nested_dissection};
use crate::lscm::local_frame;
use crate::seams::SeamSet;
use crate::sparse::{BlockCsr, solve_cg};

const MAX_FACTOR_NNZ: usize = 24_000_000;
/// Most relaxation iterations one call runs.
pub const MAX_RELAX_ITERATIONS: u32 = 500;

/// What each triangle is fitted to in the local step of [`relax_corners`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RelaxMode {
    /// The closest rotation: keeps lengths and angles (as rigid as possible).
    #[default]
    Rigid,
    /// The closest similarity (rotation and a uniform scale per triangle): keeps angles and lets
    /// triangle sizes change (as similar as possible).
    Conformal,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelaxOptions {
    pub iterations: u32,
    pub mode: RelaxMode,
}

impl Default for RelaxOptions {
    fn default() -> Self {
        RelaxOptions { iterations: 10, mode: RelaxMode::Rigid }
    }
}

/// What [`relax_corners`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RelaxReport {
    /// Islands that had free vertices and were relaxed.
    pub islands: usize,
    pub iterations: usize,
    /// Distortion energy `Σ A (|J − L|²)` over those islands before and after (`L` is the fit of the
    /// mode); lower is better, and it never increases.
    pub energy_before: f64,
    pub energy_after: f64,
    /// An island could not be relaxed (a failed solve) and was left alone.
    pub skipped: usize,
}

/// Position of every UV vertex, with v up (the solvers' convention; ModelCraft stores v down).
fn uv_vertex_positions(mesh: &PolyMesh, charts: &Charts) -> Vec<DVec2> {
    let mut pos = vec![DVec2::ZERO; charts.vertex_source.len()];
    let mut have = vec![false; pos.len()];
    for c in 0..mesh.corner_count() as u32 {
        let Some(&v) = charts.corner_vertex.get(c as usize) else { continue };
        if let (Some(h), Some(p)) = (have.get_mut(v as usize), pos.get_mut(v as usize))
            && !*h
        {
            *h = true;
            *p = DVec2::new(f64::from(mesh.uv(c).x), -f64::from(mesh.uv(c).y));
        }
    }
    pos
}

fn write_positions(mesh: &PolyMesh, charts: &Charts, pos: &[DVec2]) -> Result<PolyMesh, UvError> {
    let mut uvs = mesh.corner_uvs().to_vec();
    for (c, u) in uvs.iter_mut().enumerate() {
        if let Some(p) = charts.corner_vertex.get(c).and_then(|v| pos.get(*v as usize))
            && p.is_finite()
        {
            *u = Vec2::new(p.x as f32, (-p.y) as f32);
        }
    }
    let mut out = mesh.clone();
    out.set_uvs(&uvs)?;
    Ok(out)
}

struct Tri {
    l: [usize; 3],
    /// The triangle's corners in its own frame, scaled to the island's UV scale (mirrored when the
    /// island is).
    q: [DVec2; 3],
    area: f64,
    /// Cotangent of the angle at each corner (the weight of the opposite edge).
    cot: [f64; 3],
}

/// `J = P Q⁻¹` of a triangle: the linear map taking its frame corners to its UV corners.
fn jacobian(t: &Tri, x: &[DVec2]) -> [f64; 4] {
    let (p1, p2) = (x[t.l[1]] - x[t.l[0]], x[t.l[2]] - x[t.l[0]]);
    let (q1, q2) = (t.q[1] - t.q[0], t.q[2] - t.q[0]);
    let det = q1.perp_dot(q2);
    // Q⁻¹ = 1/det [[q2.y, −q2.x], [−q1.y, q1.x]] with columns q1, q2.
    let (i00, i01, i10, i11) = (q2.y / det, -q2.x / det, -q1.y / det, q1.x / det);
    [p1.x * i00 + p2.x * i10, p1.x * i01 + p2.x * i11, p1.y * i00 + p2.y * i10, p1.y * i01 + p2.y * i11]
}

/// The fit of a Jacobian `[[a, b], [c, d]]`: the closest rotation, or the closest similarity.
fn fit(j: [f64; 4], mode: RelaxMode) -> [f64; 4] {
    let (e, h) = ((j[0] + j[3]) * 0.5, (j[2] - j[1]) * 0.5);
    let norm = e.hypot(h);
    if norm < 1e-300 {
        return [1.0, 0.0, 0.0, 1.0];
    }
    let (c, s) = (e / norm, h / norm);
    let k = if mode == RelaxMode::Conformal { norm } else { 1.0 };
    [k * c, -k * s, k * s, k * c]
}

fn energy(tris: &[Tri], x: &[DVec2], mode: RelaxMode) -> f64 {
    tris.iter()
        .map(|t| {
            let (j, l) = (jacobian(t, x), fit(jacobian(t, x), mode));
            t.area * j.iter().zip(&l).map(|(a, b)| (a - b) * (a - b)).sum::<f64>()
        })
        .sum()
}

/// Relax the UVs of the listed corners (and every corner that shares a UV vertex with them); the
/// rest of the layout stays put. An empty list relaxes everything. See the module docs.
pub fn relax_corners(mesh: &PolyMesh, corners: &[u32], opts: &RelaxOptions) -> Result<(PolyMesh, RelaxReport), UvError> {
    if !mesh.has_uvs() {
        return Err(UvError::NoUvs);
    }
    let charts = Charts::build(mesh, &SeamSet::from_uv_discontinuities(mesh))?;
    let mut pos = uv_vertex_positions(mesh, &charts);
    let all = corners.is_empty();
    let mut chosen = vec![all; pos.len()];
    for &c in corners {
        if let Some(c) = charts.corner_vertex.get(c as usize).and_then(|v| chosen.get_mut(*v as usize)) {
            *c = true;
        }
    }
    let iterations = opts.iterations.clamp(1, MAX_RELAX_ITERATIONS) as usize;
    let mut report = RelaxReport { iterations, ..RelaxReport::default() };
    for island in &charts.islands {
        if !island.vertices.iter().any(|v| chosen.get(*v as usize).copied().unwrap_or(false)) {
            continue;
        }
        match relax_island(mesh, &charts, &island.vertices, &island.faces, &chosen, &mut pos, opts.mode, iterations) {
            Some((before, after)) => {
                report.islands += 1;
                report.energy_before += before;
                report.energy_after += after;
            }
            None => report.skipped += 1,
        }
    }
    Ok((write_positions(mesh, &charts, &pos)?, report))
}

#[allow(clippy::too_many_arguments)]
fn relax_island(
    mesh: &PolyMesh,
    charts: &Charts,
    vertices: &[u32],
    faces: &[u32],
    chosen: &[bool],
    pos: &mut [DVec2],
    mode: RelaxMode,
    iterations: usize,
) -> Option<(f64, f64)> {
    const NO: usize = usize::MAX;
    let n = vertices.len();
    let local = |uvv: u32| vertices.binary_search(&uvv).unwrap_or(NO);
    let pos3: Vec<DVec3> = vertices.iter().map(|v| mesh.position(charts.vertex_source.get(*v as usize).copied().unwrap_or(0)).as_dvec3()).collect();
    let mut x: Vec<DVec2> = vertices.iter().map(|v| pos.get(*v as usize).copied().unwrap_or(DVec2::ZERO)).collect();
    if x.iter().any(|p| !p.is_finite()) {
        return None;
    }
    let mut tris: Vec<Tri> = Vec::new();
    let mut buf = Vec::new();
    for &f in faces {
        buf.clear();
        face_triangles(mesh, f, &mut buf);
        for t in &buf {
            let l = [0, 1, 2].map(|k| local(charts.corner_vertex.get(t[k] as usize).copied().unwrap_or(0)));
            if l.contains(&NO) || l[0] == l[1] || l[1] == l[2] || l[0] == l[2] {
                continue;
            }
            if let Some((q, area)) = local_frame([pos3[l[0]], pos3[l[1]], pos3[l[2]]]) {
                tris.push(Tri { l, q, area, cot: [0.0; 3] });
            }
        }
    }
    if tris.is_empty() {
        return None;
    }
    // Scale the frames to the layout's own scale, and mirror them when the island is mirrored.
    let (mut a3, mut auv, mut signed) = (0.0, 0.0, 0.0);
    for t in &tris {
        a3 += t.area;
        let s = (x[t.l[1]] - x[t.l[0]]).perp_dot(x[t.l[2]] - x[t.l[0]]) * 0.5;
        auv += s.abs();
        signed += s;
    }
    let scale = if auv > 0.0 && a3 > 0.0 { (auv / a3).sqrt() } else { 1.0 };
    let mirror = if signed < 0.0 { -1.0 } else { 1.0 };
    for t in &mut tris {
        t.q = t.q.map(|p| DVec2::new(p.x * scale, p.y * scale * mirror));
        t.area *= scale * scale;
        for k in 0..3 {
            let (a, b, c) = (t.q[k], t.q[(k + 1) % 3], t.q[(k + 2) % 3]);
            let (u, v) = (b - a, c - a);
            t.cot[k] = (u.dot(v) / u.perp_dot(v).abs().max(1e-300)).max(1e-4);
        }
    }
    let used: Vec<bool> = {
        let mut u = vec![false; n];
        for t in &tris {
            for l in t.l {
                u[l] = true;
            }
        }
        u
    };
    let mut free: Vec<bool> = (0..n).map(|l| used[l] && chosen.get(vertices[l] as usize).copied().unwrap_or(false)).collect();
    let anchored = !free.iter().zip(&used).any(|(f, u)| !*f && *u);
    let (old, centroid) = (x.clone(), {
        let (s, c) = (0..n).filter(|l| used[*l]).fold((DVec2::ZERO, 0.0), |(s, c), l| (s + x[l], c + 1.0));
        s / f64::max(c, 1.0)
    });
    if anchored {
        // Nothing is held: hold the vertex nearest the centre so the system has a unique answer.
        let anchor = (0..n).filter(|l| used[*l]).min_by(|a, b| x[*a].distance_squared(centroid).total_cmp(&x[*b].distance_squared(centroid)))?;
        free[anchor] = false;
    }
    let mut free_of = vec![NO; n];
    let mut nfree = 0;
    for l in 0..n {
        if free[l] {
            free_of[l] = nfree;
            nfree += 1;
        }
    }
    if nfree == 0 {
        return Some((0.0, 0.0));
    }
    let before = energy(&tris, &x, mode);
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); nfree];
    for t in &tris {
        for i in t.l {
            for j in t.l {
                if free_of[i] != NO && free_of[j] != NO {
                    rows[free_of[i]].push(free_of[j] as u32);
                }
            }
        }
    }
    for r in &mut rows {
        r.sort_unstable();
        r.dedup();
    }
    let mut a = BlockCsr::from_pattern(&rows)?;
    for t in &tris {
        for k in 0..3 {
            let (i, j) = (t.l[(k + 1) % 3], t.l[(k + 2) % 3]);
            let w = t.cot[k];
            for (p, q) in [(i, j), (j, i)] {
                if free_of[p] == NO {
                    continue;
                }
                a.add(free_of[p], free_of[p] as u32, [w, 0.0, 0.0, w]);
                if free_of[q] != NO {
                    a.add(free_of[p], free_of[q] as u32, [-w, 0.0, 0.0, -w]);
                }
            }
        }
    }
    let coords: Vec<[f64; 3]> = (0..n).filter(|l| free_of[*l] != NO).map(|l| pos3[l].to_array()).collect();
    let order = nested_dissection(&rows, &coords);
    drop(rows);
    let factor = Factor::new(&a, &order, MAX_FACTOR_NNZ, 0.0).or_else(|| Factor::new(&a, &order, MAX_FACTOR_NNZ, 1e-10));
    let mut rhs = vec![0.0; 2 * nfree];
    let mut sol = vec![0.0; 2 * nfree];
    for _ in 0..iterations {
        rhs.iter_mut().for_each(|v| *v = 0.0);
        for t in &tris {
            let lm = fit(jacobian(t, &x), mode);
            for k in 0..3 {
                let (i, j) = (t.l[(k + 1) % 3], t.l[(k + 2) % 3]);
                let w = t.cot[k];
                for (p, q) in [(i, j), (j, i)] {
                    if free_of[p] == NO {
                        continue;
                    }
                    let dq = t.q[p_corner(t, p)] - t.q[p_corner(t, q)];
                    let target = DVec2::new(lm[0] * dq.x + lm[1] * dq.y, lm[2] * dq.x + lm[3] * dq.y);
                    let mut r = w * target;
                    if free_of[q] == NO {
                        r += w * x[q];
                    }
                    rhs[2 * free_of[p]] += r.x;
                    rhs[2 * free_of[p] + 1] += r.y;
                }
            }
        }
        for l in 0..n {
            if free_of[l] != NO {
                sol[2 * free_of[l]] = x[l].x;
                sol[2 * free_of[l] + 1] = x[l].y;
            }
        }
        if let Some(f) = &factor {
            f.solve(&rhs, &mut sol);
        } else {
            let cg = solve_cg(&a, &rhs, &mut sol, 1e-10, 4000);
            if !cg.residual.is_finite() {
                return None;
            }
        }
        if sol.iter().any(|v| !v.is_finite()) {
            return None;
        }
        for l in 0..n {
            if free_of[l] != NO {
                x[l] = DVec2::new(sol[2 * free_of[l]], sol[2 * free_of[l] + 1]);
            }
        }
    }
    if anchored {
        // Turn the result back to where the island was, about its centre.
        let c_new = {
            let (s, c) = (0..n).filter(|l| used[*l]).fold((DVec2::ZERO, 0.0), |(s, c), l| (s + x[l], c + 1.0));
            s / f64::max(c, 1.0)
        };
        let (mut dot, mut cross) = (0.0, 0.0);
        for l in (0..n).filter(|l| used[*l]) {
            let (p, q) = (x[l] - c_new, old[l] - centroid);
            dot += p.dot(q);
            cross += p.perp_dot(q);
        }
        let (s, c) = cross.atan2(dot).sin_cos();
        for l in (0..n).filter(|l| used[*l]) {
            let p = x[l] - c_new;
            x[l] = centroid + DVec2::new(c * p.x - s * p.y, s * p.x + c * p.y);
        }
    }
    let after = energy(&tris, &x, mode);
    if !after.is_finite() || after > before * (1.0 + 1e-9) + 1e-12 {
        // The relaxation must never make the layout worse; keep what was there.
        return Some((before, before));
    }
    for l in 0..n {
        if let Some(p) = pos.get_mut(vertices[l] as usize) {
            *p = x[l];
        }
    }
    Some((before, after))
}

/// Which corner (0, 1 or 2) of the triangle is the local vertex `l`.
fn p_corner(t: &Tri, l: usize) -> usize {
    t.l.iter().position(|v| *v == l).unwrap_or(0)
}

/// Straighten the chains of UV edges whose both ends are among the listed corners (and the corners
/// that share their UV vertices): each chain is laid on the line between its two ends, every vertex
/// keeping its share of the chain's length. `strength` (0 to 1) blends from the old layout to the
/// straight one. Closed loops and branching chains are left alone. Returns the mesh and the number
/// of chains straightened.
pub fn straighten_corners(mesh: &PolyMesh, corners: &[u32], strength: f32) -> Result<(PolyMesh, usize), UvError> {
    if !mesh.has_uvs() {
        return Err(UvError::NoUvs);
    }
    if !strength.is_finite() {
        return Err(UvError::Invalid("the strength is not a number".into()));
    }
    let strength = f64::from(strength.clamp(0.0, 1.0));
    let charts = Charts::build(mesh, &SeamSet::from_uv_discontinuities(mesh))?;
    let mut pos = uv_vertex_positions(mesh, &charts);
    let chosen: HashSet<u32> = corners.iter().filter_map(|c| charts.corner_vertex.get(*c as usize).copied()).collect();
    let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    for h in 0..mesh.corner_count() as u32 {
        let (Some(&a), Some(&b)) = (charts.corner_vertex.get(h as usize), charts.corner_vertex.get(mesh.next(h) as usize)) else { continue };
        if a != b && chosen.contains(&a) && chosen.contains(&b) && seen.insert((a.min(b), a.max(b))) {
            adj.entry(a).or_default().push(b);
            adj.entry(b).or_default().push(a);
        }
    }
    let mut visited: HashSet<u32> = HashSet::new();
    let mut starts: Vec<u32> = adj.iter().filter(|(_, n)| n.len() == 1).map(|(v, _)| *v).collect();
    starts.sort_unstable();
    let mut done = 0;
    for s in starts {
        if visited.contains(&s) {
            continue;
        }
        // Walk the chain from this end; give up on a branch.
        let mut chain = vec![s];
        let mut prev = u32::MAX;
        let mut cur = s;
        let mut simple = true;
        loop {
            let next: Vec<u32> = adj.get(&cur).map(|n| n.iter().copied().filter(|v| *v != prev).collect()).unwrap_or_default();
            match next.as_slice() {
                [] => break,
                [n] if !chain.contains(n) && adj.get(n).is_some_and(|d| d.len() <= 2) => {
                    prev = cur;
                    cur = *n;
                    chain.push(cur);
                }
                _ => {
                    simple = false;
                    break;
                }
            }
        }
        visited.extend(chain.iter().copied());
        if !simple || chain.len() < 3 {
            continue;
        }
        let pts: Vec<DVec2> = chain.iter().map(|v| pos.get(*v as usize).copied().unwrap_or(DVec2::ZERO)).collect();
        let total: f64 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        let dir = pts[pts.len() - 1] - pts[0];
        if !(total > 1e-12 && dir.length() > 1e-12) {
            continue;
        }
        let mut run = 0.0;
        for (i, v) in chain.iter().enumerate() {
            if i > 0 {
                run += pts[i - 1].distance(pts[i]);
            }
            let straight = pts[0] + dir * (run / total);
            if let Some(p) = pos.get_mut(*v as usize) {
                *p = pts[i] + (straight - pts[i]) * strength;
            }
        }
        done += 1;
    }
    Ok((write_positions(mesh, &charts, &pos)?, done))
}

/// What [`stitch_islands`] does besides welding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StitchOptions {
    /// Move each island onto its neighbour so the stitched edges meet (rotation and shift).
    pub align: bool,
    /// While aligning, also scale the moved island so the stitched edges have the same length.
    pub scale: bool,
}

/// Stitch along the listed mesh edges: the two sides of each edge are welded at their common
/// centre (see [`crate::stitch_edges`]). With `align`, the islands on the two sides are first
/// brought together: starting from the lowest-numbered island of each connected group (it stays
/// where it is), each island that shares edges with a placed one is turned, and with `scale`
/// resized, by the similarity that best lays its ends of those edges on the placed island's, and
/// moved with all its corners. Edges inside one island, or a mirrored neighbour, are only welded.
pub fn stitch_islands(mesh: &PolyMesh, edges: &[u32], opts: &StitchOptions) -> Result<PolyMesh, UvError> {
    if !mesh.has_uvs() {
        return Err(UvError::NoUvs);
    }
    if !opts.align {
        return crate::stitch_edges(mesh, edges);
    }
    let charts = Charts::build(mesh, &SeamSet::from_uv_discontinuities(mesh))?;
    // For each seam edge between two islands: the pairs of corners that should coincide.
    struct Link {
        a: u32,
        b: u32,
        pairs: [(u32, u32); 2],
    }
    let island_of = |c: u32| charts.face_island.get(mesh.face_of(c) as usize).copied();
    let mut links: Vec<Link> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    for &e in edges {
        if (e as usize) >= mesh.edge_count() || !seen.insert(e) {
            continue;
        }
        let h = mesh.edge_half_edge(e);
        let t = mesh.twin(h);
        if mesh.face_of(h) == NONE || mesh.face_of(t) == NONE {
            continue;
        }
        let (Some(a), Some(b)) = (island_of(h), island_of(t)) else { continue };
        if a != b {
            links.push(Link { a, b, pairs: [(h, mesh.next(t)), (mesh.next(h), t)] });
        }
    }
    let mut uv: Vec<DVec2> = mesh.corner_uvs().iter().map(|u| DVec2::new(f64::from(u.x), f64::from(u.y))).collect();
    let mut placed: HashSet<u32> = HashSet::new();
    let mut touched: Vec<u32> = links.iter().flat_map(|l| [l.a, l.b]).collect();
    touched.sort_unstable();
    touched.dedup();
    // The corners of each island, for moving them.
    let mut island_corners: HashMap<u32, Vec<u32>> = HashMap::new();
    for c in 0..mesh.corner_count() as u32 {
        if let Some(i) = island_of(c)
            && touched.binary_search(&i).is_ok()
        {
            island_corners.entry(i).or_default().push(c);
        }
    }
    for &anchor in &touched {
        if placed.contains(&anchor) {
            continue;
        }
        placed.insert(anchor);
        loop {
            // The next island that shares edges with the placed ones.
            let next = touched
                .iter()
                .copied()
                .find(|q| !placed.contains(q) && links.iter().any(|l| (l.a == *q && placed.contains(&l.b)) || (l.b == *q && placed.contains(&l.a))));
            let Some(q) = next else { break };
            let (mut from, mut to) = (Vec::new(), Vec::new());
            for l in &links {
                for (ca, cb) in l.pairs {
                    if l.a == q && placed.contains(&l.b) {
                        from.push(uv[ca as usize]);
                        to.push(uv[cb as usize]);
                    } else if l.b == q && placed.contains(&l.a) {
                        from.push(uv[cb as usize]);
                        to.push(uv[ca as usize]);
                    }
                }
            }
            let (mp, mq) = (from.iter().sum::<DVec2>() / from.len() as f64, to.iter().sum::<DVec2>() / to.len() as f64);
            let (mut dot, mut cross, mut norm) = (0.0, 0.0, 0.0);
            for (p, r) in from.iter().zip(&to) {
                let (p, r) = (*p - mp, *r - mq);
                dot += p.dot(r);
                cross += p.perp_dot(r);
                norm += p.length_squared();
            }
            let (theta, k) =
                if norm > 1e-18 && dot.hypot(cross) > 1e-18 { (cross.atan2(dot), if opts.scale { dot.hypot(cross) / norm } else { 1.0 }) } else { (0.0, 1.0) };
            let (s, c) = theta.sin_cos();
            if let Some(corners) = island_corners.get(&q) {
                for &cn in corners {
                    let p = uv[cn as usize] - mp;
                    uv[cn as usize] = mq + k * DVec2::new(c * p.x - s * p.y, s * p.x + c * p.y);
                }
            }
            placed.insert(q);
        }
    }
    let uvs: Vec<Vec2> = uv.iter().map(|p| Vec2::new(p.x as f32, p.y as f32)).collect();
    let mut moved = mesh.clone();
    moved.set_uvs(&uvs)?;
    crate::stitch_edges(&moved, edges)
}
