//! Detail levels: sculpting the same surface at several resolutions.
//!
//! Each level is one smooth-cage subdivision of the level below (every n-gon becomes n quads; the
//! new mesh lists the old vertices first, then one point per edge, then one per face). What a
//! finer level adds on top of the smoothed coarser surface is kept as a *displacement* per vertex,
//! measured in a frame that rides on that surface (normal, tangent, bitangent). Because the frame
//! follows the surface, a change made on a coarse level carries the fine detail with it.
//!
//! [`Refine`] is the subdivision written as a sparse linear map (a short weighted list of coarse
//! vertices per fine vertex), so it applies equally to positions, masks and shape-layer offsets,
//! and stepping never rebuilds topology here: the caller supplies the meshes.

use glam::Vec3;
use modelcraft_mesh::{NONE, PolyMesh};

/// Highest detail level above the base mesh.
pub const MAX_DETAIL_LEVEL: u8 = 8;

/// Fit passes when stepping down. Each pass shrinks the misfit (slowest at three-edge corners);
/// what is left after the last pass goes into the displacement, so the fit never loses detail.
const FIT_PASSES: usize = 12;

/// One subdivision step as a linear map from coarse to fine per-vertex values.
#[derive(Clone, Debug, Default)]
pub struct Refine {
    coarse: usize,
    start: Vec<u32>,
    idx: Vec<u32>,
    w: Vec<f32>,
}

/// Values a [`Refine`] can carry (positions, offsets, masks).
pub trait Blend: Copy + Default + std::ops::Add<Output = Self> + std::ops::Mul<f32, Output = Self> {}
impl Blend for Vec3 {}
impl Blend for f32 {}

impl Refine {
    /// The map for subdividing `m` once. The weights are the smooth-cage rules: face points are
    /// face averages, edge points average the edge and its two face points (the midpoint on a
    /// border), and old vertices move towards their neighbourhood (border vertices along the
    /// border curve only).
    pub fn new(m: &PolyMesh) -> Refine {
        let (nv, ne, nf) = (m.vertex_count(), m.edge_count(), m.face_count());
        let mut r = Refine { coarse: nv, start: Vec::with_capacity(nv + ne + nf + 1), idx: Vec::new(), w: Vec::new() };
        r.start.push(0);
        let mut row: Vec<(u32, f32)> = Vec::new();
        let add_face = |row: &mut Vec<(u32, f32)>, f: u32, weight: f32| {
            let len = m.face_len(f).max(1) as f32;
            for v in m.face_vertices(f) {
                row.push((v, weight / len));
            }
        };
        for v in 0..nv as u32 {
            row.clear();
            let mut k = 0.0f32;
            let mut border: Vec<u32> = Vec::new();
            for h in m.vertex_out(v) {
                k += 1.0;
                if m.face_of(h) == NONE || m.face_of(m.twin(h)) == NONE {
                    border.push(m.dest(h));
                }
            }
            if k == 0.0 {
                row.push((v, 1.0));
            } else if !border.is_empty() {
                match border.as_slice() {
                    [a, b] => row.extend([(*a, 0.125), (*b, 0.125), (v, 0.75)]),
                    _ => row.push((v, 1.0)),
                }
            } else {
                let k2 = k * k;
                for h in m.vertex_out(v) {
                    let f = m.face_of(h);
                    if f != NONE {
                        add_face(&mut row, f, 1.0 / k2);
                    }
                    row.push((m.dest(h), 1.0 / k2));
                }
                row.push((v, 1.0 / k + (k - 3.0) / k));
            }
            r.push_row(&mut row);
        }
        for e in 0..ne as u32 {
            row.clear();
            let h = m.edge_half_edge(e);
            let (a, b) = (m.origin(h), m.dest(h));
            let (f1, f2) = (m.face_of(h), m.face_of(m.twin(h)));
            if f1 == NONE || f2 == NONE {
                row.extend([(a, 0.5), (b, 0.5)]);
            } else {
                row.extend([(a, 0.25), (b, 0.25)]);
                add_face(&mut row, f1, 0.25);
                add_face(&mut row, f2, 0.25);
            }
            r.push_row(&mut row);
        }
        for f in 0..nf as u32 {
            row.clear();
            add_face(&mut row, f, 1.0);
            r.push_row(&mut row);
        }
        r
    }

