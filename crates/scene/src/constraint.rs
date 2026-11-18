//! Constraints: relations that pose objects from other objects.
//!
//! A [`Constraint`] drives its *target* from its *sources*:
//!
//! | Kind | Drives | From |
//! |---|---|---|
//! | point | position | the weighted mean position of the sources |
//! | orient | rotation | the weighted mean rotation of the sources |
//! | scale | scale | the weighted mean scale of the sources |
//! | parent | position and rotation | as if the target were a child of the sources |
//! | aim | rotation | one axis looks at the sources, another keeps up |
//! | ik | the rotations of a joint chain | the chain's end reaches a goal; a pole sets the bend |
//! | driven | one attribute | another attribute, through a curve ("driven key") |
//! | expression | one attribute | a formula over attributes and the frame |
//!
//! # How they evaluate
//!
//! The scene always holds the pose of the current frame, so constraints write their results
//! into the objects ([`Scene::solve_constraints`], called after the frame changes and after every
//! command). To keep that repeatable, the transform an object had *before* the constraints ran
//! (its "authored" transform: what the user or the curves set) is kept beside the solved one:
//! every solve starts again from the authored transforms, so a half-weight constraint or an
//! IK/FK blend gives the same result however often it runs, and switching a constraint off puts
//! the object back. A transform that differs from what the last solve wrote was edited since and
//! becomes the new authored transform.
//!
//! Constraints run in dependency order: each one names what it reads and what it writes
//! (object and channel), a constraint that reads what another writes runs after it, and
//! constraints that depend on themselves, directly or through others, are skipped and reported
//! in [`Constraints::problems`] instead of looping.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
use modelcraft_anim::expr::Expr;
use modelcraft_anim::{Curve, ik, skeleton};
use serde::{Deserialize, Serialize};

use crate::{MAX_DEPTH, ObjectId, Scene, SceneError, Transform, attr};

/// Most constraints in one scene.
pub const MAX_CONSTRAINTS: usize = 4096;
/// Most sources of one constraint.
pub const MAX_SOURCES: usize = 16;
/// Longest constraint name.
pub const MAX_NAME: usize = 64;
/// The attribute-path prefix of a constraint's weight on its target: `constraint.<id>.weight`.
pub const WEIGHT_PREFIX: &str = "constraint.";

fn one() -> f32 {
    1.0
}
fn is_one(v: &f32) -> bool {
    *v == 1.0
}
fn yes() -> bool {
    true
}
fn is_true(v: &bool) -> bool {
    *v
}
fn is_identity(t: &Transform) -> bool {
    *t == Transform::default()
}
fn aim_y() -> u8 {
    1
}
fn up_z() -> u8 {
    2
}
fn world_up() -> Vec3 {
    Vec3::Y
}
fn two() -> u32 {
    2
}
fn default_iterations() -> u32 {
    24
}
fn default_tolerance() -> f32 {
    1e-4
}

/// An object a constraint follows, with its share and its offset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub object: ObjectId,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub weight: f32,
    /// Added to what the source gives: a position offset for point, a rotation for orient, a
    /// factor for scale, a whole transform in the source's space for parent.
    #[serde(default, skip_serializing_if = "is_identity")]
    pub offset: Transform,
}

impl Source {
    pub fn new(object: ObjectId) -> Source {
        Source { object, weight: 1.0, offset: Transform::default() }
    }
}

/// Which solver an IK constraint uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IkSolver {
    /// The closed form for two bones, forward-and-backward reaching for longer chains.
    #[default]
    Auto,
    TwoBone,
    Fabrik,
    Ccd,
}

