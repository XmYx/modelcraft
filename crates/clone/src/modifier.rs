//! Clone modifiers: they change where each clone sits, how it is turned and sized, and its tint.
//!
//! A [`Modifier`] has a [`Mode`] that decides *how much* of its change each clone gets (the
//! factor), and four channels that say *what* the change is: a move, a turn, a size change and a
//! tint. The change a clone receives is `channel * factor * strength * weight`, where `weight`
//! comes from the falloff fields the modifier holds (1 where it has none).
//!
//! | mode     | factor of clone `i`                                                         |
//! |----------|-----------------------------------------------------------------------------|
//! | `plain`  | 1 for every clone                                                           |
//! | `random` | a seeded random number per clone and channel; it does not change when the count does |
//! | `noise`  | smooth 3D noise at the clone's place, drifting with time                     |
//! | `step`   | a ramp along the clone order, optionally cut into steps                      |
//! | `delay`  | 0 to 1 over time, starting later for each clone (time based)                 |
//! | `target` | how close the clone is to a target point; also pulls and turns toward it     |
//! | `formula`| a formula over `i`, `n`, `t` (`i / (n - 1)`), `x`, `y`, `z` and `time`          |
//!
//! A formula is parsed once per run and its work is capped at [`MAX_FORMULA_WORK`] steps (clones
//! times formula nodes); over the cap, or when it does not parse, the modifier changes nothing and
//! [`Modifier::problem`] says why.
//!
//! Several modifiers apply one after the other, in order. Everything is total and bounded: every
//! parameter is sanitized first, so hostile numbers give finite transforms, and the number of
//! clones never changes.

use glam::{EulerRot, Mat4, Quat, Vec3};
use modelcraft_anim::expr::Expr;
use serde::{Deserialize, Serialize};

use crate::{Axis, CloneSet, MAX_DEG, MAX_LEN, MIN_SCALE};

/// Most steps one formula modifier takes in one run: the clones times the nodes of its formula.
pub const MAX_FORMULA_WORK: usize = 1 << 21;
/// The names a formula modifier reads.
pub const FORMULA_NAMES: &[&str] = &["i", "n", "t", "x", "y", "z", "time"];
/// A formula factor is kept within this many times the change.
const MAX_FORMULA_FACTOR: f64 = 10.0;

fn default_formula() -> String {
    "sin(t * tau + time * 2) * 0.5 + 0.5".into()
}

/// Longest time span a modifier accepts, in seconds.
const MAX_SECONDS: f32 = 3600.0;
/// Most noise octaves.
pub const MAX_NOISE_OCTAVES: u32 = 4;
/// Most steps a step modifier cuts its ramp into.
pub const MAX_STEPS: u32 = 10_000;

/// How a factor eases between 0 and 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    #[default]
    Linear,
    /// Slow at both ends.
    Smooth,
    /// Slow at the start.
    In,
    /// Slow at the end.
    Out,
}

pub const EASES: &[&str] = &["linear", "smooth", "in", "out"];

impl Ease {
    pub fn name(self) -> &'static str {
        match self {
            Ease::Linear => "linear",
            Ease::Smooth => "smooth",
            Ease::In => "in",
            Ease::Out => "out",
        }
    }

    pub fn from_name(n: &str) -> Option<Ease> {
        match n {
            "linear" => Some(Ease::Linear),
            "smooth" => Some(Ease::Smooth),
            "in" => Some(Ease::In),
            "out" => Some(Ease::Out),
            _ => None,
        }
    }

    /// Map `t` (clamped to 0..=1) through the easing; 0 stays 0 and 1 stays 1.
    pub fn apply(self, t: f32) -> f32 {
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        match self {
            Ease::Linear => t,
            Ease::Smooth => t * t * (3.0 - 2.0 * t),
            Ease::In => t * t,
            Ease::Out => t * (2.0 - t),
        }
    }
}

