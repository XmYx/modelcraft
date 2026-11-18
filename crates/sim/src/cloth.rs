//! Cloth: extended position-based dynamics on a polygon mesh.
//!
//! The method is XPBD, from "XPBD: Position-Based Simulation of Compliant Constrained Dynamics"
//! (Macklin, Müller, Chentanez, 2016), with the small-steps idea of "Small Steps in Physics
//! Simulation" (Macklin et al., 2019): many substeps with few iterations. Each substep predicts
//! positions from velocity and forces, projects the constraints with their compliance, resolves
//! collisions, and takes velocities from the corrected positions.
//!
//! - **Stretch**: a distance constraint on every edge of the mesh.
//! - **Shear**: distance constraints on the diagonals of every quad.
//! - **Bend**: the dihedral angle across every interior edge, held at its rest value (the bending
//!   constraint of "Position Based Dynamics", Müller et al. 2007, with the gradients of the
//!   angle between the two face normals).
//!
//! Stiffness is a number from 0 to 1 for each kind. It becomes the natural frequency of the
//! constraint, so the same setting behaves the same on a coarse and on a fine mesh: the compliance
//! of a constraint is `(w_a + w_b) / ω²` with `w` the inverse masses. A stiffness of 1 is
//! inextensible (zero compliance).
//!
//! - **Pins**: listed vertices have infinite mass and stay where they were, or follow a moving
//!   object's motion.
//! - **Collisions**: vertices are spheres of `thickness` against the triangles of the colliders
//!   (a [`ColliderProvider`] gives their world-space positions at each frame, so moving, deformed
//!   and simulated colliders work) with friction and the collider's own motion, and optionally
//!   against each other (self collision through a spatial hash).
//! - **Wind**: per-triangle quadratic drag against a [`crate::field::Wind`] with turbulence.
//!
//! Everything runs in a fixed order on one thread, so a bake repeats bit for bit.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::field::Wind;
use crate::hash::SpatialHash;
use crate::{MAX_POINTS, SimError, Solver, StepContext};

/// Most constraints one cloth holds.
pub const MAX_CONSTRAINTS: usize = 8_000_000;
/// Most collider triangles in one frame.
pub const MAX_COLLIDER_TRIANGLES: usize = 2_000_000;

/// Frequencies (Hz) a stiffness of 1 maps towards; see [`stiffness_omega`].
const STRETCH_HZ: f32 = 400.0;
const SHEAR_HZ: f32 = 250.0;
const BEND_HZ: f32 = 120.0;
/// Air density times a drag coefficient of one, kg/m³.
const AIR: f32 = 1.2;
const MAX_SPEED: f32 = 300.0;

/// The settings of one cloth.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClothSettings {
    /// Total mass in kilograms, spread over the vertices by the area they hold.
    pub mass: f32,
    /// How stiff the edges are against stretching, 0 to 1 (1: inextensible).
    pub stretch: f32,
    /// Resistance to shearing the quads, 0 to 1.
    pub shear: f32,
    /// Resistance to folding along the edges, 0 to 1.
    pub bend: f32,
    /// Air drag on the motion of the vertices, per second.
    pub damping: f32,
    /// The radius of a vertex in collisions, metres.
    pub thickness: f32,
    /// Friction against colliders, 0 to 2.
    pub friction: f32,
    /// Constraint iterations per substep (more is stiffer and slower).
    pub iterations: u32,
    pub gravity: [f32; 3],
    pub wind: Wind,
    /// Collide with other objects of the scene.
    pub collide: bool,
    /// Keep the cloth from passing through itself.
    pub self_collision: bool,
    /// Vertices held in place (a vertex group, by index).
    pub pinned: Vec<u32>,
    /// Also hold every vertex whose sculpt mask is above one half.
    pub pin_mask: bool,
    /// The scene object id of the object the pinned vertices follow (they stay in place without one).
    pub pin_follow: Option<u64>,
    /// The scene object ids of the objects to collide with; every other visible mesh without any.
    pub collide_with: Vec<u64>,
}

impl Default for ClothSettings {
    fn default() -> Self {
        ClothSettings {
            mass: 1.0,
            stretch: 0.95,
            shear: 0.8,
            bend: 0.3,
            damping: 0.4,
            thickness: 0.02,
            friction: 0.4,
            iterations: 2,
            gravity: [0.0, -9.81, 0.0],
            wind: Wind::default(),
            collide: true,
            self_collision: false,
            pinned: Vec::new(),
            pin_mask: false,
            pin_follow: None,
            collide_with: Vec::new(),
        }
    }
}

