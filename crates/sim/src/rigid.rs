//! Rigid bodies: active and passive bodies with box, sphere, convex hull and mesh colliders,
//! gravity, friction, bounciness, sleeping and fixed, hinge and point constraints.
//!
//! The dynamics run on Rapier (Apache-2.0, pure Rust), configured for determinism (no threads,
//! `enhanced-determinism`), so a bake is repeatable. This module owns everything around it:
//! the settings a scene stores ([`RigidSettings`]), turning meshes into bodies, animated passive
//! bodies, joints between bodies, and writing each active body's mesh points per frame, so a
//! rigid-body bake is an ordinary [`Solver`] cached by [`crate::bake`].
//!
//! **Frames and units.** Positions are in world space and metres, time in seconds. Every body's
//! own frame starts axis-aligned at the centre of its points' bounding box, so a body is
//! `pose * (point − centre)`. A *box* collider fits the bounding box of the points at the start,
//! a *sphere* has the mean of the three half-extents as its radius, a *hull* is the convex hull of
//! the points, and a *mesh* is the triangles themselves (passive bodies only: a triangle mesh has
//! no volume to give an active body mass). An animated passive body follows its keyed motion
//! (rotation and translation; scale is ignored) and pushes active bodies out of its way with the
//! velocity that motion has.

use glam::{Mat4, Quat, Vec3};
use rapier3d::prelude::{ColliderBuilder, FixedJointBuilder, PhysicsWorld, RevoluteJointBuilder, RigidBodyBuilder, RigidBodyHandle, SphericalJointBuilder};
use serde::{Deserialize, Serialize};

use crate::{MAX_POINTS, SimError, Solver, StepContext};

/// Most bodies in one world.
pub const MAX_BODIES: usize = 4096;
/// Most joints one body declares.
pub const MAX_JOINTS: usize = 16;

/// Whether the dynamics move a body, or it only gets in their way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyRole {
    /// Falls, collides and is pushed: the simulation moves it.
    #[default]
    Active,
    /// A collider: it does not react, but moves along its keyed animation when it has one.
    Passive,
}

/// The collision shape of a body.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeKind {
    /// A convex hull for an active body, the mesh itself for a passive one.
    #[default]
    Auto,
    Box,
    Sphere,
    Hull,
    /// The triangles of the mesh (passive bodies).
    Mesh,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JointKind {
    /// No relative movement at all.
    #[default]
    Fixed,
    /// Turns about one axis through the anchor.
    Hinge,
    /// Turns freely about the anchor (a ball joint).
    Point,
}

/// A constraint from the body that declares it to another body, or to the world.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct JointSpec {
    pub kind: JointKind,
    /// The scene object id of the other body's mesh; the world (fixed in space) without one.
    #[serde(default)]
    pub other: Option<u64>,
    /// Where the joint sits, in world space at the start frame.
    pub anchor: [f32; 3],
    /// A hinge's axis in world space at the start frame.
    #[serde(default = "up")]
    pub axis: [f32; 3],
    /// A hinge's angle limits in degrees (`[min, max]`).
    #[serde(default)]
    pub limit: Option<[f32; 2]>,
}

fn up() -> [f32; 3] {
    [0.0, 1.0, 0.0]
}

impl JointSpec {
    pub fn new(kind: JointKind, other: Option<u64>, anchor: [f32; 3]) -> JointSpec {
        JointSpec { kind, other, anchor, axis: up(), limit: None }
    }
}

/// The settings of one rigid body, with the world's gravity and ground as the first one baked
/// sets them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RigidSettings {
    pub role: BodyRole,
    pub shape: ShapeKind,
    /// Kilograms (active bodies).
    pub mass: f32,
    pub friction: f32,
    /// How much speed a hit keeps, 0 to 1.
    pub bounciness: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    /// Starting velocity in metres per second, world space.
    pub velocity: [f32; 3],
    /// Starting spin in radians per second about each world axis.
    pub spin: [f32; 3],
    /// Let the body go to sleep once it has settled.
    pub can_sleep: bool,
    /// Gravity, metres per second squared (the world's: taken from the body that is baked).
    pub gravity: [f32; 3],
    /// A ground plane at this height, so a scene needs no floor object (the world's).
    pub ground: Option<f32>,
    pub joints: Vec<JointSpec>,
}

impl Default for RigidSettings {
    fn default() -> Self {
        RigidSettings {
            role: BodyRole::Active,
            shape: ShapeKind::Auto,
            mass: 1.0,
            friction: 0.5,
            bounciness: 0.2,
            linear_damping: 0.05,
            angular_damping: 0.05,
            velocity: [0.0; 3],
            spin: [0.0; 3],
            can_sleep: true,
            gravity: [0.0, -9.81, 0.0],
            ground: None,
            joints: Vec::new(),
        }
    }
}

