//! Particles: emitters, forces, collisions and a fixed pool whose slots are cached as points.
//!
//! - **Emitters** give a birth position and a direction: a [`Emitter::Point`] at one place, a
//!   [`Emitter::Surface`] anywhere on a mesh (uniform by area, along the face normal) or a
//!   [`Emitter::Volume`] anywhere inside a closed mesh. They emit at a rate per second, plus an
//!   optional burst at the start; lifetime, speed and direction have a variation and a cone
//!   spread. An emitter that moves is given as a matrix per frame, and its speed can be inherited.
//! - **Forces** act on every live particle: gravity, drag against a wind with turbulence, noise
//!   (curl-free random acceleration), and attractors (pull or swirl, with a falloff radius).
//! - **Collisions**: the path of every particle in a substep is tested against the triangles of
//!   the colliders (swept, so a fast particle cannot tunnel), with bounce, friction and the
//!   collider's own motion.
//! - **The pool** has `capacity` slots, filled in ascending order; a dead slot is stored as
//!   [`DEAD`] (NaN), so a frame of the cache is the whole pool and a particle keeps its slot (and
//!   so its size) for its life. A pool that is full simply stops emitting.
//!
//! Everything runs from one seeded [`Rng`] in a fixed order: the same seed gives the same
//! particles bit for bit.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};

use crate::cloth::{ColliderMesh, ColliderProvider};
use crate::field::{Wind, vector_noise};
use crate::hash::SpatialHash;
use crate::{DEAD, MAX_POINTS, Rng, SimError, Solver, StepContext};

/// Most slots in one pool.
pub const MAX_CAPACITY: u32 = 1_000_000;
/// Most attractors one system has.
pub const MAX_ATTRACTORS: usize = 64;
/// Most triangles an emitter mesh may have for volume sampling.
const MAX_VOLUME_TRIANGLES: usize = 20_000;

/// Where particles are born.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmitterKind {
    /// At the emitter object's origin.
    Point,
    /// On the surface of the emitter mesh.
    #[default]
    Surface,
    /// Inside the emitter mesh (which should be closed).
    Volume,
}

/// Which way particles leave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Along the face normal on a surface; any way elsewhere.
    #[default]
    Normal,
    /// Along the `direction` setting.
    Fixed,
    /// Any way.
    Random,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttractorKind {
    /// Pulls towards the position.
    #[default]
    Attract,
    /// Swirls around the axis through the position.
    Vortex,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attractor {
    pub kind: AttractorKind,
    /// World position at the start frame.
    pub position: [f32; 3],
    /// Acceleration at the position, metres per second squared (negative pushes away).
    pub strength: f32,
    /// Beyond this distance the pull is gone (0: no limit). Inside it falls off smoothly.
    pub radius: f32,
    /// A vortex's axis.
    pub axis: [f32; 3],
}

impl Default for Attractor {
    fn default() -> Self {
        Attractor { kind: AttractorKind::Attract, position: [0.0; 3], strength: 5.0, radius: 0.0, axis: [0.0, 1.0, 0.0] }
    }
}

/// The settings of one particle system.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParticleSettings {
    pub emitter: EmitterKind,
    /// Particles born per second.
    pub rate: f32,
    /// Particles born at the start frame, on top of the rate.
    pub burst: u32,
    /// Seconds a particle lives.
    pub lifetime: f32,
    /// Random share of the lifetime, 0 to 1.
    pub lifetime_variation: f32,
    /// Birth speed in metres per second.
    pub speed: f32,
    /// Random share of the speed, 0 to 1.
    pub speed_variation: f32,
    pub direction: Direction,
    /// The direction of a `fixed` emitter, any length.
    pub fixed_direction: [f32; 3],
    /// Half-angle of the cone the direction is spread over, degrees (180: all ways).
    pub spread: f32,
    /// How much of the emitter's own velocity a new particle takes, 0 to 1.
    pub inherit: f32,
    pub gravity: [f32; 3],
    /// Drag against the air (the wind, or still air): per second.
    pub drag: f32,
    pub wind: Wind,
    /// Strength of a random acceleration field, metres per second squared.
    pub noise_strength: f32,
    /// Size of the noise features, metres.
    pub noise_scale: f32,
    /// How fast the noise changes, features per second.
    pub noise_speed: f32,
    pub attractors: Vec<Attractor>,
    /// Collide with other visible meshes (or only `collide_with`).
    pub collide: bool,
    /// How much speed a hit keeps, 0 to 1.
    pub bounciness: f32,
    /// How much sliding speed a hit removes, 0 to 1.
    pub friction: f32,
    pub collide_with: Vec<u64>,
    /// Slots in the pool: the most particles alive at once.
    pub capacity: u32,
    /// Radius of a particle when drawn, metres.
    pub size: f32,
    /// Random share of the size, 0 to 1.
    pub size_variation: f32,
}