impl IkSolver {
    pub const ALL: [IkSolver; 4] = [IkSolver::Auto, IkSolver::TwoBone, IkSolver::Fabrik, IkSolver::Ccd];
    pub fn id(self) -> &'static str {
        match self {
            IkSolver::Auto => "auto",
            IkSolver::TwoBone => "two_bone",
            IkSolver::Fabrik => "fabrik",
            IkSolver::Ccd => "ccd",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConstraintKind {
    Point {
        sources: Vec<Source>,
    },
    Orient {
        sources: Vec<Source>,
    },
    Scale {
        sources: Vec<Source>,
    },
    Parent {
        sources: Vec<Source>,
    },
    /// The target's `aim` axis (0 X, 1 Y, 2 Z) points at the sources; its `up` axis points as
    /// near as it can to `up_object` or, without one, along the world vector `up_vector`.
    Aim {
        sources: Vec<Source>,
        #[serde(default = "aim_y")]
        aim: u8,
        #[serde(default = "up_z")]
        up: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        up_object: Option<ObjectId>,
        #[serde(default = "world_up")]
        up_vector: Vec3,
        /// Applied after aiming (keeps the rotation the target had when constrained).
        #[serde(default)]
        offset: Quat,
    },
    /// The target is the end joint of a chain of `bones` bones; the joints above it turn so
    /// that it reaches `goal`. The constraint's weight blends from the authored rotations
    /// (forward kinematics, 0) to the solved ones (1).
    Ik {
        goal: ObjectId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pole: Option<ObjectId>,
        #[serde(default = "two")]
        bones: u32,
        #[serde(default)]
        solver: IkSolver,
        #[serde(default = "default_iterations")]
        iterations: u32,
        #[serde(default = "default_tolerance")]
        tolerance: f32,
    },
    /// `path` of the target follows `driver_path` of `driver` through `curve` (the curve's
    /// time axis is the driver's value).
    Driven {
        path: String,
        driver: ObjectId,
        driver_path: String,
        curve: Arc<Curve>,
    },
    /// `path` of the target is the value of `expr` ([`modelcraft_anim::expr`]). Names:
    /// `frame`, `time` (seconds), `fps`, `self.<path>` and `@<object id>.<path>`.
    Expression {
        path: String,
        expr: String,
    },
}

impl ConstraintKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            ConstraintKind::Point { .. } => "Point",
            ConstraintKind::Orient { .. } => "Orient",
            ConstraintKind::Scale { .. } => "Scale",
            ConstraintKind::Parent { .. } => "Parent",
            ConstraintKind::Aim { .. } => "Aim",
            ConstraintKind::Ik { .. } => "IK",
            ConstraintKind::Driven { .. } => "Driven key",
            ConstraintKind::Expression { .. } => "Expression",
        }
    }
    pub fn sources(&self) -> &[Source] {
        match self {
            ConstraintKind::Point { sources }
            | ConstraintKind::Orient { sources }
            | ConstraintKind::Scale { sources }
            | ConstraintKind::Parent { sources }
            | ConstraintKind::Aim { sources, .. } => sources,
            _ => &[],
        }
    }
    pub fn sources_mut(&mut self) -> Option<&mut Vec<Source>> {
        match self {
            ConstraintKind::Point { sources }
            | ConstraintKind::Orient { sources }
            | ConstraintKind::Scale { sources }
            | ConstraintKind::Parent { sources }
            | ConstraintKind::Aim { sources, .. } => Some(sources),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    pub id: u32,
    pub name: String,
    pub target: ObjectId,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// 0 leaves the target as authored, 1 applies the constraint fully. Keyable on the target
    /// as `constraint.<id>.weight`. Driven keys and expressions are either on or off.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub weight: f32,
    #[serde(flatten)]
    pub kind: ConstraintKind,
}

/// Something a solve could not do.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub constraint: u32,
    pub message: String,
}

/// The constraints of a scene and the authored transforms of the objects they drive.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Constraints {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    list: Vec<Constraint>,
    #[serde(default)]
    next_id: u32,
    /// What each driven object's transform is without its constraints.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    authored: BTreeMap<ObjectId, Transform>,
    /// What the last solve wrote (to tell a later edit from the solve's own result). Not saved:
    /// a scene from a file holds the solved pose.
    #[serde(skip)]
    solved: BTreeMap<ObjectId, Transform>,
    #[serde(skip)]
    problems: Vec<Problem>,
}

impl PartialEq for Constraints {
    fn eq(&self, other: &Self) -> bool {
        self.list == other.list && self.authored == other.authored
    }
}