fn clamp_finite(v: f32, lo: f32, hi: f32, default: f32) -> f32 {
    if v.is_finite() { v.clamp(lo, hi) } else { default }
}

fn clamp_vec(v: [f32; 3], limit: f32) -> [f32; 3] {
    v.map(|x| clamp_finite(x, -limit, limit, 0.0))
}

impl RigidSettings {
    pub fn passive() -> RigidSettings {
        RigidSettings { role: BodyRole::Passive, ..RigidSettings::default() }
    }

    /// Clamp every number into a range the solver is stable in, and cap the joints.
    pub fn sanitize(&mut self) {
        self.mass = clamp_finite(self.mass, 1e-3, 1e6, 1.0);
        self.friction = clamp_finite(self.friction, 0.0, 10.0, 0.5);
        self.bounciness = clamp_finite(self.bounciness, 0.0, 1.0, 0.2);
        self.linear_damping = clamp_finite(self.linear_damping, 0.0, 100.0, 0.05);
        self.angular_damping = clamp_finite(self.angular_damping, 0.0, 100.0, 0.05);
        self.velocity = clamp_vec(self.velocity, 1000.0);
        self.spin = clamp_vec(self.spin, 1000.0);
        self.gravity = clamp_vec(self.gravity, 1000.0);
        self.ground = self.ground.map(|g| clamp_finite(g, -1e5, 1e5, 0.0));
        self.joints.truncate(MAX_JOINTS);
        for j in &mut self.joints {
            j.anchor = clamp_vec(j.anchor, 1e5);
            j.axis = clamp_vec(j.axis, 1.0);
            if Vec3::from_array(j.axis).length() < 1e-4 {
                j.axis = up();
            }
            j.limit = j.limit.map(|[a, b]| {
                let (a, b) = (clamp_finite(a, -360.0, 360.0, 0.0), clamp_finite(b, -360.0, 360.0, 0.0));
                [a.min(b), a.max(b)]
            });
        }
    }
}

/// One body to simulate.
#[derive(Clone, Debug)]
pub struct BodyDesc {
    /// The scene object id of the mesh (joints name bodies by it).
    pub id: u64,
    /// The mesh's points in world space at the start frame.
    pub points: Vec<Vec3>,
    /// The mesh's triangles (indices into `points`).
    pub triangles: Vec<[u32; 3]>,
    pub settings: RigidSettings,
    /// For an animated passive body: at each frame from the start, the matrix that takes the
    /// body's start-frame world space to its world space at that frame. Empty: it stays put.
    pub motion: Vec<Mat4>,
}

/// The whole world.
#[derive(Clone, Debug)]
pub struct WorldDesc {
    pub gravity: Vec3,
    pub ground: Option<f32>,
    pub bodies: Vec<BodyDesc>,
}

struct Inner {
    world: PhysicsWorld,
    handles: Vec<RigidBodyHandle>,
}

/// A rigid-body world, ready to bake: it writes the points of every active body, in body order.
pub struct RigidWorld {
    desc: WorldDesc,
    /// Every body's points relative to its centre.
    local: Vec<Vec<Vec3>>,
    centres: Vec<Vec3>,
    /// Indices of the active bodies, and where each one's points start in the output.
    active: Vec<usize>,
    ranges: Vec<std::ops::Range<usize>>,
    inner: Option<Inner>,
    /// Frames since the start, for animated passive bodies.
    frame: usize,
}

fn rv(v: Vec3) -> rapier3d::math::Vector {
    rapier3d::math::Vector::new(v.x, v.y, v.z)
}