impl Default for ParticleSettings {
    fn default() -> Self {
        ParticleSettings {
            emitter: EmitterKind::Surface,
            rate: 200.0,
            burst: 0,
            lifetime: 2.0,
            lifetime_variation: 0.25,
            speed: 2.0,
            speed_variation: 0.3,
            direction: Direction::Normal,
            fixed_direction: [0.0, 1.0, 0.0],
            spread: 20.0,
            inherit: 0.0,
            gravity: [0.0, -9.81, 0.0],
            drag: 0.1,
            wind: Wind::default(),
            noise_strength: 0.0,
            noise_scale: 1.0,
            noise_speed: 0.5,
            attractors: Vec::new(),
            collide: true,
            bounciness: 0.4,
            friction: 0.2,
            collide_with: Vec::new(),
            capacity: 5000,
            size: 0.03,
            size_variation: 0.3,
        }
    }
}

fn fin(v: f32, lo: f32, hi: f32, d: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { d }
}

impl ParticleSettings {
    /// Clamp every number into a usable range and cap the lists.
    pub fn sanitize(&mut self) {
        self.rate = fin(self.rate, 0.0, 1e6, 0.0);
        self.burst = self.burst.min(MAX_CAPACITY);
        self.lifetime = fin(self.lifetime, 0.01, 1e5, 2.0);
        self.lifetime_variation = fin(self.lifetime_variation, 0.0, 1.0, 0.0);
        self.speed = fin(self.speed, 0.0, 1e3, 1.0);
        self.speed_variation = fin(self.speed_variation, 0.0, 1.0, 0.0);
        self.fixed_direction = self.fixed_direction.map(|x| fin(x, -1e3, 1e3, 0.0));
        if Vec3::from_array(self.fixed_direction).length() < 1e-6 {
            self.fixed_direction = [0.0, 1.0, 0.0];
        }
        self.spread = fin(self.spread, 0.0, 180.0, 0.0);
        self.inherit = fin(self.inherit, 0.0, 1.0, 0.0);
        self.gravity = self.gravity.map(|x| fin(x, -1e3, 1e3, 0.0));
        self.drag = fin(self.drag, 0.0, 1e3, 0.0);
        self.wind.sanitize();
        self.noise_strength = fin(self.noise_strength, 0.0, 1e3, 0.0);
        self.noise_scale = fin(self.noise_scale, 0.01, 1e3, 1.0);
        self.noise_speed = fin(self.noise_speed, 0.0, 1e3, 0.5);
        self.attractors.truncate(MAX_ATTRACTORS);
        for a in &mut self.attractors {
            a.position = a.position.map(|x| fin(x, -1e6, 1e6, 0.0));
            a.strength = fin(a.strength, -1e4, 1e4, 0.0);
            a.radius = fin(a.radius, 0.0, 1e6, 0.0);
            a.axis = a.axis.map(|x| fin(x, -1.0, 1.0, 0.0));
            if Vec3::from_array(a.axis).length() < 1e-6 {
                a.axis = [0.0, 1.0, 0.0];
            }
        }
        self.bounciness = fin(self.bounciness, 0.0, 1.0, 0.4);
        self.friction = fin(self.friction, 0.0, 1.0, 0.2);
        self.collide_with.sort_unstable();
        self.collide_with.dedup();
        self.collide_with.truncate(1024);
        self.capacity = self.capacity.clamp(1, MAX_CAPACITY);
        self.size = fin(self.size, 1e-4, 1e3, 0.03);
        self.size_variation = fin(self.size_variation, 0.0, 1.0, 0.0);
    }