fn fin(v: f32, lo: f32, hi: f32, d: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { d }
}

impl ClothSettings {
    /// Clamp every number into the range the solver is stable in.
    pub fn sanitize(&mut self) {
        self.mass = fin(self.mass, 1e-3, 1e5, 1.0);
        self.stretch = fin(self.stretch, 0.0, 1.0, 0.95);
        self.shear = fin(self.shear, 0.0, 1.0, 0.8);
        self.bend = fin(self.bend, 0.0, 1.0, 0.3);
        self.damping = fin(self.damping, 0.0, 100.0, 0.4);
        self.thickness = fin(self.thickness, 1e-4, 10.0, 0.02);
        self.friction = fin(self.friction, 0.0, 2.0, 0.4);
        self.iterations = self.iterations.clamp(1, 16);
        self.gravity = self.gravity.map(|g| fin(g, -1000.0, 1000.0, 0.0));
        self.wind.sanitize();
        self.pinned.sort_unstable();
        self.pinned.dedup();
        self.pinned.truncate(MAX_POINTS);
        self.collide_with.sort_unstable();
        self.collide_with.dedup();
        self.collide_with.truncate(1024);
    }
}

/// `ω² dt²` for a stiffness: the quantity compliance depends on. `None` is rigid (zero compliance).
fn stiffness_omega(stiffness: f32, hz: f32, dt: f32) -> Option<f32> {
    if stiffness >= 0.999 {
        return None;
    }
    let omega = (std::f32::consts::TAU * hz * stiffness * stiffness).max(2.0);
    Some(omega * omega * dt * dt)
}

/// One collider mesh at one frame: world-space positions and its triangles.
#[derive(Clone, Debug)]
pub struct ColliderMesh {
    pub positions: Arc<Vec<Vec3>>,
    pub triangles: Arc<Vec<[u32; 3]>>,
}

/// Gives the colliders' world-space positions at a scene frame. Called once per frame; return the
/// same `Arc`s for a collider that did not move so the solver can reuse its hash.
pub type ColliderProvider = Box<dyn FnMut(f64) -> Vec<ColliderMesh> + Send>;

/// The cloth to simulate.
pub struct ClothDesc {
    /// The mesh's points in world space at the start frame.
    pub points: Vec<Vec3>,
    pub polygons: Vec<Vec<u32>>,
    pub settings: ClothSettings,
    /// For pins that follow an object: at each frame from the start, the matrix taking the
    /// object's start-frame world space to its world space then. Empty: pins stay put.
    pub pin_motion: Vec<Mat4>,
    pub start_frame: f64,
}

#[derive(Clone, Copy, Debug)]
struct Link {
    a: u32,
    b: u32,
    rest: f32,
}

/// The hinge across the interior edge `a`–`b` between the triangles `a b c` and `b a d`.
#[derive(Clone, Copy, Debug)]
struct Hinge {
    a: u32,
    b: u32,
    c: u32,
    d: u32,
    rest: f32,
}

/// The angle between the faces of a hinge (0 when flat) and the gradient of it with respect to
/// the four points, or `None` for a degenerate hinge.
fn hinge_angle(p: [Vec3; 4]) -> Option<(f32, [Vec3; 4])> {
    let (p1, p2, p3, p4) = (Vec3::ZERO, p[1] - p[0], p[2] - p[0], p[3] - p[0]);
    let c23 = p2.cross(p3);
    let c24 = p2.cross(p4);
    let (l23, l24) = (c23.length(), c24.length());
    if l23 < 1e-9 || l24 < 1e-9 {
        return None;
    }
    let (n1, n2) = (c23 / l23, c24 / l24);
    let d = n1.dot(n2).clamp(-1.0, 1.0);
    let q3 = (p2.cross(n2) + n1.cross(p2) * d) / l23;
    let q4 = (p2.cross(n1) + n2.cross(p2) * d) / l24;
    let q2 = -(p3.cross(n2) + n1.cross(p3) * d) / l23 - (p4.cross(n1) + n2.cross(p4) * d) / l24;
    let q1 = -(q2 + q3 + q4);
    let _ = p1;
    // d(angle)/d(point) = q / sin(angle) (the q of the paper are the gradients of the negated
    // cosine); near flat q shrinks with the sine, so cap the divisor.
    let sin = (1.0 - d * d).max(0.0).sqrt().max(1e-3);
    let g = [q1 / sin, q2 / sin, q3 / sin, q4 / sin];
    Some((d.acos(), g))
}