    fn push_row(&mut self, row: &mut [(u32, f32)]) {
        row.sort_unstable_by_key(|e| e.0);
        let mut last = NONE;
        for (i, w) in row.iter() {
            if *i == last
                && let Some(x) = self.w.last_mut()
            {
                *x += *w;
            } else {
                self.idx.push(*i);
                self.w.push(*w);
                last = *i;
            }
        }
        self.start.push(self.idx.len() as u32);
    }

    /// Vertices of the coarse mesh.
    pub fn coarse_count(&self) -> usize {
        self.coarse
    }
    /// Vertices of the fine mesh.
    pub fn fine_count(&self) -> usize {
        self.start.len().saturating_sub(1)
    }

    fn row<T: Blend>(&self, i: usize, src: &[T]) -> T {
        let a = self.start.get(i).copied().unwrap_or(0) as usize;
        let b = self.start.get(i + 1).copied().unwrap_or(0) as usize;
        let mut s = T::default();
        for (j, w) in self.idx.get(a..b).unwrap_or_default().iter().zip(self.w.get(a..b).unwrap_or_default()) {
            s = s + src.get(*j as usize).copied().unwrap_or_default() * *w;
        }
        s
    }

    /// Fine values from coarse ones (missing coarse values count as zero).
    pub fn apply<T: Blend>(&self, src: &[T]) -> Vec<T> {
        (0..self.fine_count()).map(|i| self.row(i, src)).collect()
    }

    /// Only the fine values that sit on the old vertices.
    fn apply_old<T: Blend>(&self, src: &[T]) -> Vec<T> {
        (0..self.coarse.min(self.fine_count())).map(|i| self.row(i, src)).collect()
    }
}

/// The frame riding on the smooth surface at each vertex: `[normal, tangent, bitangent]`. The
/// tangent points at a neighbour fixed by the topology, so the same mesh always gives the same
/// frames and a rigid motion of the surface turns the frames with it.
fn frames(mesh: &PolyMesh, smooth: &[Vec3]) -> Vec<[Vec3; 3]> {
    let p = |v: u32| smooth.get(v as usize).copied().unwrap_or_default();
    let mut normals = vec![Vec3::ZERO; smooth.len()];
    for f in 0..mesh.face_count() as u32 {
        let mut n = Vec3::ZERO;
        for h in mesh.face_half_edges(f) {
            let (a, b) = (p(mesh.origin(h)), p(mesh.dest(h)));
            n += Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
        }
        for v in mesh.face_vertices(f) {
            if let Some(x) = normals.get_mut(v as usize) {
                *x += n;
            }
        }
    }
    normals
        .iter()
        .enumerate()
        .map(|(v, n)| {
            let n = n.normalize_or(Vec3::Y);
            let h = mesh.vertex_half_edge(v as u32);
            let towards = if h == NONE { Vec3::ZERO } else { p(mesh.dest(h)) - p(v as u32) };
            let mut t = towards - n * towards.dot(n);
            if t.length_squared() < 1e-24 {
                t = n.any_orthogonal_vector();
            }
            let t = t.normalize_or(Vec3::X);
            [n, t, n.cross(t)]
        })
        .collect()
}

/// Why a level change was refused.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum DetailError {
    #[error("the detail meshes do not belong together ({0})")]
    Mismatch(&'static str),
}