    /// The display radius of the particle in `slot`: the size varied by a hash of the slot, so a
    /// particle keeps its size and no per-particle data needs caching.
    pub fn size_of(&self, slot: usize) -> f32 {
        let mut h = (slot as u32).wrapping_mul(0x9e37_79b1) ^ 0x85eb_ca6b;
        h ^= h >> 15;
        h = h.wrapping_mul(0x2c1b_3c6d);
        h ^= h >> 12;
        let u = (h & 0xffff) as f32 / 65535.0;
        self.size * (1.0 + self.size_variation * (u * 2.0 - 1.0))
    }
}

/// Where particles are born, in world space at the start frame.
#[derive(Clone, Debug)]
pub struct EmitterGeometry {
    /// The emitter object's origin (for a point emitter).
    pub origin: Vec3,
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    /// At each frame from the start, the matrix taking the start-frame world space to the world
    /// space then. Empty: the emitter does not move.
    pub motion: Vec<Mat4>,
}

pub struct ParticleDesc {
    pub settings: ParticleSettings,
    pub emitter: EmitterGeometry,
    pub start_frame: f64,
    /// Frames per second (to turn the emitter's motion per frame into a velocity).
    pub fps: f64,
}

/// A system ready to bake. It writes the whole pool every frame.
pub struct ParticleSolver {
    desc: ParticleDesc,
    rng: Rng,
    /// Cumulative triangle areas, for sampling a surface.
    cumulative: Vec<f32>,
    pos: Vec<Vec3>,
    vel: Vec<Vec3>,
    age: Vec<f32>,
    life: Vec<f32>,
    /// Next slot to look at when emitting.
    cursor: usize,
    /// Fraction of a particle owed to the next substep.
    owed: f32,
    frame: usize,
    time: f32,
    noise_seed: u32,
    provider: Option<ColliderProvider>,
    prev: Vec<ColliderMesh>,
    cur: Vec<ColliderMesh>,
    coll_tris: Vec<(u32, u32)>,
    coll_boxes: Vec<(Vec3, Vec3)>,
    coll_hash: SpatialHash,
    coll_static: bool,
    /// How many particles were born and have died, for tests and statistics.
    born: u64,
    died: u64,
}

fn tri_area(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    0.5 * (b - a).cross(c - a).length()
}

/// Möller–Trumbore: where the segment `from + t·d` (t in 0..=1) crosses the triangle, if it does.
fn segment_hits(from: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let h = d.cross(e2);
    let det = e1.dot(h);
    if det.abs() < 1e-14 {
        return None;
    }
    let inv = 1.0 / det;
    let s = from - a;
    let u = s.dot(h) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (0.0..=1.0).contains(&t).then_some(t)
}