/// A cloth ready to bake.
pub struct ClothSolver {
    desc: ClothDesc,
    rest: Vec<Vec3>,
    x: Vec<Vec3>,
    v: Vec<Vec3>,
    p: Vec<Vec3>,
    w: Vec<f32>,
    tris: Vec<[u32; 3]>,
    stretch: Vec<Link>,
    shear: Vec<Link>,
    bend: Vec<Hinge>,
    /// The most a bend constraint may move a vertex in one projection: half an edge length.
    bend_limit: f32,
    lambda: [Vec<f32>; 3],
    pinned: Vec<bool>,
    frame: usize,
    time: f32,
    noise_seed: u32,
    provider: Option<ColliderProvider>,
    prev: Vec<ColliderMesh>,
    cur: Vec<ColliderMesh>,
    /// (collider, triangle) of every item in `coll_hash`.
    coll_tris: Vec<(u32, u32)>,
    /// The box of every item of `coll_hash`, grown by a thickness: a cheap test before any maths.
    coll_boxes: Vec<(Vec3, Vec3)>,
    coll_hash: SpatialHash,
    coll_built_static: bool,
    /// The box around every collider triangle (and where it was), grown by a thickness.
    coll_bounds: (Vec3, Vec3),
    stamp: Vec<u32>,
    stamp_now: u32,
    self_hash: SpatialHash,
}

fn tri_area_vec(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    0.5 * (b - a).cross(c - a)
}

impl ClothSolver {
    /// Check the description and build the constraints.
    pub fn new(mut desc: ClothDesc, provider: Option<ColliderProvider>) -> Result<ClothSolver, SimError> {
        let n = desc.points.len();
        if n == 0 || n > MAX_POINTS {
            return Err(SimError::Invalid(format!("a cloth has 1 to {MAX_POINTS} vertices, not {n}")));
        }
        if desc.points.iter().any(|p| !p.is_finite() || p.abs().max_element() > 1e7) {
            return Err(SimError::Invalid("a cloth vertex is not a finite position".into()));
        }
        desc.settings.sanitize();
        let mut tris: Vec<[u32; 3]> = Vec::new();
        let mut edges: Vec<(u32, u32)> = Vec::new();
        // For every polygon edge: the vertex opposite it in the triangle that holds it.
        let mut hinge: Vec<((u32, u32), u32)> = Vec::new();
        let mut shear: Vec<(u32, u32)> = Vec::new();
        for poly in &desc.polygons {
            if poly.len() < 3 || poly.iter().any(|i| *i as usize >= n) {
                continue;
            }
            for k in 1..poly.len() - 1 {
                tris.push([poly[0], poly[k], poly[k + 1]]);
            }
            for k in 0..poly.len() {
                let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
                if a == b {
                    continue;
                }
                let e = (a.min(b), a.max(b));
                edges.push(e);
                // The fan triangle that holds this edge, and its third vertex.
                let third = if poly.len() == 3 {
                    poly[(k + 2) % 3]
                } else if k == 0 {
                    poly[2]
                } else if k + 1 == poly.len() {
                    poly[poly.len() - 2]
                } else {
                    poly[0]
                };
                if third != a && third != b {
                    hinge.push((e, third));
                }
            }
            if poly.len() == 4 {
                shear.push((poly[0], poly[2]));
                shear.push((poly[1], poly[3]));
            }
        }
        if tris.is_empty() {
            return Err(SimError::Invalid("a cloth needs at least one face".into()));
        }
        edges.sort_unstable();
        edges.dedup();
        hinge.sort_unstable();
        let mut bend_hinges: Vec<(u32, u32, u32, u32)> = Vec::new();
        let mut i = 0;
        while i < hinge.len() {
            let mut j = i;
            while j < hinge.len() && hinge[j].0 == hinge[i].0 {
                j += 1;
            }
            // Exactly two faces share the edge: the vertices opposite it are the hinge.
            if j - i == 2 && hinge[i].1 != hinge[i + 1].1 {
                let ((a, b), (c, d)) = (hinge[i].0, (hinge[i].1, hinge[i + 1].1));
                bend_hinges.push((a, b, c, d));
            }
            i = j;
        }
        shear.iter_mut().for_each(|(a, b)| (*a, *b) = ((*a).min(*b), (*a).max(*b)));
        shear.sort_unstable();
        shear.dedup();
        if edges.len() + shear.len() + bend_hinges.len() > MAX_CONSTRAINTS {
            return Err(SimError::Invalid("the cloth has too many constraints".into()));
        }
        let links = |pairs: &[(u32, u32)]| -> Vec<Link> {
            pairs
                .iter()
                .filter_map(|(a, b)| {
                    let rest = desc.points[*a as usize].distance(desc.points[*b as usize]);
                    (rest > 1e-6).then_some(Link { a: *a, b: *b, rest })
                })
                .collect()
        };
        let (stretch, shear) = (links(&edges), links(&shear));
        let bend: Vec<Hinge> = bend_hinges
            .iter()
            .filter_map(|(a, b, c, d)| {
                let q = |i: &u32| desc.points[*i as usize];
                hinge_angle([q(a), q(b), q(c), q(d)]).map(|(rest, _)| Hinge { a: *a, b: *b, c: *c, d: *d, rest })
            })
            .collect();
        // Mass by area: a third of each triangle to each corner.
        let mut area = vec![0.0f32; n];
        for t in &tris {
            let a = tri_area_vec(desc.points[t[0] as usize], desc.points[t[1] as usize], desc.points[t[2] as usize]).length() / 3.0;
            for v in t {
                area[*v as usize] += a;
            }
        }
        let total: f32 = area.iter().sum();
        let s = &desc.settings;
        let w: Vec<f32> = area
            .iter()
            .map(|a| {
                let m = if total > 1e-12 { s.mass * a / total } else { s.mass / n as f32 };
                1.0 / m.max(s.mass * 1e-6).max(1e-9)
            })
            .collect();
        let mut pinned = vec![false; n];
        for p in desc.settings.pinned.iter().filter(|p| (**p as usize) < n) {
            pinned[*p as usize] = true;
        }
        let bend_limit = 0.5 * stretch.iter().map(|l| l.rest).sum::<f32>() / stretch.len().max(1) as f32;
        let rest = desc.points.clone();
        let lambda = [vec![0.0; stretch.len()], vec![0.0; shear.len()], vec![0.0; bend.len()]];
        let thickness = desc.settings.thickness;
        Ok(ClothSolver {
            x: rest.clone(),
            v: vec![Vec3::ZERO; n],
            p: rest.clone(),
            rest,
            w,
            tris,
            stretch,
            shear,
            bend,
            bend_limit,
            lambda,
            pinned,
            frame: 0,
            time: 0.0,
            noise_seed: 0,
            provider,
            prev: Vec::new(),
            cur: Vec::new(),
            coll_tris: Vec::new(),
            coll_boxes: Vec::new(),
            coll_hash: SpatialHash::new(1.0),
            coll_built_static: false,
            coll_bounds: (Vec3::ZERO, Vec3::ZERO),
            stamp: Vec::new(),
            stamp_now: 0,
            self_hash: SpatialHash::new(thickness * 2.0),
            desc,
        })
    }

