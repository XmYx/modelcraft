//! Keyed curves: a value as a function of time (in frames).
//!
//! A [`Curve`] is a list of [`Key`]s sorted by time. Each key says how the value travels to the
//! next key ([`Interp`]: stepped, linear or a cubic Bézier segment) and how its tangents are made
//! ([`Tangent`]). A Bézier segment between keys `a` and `b` that are `dt` apart has the control
//! points
//!
//! ```text
//! (a.t, a.v)   (a.t + a.out_weight·dt, a.v + a.out_slope·a.out_weight·dt)
//! (b.t - b.in_weight·dt, b.v - b.in_slope·b.in_weight·dt)   (b.t, b.v)
//! ```
//!
//! Weights are clamped to `0..=1`, so time always runs forward along a segment and the curve is a
//! function. With both weights at ⅓ the segment is the cubic Hermite spline of the two slopes.
//!
//! [`Tangent::Auto`] slopes follow the monotone rule of Fritsch and Carlson ("Monotone Piecewise
//! Cubic Interpolation", SIAM J. Numer. Anal. 17, 1980): the slope through the neighbours, zero at
//! a peak or valley, and never more than three times the slope of either adjoining chord. A curve
//! with automatic tangents and default weights therefore never overshoots its keys.

use serde::{Deserialize, Serialize};

/// Most keys one curve holds (hostile files, runaway bakes).
pub const MAX_KEYS: usize = 1 << 20;
/// Two keys closer than this (in frames) are the same key.
pub const TIME_EPSILON: f64 = 1e-6;
/// The default handle length, as a share of the segment.
pub const DEFAULT_WEIGHT: f64 = 1.0 / 3.0;

/// How the value travels from a key to the next one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interp {
    /// Hold the key's value until the next key.
    Stepped,
    /// A straight line to the next key.
    Linear,
    /// A cubic Bézier segment shaped by the two keys' tangents.
    #[default]
    Bezier,
}

impl Interp {
    pub const ALL: [Interp; 3] = [Interp::Stepped, Interp::Linear, Interp::Bezier];
    pub fn id(self) -> &'static str {
        match self {
            Interp::Stepped => "stepped",
            Interp::Linear => "linear",
            Interp::Bezier => "bezier",
        }
    }
}

/// How a key's slopes are made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tangent {
    /// Smooth and clamped: never overshoots the neighbouring keys.
    #[default]
    Auto,
    /// Horizontal.
    Flat,
    /// One slope chosen by the user for both sides.
    Free,
    /// Separate slopes for the incoming and outgoing side.
    Broken,
}

impl Tangent {
    pub const ALL: [Tangent; 4] = [Tangent::Auto, Tangent::Flat, Tangent::Free, Tangent::Broken];
    pub fn id(self) -> &'static str {
        match self {
            Tangent::Auto => "auto",
            Tangent::Flat => "flat",
            Tangent::Free => "free",
            Tangent::Broken => "broken",
        }
    }
}

/// What the curve does before its first key or after its last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Extrapolation {
    /// Keep the end key's value.
    #[default]
    Hold,
    /// Continue along the end key's slope.
    Linear,
    /// Repeat the keyed span.
    Cycle,
    /// Repeat the keyed span, each time starting where the last one ended.
    CycleOffset,
    /// Play the keyed span forwards, then backwards.
    PingPong,
}

impl Extrapolation {
    pub const ALL: [Extrapolation; 5] = [Extrapolation::Hold, Extrapolation::Linear, Extrapolation::Cycle, Extrapolation::CycleOffset, Extrapolation::PingPong];
    pub fn id(self) -> &'static str {
        match self {
            Extrapolation::Hold => "hold",
            Extrapolation::Linear => "linear",
            Extrapolation::Cycle => "cycle",
            Extrapolation::CycleOffset => "cycle_offset",
            Extrapolation::PingPong => "ping_pong",
        }
    }
}