impl ParticleSolver {
    pub fn new(mut desc: ParticleDesc, provider: Option<ColliderProvider>) -> Result<ParticleSolver, SimError> {
        desc.settings.sanitize();
        let g = &desc.emitter;
        if !g.origin.is_finite() || g.positions.iter().any(|p| !p.is_finite()) {
            return Err(SimError::Invalid("the emitter has a point that is not a finite position".into()));
        }
        if g.triangles.iter().any(|t| t.iter().any(|v| *v as usize >= g.positions.len())) {
            return Err(SimError::Invalid("the emitter has a triangle that names a missing point".into()));
        }
        if desc.settings.emitter != EmitterKind::Point && g.triangles.is_empty() {
            return Err(SimError::Invalid("a surface or volume emitter needs a mesh with faces".into()));
        }
        let n = desc.settings.capacity as usize;
        if n > MAX_POINTS {
            return Err(SimError::Invalid(format!("{n} slots is more than the {MAX_POINTS} a simulation may have")));
        }
        let mut sum = 0.0f32;
        let cumulative: Vec<f32> = g
            .triangles
            .iter()
            .map(|t| {
                sum += tri_area(g.positions[t[0] as usize], g.positions[t[1] as usize], g.positions[t[2] as usize]);
                sum
            })
            .collect();
        Ok(ParticleSolver {
            rng: Rng::new(1),
            cumulative,
            pos: vec![DEAD; n],
            vel: vec![Vec3::ZERO; n],
            age: vec![0.0; n],
            life: vec![0.0; n],
            cursor: 0,
            owed: 0.0,
            frame: 0,
            time: 0.0,
            noise_seed: 0,
            provider,
            prev: Vec::new(),
            cur: Vec::new(),
            coll_tris: Vec::new(),
            coll_boxes: Vec::new(),
            coll_hash: SpatialHash::new(1.0),
            coll_static: false,
            born: 0,
            died: 0,
            desc,
        })
    }

    /// Particles alive now.
    pub fn alive(&self) -> usize {
        self.pos.iter().filter(|p| p.is_finite()).count()
    }

    /// Particles born and died since the start of the run.
    pub fn counts(&self) -> (u64, u64) {
        (self.born, self.died)
    }

    pub fn settings(&self) -> &ParticleSettings {
        &self.desc.settings
    }

    /// The emitter's matrix at `frame` (frames since the start) and the one before it.
    fn emitter_matrix(&self, frame: usize) -> Mat4 {
        let m = &self.desc.emitter.motion;
        if m.is_empty() { Mat4::IDENTITY } else { m.get(frame.min(m.len() - 1)).copied().unwrap_or(Mat4::IDENTITY) }
    }