impl Constraints {
    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.authored.is_empty()
    }
    pub fn list(&self) -> &[Constraint] {
        &self.list
    }
    pub fn get(&self, id: u32) -> Option<&Constraint> {
        self.list.iter().find(|c| c.id == id)
    }
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Constraint> {
        self.list.iter_mut().find(|c| c.id == id)
    }
    /// The constraints whose target is `object`.
    pub fn of(&self, object: ObjectId) -> impl Iterator<Item = &Constraint> {
        self.list.iter().filter(move |c| c.target == object)
    }
    /// What the last solve skipped, and why.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }
    /// The transform `object` has without its constraints, when it is driven by one.
    pub fn authored(&self, object: ObjectId) -> Option<Transform> {
        self.authored.get(&object).copied()
    }
    /// Replace the authored transform of a driven object (matching FK to IK).
    pub fn set_authored(&mut self, object: ObjectId, t: Transform) {
        if let Some(slot) = self.authored.get_mut(&object) {
            *slot = t.sanitized();
        }
    }
    pub fn heap_bytes(&self) -> usize {
        self.list.iter().map(|c| 160 + c.name.len() + c.kind.sources().len() * 64).sum::<usize>() + self.authored.len() * 96
    }

    /// Add a constraint; its id is assigned here.
    pub fn add(&mut self, name: &str, target: ObjectId, kind: ConstraintKind) -> Result<u32, SceneError> {
        if self.list.len() >= MAX_CONSTRAINTS {
            return Err(SceneError::Invalid(format!("a scene holds at most {MAX_CONSTRAINTS} constraints")));
        }
        self.next_id = self.next_id.max(self.list.iter().map(|c| c.id).max().unwrap_or(0)).saturating_add(1);
        let id = self.next_id;
        let mut c = Constraint { id, name: name.chars().take(MAX_NAME).collect(), target, enabled: true, weight: 1.0, kind };
        sanitize(&mut c);
        self.list.push(c);
        Ok(id)
    }

    pub fn remove(&mut self, id: u32) -> Option<Constraint> {
        let i = self.list.iter().position(|c| c.id == id)?;
        Some(self.list.remove(i))
    }

    /// Forget an object that left the scene: constraints on it go, and it is dropped as a
    /// source, goal, pole or driver (a constraint left with nothing to follow goes too).
    pub fn remove_object(&mut self, id: ObjectId) {
        self.list.retain_mut(|c| {
            if c.target == id {
                return false;
            }
            match &mut c.kind {
                ConstraintKind::Ik { goal, pole, .. } => {
                    if *pole == Some(id) {
                        *pole = None;
                    }
                    *goal != id
                }
                ConstraintKind::Driven { driver, .. } => *driver != id,
                ConstraintKind::Expression { .. } => true,
                ConstraintKind::Aim { sources, up_object, .. } => {
                    if *up_object == Some(id) {
                        *up_object = None;
                    }
                    sources.retain(|s| s.object != id);
                    !sources.is_empty()
                }
                ConstraintKind::Point { sources }
                | ConstraintKind::Orient { sources }
                | ConstraintKind::Scale { sources }
                | ConstraintKind::Parent { sources } => {
                    sources.retain(|s| s.object != id);
                    !sources.is_empty()
                }
            }
        });
        self.authored.remove(&id);
        self.solved.remove(&id);
    }

    /// The value of `constraint.<id>.weight` for a constraint on `target`.
    pub fn weight_attr(&self, target: ObjectId, path: &str) -> Option<f64> {
        let id = weight_path_id(path)?;
        self.get(id).filter(|c| c.target == target).map(|c| c.weight as f64)
    }

    /// Set `constraint.<id>.weight`; `Some(changed)` when the path names a constraint on `target`.
    pub fn set_weight_attr(&mut self, target: ObjectId, path: &str, value: f64) -> Option<bool> {
        let id = weight_path_id(path)?;
        let c = self.get_mut(id).filter(|c| c.target == target)?;
        let w = if value.is_finite() { (value as f32).clamp(0.0, 1.0) } else { return Some(false) };
        Some(std::mem::replace(&mut c.weight, w) != w)
    }

    /// Repair constraints read from a file.
    pub(crate) fn repair(&mut self, exists: impl Fn(ObjectId) -> bool) {
        self.list.truncate(MAX_CONSTRAINTS);
        let mut seen = BTreeSet::new();
        self.list.retain(|c| c.id != 0 && seen.insert(c.id) && exists(c.target));
        for c in &mut self.list {
            sanitize(c);
        }
        self.next_id = self.next_id.max(self.list.iter().map(|c| c.id).max().unwrap_or(0));
        self.authored.retain(|id, _| exists(*id));
        for t in self.authored.values_mut() {
            *t = t.sanitized();
        }
        self.solved.clear();
        self.problems.clear();
    }
}

/// The path of a constraint's weight on its target.
pub fn weight_path(id: u32) -> String {
    format!("{WEIGHT_PREFIX}{id}.weight")
}

fn weight_path_id(path: &str) -> Option<u32> {
    path.strip_prefix(WEIGHT_PREFIX)?.strip_suffix(".weight")?.parse().ok()
}