fn check(refine: &Refine, fine: &PolyMesh, coarse_len: usize, fine_len: Option<usize>) -> Result<(), DetailError> {
    if refine.coarse_count() != coarse_len {
        return Err(DetailError::Mismatch("coarse vertex count"));
    }
    if refine.fine_count() != fine.vertex_count() || fine_len.is_some_and(|n| n != fine.vertex_count()) {
        return Err(DetailError::Mismatch("fine vertex count"));
    }
    Ok(())
}

/// Positions one level up: the smoothed coarse surface plus the stored displacement of that
/// level (none: the plain smooth surface). `fine` is the subdivided topology.
pub fn step_up(refine: &Refine, fine: &PolyMesh, coarse: &[Vec3], stored: Option<&[Vec3]>) -> Result<Vec<Vec3>, DetailError> {
    check(refine, fine, coarse.len(), None)?;
    let mut out = refine.apply(coarse);
    if let Some(d) = stored.filter(|d| d.len() == out.len()) {
        let fr = frames(fine, &out);
        for ((p, d), [n, t, b]) in out.iter_mut().zip(d).zip(&fr) {
            let q = *p + *n * d.x + *t * d.y + *b * d.z;
            if q.is_finite() {
                *p = q;
            }
        }
    }
    Ok(out)
}

/// Positions one level down, and the displacement that brings the fine level back exactly.
/// The coarse vertices are fitted so the smoothed coarse surface comes close to the fine
/// positions of the old vertices; whatever is left becomes the displacement.
pub fn step_down(refine: &Refine, fine: &PolyMesh, fine_positions: &[Vec3]) -> Result<(Vec<Vec3>, Vec<Vec3>), DetailError> {
    check(refine, fine, refine.coarse_count(), Some(fine_positions.len()))?;
    let target = fine_positions.get(..refine.coarse_count()).unwrap_or_default();
    let mut coarse = target.to_vec();
    for _ in 0..FIT_PASSES {
        let now = refine.apply_old(&coarse);
        for ((c, t), s) in coarse.iter_mut().zip(target).zip(&now) {
            *c += *t - *s;
        }
    }
    let smooth = refine.apply(&coarse);
    let fr = frames(fine, &smooth);
    let disp = fine_positions
        .iter()
        .zip(&smooth)
        .zip(&fr)
        .map(|((p, s), [n, t, b])| {
            let d = *p - *s;
            Vec3::new(d.dot(*n), d.dot(*t), d.dot(*b))
        })
        .collect();
    Ok((coarse, disp))
}

/// Shape layers: named offsets blended over the sculpt, each with its own strength.
pub mod shape {
    use glam::Vec3;

    /// Strength range of a shape layer.
    pub const STRENGTH: std::ops::RangeInclusive<f32> = -1.0..=1.0;

    /// Smallest strength at which sculpting is still recorded into a layer.
    pub const MIN_RECORD: f32 = 1e-3;

    /// `positions += delta * k`.
    pub fn add_scaled(positions: &mut [Vec3], delta: &[Vec3], k: f32) {
        if !k.is_finite() || delta.len() != positions.len() {
            return;
        }
        for (p, d) in positions.iter_mut().zip(delta) {
            let q = *p + *d * k;
            if q.is_finite() {
                *p = q;
            }
        }
    }

    /// Record what was sculpted since `anchor` into `delta`, so that `strength * delta` gives
    /// the sculpted change. Returns false (and records nothing: the change stays in the base)
    /// when the strength is too close to zero or the lengths differ.
    pub fn settle(delta: &mut [Vec3], positions: &[Vec3], anchor: &[Vec3], strength: f32) -> bool {
        if !(strength.is_finite() && strength.abs() >= MIN_RECORD) || delta.len() != positions.len() || anchor.len() != positions.len() {
            return false;
        }
        for ((d, p), a) in delta.iter_mut().zip(positions).zip(anchor) {
            let q = *d + (*p - *a) / strength;
            if q.is_finite() {
                *d = q;
            }
        }
        true
    }
}