    /// The largest relative change of an edge from its rest length (for tests and diagnostics).
    pub fn max_stretch(&self) -> f32 {
        self.stretch.iter().map(|l| (self.x[l.a as usize].distance(self.x[l.b as usize]) / l.rest - 1.0).abs()).fold(0.0, f32::max)
    }

    /// Mean, 99th percentile and largest relative change of an edge from its rest length.
    pub fn stretch_stats(&self) -> (f32, f32, f32) {
        let mut e: Vec<f32> = self.stretch.iter().map(|l| (self.x[l.a as usize].distance(self.x[l.b as usize]) / l.rest - 1.0).abs()).collect();
        if e.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        e.sort_by(f32::total_cmp);
        (e.iter().sum::<f32>() / e.len() as f32, e[(e.len() * 99 / 100).min(e.len() - 1)], e[e.len() - 1])
    }

    /// How many stretch, shear and bend constraints the mesh gave.
    pub fn constraint_counts(&self) -> (usize, usize, usize) {
        (self.stretch.len(), self.shear.len(), self.bend.len())
    }

    /// Triangles of the cloth (fan-triangulated polygons).
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.tris
    }

    fn set_colliders(&mut self, frame: f64) {
        let next = match &mut self.provider {
            Some(p) if self.desc.settings.collide => {
                let mut m = p(frame);
                // Bound what one frame may hold.
                let mut left = MAX_COLLIDER_TRIANGLES;
                m.retain(|c| {
                    let keep = c.triangles.len() <= left;
                    if keep {
                        left -= c.triangles.len();
                    }
                    keep
                });
                m
            }
            _ => Vec::new(),
        };
        self.prev = if self.cur.is_empty() { next.clone() } else { std::mem::take(&mut self.cur) };
        self.cur = next;
        let same = self.prev.len() == self.cur.len()
            && self.prev.iter().zip(&self.cur).all(|(a, b)| Arc::ptr_eq(&a.positions, &b.positions) && Arc::ptr_eq(&a.triangles, &b.triangles));
        if same && self.coll_built_static {
            return;
        }
        self.coll_built_static = same;
        // Rebuild the hash over every triangle, covering where it was and where it goes. Its cell
        // is about the size of a typical triangle (sampled, so a huge collider costs little).
        let total: usize = self.cur.iter().map(|c| c.triangles.len()).sum();
        let stride = (total / 4096).max(1);
        let (mut sum_size, mut sampled) = (0.0f64, 0usize);
        for c in &self.cur {
            for t in c.triangles.iter().step_by(stride) {
                if let (Some(a), Some(b), Some(d)) = (c.positions.get(t[0] as usize), c.positions.get(t[1] as usize), c.positions.get(t[2] as usize)) {
                    sum_size += f64::from((*a - *b).length().max((*b - *d).length()));
                    sampled += 1;
                }
            }
        }
        let avg = (sum_size / sampled.max(1) as f64) as f32;
        let cell = avg.max(self.desc.settings.thickness * 4.0).max(1e-3);
        self.coll_hash = SpatialHash::new(cell);
        self.coll_tris.clear();
        self.coll_boxes.clear();
        let r = self.desc.settings.thickness;
        let mut bounds = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for (m, c) in self.cur.iter().enumerate() {
            let prev = self.prev.get(m).filter(|q| q.positions.len() == c.positions.len());
            for (ti, t) in c.triangles.iter().enumerate() {
                let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                let mut ok = true;
                for v in t {
                    let Some(q) = c.positions.get(*v as usize) else {
                        ok = false;
                        break;
                    };
                    lo = lo.min(*q);
                    hi = hi.max(*q);
                    if let Some(o) = prev.and_then(|p| p.positions.get(*v as usize)) {
                        lo = lo.min(*o);
                        hi = hi.max(*o);
                    }
                }
                if !ok || !lo.is_finite() || !hi.is_finite() {
                    continue;
                }
                bounds = (bounds.0.min(lo - Vec3::splat(r)), bounds.1.max(hi + Vec3::splat(r)));
                self.coll_hash.insert_box(self.coll_tris.len() as u32, lo - Vec3::splat(r), hi + Vec3::splat(r));
                self.coll_boxes.push((lo - Vec3::splat(r), hi + Vec3::splat(r)));
                self.coll_tris.push((m as u32, ti as u32));
            }
        }
        self.coll_bounds = bounds;
        self.coll_hash.finish();
        self.stamp.clear();
        self.stamp.resize(self.coll_tris.len(), 0);
        self.stamp_now = 0;
    }

    /// The pin target of vertex `i` this substep.
    fn pin_target(&self, i: usize, alpha: f32) -> Vec3 {
        let motion = &self.desc.pin_motion;
        if motion.is_empty() {
            return self.rest[i];
        }
        let at = |f: usize| motion.get(f.min(motion.len() - 1)).map_or(self.rest[i], |m| m.transform_point3(self.rest[i]));
        at(self.frame.saturating_sub(1)).lerp(at(self.frame), alpha)
    }

    fn apply_wind(&mut self, dt: f32) {
        let wind = self.desc.settings.wind;
        if wind.strength <= 0.0 {
            return;
        }
        for t in &self.tris {
            let (a, b, c) = (self.x[t[0] as usize], self.x[t[1] as usize], self.x[t[2] as usize]);
            let nv = tri_area_vec(a, b, c);
            let area = nv.length();
            if area < 1e-12 {
                continue;
            }
            let n = nv / area;
            let centre = (a + b + c) / 3.0;
            let vel = (self.v[t[0] as usize] + self.v[t[1] as usize] + self.v[t[2] as usize]) / 3.0;
            let rel = wind.velocity_at(centre, self.time, self.noise_seed) - vel;
            let vn = rel.dot(n);
            let force = n * (0.5 * AIR * area * vn * vn.abs());
            for v in t {
                self.v[*v as usize] += force / 3.0 * self.w[*v as usize] * dt;
            }
        }
    }

    fn solve_constraints(&mut self, dt: f32) {
        let s = &self.desc.settings;
        let kinds = [(&self.stretch, stiffness_omega(s.stretch, STRETCH_HZ, dt)), (&self.shear, stiffness_omega(s.shear, SHEAR_HZ, dt))];
        for (k, (links, od)) in kinds.into_iter().enumerate() {
            let lam = &mut self.lambda[k];
            for (li, l) in links.iter().enumerate() {
                let (a, b) = (l.a as usize, l.b as usize);
                let (wa, wb) = (self.w[a] * f32::from(!self.pinned[a]), self.w[b] * f32::from(!self.pinned[b]));
                let ws = wa + wb;
                if ws <= 0.0 {
                    continue;
                }
                let d = self.p[a] - self.p[b];
                let len = d.length();
                if len < 1e-9 {
                    continue;
                }
                let alpha = od.map_or(0.0, |o| ws / o);
                let c = len - l.rest;
                let dl = (-c - alpha * lam[li]) / (ws + alpha);
                lam[li] += dl;
                let n = d / len;
                self.p[a] += n * (wa * dl);
                self.p[b] -= n * (wb * dl);
            }
        }
        let od = stiffness_omega(s.bend, BEND_HZ, dt);
        let lam = &mut self.lambda[2];
        for (li, h) in self.bend.iter().enumerate() {
            let idx = [h.a as usize, h.b as usize, h.c as usize, h.d as usize];
            let w = idx.map(|i| self.w[i] * f32::from(!self.pinned[i]));
            let Some((angle, grad)) = hinge_angle(idx.map(|i| self.p[i])) else { continue };
            let sum: f32 = (0..4).map(|i| w[i] * grad[i].length_squared()).sum();
            if sum <= 1e-12 {
                continue;
            }
            let alpha = od.map_or(0.0, |o| sum / o);
            let mut dl = (-(angle - h.rest) - alpha * lam[li]) / (sum + alpha);
            // Near a flat or fully folded hinge the gradient vanishes and the step would not: cap
            // how far one projection may move a vertex.
            let reach = (0..4).map(|i| w[i] * grad[i].length() * dl.abs()).fold(0.0, f32::max);
            if reach > self.bend_limit {
                dl *= self.bend_limit / reach;
            }
            lam[li] += dl;
            for i in 0..4 {
                self.p[idx[i]] += grad[i] * (w[i] * dl);
            }
        }
    }

    /// Push the predicted positions out of the colliders. `alpha0` and `alpha1` are how far
    /// through the frame the substep starts and ends.
    /// Only the vertices in `only` are tested when given; returns the ones that were moved.
    fn collide_colliders(&mut self, alpha0: f32, alpha1: f32, only: Option<&[u32]>) -> Vec<u32> {
        let mut touched: Vec<u32> = Vec::new();
        if self.cur.is_empty() || self.coll_tris.is_empty() {
            return touched;
        }
        let s = &self.desc.settings;
        let (r, mu) = (s.thickness, s.friction);
        let pos_at = |m: &ColliderMesh, pm: Option<&ColliderMesh>, v: u32, a: f32| -> Vec3 {
            let c = m.positions.get(v as usize).copied().unwrap_or(Vec3::ZERO);
            match pm.and_then(|p| p.positions.get(v as usize)) {
                Some(o) => o.lerp(c, a),
                None => c,
            }
        };
        let mut hits: Vec<u32> = Vec::new();
        let count = only.map_or(self.p.len(), <[u32]>::len);
        for k in 0..count {
            let i = only.and_then(|o| o.get(k)).map_or(k, |v| *v as usize);
            if self.pinned[i] {
                continue;
            }
            let prev = self.x[i];
            let travel = self.p[i].distance(prev);
            // Far from every collider: nothing to do.
            let reach = Vec3::splat(travel + 2.0 * r);
            let q0 = self.p[i];
            if q0.cmplt(self.coll_bounds.0 - reach).any() || q0.cmpgt(self.coll_bounds.1 + reach).any() {
                continue;
            }
            hits.clear();
            self.coll_hash.query(self.p[i], travel + 2.0 * r, |k| hits.push(k));
            self.stamp_now = self.stamp_now.wrapping_add(1);
            if self.stamp_now == 0 {
                self.stamp.iter_mut().for_each(|s| *s = 0);
                self.stamp_now = 1;
            }
            for k in &hits {
                let Some(seen) = self.stamp.get_mut(*k as usize) else { continue };
                if *seen == self.stamp_now {
                    continue;
                }
                *seen = self.stamp_now;
                let Some((m, ti)) = self.coll_tris.get(*k as usize).copied() else { continue };
                if let Some((lo, hi)) = self.coll_boxes.get(*k as usize)
                    && (q0.cmplt(*lo - reach).any() || q0.cmpgt(*hi + reach).any())
                {
                    continue;
                }
                let (Some(mesh), Some(t)) = (self.cur.get(m as usize), self.cur.get(m as usize).and_then(|c| c.triangles.get(ti as usize))) else { continue };
                let pm = self.prev.get(m as usize);
                let (a, b, c) = (pos_at(mesh, pm, t[0], alpha1), pos_at(mesh, pm, t[1], alpha1), pos_at(mesh, pm, t[2], alpha1));
                let normal = (b - a).cross(c - a);
                let nl = normal.length();
                if nl < 1e-12 {
                    continue;
                }
                let n = normal / nl;
                let (a0, b0, c0) = (pos_at(mesh, pm, t[0], alpha0), pos_at(mesh, pm, t[1], alpha0), pos_at(mesh, pm, t[2], alpha0));
                // Which side the vertex started the substep on, relative to the triangle then.
                let n0 = (b0 - a0).cross(c0 - a0).try_normalize().unwrap_or(n);
                let side = if (prev - a0).dot(n0) >= 0.0 { 1.0 } else { -1.0 };
                let q = self.p[i];
                // Heights above the face, on the side the vertex came from.
                let h_prev = (prev - a).dot(n) * side;
                let height = (q - a).dot(n) * side;
                // Passed through the face during the substep: where the path crossed it.
                let crossed = h_prev >= 0.0 && height < 0.0 && {
                    let t = h_prev / (h_prev - height).max(1e-20);
                    inside_margin(prev + (q - prev) * t, [a, b, c], n, r)
                };
                // Resting on it: over the face (a hair beyond its edges, so a vertex over the seam
                // between two coplanar triangles is caught by both).
                let resting = height < r && height > -(2.0 * r + travel) && inside_margin(q - n * ((q - a).dot(n)), [a, b, c], n, r * 0.01);
                let target = if crossed || resting {
                    // Lift to one thickness above the face.
                    q + n * (side * (r - height))
                } else {
                    // Beside an edge or corner: a sphere around the nearest point.
                    let (near, _) = closest_on_triangle(q, a, b, c);
                    let d = q - near;
                    let dist = d.length();
                    if dist >= r || d.dot(n) * side < 0.0 || dist > r + travel + 1e-6 {
                        continue;
                    }
                    near + d / dist.max(1e-9) * r
                };
                let moved = target - q;
                if !moved.is_finite() {
                    continue;
                }
                // Friction: the vertex follows the triangle's own motion, and the tangential
                // slip against it is reduced by up to friction times the push.
                let motion = ((a + b + c) - (a0 + b0 + c0)) / 3.0;
                let mut out = target;
                let rel = (out - prev) - motion;
                let tangent = rel - n * rel.dot(n);
                let tl = tangent.length();
                if tl > 1e-9 && mu > 0.0 {
                    out -= tangent / tl * tl.min(mu * moved.length());
                }
                self.p[i] = out;
                if touched.last() != Some(&(i as u32)) {
                    touched.push(i as u32);
                }
            }
        }
        touched
    }

    fn collide_self(&mut self) {
        let d_min = self.desc.settings.thickness * 2.0;
        self.self_hash = SpatialHash::new(d_min);
        for (i, p) in self.p.iter().enumerate() {
            self.self_hash.insert_point(i as u32, *p);
        }
        self.self_hash.finish();
        // Neighbours in the rest pose are meant to be close: leave them to the constraints.
        let skip2 = (d_min * 3.0) * (d_min * 3.0);
        let mut near: Vec<u32> = Vec::new();
        for i in 0..self.p.len() {
            near.clear();
            self.self_hash.query(self.p[i], d_min, |j| near.push(j));
            for j in near.iter().copied() {
                let j = j as usize;
                if j <= i {
                    continue;
                }
                let (wi, wj) = (self.w[i] * f32::from(!self.pinned[i]), self.w[j] * f32::from(!self.pinned[j]));
                let ws = wi + wj;
                if ws <= 0.0 || self.rest[i].distance_squared(self.rest[j]) < skip2 {
                    continue;
                }
                let d = self.p[i] - self.p[j];
                let dist = d.length();
                if dist >= d_min || dist < 1e-9 {
                    continue;
                }
                let push = d / dist * (d_min - dist);
                self.p[i] += push * (wi / ws);
                self.p[j] -= push * (wj / ws);
            }
        }
    }
}