fn yes() -> bool {
    true
}
fn one() -> f32 {
    1.0
}
fn default_frequency() -> f32 {
    1.0
}
fn default_octaves() -> u32 {
    2
}
fn default_steps() -> u32 {
    0
}
fn default_stagger() -> f32 {
    0.05
}
fn default_duration() -> f32 {
    0.5
}
fn default_pull() -> f32 {
    0.5
}

/// What a modifier does to the factor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Mode {
    /// The same change for every clone.
    Plain,
    /// A seeded random factor per clone. `signed` spreads it over -1 to 1, otherwise 0 to 1.
    Random {
        #[serde(default)]
        seed: u32,
        #[serde(default = "yes")]
        signed: bool,
    },
    /// Smooth noise of the clone's place. `frequency` is features per unit, `speed` how far the
    /// pattern drifts per second (the only time dependence).
    Noise {
        #[serde(default)]
        seed: u32,
        #[serde(default = "default_frequency")]
        frequency: f32,
        #[serde(default = "default_octaves")]
        octaves: u32,
        #[serde(default)]
        speed: Vec3,
        #[serde(default = "yes")]
        signed: bool,
    },
    /// A ramp from 0 at the first clone to 1 at the last; `steps` of 2 or more cuts it into that
    /// many levels.
    Step {
        #[serde(default = "default_steps")]
        steps: u32,
        #[serde(default)]
        ease: Ease,
        #[serde(default)]
        reverse: bool,
    },
    /// Each clone eases from 0 to 1 over `duration` seconds, starting `start + order * stagger`
    /// seconds into the animation. `reverse` starts from the last clone.
    Delay {
        #[serde(default)]
        start: f32,
        #[serde(default = "default_stagger")]
        stagger: f32,
        #[serde(default = "default_duration")]
        duration: f32,
        #[serde(default)]
        ease: Ease,
        #[serde(default)]
        reverse: bool,
    },
    /// Strongest at a target point and fading to nothing at `reach` away (everywhere when
    /// `reach` is 0). `pull` moves clones toward the point, `aim` turns their `aim_axis` to it.
    Target {
        #[serde(default = "default_pull")]
        pull: f32,
        #[serde(default = "yes")]
        aim: bool,
        #[serde(default)]
        aim_axis: Axis,
        #[serde(default)]
        reach: f32,
    },
    /// The factor is a formula over the clone: `i` (its index), `n` (the count), `t` (`i / (n -
    /// 1)`, 0 to 1), `x`, `y`, `z` (where it sits in the generator) and `time` (seconds). The
    /// value is kept within -10 to 10; a clone where it gives no number gets no change.
    Formula {
        #[serde(default = "default_formula")]
        expr: String,
    },
}

pub const MODES: &[&str] = &["plain", "random", "noise", "step", "delay", "target", "formula"];

impl Mode {
    pub fn default_of(kind: &str) -> Option<Mode> {
        Some(match kind {
            "plain" => Mode::Plain,
            "random" => Mode::Random { seed: 1, signed: true },
            "noise" => Mode::Noise { seed: 1, frequency: 1.0, octaves: 2, speed: Vec3::ZERO, signed: true },
            "step" => Mode::Step { steps: 0, ease: Ease::Linear, reverse: false },
            "delay" => Mode::Delay { start: 0.0, stagger: 0.05, duration: 0.5, ease: Ease::Smooth, reverse: false },
            "target" => Mode::Target { pull: 0.5, aim: true, aim_axis: Axis::Z, reach: 0.0 },
            "formula" => Mode::Formula { expr: default_formula() },
            _ => return None,
        })
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Mode::Plain => "plain",
            Mode::Random { .. } => "random",
            Mode::Noise { .. } => "noise",
            Mode::Step { .. } => "step",
            Mode::Delay { .. } => "delay",
            Mode::Target { .. } => "target",
            Mode::Formula { .. } => "formula",
        }
    }
}