fn sanitize(c: &mut Constraint) {
    if c.name.chars().count() > MAX_NAME {
        c.name = c.name.chars().take(MAX_NAME).collect();
    }
    c.weight = if c.weight.is_finite() { c.weight.clamp(0.0, 1.0) } else { 1.0 };
    if let Some(sources) = c.kind.sources_mut() {
        sources.truncate(MAX_SOURCES);
        for s in sources {
            s.weight = if s.weight.is_finite() { s.weight.clamp(0.0, 1e6) } else { 1.0 };
            // An offset may hold a zero scale on purpose only for scale constraints; sanitized
            // keeps it finite and non-zero, which is right for all four kinds.
            s.offset = s.offset.sanitized();
        }
    }
    match &mut c.kind {
        ConstraintKind::Aim { aim, up, up_vector, offset, .. } => {
            if *aim > 2 || *up > 2 || *aim == *up {
                (*aim, *up) = (aim_y(), up_z());
            }
            if !up_vector.is_finite() || up_vector.length_squared() < 1e-12 {
                *up_vector = Vec3::Y;
            }
            *offset = if offset.is_finite() && offset.length() > 1e-6 { offset.normalize() } else { Quat::IDENTITY };
        }
        ConstraintKind::Ik { bones, iterations, tolerance, .. } => {
            *bones = (*bones).clamp(1, (ik::MAX_CHAIN - 1).min(MAX_DEPTH - 1) as u32);
            *iterations = (*iterations).clamp(1, ik::MAX_ITERATIONS);
            *tolerance = if tolerance.is_finite() { tolerance.clamp(1e-6, 1.0) } else { default_tolerance() };
        }
        ConstraintKind::Driven { path, driver_path, curve, .. } => {
            truncate(path, attr::MAX_PATH);
            truncate(driver_path, attr::MAX_PATH);
            Arc::make_mut(curve).sanitize();
        }
        ConstraintKind::Expression { path, expr } => {
            truncate(path, attr::MAX_PATH);
            truncate(expr, modelcraft_anim::expr::MAX_SOURCE);
        }
        _ => {}
    }
}

fn truncate(s: &mut String, max: usize) {
    while s.len() > max {
        s.pop();
    }
}

/// A channel of an object, as fine as the dependency order needs.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Chan {
    Translate,
    Rotate,
    Scale,
    Attr(String),
}

type Slot = (ObjectId, Chan);

fn chan_of(path: &str) -> Chan {
    match path.split('.').next() {
        Some("translate") => Chan::Translate,
        Some("rotate") => Chan::Rotate,
        Some("scale") => Chan::Scale,
        _ => Chan::Attr(path.to_string()),
    }
}

/// What a constraint reads and writes.
#[derive(Default)]
struct Io {
    reads: Vec<Slot>,
    writes: Vec<Slot>,
}

fn rotation_of(m: Mat4) -> Quat {
    let (_, r, _) = m.to_scale_rotation_translation();
    if r.is_finite() { r.normalize() } else { Quat::IDENTITY }
}

fn scale_of(m: Mat4) -> Vec3 {
    let (s, _, _) = m.to_scale_rotation_translation();
    if s.is_finite() { s } else { Vec3::ONE }
}

fn position_of(m: Mat4) -> Vec3 {
    m.transform_point3(Vec3::ZERO)
}

/// The weighted mean of rotations (signs aligned to the first, then normalized).
fn mean_rotation(items: &[(Quat, f32)]) -> Option<Quat> {
    let first = items.iter().find(|(_, w)| *w > 0.0)?.0;
    let mut sum = glam::Vec4::ZERO;
    for (q, w) in items {
        let v = glam::Vec4::from(*q);
        sum += if q.dot(first) < 0.0 { -v } else { v } * *w;
    }
    let q = Quat::from_vec4(sum);
    (q.is_finite() && q.length() > 1e-6).then(|| q.normalize())
}

impl Scene {
    /// The joints of an IK chain ending at `end`, root first: `bones + 1` objects, or fewer when
    /// the tree is not that deep.
    pub fn ik_chain(&self, end: ObjectId, bones: u32) -> Vec<ObjectId> {
        let mut chain = vec![end];
        let mut at = end;
        for _ in 0..(bones as usize).min(MAX_DEPTH) {
            match self.get(at).and_then(|o| o.parent) {
                Some(p) if self.get(p).is_some() => {
                    chain.push(p);
                    at = p;
                }
                _ => break,
            }
        }
        chain.reverse();
        chain
    }

    /// The objects whose transform a constraint writes.
    pub fn constraint_drives(&self, c: &Constraint) -> Vec<ObjectId> {
        match &c.kind {
            ConstraintKind::Ik { bones, .. } => {
                let mut chain = self.ik_chain(c.target, *bones);
                chain.pop();
                chain
            }
            ConstraintKind::Driven { .. } | ConstraintKind::Expression { .. } => Vec::new(),
            _ => vec![c.target],
        }
    }

    fn world_reads(&self, id: ObjectId, out: &mut Vec<Slot>) {
        let mut at = Some(id);
        for _ in 0..=MAX_DEPTH {
            let Some(o) = at.and_then(|i| self.get(i)) else { break };
            out.extend([(o.id, Chan::Translate), (o.id, Chan::Rotate), (o.id, Chan::Scale)]);
            at = o.parent;
        }
    }