    fn set_colliders(&mut self, frame: f64) {
        let next = match &mut self.provider {
            Some(p) if self.desc.settings.collide => p(frame),
            _ => Vec::new(),
        };
        self.prev = if self.cur.is_empty() { next.clone() } else { std::mem::take(&mut self.cur) };
        self.cur = next;
        let same = self.prev.len() == self.cur.len()
            && self.prev.iter().zip(&self.cur).all(|(a, b)| Arc::ptr_eq(&a.positions, &b.positions) && Arc::ptr_eq(&a.triangles, &b.triangles));
        if same && self.coll_static {
            return;
        }
        self.coll_static = same;
        let total: usize = self.cur.iter().map(|c| c.triangles.len()).sum();
        let stride = (total / 4096).max(1);
        let (mut sum, mut n) = (0.0f64, 0usize);
        for c in &self.cur {
            for t in c.triangles.iter().step_by(stride) {
                if let (Some(a), Some(b), Some(d)) = (c.positions.get(t[0] as usize), c.positions.get(t[1] as usize), c.positions.get(t[2] as usize)) {
                    sum += f64::from((*a - *b).length().max((*b - *d).length()));
                    n += 1;
                }
            }
        }
        let cell = ((sum / n.max(1) as f64) as f32).max(self.desc.settings.size * 4.0).max(1e-3);
        self.coll_hash = SpatialHash::new(cell);
        self.coll_tris.clear();
        self.coll_boxes.clear();
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
                self.coll_hash.insert_box(self.coll_tris.len() as u32, lo, hi);
                self.coll_boxes.push((lo, hi));
                self.coll_tris.push((m as u32, ti as u32));
            }
        }
        self.coll_hash.finish();
    }

    /// A point on or in the emitter, and the direction the face points there.
    fn birth_point(&mut self, m: Mat4) -> (Vec3, Option<Vec3>) {
        let g = &self.desc.emitter;
        match self.desc.settings.emitter {
            EmitterKind::Point => (m.transform_point3(g.origin), None),
            EmitterKind::Surface => {
                let total = self.cumulative.last().copied().unwrap_or(0.0);
                let pick = self.rng.f32() * total;
                let ti = self.cumulative.partition_point(|c| *c < pick).min(g.triangles.len().saturating_sub(1));
                let t = g.triangles[ti];
                let (a, b, c) = (g.positions[t[0] as usize], g.positions[t[1] as usize], g.positions[t[2] as usize]);
                let (mut u, mut v) = (self.rng.f32(), self.rng.f32());
                if u + v > 1.0 {
                    (u, v) = (1.0 - u, 1.0 - v);
                }
                let p = a + (b - a) * u + (c - a) * v;
                let n = (b - a).cross(c - a).try_normalize();
                (m.transform_point3(p), n.map(|n| m.transform_vector3(n).normalize_or_zero()))
            }
            EmitterKind::Volume => {
                let (lo, hi) = g.positions.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(*p), h.max(*p)));
                let small = g.triangles.len() <= MAX_VOLUME_TRIANGLES;
                for _ in 0..24 {
                    let p = lo + (hi - lo) * Vec3::new(self.rng.f32(), self.rng.f32(), self.rng.f32());
                    if !small || inside(p, &g.positions, &g.triangles) {
                        return (m.transform_point3(p), None);
                    }
                }
                // Nothing inside after many tries (an open or thin mesh): the middle of its box.
                (m.transform_point3((lo + hi) * 0.5), None)
            }
        }
    }

    fn spawn(&mut self, count: usize, ctx_frame: usize, alpha: f32) {
        let s = self.desc.settings.clone();
        let n = self.pos.len();
        let (m0, m1) = (self.emitter_matrix(ctx_frame.saturating_sub(1)), self.emitter_matrix(ctx_frame));
        let m = if self.desc.emitter.motion.is_empty() {
            Mat4::IDENTITY
        } else {
            Mat4::from_cols_array(&std::array::from_fn(|i| m0.to_cols_array()[i] + (m1.to_cols_array()[i] - m0.to_cols_array()[i]) * alpha))
        };
        for _ in 0..count {
            // Find a free slot, starting after the last one used.
            let Some(slot) = (0..n).map(|k| (self.cursor + k) % n).find(|i| !self.pos[*i].is_finite()) else { break };
            self.cursor = (slot + 1) % n;
            let (p, normal) = self.birth_point(m);
            let dir = match (s.direction, normal) {
                (Direction::Normal, Some(nrm)) if nrm != Vec3::ZERO => nrm,
                (Direction::Fixed, _) => Vec3::from_array(s.fixed_direction).normalize_or(Vec3::Y),
                _ => self.rng.unit_vec3(),
            };
            let dir = cone(dir, s.spread.to_radians(), &mut self.rng);
            let speed = s.speed * (1.0 + s.speed_variation * self.rng.range(-1.0, 1.0));
            let life = s.lifetime * (1.0 + s.lifetime_variation * self.rng.range(-1.0, 1.0));
            let mut v = dir * speed.max(0.0);
            if s.inherit > 0.0 && !self.desc.emitter.motion.is_empty() {
                // The emitter's own velocity at the birth point: how far this frame carried it.
                let step = (m1 * m0.inverse()).transform_point3(p) - p;
                v += step * (s.inherit * self.desc.fps as f32);
            }
            self.pos[slot] = p;
            self.vel[slot] = v;
            self.age[slot] = 0.0;
            self.life[slot] = life.max(1e-3);
            self.born += 1;
        }
    }

    fn collide(&mut self, i: usize, from: Vec3, to: Vec3, alpha0: f32, alpha1: f32, dt: f32) {
        let s = &self.desc.settings;
        let (e, mu) = (s.bounciness, s.friction);
        let mut start = from;
        let mut end = to;
        let mut vel = self.vel[i];
        let mut hits: Vec<u32> = Vec::new();
        for _ in 0..3 {
            let d = end - start;
            if d.length_squared() < 1e-18 {
                break;
            }
            let reach = d.length() + s.size;
            let mid = (start + end) * 0.5;
            hits.clear();
            self.coll_hash.query(mid, reach, |k| hits.push(k));
            let mut best: Option<(f32, Vec3, Vec3)> = None;
            for k in &hits {
                let Some((lo, hi)) = self.coll_boxes.get(*k as usize) else { continue };
                if start.min(end).cmpgt(*hi + Vec3::splat(s.size)).any() || start.max(end).cmplt(*lo - Vec3::splat(s.size)).any() {
                    continue;
                }
                let Some((m, ti)) = self.coll_tris.get(*k as usize).copied() else { continue };
                let (Some(mesh), Some(t)) = (self.cur.get(m as usize), self.cur.get(m as usize).and_then(|c| c.triangles.get(ti as usize))) else { continue };
                let pm = self.prev.get(m as usize);
                let at = |v: u32, a: f32| -> Vec3 {
                    let c = mesh.positions.get(v as usize).copied().unwrap_or(Vec3::ZERO);
                    pm.and_then(|p| p.positions.get(v as usize)).map_or(c, |o| o.lerp(c, a))
                };
                // In the frame of the triangle: it moved by `disp` over the substep, so the path
                // relative to it ends `disp` short of where the particle really ends.
                let (a, b, c) = (at(t[0], alpha1), at(t[1], alpha1), at(t[2], alpha1));
                let (a0, b0, c0) = (at(t[0], alpha0), at(t[1], alpha0), at(t[2], alpha0));
                let disp = ((a + b + c) - (a0 + b0 + c0)) / 3.0;
                let rel = d - disp;
                let Some(tt) = segment_hits(start, rel, a0, b0, c0) else { continue };
                if best.is_none_or(|(bt, ..)| tt < bt) {
                    let mut n = (b0 - a0).cross(c0 - a0).normalize_or_zero();
                    if n.dot(rel) > 0.0 {
                        n = -n;
                    }
                    best = Some((tt, n, disp / dt.max(1e-9)));
                }
            }
            let Some((t, n, collider_velocity)) = best else { break };
            let hit = start + d * t;
            let rel = vel - collider_velocity;
            let vn = rel.dot(n);
            let (rn, rt) = (n * vn, rel - n * vn);
            // Slow impacts do not bounce, or a resting particle would chatter.
            let bounce = if vn.abs() < 0.8 { 0.0 } else { e };
            vel = collider_velocity - rn * bounce + rt * (1.0 - mu);
            start = hit + n * 1e-4;
            end = start + vel * dt * (1.0 - t);
        }
        self.pos[i] = end;
        self.vel[i] = vel;
    }
}