/// Whether `p` (in the plane of the triangle) lies within `margin` of the inside of triangle
/// `abc`, whose normal is `n` and whose winding is counter-clockwise about it.
fn inside_margin(p: Vec3, t: [Vec3; 3], n: Vec3, margin: f32) -> bool {
    (0..3).all(|k| {
        let (e0, e1) = (t[k], t[(k + 1) % 3]);
        let edge = e1 - e0;
        let inward = n.cross(edge).try_normalize().unwrap_or(Vec3::ZERO);
        (p - e0).dot(inward) >= -margin
    })
}

/// The point of triangle `abc` nearest to `p`, and whether it is inside the face (not on an
/// edge or corner). The region walk of the standard closest-point-on-triangle test.
fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> (Vec3, bool) {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, false);
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return (b, false);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let t = d1 / (d1 - d3).max(1e-20);
        return (a + ab * t, false);
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return (c, false);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let t = d2 / (d2 - d6).max(1e-20);
        return (a + ac * t, false);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let t = (d4 - d3) / ((d4 - d3) + (d5 - d6)).max(1e-20);
        return (b + (c - b) * t, false);
    }
    let denom = 1.0 / (va + vb + vc).max(1e-30);
    (a + ab * (vb * denom) + ac * (vc * denom), true)
}