fn third() -> f64 {
    DEFAULT_WEIGHT
}
fn is_third(w: &f64) -> bool {
    (*w - DEFAULT_WEIGHT).abs() < 1e-12
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// One key: a value at a time, the interpolation towards the next key and its tangents.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    /// Time in frames.
    pub t: f64,
    pub v: f64,
    #[serde(default, skip_serializing_if = "is_default")]
    pub interp: Interp,
    #[serde(default, skip_serializing_if = "is_default")]
    pub tangent: Tangent,
    /// Slope (value per frame) arriving at the key. Kept up to date for `Auto` and `Flat`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub in_slope: f64,
    /// Slope leaving the key.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub out_slope: f64,
    /// Length of the incoming handle as a share (0..=1) of the segment before the key.
    #[serde(default = "third", skip_serializing_if = "is_third")]
    pub in_weight: f64,
    /// Length of the outgoing handle as a share (0..=1) of the segment after the key.
    #[serde(default = "third", skip_serializing_if = "is_third")]
    pub out_weight: f64,
}

impl Key {
    pub fn new(t: f64, v: f64) -> Key {
        Key { t, v, interp: Interp::Bezier, tangent: Tangent::Auto, in_slope: 0.0, out_slope: 0.0, in_weight: DEFAULT_WEIGHT, out_weight: DEFAULT_WEIGHT }
    }
    pub fn with_interp(mut self, interp: Interp) -> Key {
        self.interp = interp;
        self
    }
    fn is_finite(&self) -> bool {
        self.t.is_finite() && self.v.is_finite()
    }
    fn sanitize(&mut self) {
        if !self.in_slope.is_finite() {
            self.in_slope = 0.0;
        }
        if !self.out_slope.is_finite() {
            self.out_slope = 0.0;
        }
        self.in_weight = if self.in_weight.is_finite() { self.in_weight.clamp(0.0, 1.0) } else { DEFAULT_WEIGHT };
        self.out_weight = if self.out_weight.is_finite() { self.out_weight.clamp(0.0, 1.0) } else { DEFAULT_WEIGHT };
    }
}

/// A value over time. Keys stay sorted by time with distinct times; build and change a curve
/// through its methods, or call [`Curve::sanitize`] after filling `keys` by hand.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    #[serde(default)]
    pub keys: Vec<Key>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub pre: Extrapolation,
    #[serde(default, skip_serializing_if = "is_default")]
    pub post: Extrapolation,
}

impl Curve {
    pub fn new() -> Curve {
        Curve::default()
    }