    fn io(&self, c: &Constraint) -> Io {
        let mut io = Io::default();
        let parent = self.get(c.target).and_then(|o| o.parent);
        let t = c.target;
        match &c.kind {
            ConstraintKind::Point { sources } | ConstraintKind::Orient { sources } | ConstraintKind::Scale { sources } | ConstraintKind::Parent { sources } => {
                for s in sources {
                    self.world_reads(s.object, &mut io.reads);
                }
                if let Some(p) = parent {
                    self.world_reads(p, &mut io.reads);
                }
                match &c.kind {
                    ConstraintKind::Point { .. } => io.writes.push((t, Chan::Translate)),
                    ConstraintKind::Orient { .. } => io.writes.push((t, Chan::Rotate)),
                    ConstraintKind::Scale { .. } => io.writes.push((t, Chan::Scale)),
                    _ => io.writes.extend([(t, Chan::Translate), (t, Chan::Rotate)]),
                }
            }
            ConstraintKind::Aim { sources, up_object, .. } => {
                for s in sources {
                    self.world_reads(s.object, &mut io.reads);
                }
                if let Some(u) = up_object {
                    self.world_reads(*u, &mut io.reads);
                }
                if let Some(p) = parent {
                    self.world_reads(p, &mut io.reads);
                }
                io.reads.push((t, Chan::Translate));
                io.writes.push((t, Chan::Rotate));
            }
            ConstraintKind::Ik { goal, pole, bones, .. } => {
                let chain = self.ik_chain(t, *bones);
                self.world_reads(*goal, &mut io.reads);
                if let Some(p) = pole {
                    self.world_reads(*p, &mut io.reads);
                }
                if let Some(above) = chain.first().and_then(|r| self.get(*r)).and_then(|o| o.parent) {
                    self.world_reads(above, &mut io.reads);
                }
                for (i, j) in chain.iter().enumerate() {
                    io.reads.extend([(*j, Chan::Translate), (*j, Chan::Scale)]);
                    if i + 1 < chain.len() {
                        io.writes.push((*j, Chan::Rotate));
                    }
                }
            }
            ConstraintKind::Driven { path, driver, driver_path, .. } => {
                io.reads.push((*driver, chan_of(driver_path)));
                io.writes.push((t, chan_of(path)));
            }
            ConstraintKind::Expression { path, expr } => {
                if let Ok(e) = Expr::parse(expr) {
                    for v in e.variables() {
                        if let Some((object, p)) = variable_target(v, t) {
                            io.reads.push((object, chan_of(p)));
                        }
                    }
                }
                io.writes.push((t, chan_of(path)));
            }
        }
        io
    }