impl Solver for ClothSolver {
    fn point_count(&self) -> usize {
        self.x.len()
    }

    fn reset(&mut self, seed: u64) {
        self.x.clone_from(&self.rest);
        self.p.clone_from(&self.rest);
        self.v.iter_mut().for_each(|v| *v = Vec3::ZERO);
        self.frame = 0;
        self.time = 0.0;
        self.noise_seed = (seed ^ (seed >> 32)) as u32;
        self.prev.clear();
        self.cur.clear();
        self.coll_built_static = false;
        let start = self.desc.start_frame;
        self.set_colliders(start);
    }

    fn begin_frame(&mut self, frame: f64) {
        self.frame += 1;
        self.set_colliders(frame);
    }

    fn step(&mut self, ctx: &StepContext) {
        let dt = ctx.dt;
        let s = self.desc.settings.clone();
        let g = Vec3::from_array(s.gravity);
        let (alpha1, alpha0) = (ctx.alpha(), (ctx.substep as f32) / ctx.substeps.max(1) as f32);
        self.time += dt;
        self.apply_wind(dt);
        let damp = (-s.damping * dt).exp();
        for i in 0..self.x.len() {
            if self.pinned[i] {
                self.v[i] = Vec3::ZERO;
                self.p[i] = self.pin_target(i, alpha1);
                continue;
            }
            self.v[i] = (self.v[i] + g * dt) * damp;
            let speed = self.v[i].length();
            if speed > MAX_SPEED {
                self.v[i] *= MAX_SPEED / speed;
            }
            self.p[i] = self.x[i] + self.v[i] * dt;
        }
        for l in &mut self.lambda {
            l.iter_mut().for_each(|x| *x = 0.0);
        }
        for _ in 0..s.iterations {
            self.solve_constraints(dt);
        }
        if s.self_collision {
            self.collide_self();
        }
        // Two passes: resolving one triangle can push a vertex into the next one.
        let touched = self.collide_colliders(alpha0, alpha1, None);
        if !touched.is_empty() {
            self.collide_colliders(alpha0, alpha1, Some(&touched));
        }
        for i in 0..self.x.len() {
            if !self.p[i].is_finite() {
                // Keep the last good position and stop: better a stuck vertex than a lost cloth.
                self.p[i] = self.x[i];
                self.v[i] = Vec3::ZERO;
            }
            if !self.pinned[i] {
                self.v[i] = (self.p[i] - self.x[i]) / dt.max(1e-9);
            }
            self.x[i] = self.p[i];
        }
    }