    /// A curve through `(time, value)` pairs with automatic tangents.
    pub fn from_points(points: &[(f64, f64)]) -> Curve {
        let mut c = Curve { keys: points.iter().map(|(t, v)| Key::new(*t, *v)).collect(), ..Curve::default() };
        c.sanitize();
        c
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub fn heap_bytes(&self) -> usize {
        self.keys.len() * std::mem::size_of::<Key>()
    }

    /// Repair a curve read from a file or filled by hand: drop non-finite keys, sort, merge keys
    /// at the same time (the later one wins), cap the count, clamp weights, rebuild tangents.
    pub fn sanitize(&mut self) {
        self.keys.retain(Key::is_finite);
        self.keys.sort_by(|a, b| a.t.total_cmp(&b.t));
        let mut out: Vec<Key> = Vec::with_capacity(self.keys.len().min(MAX_KEYS));
        for k in self.keys.drain(..) {
            let same = out.last().is_some_and(|last| (k.t - last.t).abs() < TIME_EPSILON);
            if same {
                if let Some(last) = out.last_mut() {
                    *last = Key { t: last.t, ..k };
                }
            } else if out.len() < MAX_KEYS {
                out.push(k);
            }
        }
        self.keys = out;
        for k in &mut self.keys {
            k.sanitize();
        }
        self.retangent();
    }

    /// Recompute the slopes of `Auto` and `Flat` keys and unify `Free` ones.
    pub fn retangent(&mut self) {
        let n = self.keys.len();
        for i in 0..n {
            let prev = i.checked_sub(1).and_then(|p| self.keys.get(p)).map(|k| (k.t, k.v));
            let next = self.keys.get(i + 1).map(|k| (k.t, k.v));
            let Some(k) = self.keys.get_mut(i) else { continue };
            match k.tangent {
                Tangent::Auto => {
                    let s = auto_slope(prev, (k.t, k.v), next);
                    k.in_slope = s;
                    k.out_slope = s;
                }
                Tangent::Flat => {
                    k.in_slope = 0.0;
                    k.out_slope = 0.0;
                }
                Tangent::Free => k.in_slope = k.out_slope,
                Tangent::Broken => {}
            }
        }
    }

    /// Index of the key at `t` (within [`TIME_EPSILON`]).
    pub fn key_at(&self, t: f64) -> Option<usize> {
        let i = self.keys.partition_point(|k| k.t < t - TIME_EPSILON);
        self.keys.get(i).filter(|k| (k.t - t).abs() < TIME_EPSILON).map(|_| i)
    }

    /// Set the value at `t`: replaces the key there or inserts a new one (which takes the
    /// interpolation of the key before it). Returns the key's index, or `None` when `t` or `v` is
    /// not finite or the curve is full.
    pub fn set_key(&mut self, t: f64, v: f64) -> Option<usize> {
        if !t.is_finite() || !v.is_finite() {
            return None;
        }
        let i = match self.key_at(t) {
            Some(i) => {
                if let Some(k) = self.keys.get_mut(i) {
                    k.v = v;
                }
                i
            }
            None => {
                if self.keys.len() >= MAX_KEYS {
                    return None;
                }
                let i = self.keys.partition_point(|k| k.t < t);
                let interp = i.checked_sub(1).and_then(|p| self.keys.get(p)).map_or(Interp::Bezier, |k| k.interp);
                self.keys.insert(i, Key::new(t, v).with_interp(interp));
                i
            }
        };
        self.retangent();
        Some(i)
    }

    /// Insert a fully specified key (replacing one at the same time).
    pub fn insert(&mut self, key: Key) -> Option<usize> {
        if !key.is_finite() {
            return None;
        }
        let mut key = key;
        key.sanitize();
        let i = match self.key_at(key.t) {
            Some(i) => {
                if let Some(k) = self.keys.get_mut(i) {
                    *k = Key { t: k.t, ..key };
                }
                i
            }
            None => {
                if self.keys.len() >= MAX_KEYS {
                    return None;
                }
                let i = self.keys.partition_point(|k| k.t < key.t);
                self.keys.insert(i, key);
                i
            }
        };
        self.retangent();
        Some(i)
    }

    pub fn remove(&mut self, index: usize) -> Option<Key> {
        if index >= self.keys.len() {
            return None;
        }
        let k = self.keys.remove(index);
        self.retangent();
        Some(k)
    }

    /// Move the keys at `indices` by `dt` frames and `dv` in value. A moved key that lands on a
    /// key that did not move replaces it. Returns the moved keys' new indices, in the order given.
    pub fn move_keys(&mut self, indices: &[usize], dt: f64, dv: f64) -> Vec<usize> {
        self.map_keys(indices, |k| {
            k.t += dt;
            k.v += dv;
        })
    }

    /// Scale the keys at `indices` around a pivot: `t' = pivot_t + (t - pivot_t)·scale_t`, the
    /// same for values. Slopes follow, so the shape is kept.
    pub fn scale_keys(&mut self, indices: &[usize], pivot_t: f64, scale_t: f64, pivot_v: f64, scale_v: f64) -> Vec<usize> {
        if !scale_t.is_finite() || !scale_v.is_finite() || scale_t.abs() < 1e-9 {
            return indices.iter().copied().filter(|i| *i < self.keys.len()).collect();
        }
        let flip = scale_t < 0.0;
        self.map_keys(indices, |k| {
            k.t = pivot_t + (k.t - pivot_t) * scale_t;
            k.v = pivot_v + (k.v - pivot_v) * scale_v;
            let f = scale_v / scale_t;
            (k.in_slope, k.out_slope) = (k.in_slope * f, k.out_slope * f);
            if flip {
                std::mem::swap(&mut k.in_slope, &mut k.out_slope);
                std::mem::swap(&mut k.in_weight, &mut k.out_weight);
            }
        })
    }

    fn map_keys(&mut self, indices: &[usize], f: impl Fn(&mut Key)) -> Vec<usize> {
        let mut picked: Vec<usize> = indices.iter().copied().filter(|i| *i < self.keys.len()).collect();
        picked.sort_unstable();
        picked.dedup();
        let mut moved: Vec<Key> = Vec::with_capacity(picked.len());
        for i in picked.iter().rev() {
            let mut k = self.keys.remove(*i);
            let before = k;
            f(&mut k);
            moved.push(if k.is_finite() { k } else { before });
        }
        // Moved keys win over the keys they land on.
        let mut times: Vec<f64> = Vec::with_capacity(moved.len());
        for k in moved {
            if let Some(i) = self.key_at(k.t) {
                self.keys.remove(i);
            }
            let i = self.keys.partition_point(|o| o.t < k.t);
            self.keys.insert(i, k);
            times.push(k.t);
        }
        self.retangent();
        times.iter().filter_map(|t| self.key_at(*t)).collect()
    }

    /// The time of the last key before `t`, if any.
    pub fn prev_key_time(&self, t: f64) -> Option<f64> {
        let i = self.keys.partition_point(|k| k.t < t - TIME_EPSILON);
        i.checked_sub(1).and_then(|p| self.keys.get(p)).map(|k| k.t)
    }
    /// The time of the first key after `t`, if any.
    pub fn next_key_time(&self, t: f64) -> Option<f64> {
        let i = self.keys.partition_point(|k| k.t <= t + TIME_EPSILON);
        self.keys.get(i).map(|k| k.t)
    }

    /// First and last key time.
    pub fn time_range(&self) -> Option<(f64, f64)> {
        Some((self.keys.first()?.t, self.keys.last()?.t))
    }
    /// Lowest and highest key value.
    pub fn value_range(&self) -> Option<(f64, f64)> {
        let first = self.keys.first()?.v;
        Some(self.keys.iter().fold((first, first), |(lo, hi), k| (lo.min(k.v), hi.max(k.v))))
    }

    /// The value at time `t`; `None` for a curve without keys.
    pub fn eval(&self, t: f64) -> Option<f64> {
        let (first, last) = (self.keys.first()?, self.keys.last()?);
        if !t.is_finite() {
            return Some(first.v);
        }
        if t < first.t {
            return Some(self.outside(t, self.pre, first, last, true));
        }
        if t > last.t {
            return Some(self.outside(t, self.post, first, last, false));
        }
        Some(self.inside(t))
    }

    /// The value at `t`, or `fallback` when the curve has no keys.
    pub fn eval_or(&self, t: f64, fallback: f64) -> f64 {
        self.eval(t).unwrap_or(fallback)
    }

    fn inside(&self, t: f64) -> f64 {
        // The segment [a, b] with a.t <= t <= b.t.
        let i = self.keys.partition_point(|k| k.t <= t);
        let (Some(a), b) = (i.checked_sub(1).and_then(|p| self.keys.get(p)), self.keys.get(i)) else {
            return self.keys.first().map_or(0.0, |k| k.v);
        };
        let Some(b) = b else { return a.v };
        segment(a, b, t)
    }

    fn outside(&self, t: f64, mode: Extrapolation, first: &Key, last: &Key, before: bool) -> f64 {
        let end = if before { first } else { last };
        let span = last.t - first.t;
        match mode {
            Extrapolation::Hold => end.v,
            Extrapolation::Linear => {
                let slope = if before { first.in_slope } else { last.out_slope };
                end.v + slope * (t - end.t)
            }
            _ if span < TIME_EPSILON => end.v,
            Extrapolation::Cycle => self.inside(first.t + (t - first.t).rem_euclid(span)),
            Extrapolation::CycleOffset => {
                let n = ((t - first.t) / span).floor();
                self.inside(first.t + (t - first.t).rem_euclid(span)) + n * (last.v - first.v)
            }
            Extrapolation::PingPong => {
                let x = (t - first.t).rem_euclid(2.0 * span);
                self.inside(first.t + if x > span { 2.0 * span - x } else { x })
            }
        }
    }

    /// Replace the curve between `start` and `end` with one linear key every `step` frames that
    /// samples it (extrapolation included). Keys outside the span are kept.
    pub fn bake(&mut self, start: f64, end: f64, step: f64) {
        if !(start.is_finite() && end.is_finite() && step.is_finite()) || end < start || self.keys.is_empty() {
            return;
        }
        let step = step.max(1e-3);
        let count = (((end - start) / step).floor() as usize).min(MAX_KEYS - 1);
        let samples: Vec<Key> =
            (0..=count).map(|i| start + i as f64 * step).filter_map(|t| self.eval(t).map(|v| Key::new(t, v).with_interp(Interp::Linear))).collect();
        self.keys.retain(|k| k.t < start - TIME_EPSILON || k.t > end + TIME_EPSILON);
        self.keys.extend(samples);
        self.sanitize();
    }

    /// Remove keys the curve does not need: a key goes when the curve without it stays within
    /// `tolerance` of the original at every whole frame of its two segments. End keys stay.
    /// Returns how many keys were removed.
    pub fn simplify(&mut self, tolerance: f64) -> usize {
        let tolerance = if tolerance.is_finite() { tolerance.max(0.0) } else { 0.0 };
        let original = self.clone();
        let mut removed = 0;
        let mut i = 1;
        while i + 1 < self.keys.len() {
            let (Some(a), Some(b)) = (self.keys.get(i - 1).map(|k| k.t), self.keys.get(i + 1).map(|k| k.t)) else { break };
            let mut trial = self.clone();
            trial.keys.remove(i);
            trial.retangent();
            let steps = ((b - a).ceil() as usize).clamp(2, 4096);
            let ok = (0..=steps).all(|s| {
                let t = a + (b - a) * s as f64 / steps as f64;
                (trial.eval_or(t, 0.0) - original.eval_or(t, 0.0)).abs() <= tolerance
            });
            if ok {
                *self = trial;
                removed += 1;
            } else {
                i += 1;
            }
        }
        removed
    }
}

/// The clamped automatic slope at `k` between its neighbours.
fn auto_slope(prev: Option<(f64, f64)>, k: (f64, f64), next: Option<(f64, f64)>) -> f64 {
    let (Some(p), Some(n)) = (prev, next) else { return 0.0 };
    let (dp, dn) = (k.0 - p.0, n.0 - k.0);
    if dp < TIME_EPSILON || dn < TIME_EPSILON {
        return 0.0;
    }
    let (sp, sn) = ((k.1 - p.1) / dp, (n.1 - k.1) / dn);
    if sp * sn <= 0.0 {
        return 0.0;
    }
    let through = (n.1 - p.1) / (n.0 - p.0);
    let limit = 3.0 * sp.abs().min(sn.abs());
    through.clamp(-limit, limit)
}

/// The value on the segment from `a` to `b` at `t` (`a.t <= t <= b.t`).
fn segment(a: &Key, b: &Key, t: f64) -> f64 {
    let dt = b.t - a.t;
    if dt < TIME_EPSILON || t >= b.t {
        return if t >= b.t { b.v } else { a.v };
    }
    let s = ((t - a.t) / dt).clamp(0.0, 1.0);
    match a.interp {
        Interp::Stepped => a.v,
        Interp::Linear => a.v + (b.v - a.v) * s,
        Interp::Bezier => {
            let (wa, wb) = (a.out_weight.clamp(0.0, 1.0), b.in_weight.clamp(0.0, 1.0));
            let u = if is_third(&wa) && is_third(&wb) { s } else { solve_time(wa, 1.0 - wb, s) };
            let (p1, p2) = (a.v + a.out_slope * wa * dt, b.v - b.in_slope * wb * dt);
            bezier(a.v, p1, p2, b.v, u)
        }
    }
}

fn bezier(p0: f64, p1: f64, p2: f64, p3: f64, u: f64) -> f64 {
    let w = 1.0 - u;
    w * w * w * p0 + 3.0 * w * w * u * p1 + 3.0 * w * u * u * p2 + u * u * u * p3
}

/// Solve `bezier(0, x1, x2, 1, u) = s` for `u` in `0..=1`. With `0 <= x1, x2 <= 1` the left side
/// never decreases, so bisection always converges.
fn solve_time(x1: f64, x2: f64, s: f64) -> f64 {
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..48 {
        let mid = 0.5 * (lo + hi);
        if bezier(0.0, x1, x2, 1.0, mid) < s {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn stepped_linear_and_bezier_segments() {
        let mut c = Curve::new();
        c.set_key(0.0, 0.0);
        c.set_key(10.0, 10.0);
        c.set_key(20.0, 0.0);
        // Automatic tangents: flat at the ends and at the peak.
        assert_eq!(c.eval(10.0), Some(10.0));
        assert!((c.eval(5.0).unwrap() - 5.0).abs() < 1e-9, "a flat-flat Hermite segment is symmetric");
        assert!(c.eval(2.0).unwrap() < 2.0, "ease in");
        c.keys[0].interp = Interp::Linear;
        assert!((c.eval(2.0).unwrap() - 2.0).abs() < 1e-12);
        c.keys[0].interp = Interp::Stepped;
        assert_eq!(c.eval(9.999), Some(0.0));
        assert_eq!(c.eval(10.0), Some(10.0));
        // Outside the keys the value holds.
        assert_eq!(c.eval(-5.0), Some(0.0));
        assert_eq!(c.eval(99.0), Some(0.0));
        assert_eq!(Curve::new().eval(1.0), None);
    }

    #[test]
    fn auto_tangents_follow_the_neighbours_and_clamp() {
        let c = Curve::from_points(&[(0.0, 0.0), (10.0, 1.0), (20.0, 2.0)]);
        assert!((c.keys[1].out_slope - 0.1).abs() < 1e-12, "the slope through the neighbours");
        let c = Curve::from_points(&[(0.0, 0.0), (10.0, 0.1), (11.0, 10.0)]);
        assert!((c.keys[1].out_slope - 0.03).abs() < 1e-12, "three times the flatter chord");
        let c = Curve::from_points(&[(0.0, 0.0), (10.0, 5.0), (20.0, 1.0)]);
        assert_eq!(c.keys[1].out_slope, 0.0, "a peak is flat");
    }

    #[test]
    fn free_and_broken_tangents_and_weights() {
        let mut c = Curve::from_points(&[(0.0, 0.0), (10.0, 0.0)]);
        c.keys[0].tangent = Tangent::Free;
        c.keys[0].out_slope = 1.0;
        c.retangent();
        assert_eq!(c.keys[0].in_slope, 1.0);
        let bump = c.eval(3.0).unwrap();
        assert!(bump > 0.5, "the handle lifts the curve: {bump}");
        // A longer handle lifts it further, and the ends still hold.
        c.keys[0].out_weight = 0.9;
        assert!(c.eval(3.0).unwrap() > bump);
        assert!((c.eval(0.0).unwrap()).abs() < 1e-9);
        assert!((c.eval(10.0).unwrap()).abs() < 1e-9);
        c.keys[1].tangent = Tangent::Broken;
        c.keys[1].in_slope = -2.0;
        c.keys[1].out_slope = 5.0;
        c.retangent();
        assert_eq!((c.keys[1].in_slope, c.keys[1].out_slope), (-2.0, 5.0));
        c.post = Extrapolation::Linear;
        assert!((c.eval(12.0).unwrap() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn extrapolation_modes() {
        let mut c = Curve::from_points(&[(0.0, 0.0), (10.0, 10.0)]);
        c.keys[0].interp = Interp::Linear;
        c.post = Extrapolation::Cycle;
        assert!((c.eval(13.0).unwrap() - 3.0).abs() < 1e-9);
        c.post = Extrapolation::CycleOffset;
        assert!((c.eval(13.0).unwrap() - 13.0).abs() < 1e-9);
        assert!((c.eval(27.0).unwrap() - 27.0).abs() < 1e-9);
        c.post = Extrapolation::PingPong;
        assert!((c.eval(13.0).unwrap() - 7.0).abs() < 1e-9);
        c.pre = Extrapolation::CycleOffset;
        assert!((c.eval(-3.0).unwrap() + 3.0).abs() < 1e-9);
        c.pre = Extrapolation::PingPong;
        assert!((c.eval(-3.0).unwrap() - 3.0).abs() < 1e-9);
        // One key: every mode holds.
        let mut one = Curve::from_points(&[(4.0, 2.0)]);
        for m in Extrapolation::ALL {
            one.pre = m;
            one.post = m;
            assert_eq!((one.eval(-9.0), one.eval(99.0)), (Some(2.0), Some(2.0)));
        }
    }

    #[test]
    fn editing_keeps_keys_sorted_and_distinct() {
        let mut c = Curve::new();
        assert_eq!(c.set_key(10.0, 1.0), Some(0));
        assert_eq!(c.set_key(0.0, 2.0), Some(0));
        assert_eq!(c.set_key(10.0, 3.0), Some(1));
        assert_eq!(c.len(), 2);
        assert_eq!(c.set_key(f64::NAN, 3.0), None);
        // Move the first key past the second and onto a third.
        c.set_key(20.0, 5.0);
        let moved = c.move_keys(&[0], 20.0, 1.0);
        assert_eq!(moved, vec![1]);
        assert_eq!(c.keys.iter().map(|k| (k.t, k.v)).collect::<Vec<_>>(), vec![(10.0, 3.0), (20.0, 3.0)]);
        assert_eq!(c.prev_key_time(15.0), Some(10.0));
        assert_eq!(c.next_key_time(10.0), Some(20.0));
        assert_eq!(c.next_key_time(20.0), None);
        let scaled = c.scale_keys(&[0, 1], 10.0, 2.0, 0.0, 1.0);
        assert_eq!(scaled.len(), 2);
        assert_eq!(c.time_range(), Some((10.0, 30.0)));
        assert!(c.remove(7).is_none());
        assert!(c.remove(0).is_some());
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn bake_and_simplify() {
        let mut c = Curve::from_points(&[(0.0, 0.0), (10.0, 4.0), (20.0, -2.0)]);
        let original = c.clone();
        c.bake(0.0, 20.0, 1.0);
        assert_eq!(c.len(), 21);
        assert!(c.keys.iter().all(|k| (k.v - original.eval_or(k.t, 0.0)).abs() < 1e-9));
        let mut line = Curve::from_points(&[(0.0, 0.0), (5.0, 5.0), (10.0, 10.0), (20.0, 20.0)]);
        for k in &mut line.keys {
            k.interp = Interp::Linear;
        }
        assert_eq!(line.simplify(1e-6), 2);
        assert_eq!(line.len(), 2);
    }

    #[test]
    fn hostile_keys_are_repaired() {
        let mut c = Curve {
            keys: vec![
                Key::new(5.0, 1.0),
                Key::new(f64::NAN, 1.0),
                Key::new(1.0, f64::INFINITY),
                Key { in_weight: 9.0, out_weight: f64::NAN, out_slope: f64::NAN, ..Key::new(0.0, 3.0) },
                Key::new(5.0, 7.0),
            ],
            ..Curve::default()
        };
        c.sanitize();
        assert_eq!(c.keys.iter().map(|k| (k.t, k.v)).collect::<Vec<_>>(), vec![(0.0, 3.0), (5.0, 7.0)]);
        assert_eq!(c.keys[0].in_weight, 1.0);
        assert_eq!(c.keys[0].out_weight, DEFAULT_WEIGHT);
        assert!(c.eval(f64::NAN).unwrap().is_finite());
        assert!(c.eval(2.5).unwrap().is_finite());
    }

    #[test]
    fn json_is_compact_and_round_trips() {
        let mut c = Curve::from_points(&[(0.0, 1.0), (12.0, 3.0)]);
        c.keys[0].interp = Interp::Stepped;
        c.post = Extrapolation::Cycle;
        let j = serde_json::to_string(&c).unwrap();
        assert_eq!(j, r#"{"keys":[{"t":0.0,"v":1.0,"interp":"stepped"},{"t":12.0,"v":3.0}],"post":"cycle"}"#);
        assert_eq!(serde_json::from_str::<Curve>(&j).unwrap(), c);
    }

    fn arb_curve() -> impl Strategy<Value = Curve> {
        proptest::collection::vec((-200.0f64..200.0, -50.0f64..50.0, 0usize..3, 0usize..4, -5.0f64..5.0, 0.0f64..1.0, 0.0f64..1.0), 1..12).prop_map(|ks| {
            let mut c = Curve {
                keys: ks
                    .into_iter()
                    .map(|(t, v, i, tan, slope, wi, wo)| Key {
                        interp: Interp::ALL[i],
                        tangent: Tangent::ALL[tan],
                        in_slope: slope,
                        out_slope: -slope,
                        in_weight: wi,
                        out_weight: wo,
                        ..Key::new(t.round(), v)
                    })
                    .collect(),
                ..Curve::default()
            };
            c.sanitize();
            c
        })
    }

    proptest! {
        #[test]
        fn passes_through_every_key(c in arb_curve()) {
            for k in &c.keys {
                let v = c.eval(k.t).unwrap();
                prop_assert!((v - k.v).abs() < 1e-9, "at {} got {v}, key {}", k.t, k.v);
            }
        }

        #[test]
        fn automatic_tangents_never_overshoot(points in proptest::collection::vec((-100.0f64..100.0, -50.0f64..50.0), 2..10), x in 0.0f64..1.0) {
            let c = Curve::from_points(&points.iter().map(|(t, v)| (t.round(), *v)).collect::<Vec<_>>());
            for w in c.keys.windows(2) {
                let (a, b) = (w[0], w[1]);
                let v = c.eval(a.t + (b.t - a.t) * x).unwrap();
                let (lo, hi) = (a.v.min(b.v), a.v.max(b.v));
                prop_assert!(v >= lo - 1e-9 && v <= hi + 1e-9, "{v} outside {lo}..{hi}");
            }
        }

        #[test]
        fn evaluation_is_finite_and_continuous(c in arb_curve(), t in -400.0f64..400.0, pre in 0usize..5, post in 0usize..5) {
            let mut c = c;
            c.pre = Extrapolation::ALL[pre];
            c.post = Extrapolation::ALL[post];
            let v = c.eval(t).unwrap();
            prop_assert!(v.is_finite());
            // Weighted segments are continuous inside a segment (stepped ones jump only at keys).
            if let (Some(a), Some(b)) = (c.prev_key_time(t), c.next_key_time(t)) {
                let i = c.key_at(a).unwrap();
                if c.keys[i].interp != Interp::Stepped && b - t > 1e-3 && t - a > 1e-3 {
                    let near = c.eval(t + 1e-7).unwrap();
                    prop_assert!((near - v).abs() < 1e-2, "jump from {v} to {near}");
                }
            }
        }

        #[test]
        fn edits_keep_the_invariants(c in arb_curve(), ops in proptest::collection::vec((0usize..4, 0usize..12, -30.0f64..30.0, -5.0f64..5.0), 0..20)) {
            let mut c = c;
            for (op, i, a, b) in ops {
                match op {
                    0 => { c.set_key(a.round(), b); }
                    1 => { c.remove(i); }
                    2 => { c.move_keys(&[i, i / 2], a.round(), b); }
                    _ => { c.scale_keys(&[i, i + 1], 0.0, if b.abs() < 0.2 { 2.0 } else { b }, 0.0, a / 10.0); }
                }
                prop_assert!(c.keys.windows(2).all(|w| w[1].t - w[0].t >= TIME_EPSILON));
                prop_assert!(c.keys.iter().all(|k| k.t.is_finite() && k.v.is_finite() && k.in_slope.is_finite() && k.out_slope.is_finite()));
            }
        }
    }
}