/// A direction within `half` radians of `dir`, uniform over the cone.
fn cone(dir: Vec3, half: f32, rng: &mut Rng) -> Vec3 {
    if half <= 1e-6 {
        return dir;
    }
    let cos_h = half.cos();
    let z = 1.0 - rng.f32() * (1.0 - cos_h);
    let a = rng.range(0.0, std::f32::consts::TAU);
    let r = (1.0 - z * z).max(0.0).sqrt();
    let (u, v) = dir.any_orthonormal_pair();
    (u * (r * a.cos()) + v * (r * a.sin()) + dir * z).normalize_or(dir)
}

/// Whether `p` is inside the closed mesh: the parity of crossings of a ray along +X.
fn inside(p: Vec3, positions: &[Vec3], triangles: &[[u32; 3]]) -> bool {
    let d = Vec3::new(1e6, 1.0e3, 7.0e2);
    let mut crossings = 0u32;
    for t in triangles {
        if segment_hits(p, d, positions[t[0] as usize], positions[t[1] as usize], positions[t[2] as usize]).is_some() {
            crossings += 1;
        }
    }
    crossings % 2 == 1
}

impl Solver for ParticleSolver {
    fn point_count(&self) -> usize {
        self.pos.len()
    }

    fn reset(&mut self, seed: u64) {
        self.rng = Rng::new(seed);
        self.pos.iter_mut().for_each(|p| *p = DEAD);
        self.vel.iter_mut().for_each(|v| *v = Vec3::ZERO);
        self.age.iter_mut().for_each(|a| *a = 0.0);
        self.life.iter_mut().for_each(|l| *l = 0.0);
        self.cursor = 0;
        self.owed = 0.0;
        self.frame = 0;
        self.time = 0.0;
        self.born = 0;
        self.died = 0;
        self.noise_seed = (seed ^ (seed >> 32)) as u32;
        self.prev.clear();
        self.cur.clear();
        self.coll_static = false;
        let start = self.desc.start_frame;
        self.set_colliders(start);
        // The burst is born at the start frame, before any step.
        let burst = self.desc.settings.burst as usize;
        self.spawn(burst, 0, 0.0);
    }