/// One clone modifier.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Modifier {
    #[serde(flatten)]
    pub mode: Mode,
    /// Scales the whole change; negative reverses it.
    #[serde(default = "one")]
    pub strength: f32,
    /// How far to move a clone, in the generator's units.
    #[serde(default)]
    pub position: Vec3,
    /// How far to turn a clone about its own axes, in degrees.
    #[serde(default)]
    pub rotation: Vec3,
    /// Added to the clone's scale (0.5 grows it by half).
    #[serde(default)]
    pub scale: Vec3,
    /// Use the X value of `scale` for all three axes.
    #[serde(default)]
    pub uniform_scale: bool,
    /// The colour clones move toward.
    #[serde(default)]
    pub tint: [f32; 3],
    /// How far toward `tint` (0 to 1) a clone with factor 1 goes.
    #[serde(default)]
    pub tint_mix: f32,
}

/// What a modifier needs to know about the world when it runs.
pub struct Context<'a> {
    /// The generator's seed, mixed into the random modes.
    pub seed: u32,
    /// Animation time in seconds.
    pub time: f32,
    /// The target point in the generator's space (for `target`).
    pub target: Option<Vec3>,
    /// The falloff weight, 0 to 1, at a point in the generator's space.
    pub weight: &'a dyn Fn(Vec3) -> f32,
}

fn finite_or(v: f32, d: f32) -> f32 {
    if v.is_finite() { v } else { d }
}

fn clamp_vec(v: Vec3, lim: f32) -> Vec3 {
    Vec3::new(finite_or(v.x, 0.0).clamp(-lim, lim), finite_or(v.y, 0.0).clamp(-lim, lim), finite_or(v.z, 0.0).clamp(-lim, lim))
}

impl Modifier {
    pub fn default_of(kind: &str) -> Option<Modifier> {
        Mode::default_of(kind).map(|mode| Modifier {
            mode,
            strength: 1.0,
            position: Vec3::new(0.0, 0.5, 0.0),
            rotation: Vec3::ZERO,
            scale: Vec3::ZERO,
            uniform_scale: true,
            tint: [1.0, 0.4, 0.2],
            tint_mix: 0.0,
        })
    }