fn from_rv(v: rapier3d::math::Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

fn pose_of(t: Vec3, r: Quat) -> rapier3d::math::Pose {
    rapier3d::math::Pose::from_parts(rv(t), rapier3d::math::Rotation::from_xyzw(r.x, r.y, r.z, r.w))
}

impl RigidWorld {
    /// Check the description and prepare the world. Fails on non-finite points, too many bodies
    /// or points, and a mesh shape on an active body.
    pub fn new(desc: WorldDesc) -> Result<RigidWorld, SimError> {
        if desc.bodies.len() > MAX_BODIES {
            return Err(SimError::Invalid(format!("a world holds at most {MAX_BODIES} bodies")));
        }
        if !desc.gravity.is_finite() {
            return Err(SimError::Invalid("gravity must be finite".into()));
        }
        let mut local = Vec::with_capacity(desc.bodies.len());
        let mut centres = Vec::with_capacity(desc.bodies.len());
        let mut active = Vec::new();
        let mut ranges = Vec::new();
        let mut total = 0usize;
        for (i, b) in desc.bodies.iter().enumerate() {
            if b.points.is_empty() || b.points.iter().any(|p| !p.is_finite() || p.abs().max_element() > 1e7) {
                return Err(SimError::Invalid(format!("body {} has no points, or a point that is not a finite position", b.id)));
            }
            if b.triangles.iter().any(|t| t.iter().any(|v| *v as usize >= b.points.len())) {
                return Err(SimError::Invalid(format!("body {} has a triangle that names a missing point", b.id)));
            }
            let (lo, hi) = b.points.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
            let centre = (lo + hi) * 0.5;
            local.push(b.points.iter().map(|p| *p - centre).collect::<Vec<_>>());
            centres.push(centre);
            if b.settings.role == BodyRole::Active {
                if b.settings.shape == ShapeKind::Mesh {
                    return Err(SimError::Invalid(format!("body {} moves, so its shape cannot be the mesh itself: use a hull, box or sphere", b.id)));
                }
                ranges.push(total..total + b.points.len());
                total += b.points.len();
                active.push(i);
            }
        }
        if total > MAX_POINTS {
            return Err(SimError::Invalid(format!("{total} points is more than the {MAX_POINTS} a simulation may have")));
        }
        Ok(RigidWorld { desc, local, centres, active, ranges, inner: None, frame: 0 })
    }

    /// The active bodies' scene ids with the range of the output each one's points occupy.
    pub fn outputs(&self) -> Vec<(u64, std::ops::Range<usize>)> {
        self.active.iter().zip(&self.ranges).filter_map(|(i, r)| self.desc.bodies.get(*i).map(|b| (b.id, r.clone()))).collect()
    }

    fn collider(&self, i: usize) -> ColliderBuilder {
        let b = &self.desc.bodies[i];
        let pts = &self.local[i];
        let (lo, hi) = pts.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        let half = ((hi - lo) * 0.5).max(Vec3::splat(1e-3));
        let mid = (lo + hi) * 0.5;
        let active = b.settings.role == BodyRole::Active;
        let shape = match b.settings.shape {
            ShapeKind::Auto if active => ShapeKind::Hull,
            ShapeKind::Auto => ShapeKind::Mesh,
            s => s,
        };
        let cuboid = || ColliderBuilder::cuboid(half.x, half.y, half.z).translation(rv(mid));
        let mut builder = match shape {
            ShapeKind::Box | ShapeKind::Auto => cuboid(),
            ShapeKind::Sphere => ColliderBuilder::ball(((half.x + half.y + half.z) / 3.0).max(1e-3)).translation(rv(mid)),
            ShapeKind::Hull => {
                let v: Vec<_> = pts.iter().copied().map(rv).collect();
                ColliderBuilder::convex_hull(&v).unwrap_or_else(cuboid)
            }
            ShapeKind::Mesh => {
                let v: Vec<_> = pts.iter().copied().map(rv).collect();
                match (b.triangles.is_empty(), ColliderBuilder::trimesh(v, b.triangles.clone())) {
                    (false, Ok(c)) => c,
                    _ => cuboid(),
                }
            }
        };
        builder = builder.friction(b.settings.friction).restitution(b.settings.bounciness);
        if active { builder.mass(b.settings.mass) } else { builder }
    }

    fn build(&self) -> Inner {
        let mut world = PhysicsWorld { gravity: rv(self.desc.gravity), ..PhysicsWorld::default() };
        if let Some(y) = self.desc.ground {
            let ground =
                ColliderBuilder::cuboid(5000.0, 5.0, 5000.0).translation(rapier3d::math::Vector::new(0.0, y - 5.0, 0.0)).friction(0.5).restitution(0.2);
            world.insert_collider(ground, None);
        }
        let mut handles = Vec::with_capacity(self.desc.bodies.len());
        for (i, b) in self.desc.bodies.iter().enumerate() {
            let s = &b.settings;
            let builder = match s.role {
                BodyRole::Active => RigidBodyBuilder::dynamic()
                    .linvel(rv(Vec3::from_array(s.velocity)))
                    .angvel(rv(Vec3::from_array(s.spin)))
                    .linear_damping(s.linear_damping)
                    .angular_damping(s.angular_damping)
                    .can_sleep(s.can_sleep)
                    .ccd_enabled(true),
                BodyRole::Passive if b.motion.len() > 1 => RigidBodyBuilder::kinematic_position_based(),
                BodyRole::Passive => RigidBodyBuilder::fixed(),
            }
            .translation(rv(self.centres[i]));
            let (h, _) = world.insert(builder, self.collider(i));
            handles.push(h);
        }
        let anchor_body = world.insert_body(RigidBodyBuilder::fixed());
        for (i, b) in self.desc.bodies.iter().enumerate() {
            for j in &b.settings.joints {
                let other = match j.other {
                    None => Some(None),
                    Some(id) => self.desc.bodies.iter().position(|o| o.id == id && o.id != b.id).map(Some),
                };
                // A joint to a body that is not in the world is dropped.
                let Some(other) = other else { continue };
                let anchor = Vec3::from_array(j.anchor);
                let a1 = anchor - self.centres[i];
                let (h2, a2) = match other {
                    Some(k) => (handles[k], anchor - self.centres[k]),
                    None => (anchor_body, anchor),
                };
                let axis = Vec3::from_array(j.axis).normalize_or(Vec3::Y);
                match j.kind {
                    JointKind::Fixed => {
                        world.insert_impulse_joint(handles[i], h2, FixedJointBuilder::new().local_anchor1(rv(a1)).local_anchor2(rv(a2)).build());
                    }
                    JointKind::Point => {
                        world.insert_impulse_joint(handles[i], h2, SphericalJointBuilder::new().local_anchor1(rv(a1)).local_anchor2(rv(a2)).build());
                    }
                    JointKind::Hinge => {
                        let mut h = RevoluteJointBuilder::new(rv(axis)).local_anchor1(rv(a1)).local_anchor2(rv(a2));
                        if let Some([lo, hi]) = j.limit {
                            h = h.limits([lo.to_radians(), hi.to_radians()]);
                        }
                        world.insert_impulse_joint(handles[i], h2, h.build());
                    }
                }
            }
        }
        Inner { world, handles }
    }

    /// The pose of an animated passive body `at` frames after the start.
    fn motion_pose(&self, i: usize, at: usize) -> (Vec3, Quat) {
        let motion = &self.desc.bodies[i].motion;
        let m = motion.get(at.min(motion.len().saturating_sub(1))).copied().unwrap_or(Mat4::IDENTITY);
        let (_, r, _) = m.to_scale_rotation_translation();
        let r = if r.is_finite() { r.normalize() } else { Quat::IDENTITY };
        (m.transform_point3(self.centres[i]), r)
    }

    /// Mechanical energy of the active bodies (translational kinetic plus gravitational
    /// potential, joules), for checking that a run does not gain energy.
    pub fn mechanical_energy(&self) -> f32 {
        let Some(inner) = &self.inner else { return 0.0 };
        let g = self.desc.gravity;
        self.active
            .iter()
            .filter_map(|i| inner.handles.get(*i).and_then(|h| inner.world.bodies.get(*h)))
            .map(|b| {
                let (m, v, p) = (b.mass(), from_rv(b.linvel()), from_rv(b.translation()));
                0.5 * m * v.length_squared() - m * g.dot(p)
            })
            .sum()
    }

    /// How many active bodies are asleep.
    pub fn sleeping(&self) -> usize {
        let Some(inner) = &self.inner else { return 0 };
        self.active.iter().filter_map(|i| inner.handles.get(*i).and_then(|h| inner.world.bodies.get(*h))).filter(|b| b.is_sleeping()).count()
    }
}

impl Solver for RigidWorld {
    fn point_count(&self) -> usize {
        self.ranges.last().map_or(0, |r| r.end)
    }

    fn reset(&mut self, _seed: u64) {
        self.inner = Some(self.build());
        self.frame = 0;
    }

    fn begin_frame(&mut self, _frame: f64) {
        self.frame += 1;
    }

    fn step(&mut self, ctx: &StepContext) {
        let Some(mut inner) = self.inner.take() else { return };
        inner.world.integration_parameters.dt = ctx.dt;
        // Animated passive bodies glide from where they were last frame to where they are this
        // frame over the substeps, so active bodies feel their speed.
        for (i, b) in self.desc.bodies.iter().enumerate() {
            if b.settings.role == BodyRole::Passive && b.motion.len() > 1 {
                let (t0, r0) = self.motion_pose(i, self.frame.saturating_sub(1));
                let (t1, r1) = self.motion_pose(i, self.frame);
                let a = ctx.alpha();
                let (t, r) = (t0.lerp(t1, a), r0.slerp(r1, a));
                if let Some(body) = inner.handles.get(i).and_then(|h| inner.world.bodies.get_mut(*h)) {
                    body.set_next_kinematic_position(pose_of(t, r));
                }
            }
        }
        inner.world.step();
        self.inner = Some(inner);
    }

    fn write_points(&self, out: &mut Vec<Vec3>) {
        out.clear();
        for i in &self.active {
            let pose = self.inner.as_ref().and_then(|n| n.handles.get(*i).and_then(|h| n.world.bodies.get(*h))).map(|b| *b.position());
            let local = &self.local[*i];
            match pose {
                Some(p) => out.extend(local.iter().map(|q| from_rv(p.transform_point(rv(*q))))),
                None => out.extend(local.iter().map(|q| *q + self.centres[*i])),
            }
        }
    }
}