    fn begin_frame(&mut self, frame: f64) {
        self.frame += 1;
        self.set_colliders(frame);
    }

    fn step(&mut self, ctx: &StepContext) {
        let dt = ctx.dt;
        let (alpha1, alpha0) = (ctx.alpha(), ctx.substep as f32 / ctx.substeps.max(1) as f32);
        self.time += dt;
        // Birth: the rate, carrying the fraction over.
        self.owed += self.desc.settings.rate * dt;
        let whole = self.owed.floor();
        self.owed -= whole;
        self.spawn(whole as usize, self.frame, alpha1);
        let s = self.desc.settings.clone();
        let g = Vec3::from_array(s.gravity);
        let have_attractors = !s.attractors.is_empty();
        let alpha_wind = s.wind.strength > 0.0;
        for i in 0..self.pos.len() {
            if !self.pos[i].is_finite() {
                continue;
            }
            self.age[i] += dt;
            if self.age[i] >= self.life[i] {
                self.pos[i] = DEAD;
                self.died += 1;
                continue;
            }
            let p = self.pos[i];
            let mut v = self.vel[i];
            let mut a = g;
            if s.drag > 0.0 {
                let air = if alpha_wind { s.wind.velocity_at(p, self.time, self.noise_seed) } else { Vec3::ZERO };
                a += (air - v) * s.drag;
            }
            if s.noise_strength > 0.0 {
                let q = p / s.noise_scale + Vec3::splat(self.time * s.noise_speed);
                a += vector_noise(q, self.noise_seed.wrapping_add(77)) * s.noise_strength;
            }
            if have_attractors {
                for at in &s.attractors {
                    let to = Vec3::from_array(at.position) - p;
                    let dist = to.length().max(1e-3);
                    let fall = if at.radius > 0.0 { (1.0 - dist / at.radius).clamp(0.0, 1.0) } else { 1.0 };
                    if fall <= 0.0 {
                        continue;
                    }
                    match at.kind {
                        AttractorKind::Attract => a += to / dist * (at.strength * fall),
                        AttractorKind::Vortex => {
                            let axis = Vec3::from_array(at.axis).normalize_or(Vec3::Y);
                            a += axis.cross(-to).normalize_or_zero() * (at.strength * fall);
                        }
                    }
                }
            }
            v += a * dt;
            let speed = v.length();
            if speed > 1e4 {
                v *= 1e4 / speed;
            }
            let to = p + v * dt;
            self.vel[i] = v;
            if s.collide && !self.coll_tris.is_empty() {
                self.collide(i, p, to, alpha0, alpha1, dt);
            } else {
                self.pos[i] = to;
            }
            if !self.pos[i].is_finite() || self.pos[i].abs().max_element() > 1e8 {
                self.pos[i] = DEAD;
                self.died += 1;
            }
        }
    }

    fn write_points(&self, out: &mut Vec<Vec3>) {
        out.clear();
        out.extend_from_slice(&self.pos);
    }

    fn allows_dead(&self) -> bool {
        true
    }
}