    pub fn kind(&self) -> &'static str {
        self.mode.kind()
    }

    /// True when the result depends on the animation time.
    pub fn is_time_dependent(&self) -> bool {
        match &self.mode {
            Mode::Delay { .. } => true,
            Mode::Noise { speed, .. } => *speed != Vec3::ZERO,
            // A text scan, not a parse: this runs on every cache-key check and every paint.
            Mode::Formula { expr } => modelcraft_anim::expr::mentions(expr, "time"),
            _ => false,
        }
    }

    /// Why the modifier would change nothing over `clones` clones: a formula that does not parse,
    /// reads a name the mode does not know, or needs more than [`MAX_FORMULA_WORK`] steps.
    pub fn problem(&self, clones: usize) -> Option<String> {
        let Mode::Formula { expr } = &self.mode else { return None };
        formula_of(expr, clones).err()
    }

    pub fn needs_target(&self) -> bool {
        matches!(self.mode, Mode::Target { .. })
    }

    /// Every number brought into its range; non-finite numbers become the default.
    pub fn sanitized(self) -> Modifier {
        let mode = match self.mode {
            Mode::Formula { expr } => Mode::Formula { expr: expr.chars().take(modelcraft_anim::expr::MAX_SOURCE).collect() },
            Mode::Plain => Mode::Plain,
            Mode::Random { seed, signed } => Mode::Random { seed, signed },
            Mode::Noise { seed, frequency, octaves, speed, signed } => Mode::Noise {
                seed,
                frequency: finite_or(frequency, 1.0).clamp(0.0, 1000.0),
                octaves: octaves.clamp(1, MAX_NOISE_OCTAVES),
                speed: clamp_vec(speed, MAX_LEN),
                signed,
            },
            Mode::Step { steps, ease, reverse } => Mode::Step { steps: steps.min(MAX_STEPS), ease, reverse },
            Mode::Delay { start, stagger, duration, ease, reverse } => Mode::Delay {
                start: finite_or(start, 0.0).clamp(-MAX_SECONDS, MAX_SECONDS),
                stagger: finite_or(stagger, 0.0).clamp(0.0, MAX_SECONDS),
                duration: finite_or(duration, 0.0).clamp(0.0, MAX_SECONDS),
                ease,
                reverse,
            },
            Mode::Target { pull, aim, aim_axis, reach } => {
                Mode::Target { pull: finite_or(pull, 0.0).clamp(0.0, 1.0), aim, aim_axis, reach: finite_or(reach, 0.0).clamp(0.0, MAX_LEN) }
            }
        };
        Modifier {
            mode,
            strength: finite_or(self.strength, 1.0).clamp(-10.0, 10.0),
            position: clamp_vec(self.position, MAX_LEN),
            rotation: clamp_vec(self.rotation, MAX_DEG),
            scale: clamp_vec(self.scale, 100.0),
            uniform_scale: self.uniform_scale,
            tint: self.tint.map(|c| finite_or(c, 0.0).clamp(0.0, 1.0)),
            tint_mix: finite_or(self.tint_mix, 0.0).clamp(0.0, 1.0),
        }
    }

    /// Change every clone of `set`. The count, the order and the indices stay as they are.
    pub fn apply(&self, set: &mut CloneSet, ctx: &Context) {
        let m = self.clone().sanitized();
        let n = set.instances.len();
        let formula = match &m.mode {
            Mode::Formula { expr } => match formula_of(expr, n) {
                Ok(e) => Some(e),
                Err(_) => return,
            },
            _ => None,
        };
        let target = ctx.target.filter(|t| t.is_finite());
        let time = if ctx.time.is_finite() { ctx.time.clamp(-1.0e7, 1.0e7) } else { 0.0 };
        if m.needs_target() && target.is_none() {
            return;
        }
        for (i, inst) in set.instances.iter_mut().enumerate() {
            let (s, q, t) = inst.matrix.to_scale_rotation_translation();
            if !(s.is_finite() && q.is_finite() && t.is_finite()) {
                continue;
            }
            let w = finite_or((ctx.weight)(t), 0.0).clamp(0.0, 1.0);
            let k = m.strength * w;
            if k == 0.0 {
                continue;
            }
            let f = m.factors(i, n, t, ctx.seed, time, target, formula.as_ref());
            let amount = |v: f32| v * k;
            let mut pos = t + m.position * f.position * amount(1.0);
            let turn = m.rotation * f.rotation * amount(1.0);
            let mut rot = q * Quat::from_euler(EulerRot::XYZ, turn.x.to_radians(), turn.y.to_radians(), turn.z.to_radians());
            let grow = if m.uniform_scale { Vec3::splat(m.scale.x) * f.scale } else { m.scale * f.scale };
            let size = (s * (Vec3::ONE + grow * amount(1.0)).max(Vec3::splat(MIN_SCALE))).max(Vec3::splat(MIN_SCALE));
            if let (Mode::Target { pull, aim, aim_axis, .. }, Some(goal)) = (&m.mode, target) {
                let (pull, aim, aim_axis) = (*pull, *aim, *aim_axis);
                let near = k * f.target;
                pos = pos.lerp(goal, (pull * near).clamp(0.0, 1.0));
                if aim {
                    let dir = goal - pos;
                    if dir.length_squared() > 1e-12 {
                        let from = rot * aim_axis.vector();
                        let wanted = Quat::from_rotation_arc(from.normalize_or_zero(), dir.normalize()) * rot;
                        rot = rot.slerp(wanted, near.clamp(0.0, 1.0));
                    }
                }
            }
            let out = Mat4::from_scale_rotation_translation(size, rot.normalize(), pos);
            if out.is_finite() {
                inst.matrix = out;
            }
            if m.tint_mix > 0.0 {
                let a = (m.tint_mix * f.color * k).clamp(0.0, 1.0);
                for c in 0..3 {
                    inst.color[c] += (m.tint[c] - inst.color[c]) * a;
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn factors(&self, i: usize, n: usize, p: Vec3, ctx_seed: u32, time: f32, target: Option<Vec3>, formula: Option<&Expr>) -> Factors {
        let i32_ = i as u32;
        match self.mode {
            Mode::Formula { .. } => {
                let Some(e) = formula else { return Factors::all(0.0) };
                let t = if n <= 1 { 1.0 } else { i as f64 / (n - 1) as f64 };
                let v = e.eval(&|name| match name {
                    "i" => Some(i as f64),
                    "n" => Some(n as f64),
                    "t" => Some(t),
                    "x" => Some(f64::from(p.x)),
                    "y" => Some(f64::from(p.y)),
                    "z" => Some(f64::from(p.z)),
                    "time" => Some(f64::from(time)),
                    _ => None,
                });
                Factors::all(v.map_or(0.0, |v| v.clamp(-MAX_FORMULA_FACTOR, MAX_FORMULA_FACTOR) as f32))
            }
            Mode::Plain => Factors::all(1.0),
            Mode::Random { seed, signed } => {
                let s = seed ^ ctx_seed.rotate_left(16);
                let r = |ch: u32| if signed { hash01(s, i32_, ch) * 2.0 - 1.0 } else { hash01(s, i32_, ch) };
                Factors {
                    position: Vec3::new(r(0), r(1), r(2)),
                    rotation: Vec3::new(r(3), r(4), r(5)),
                    scale: if self.uniform_scale { Vec3::splat(r(6)) } else { Vec3::new(r(6), r(7), r(8)) },
                    color: r(9),
                    target: 1.0,
                }
            }
            Mode::Noise { seed, frequency, octaves, speed, signed } => {
                let s = seed ^ ctx_seed.rotate_left(16);
                let at = p * frequency + speed * time;
                let g = |ch: u32| {
                    let v = fractal(at + channel_offset(ch), s, octaves);
                    if signed { v } else { 0.5 + 0.5 * v }
                };
                Factors {
                    position: if self.position == Vec3::ZERO { Vec3::ZERO } else { Vec3::new(g(0), g(1), g(2)) },
                    rotation: if self.rotation == Vec3::ZERO { Vec3::ZERO } else { Vec3::new(g(3), g(4), g(5)) },
                    scale: if self.scale == Vec3::ZERO {
                        Vec3::ZERO
                    } else if self.uniform_scale {
                        Vec3::splat(g(6))
                    } else {
                        Vec3::new(g(6), g(7), g(8))
                    },
                    color: if self.tint_mix > 0.0 { g(9) } else { 0.0 },
                    target: 1.0,
                }
            }
            Mode::Step { steps, ease, reverse } => {
                let mut t = if n <= 1 { 1.0 } else { i as f32 / (n - 1) as f32 };
                if reverse {
                    t = 1.0 - t;
                }
                if steps >= 2 {
                    let level = (t * steps as f32).floor().min(steps as f32 - 1.0);
                    t = level / (steps as f32 - 1.0);
                }
                Factors::all(ease.apply(t))
            }
            Mode::Delay { start, stagger, duration, ease, reverse } => {
                let order = if reverse { n.saturating_sub(1 + i) } else { i } as f32;
                let begin = start + order * stagger;
                let t = if duration <= 1e-6 { f32::from(time >= begin) } else { (time - begin) / duration };
                Factors::all(ease.apply(t))
            }
            Mode::Target { reach, .. } => {
                let near = match target {
                    Some(goal) if reach > 0.0 => 1.0 - ((goal - p).length() / reach).clamp(0.0, 1.0),
                    _ => 1.0,
                };
                Factors { target: near, ..Factors::all(near) }
            }
        }
    }
}

/// Parse a formula and check it may run over `clones` clones.
fn formula_of(expr: &str, clones: usize) -> Result<Expr, String> {
    let e = Expr::parse(expr).map_err(|e| format!("its formula does not work: {e}"))?;
    if let Some(v) = e.variables().iter().find(|v| !FORMULA_NAMES.contains(&v.as_str())) {
        return Err(format!("its formula reads {v:?}, which a clone modifier does not know"));
    }
    let work = clones.saturating_mul(e.cost());
    if work > MAX_FORMULA_WORK {
        return Err(format!("its formula needs {work} steps over {clones} clones, more than the {MAX_FORMULA_WORK} allowed"));
    }
    Ok(e)
}

/// The factor each channel gets for one clone.
struct Factors {
    position: Vec3,
    rotation: Vec3,
    scale: Vec3,
    color: f32,
    /// How near the target the clone is (the `target` mode).
    target: f32,
}

impl Factors {
    fn all(v: f32) -> Factors {
        Factors { position: Vec3::splat(v), rotation: Vec3::splat(v), scale: Vec3::splat(v), color: v, target: v }
    }
}

/// A well-mixed 64-bit value from three small numbers (SplitMix64's finalizer).
fn mix(seed: u32, a: u32, b: u32) -> u64 {
    let mut z = (u64::from(seed) << 32 | u64::from(a)).wrapping_add(u64::from(b).wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A number in 0..1 for (seed, clone, channel): the same clone keeps its number when the count
/// changes.
fn hash01(seed: u32, i: u32, ch: u32) -> f32 {
    (mix(seed, i, ch) >> 40) as f32 / (1u64 << 24) as f32
}

fn channel_offset(ch: u32) -> Vec3 {
    let k = ch as f32 * 17.37;
    Vec3::new(k, k * 0.61 + 3.1, k * 1.37 + 7.7)
}

/// Value noise in -1..1: random values on the integer lattice, blended smoothly.
fn value_noise(p: Vec3, seed: u32) -> f32 {
    let base = p.floor();
    let f = p - base;
    let u = f * f * (Vec3::splat(3.0) - 2.0 * f);
    // Lattice coordinates wrap in a bounded range, so a huge position cannot overflow.
    let lat = |v: f32| (v.rem_euclid(65_536.0)) as u32;
    let (x0, y0, z0) = (lat(base.x), lat(base.y), lat(base.z));
    let corner = |dx: u32, dy: u32, dz: u32| {
        let h = mix(seed, x0.wrapping_add(dx) & 0xffff | (y0.wrapping_add(dy) & 0xffff) << 16, z0.wrapping_add(dz) & 0xffff);
        (h >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    };
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let x00 = lerp(corner(0, 0, 0), corner(1, 0, 0), u.x);
    let x10 = lerp(corner(0, 1, 0), corner(1, 1, 0), u.x);
    let x01 = lerp(corner(0, 0, 1), corner(1, 0, 1), u.x);
    let x11 = lerp(corner(0, 1, 1), corner(1, 1, 1), u.x);
    lerp(lerp(x00, x10, u.y), lerp(x01, x11, u.y), u.z).clamp(-1.0, 1.0)
}

/// Several octaves of value noise, scaled back to -1..1.
fn fractal(p: Vec3, seed: u32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut total, mut q) = (0.0, 1.0, 0.0, p);
    for o in 0..octaves.clamp(1, MAX_NOISE_OCTAVES) {
        sum += amp * value_noise(q, seed.wrapping_add(o));
        total += amp;
        amp *= 0.5;
        q *= 2.0;
    }
    let v = sum / total;
    if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 }
}

#[cfg(test)]
mod tests;