    /// The order the enabled constraints run in (indices into the list) and the ones that
    /// depend on themselves.
    fn constraint_order(&self) -> (Vec<usize>, Vec<usize>) {
        let list = &self.constraints.list;
        let active: Vec<usize> = (0..list.len()).filter(|i| list.get(*i).is_some_and(|c| c.enabled)).collect();
        let ios: HashMap<usize, Io> = active.iter().filter_map(|i| list.get(*i).map(|c| (*i, self.io(c)))).collect();
        let mut writers: HashMap<&Slot, Vec<usize>> = HashMap::new();
        for i in &active {
            for w in ios.get(i).map(|io| io.writes.as_slice()).unwrap_or(&[]) {
                writers.entry(w).or_default().push(*i);
            }
        }
        // Edges writer → reader, each once.
        let mut after: HashMap<usize, BTreeSet<usize>> = HashMap::new();
        let mut waits: HashMap<usize, usize> = active.iter().map(|i| (*i, 0)).collect();
        for r in &active {
            let mut before: BTreeSet<usize> = BTreeSet::new();
            for slot in ios.get(r).map(|io| io.reads.as_slice()).unwrap_or(&[]) {
                before.extend(writers.get(slot).into_iter().flatten().copied());
            }
            for w in before {
                if after.entry(w).or_default().insert(*r) {
                    *waits.entry(*r).or_default() += 1;
                }
            }
        }
        // Kahn's algorithm; among the ready, list order.
        let mut ready: BTreeSet<usize> = waits.iter().filter(|(_, n)| **n == 0).map(|(i, _)| *i).collect();
        let mut order = Vec::with_capacity(active.len());
        while let Some(i) = ready.pop_first() {
            order.push(i);
            for r in after.get(&i).into_iter().flatten() {
                if let Some(n) = waits.get_mut(r) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        ready.insert(*r);
                    }
                }
            }
        }
        let ran: BTreeSet<usize> = order.iter().copied().collect();
        let cyclic: Vec<usize> = active.into_iter().filter(|i| !ran.contains(i)).collect();
        (order, cyclic)
    }

    /// Put the authored transforms back and forget them: the scene is then as it is without
    /// constraints, until the next [`Scene::solve_constraints`]. Curves are applied in between.
    pub(crate) fn unsolve_constraints(&mut self) {
        if self.constraints.authored.is_empty() {
            return;
        }
        let authored = std::mem::take(&mut self.constraints.authored);
        let solved = std::mem::take(&mut self.constraints.solved);
        for o in &mut self.objects {
            if let Some(a) = authored.get(&o.id) {
                // Only where the solve's result is still there; an edit made since stays.
                if solved.get(&o.id).is_none_or(|s| *s == o.transform) {
                    o.transform = *a;
                }
            }
        }
    }

    /// Run every enabled constraint, in dependency order, from the authored transforms. Returns
    /// how many objects changed. See the module documentation.
    pub fn solve_constraints(&mut self) -> usize {
        if self.constraints.is_empty() {
            self.constraints.problems.clear();
            return 0;
        }
        let mut driven: BTreeSet<ObjectId> = BTreeSet::new();
        for c in self.constraints.list.iter().filter(|c| c.enabled) {
            driven.extend(self.constraint_drives(c));
        }
        // Where every object that may change stands now.
        let involved: BTreeSet<ObjectId> = driven.iter().copied().chain(self.constraints.authored.keys().copied()).collect();
        let mut start: BTreeMap<ObjectId, Transform> = BTreeMap::new();
        let mut authored = std::mem::take(&mut self.constraints.authored);
        let solved = std::mem::take(&mut self.constraints.solved);
        for o in &mut self.objects {
            if !involved.contains(&o.id) {
                continue;
            }
            start.insert(o.id, o.transform);
            let edited = solved.get(&o.id).is_some_and(|s| *s != o.transform);
            if driven.contains(&o.id) {
                match authored.get(&o.id) {
                    Some(a) if !edited => o.transform = *a,
                    _ => {
                        authored.insert(o.id, o.transform);
                    }
                }
            } else if let Some(a) = authored.remove(&o.id) {
                // No longer driven: back to what it was, unless it was edited since.
                if !edited {
                    o.transform = a;
                }
            }
        }
        authored.retain(|id, _| driven.contains(id));
        self.constraints.authored = authored;

        let (order, cyclic) = self.constraint_order();
        let mut problems = Vec::new();
        for i in cyclic {
            if let Some(c) = self.constraints.list.get(i) {
                problems.push(Problem { constraint: c.id, message: format!("{} depends on its own result (a cycle) and was skipped", c.name) });
            }
        }
        let mut attr_changes = 0;
        for i in order {
            let Some(c) = self.constraints.list.get(i).cloned() else { continue };
            match self.run_constraint(&c) {
                Ok(changed) => attr_changes += usize::from(changed),
                Err(message) => problems.push(Problem { constraint: c.id, message: format!("{}: {message}", c.name) }),
            }
        }
        self.constraints.problems = problems;

        let mut changed = attr_changes;
        let mut solved = BTreeMap::new();
        for o in &mut self.objects {
            let Some(before) = start.get(&o.id) else { continue };
            o.transform = o.transform.sanitized();
            if driven.contains(&o.id) {
                solved.insert(o.id, o.transform);
            }
            if *before != o.transform {
                o.rev = crate::next_rev();
                changed += 1;
            }
        }
        self.constraints.solved = solved;
        changed
    }

    /// The sources that exist and count, with their world matrices.
    fn gather<'a>(&self, sources: &'a [Source]) -> Result<Vec<(Mat4, &'a Source)>, String> {
        let found: Vec<(Mat4, &Source)> = sources
            .iter()
            .filter(|s| s.weight > 0.0 && self.get(s.object).is_some())
            .map(|s| (self.world_matrix(s.object), s))
            .filter(|(m, _)| m.is_finite())
            .collect();
        if found.is_empty() { Err("it has nothing to follow".into()) } else { Ok(found) }
    }

    fn local_mut(&mut self, id: ObjectId) -> Option<&mut Transform> {
        self.objects.iter_mut().find(|o| o.id == id).map(|o| &mut o.transform)
    }

    /// Run one constraint. `Ok(true)` when it changed an attribute through [`Scene::set_attr`]
    /// (transform writes are counted by the caller).
    fn run_constraint(&mut self, c: &Constraint) -> Result<bool, String> {
        let target = self.get(c.target).ok_or("its object is gone")?;
        let parent_world = target.parent.map_or(Mat4::IDENTITY, |p| self.world_matrix(p));
        let to_parent = parent_world.inverse();
        if !to_parent.is_finite() {
            return Err("the parent has no size".into());
        }
        let parent_rot = rotation_of(parent_world);
        let now = target.transform;
        let w = c.weight.clamp(0.0, 1.0);
        let mut next = now;
        match &c.kind {
            ConstraintKind::Point { sources } => {
                let found = self.gather(sources)?;
                let total: f32 = found.iter().map(|(_, s)| s.weight).sum();
                let goal = found.iter().map(|(m, s)| (position_of(*m) + s.offset.translation) * s.weight).sum::<Vec3>() / total;
                next.translation = now.translation.lerp(to_parent.transform_point3(goal), w);
            }
            ConstraintKind::Orient { sources } => {
                let found = self.gather(sources)?;
                let rots: Vec<(Quat, f32)> = found.iter().map(|(m, s)| ((rotation_of(*m) * s.offset.rotation).normalize(), s.weight)).collect();
                let goal = mean_rotation(&rots).ok_or("its sources cancel out")?;
                next.rotation = now.rotation.slerp((parent_rot.inverse() * goal).normalize(), w);
            }
            ConstraintKind::Scale { sources } => {
                let found = self.gather(sources)?;
                let total: f32 = found.iter().map(|(_, s)| s.weight).sum();
                let goal = found.iter().map(|(m, s)| scale_of(*m) * s.offset.scale * s.weight).sum::<Vec3>() / total;
                let ps = scale_of(parent_world);
                let local = Vec3::new(goal.x / ps.x, goal.y / ps.y, goal.z / ps.z);
                if !local.is_finite() {
                    return Err("the parent has no size".into());
                }
                next.scale = now.scale.lerp(local, w);
            }
            ConstraintKind::Parent { sources } => {
                let found = self.gather(sources)?;
                let total: f32 = found.iter().map(|(_, s)| s.weight).sum();
                let goals: Vec<(Mat4, f32)> = found.iter().map(|(m, s)| (*m * s.offset.matrix(), s.weight)).collect();
                let position = goals.iter().map(|(m, sw)| position_of(*m) * *sw).sum::<Vec3>() / total;
                let rots: Vec<(Quat, f32)> = goals.iter().map(|(m, sw)| (rotation_of(*m), *sw)).collect();
                let rotation = mean_rotation(&rots).ok_or("its sources cancel out")?;
                next.translation = now.translation.lerp(to_parent.transform_point3(position), w);
                next.rotation = now.rotation.slerp((parent_rot.inverse() * rotation).normalize(), w);
            }
            ConstraintKind::Aim { sources, aim, up, up_object, up_vector, offset } => {
                let found = self.gather(sources)?;
                let total: f32 = found.iter().map(|(_, s)| s.weight).sum();
                let at = found.iter().map(|(m, s)| (position_of(*m) + s.offset.translation) * s.weight).sum::<Vec3>() / total;
                let here = parent_world.transform_point3(now.translation);
                let hint = match up_object.filter(|u| self.get(*u).is_some()) {
                    Some(u) => position_of(self.world_matrix(u)) - here,
                    None => *up_vector,
                };
                let aimed = skeleton::aim_rotation(at - here, hint, *aim as usize, *up as usize).ok_or("it is on top of what it aims at")?;
                next.rotation = now.rotation.slerp((parent_rot.inverse() * aimed * *offset).normalize(), w);
            }
            ConstraintKind::Ik { goal, pole, bones, solver, iterations, tolerance } => {
                self.solve_ik(c.target, *goal, *pole, *bones, *solver, *iterations, *tolerance, w)?;
                return Ok(false);
            }
            ConstraintKind::Driven { path, driver, driver_path, curve } => {
                let input = self.attr(*driver, driver_path).ok_or_else(|| format!("its driver has no attribute {driver_path:?}"))?;
                let value = curve.eval(input).ok_or("its curve has no keys")?;
                return self.set_attr(c.target, path, value).map_err(|e| e.to_string());
            }
            ConstraintKind::Expression { path, expr } => {
                let e = Expr::parse(expr).map_err(|e| e.to_string())?;
                let frame = self.anim.frame;
                let fps = self.anim.time.fps;
                let value = e
                    .eval(&|name| match name {
                        "frame" => Some(frame),
                        "fps" => Some(fps),
                        "time" => Some(if fps > 0.0 { frame / fps } else { 0.0 }),
                        _ => variable_target(name, c.target).and_then(|(object, p)| self.attr(object, p)),
                    })
                    .map_err(|e| e.to_string())?;
                return self.set_attr(c.target, path, value).map_err(|e| e.to_string());
            }
        }
        if !next.translation.is_finite() || !next.rotation.is_finite() || !next.scale.is_finite() {
            return Err("its result is not a number".into());
        }
        if let Some(t) = self.local_mut(c.target) {
            *t = next;
        }
        Ok(false)
    }

    #[allow(clippy::too_many_arguments)]
    fn solve_ik(
        &mut self,
        end: ObjectId,
        goal: ObjectId,
        pole: Option<ObjectId>,
        bones: u32,
        solver: IkSolver,
        iterations: u32,
        tolerance: f32,
        blend: f32,
    ) -> Result<(), String> {
        let chain = self.ik_chain(end, bones);
        if chain.len() < 2 {
            return Err("the end joint has no parent to turn".into());
        }
        if self.get(goal).is_none() {
            return Err("its goal is gone".into());
        }
        let points: Vec<Vec3> = chain.iter().map(|j| position_of(self.world_matrix(*j))).collect();
        let goal_at = position_of(self.world_matrix(goal));
        let pole_at = pole.filter(|p| self.get(*p).is_some()).map(|p| position_of(self.world_matrix(p)));
        if !goal_at.is_finite() || points.iter().any(|p| !p.is_finite()) {
            return Err("a joint position is not a number".into());
        }
        let two = points.len() == 3 && matches!(solver, IkSolver::Auto | IkSolver::TwoBone);
        let solved = if two {
            match points.as_slice() {
                [a, b, c] => {
                    let (mid, tip) = ik::two_bone(*a, *b, *c, goal_at, pole_at);
                    vec![*a, mid, tip]
                }
                _ => points.clone(),
            }
        } else {
            let mut p = match solver {
                IkSolver::Ccd => ik::ccd(&points, goal_at, iterations, tolerance),
                _ => ik::fabrik(&points, goal_at, iterations, tolerance),
            };
            if let Some(pole_at) = pole_at {
                ik::apply_pole(&mut p, pole_at);
            }
            p
        };
        // Positions back to rotations, root first: each joint turns its bone onto the solved one.
        for (i, joint) in chain.iter().enumerate().take(chain.len() - 1) {
            let (Some(child), Some(want_here), Some(want_child)) = (chain.get(i + 1), solved.get(i), solved.get(i + 1)) else { continue };
            let Some(o) = self.get(*joint) else { continue };
            let fk = o.transform.rotation;
            let parent_rot = o.parent.map_or(Quat::IDENTITY, |p| rotation_of(self.world_matrix(p)));
            let world = self.world_matrix(*joint);
            let have = position_of(self.world_matrix(*child)) - position_of(world);
            let turn = ik::aim_delta(have, *want_child - *want_here);
            let world_rot = (parent_rot * fk).normalize();
            let local = (parent_rot.inverse() * turn * world_rot).normalize();
            if !local.is_finite() {
                continue;
            }
            if let Some(t) = self.local_mut(*joint) {
                t.rotation = fk.slerp(local, blend).normalize();
            }
        }
        Ok(())
    }

    /// Fill a new constraint's offsets so that it leaves its target where it is now.
    pub fn keep_offsets(&self, target: ObjectId, kind: &mut ConstraintKind) {
        let world = self.world_matrix(target);
        let (here, rot, scale) = (position_of(world), rotation_of(world), scale_of(world));
        let source_world = |s: &Source| self.world_matrix(s.object);
        match kind {
            ConstraintKind::Point { sources } => {
                for s in sources {
                    s.offset.translation = here - position_of(source_world(s));
                }
            }
            ConstraintKind::Orient { sources } => {
                for s in sources {
                    s.offset.rotation = (rotation_of(source_world(s)).inverse() * rot).normalize();
                }
            }
            ConstraintKind::Scale { sources } => {
                for s in sources {
                    let ss = scale_of(source_world(s));
                    let f = Vec3::new(scale.x / ss.x, scale.y / ss.y, scale.z / ss.z);
                    s.offset.scale = if f.is_finite() { f } else { Vec3::ONE };
                }
            }
            ConstraintKind::Parent { sources } => {
                for s in sources {
                    let m = source_world(s).inverse() * world;
                    if m.is_finite() {
                        let (sc, r, t) = m.to_scale_rotation_translation();
                        s.offset = Transform { translation: t, rotation: r, scale: sc }.sanitized();
                    }
                }
            }
            ConstraintKind::Aim { sources, aim, up, up_object, up_vector, offset } => {
                let weights: f32 = sources.iter().map(|s| s.weight).sum();
                if weights <= 0.0 {
                    return;
                }
                let at = sources.iter().map(|s| position_of(source_world(s)) * s.weight).sum::<Vec3>() / weights;
                let hint = match up_object {
                    Some(u) => position_of(self.world_matrix(*u)) - here,
                    None => *up_vector,
                };
                if let Some(aimed) = skeleton::aim_rotation(at - here, hint, *aim as usize, *up as usize) {
                    *offset = (aimed.inverse() * rot).normalize();
                }
            }
            _ => {}
        }
    }
}

/// The object and attribute path a variable of an expression names: `self.<path>` or
/// `@<id>.<path>`.
fn variable_target(name: &str, this: ObjectId) -> Option<(ObjectId, &str)> {
    if let Some(path) = name.strip_prefix("self.") {
        return Some((this, path));
    }
    let (id, path) = name.strip_prefix('@')?.split_once('.')?;
    Some((ObjectId(id.parse().ok()?), path))
}

#[cfg(test)]
#[path = "constraint_tests.rs"]
mod tests;