    fn write_points(&self, out: &mut Vec<Vec3>) {
        out.clear();
        out.extend_from_slice(&self.x);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;

    #[test]
    fn the_hinge_gradient_matches_finite_differences() {
        let mut r = Rng::new(11);
        for _ in 0..200 {
            let mut p = [Vec3::ZERO; 4];
            p[0] = Vec3::ZERO;
            p[1] = Vec3::new(1.0, 0.0, 0.0) + r.in_unit_ball() * 0.2;
            p[2] = Vec3::new(0.4, 0.8, 0.0) + r.in_unit_ball() * 0.4;
            p[3] = Vec3::new(0.6, -0.8, 0.0) + r.in_unit_ball() * 0.4;
            let Some((angle, g)) = hinge_angle(p) else { continue };
            if (1.0 - (angle.cos() * angle.cos())).sqrt() < 0.05 {
                continue;
            }
            for k in 0..4 {
                for axis in 0..3 {
                    let h = 1e-3;
                    let (mut a, mut b) = (p, p);
                    a[k][axis] += h;
                    b[k][axis] -= h;
                    let fd = (hinge_angle(a).map_or(0.0, |x| x.0) - hinge_angle(b).map_or(0.0, |x| x.0)) / (2.0 * h);
                    assert!((fd - g[k][axis]).abs() < 2e-2 * (1.0 + fd.abs()), "point {k} axis {axis}: finite difference {fd} vs gradient {}", g[k][axis]);
                }
            }
        }
    }
}
