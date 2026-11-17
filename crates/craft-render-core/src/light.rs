//! Lights and how one is chosen for each shading point.
//!
//! - Emissive triangles, point and spot lights sit in a **light tree**: a binary hierarchy over
//!   their bounds and power that is descended at random, weighting each branch by its power over
//!   its squared distance (Conty Estevez and Kulla, "Importance Sampling of Many Lights with
//!   Adaptive Tree Splitting", 2018; this version has no orientation cones and no splitting).
//! - An image of the surroundings is sampled in proportion to its brightness times the solid
//!   angle of each pixel (the usual two-level table over a latitude–longitude image).
//! - Lights at infinity (directional, sun, environment) are chosen by power.

use std::f32::consts::PI;

use craft_texture::Image;
use glam::{Vec2, Vec3, Vec3A};

use crate::math::{Frame, Rgb, concentric_disk, luminance, uniform_sphere, uniform_triangle};

/// A light without a surface in the scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Light {
    /// Emits `intensity` (radiant intensity, W/sr) equally in every direction.
    Point { position: Vec3, intensity: Vec3 },
    /// A point light restricted to a cone around `direction`: full inside
    /// `cone_angle · (1 − blend)` from the axis, fading smoothly to nothing at `cone_angle`
    /// (radians, half-angle).
    Spot { position: Vec3, direction: Vec3, intensity: Vec3, cone_angle: f32, blend: f32 },
    /// Parallel light arriving from `direction` (pointing towards the light) with `irradiance`
    /// on a surface facing it.
    Directional { direction: Vec3, irradiance: Vec3 },
    /// A directional light with a disk of `angular_radius` radians, which softens its shadows.
    Sun { direction: Vec3, irradiance: Vec3, angular_radius: f32 },
}

impl Light {
    fn positional(&self) -> Option<Vec3> {
        match self {
            Light::Point { position, .. } | Light::Spot { position, .. } => Some(*position),
            _ => None,
        }
    }
}

/// One triangle that emits light.
#[derive(Clone, Copy, Debug)]
pub struct EmissiveTri {
    pub tri: u32,
    pub area: f32,
    pub radiance: Vec3,
    pub two_sided: bool,
}

/// A direction towards a light, drawn for next-event estimation.
#[derive(Clone, Copy, Debug)]
pub struct LightSample {
    /// Unit direction from the shaded point towards the light.
    pub wi: Vec3A,
    pub distance: f32,
    pub radiance: Rgb,
    /// Density over solid angle (lights with area) or the probability of choosing the light
    /// (`delta` lights), selection included.
    pub pdf: f32,
    pub delta: bool,
}

/// How lights with a position are chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LightSampling {
    /// By the light tree: nearby and bright lights are preferred at every shading point.
    #[default]
    Tree,
    /// By power alone, the same everywhere (what the GPU path does).
    Power,
}

/// An image of the surroundings with its sampling tables.
#[derive(Clone, Debug)]
pub struct EnvMap {
    pub image: Image,
    /// Rotation about the vertical axis, as a fraction of a turn added to `u`.
    pub turn: f32,
    /// Running sums: one row of `width` per image row, then normalized.
    conditional: Vec<f32>,
    marginal: Vec<f32>,
    /// Mean luminance over the sphere.
    pub mean_luminance: f32,
}

impl EnvMap {
    /// `None` when the image has no light in it. With `compensated`, the sampling table holds
    /// each pixel's brightness above the image's mean instead of its brightness: directions the
    /// surface's own sampling already covers well get no light samples, which lowers the variance
    /// of the combined estimator (Karlík et al., "MIS Compensation: Optimizing Sampling Techniques
    /// in Multiple Importance Sampling", 2019). A compensated table is only correct together with
    /// BSDF sampling ([`crate::Strategy::Mis`]).
    pub fn new(image: Image, rotation: f32, compensated: bool) -> Option<EnvMap> {
        let plain = EnvMap::tabulate(image.clone(), rotation, 0.0)?;
        if !compensated {
            return Some(plain);
        }
        let mean = plain.mean_luminance;
        Some(EnvMap::tabulate(image, rotation, mean).map_or(plain, |mut m| {
            m.mean_luminance = mean;
            m
        }))
    }

    fn tabulate(image: Image, rotation: f32, subtract: f32) -> Option<EnvMap> {
        let (w, h) = (image.width() as usize, image.height() as usize);
        if w == 0 || h == 0 {
            return None;
        }
        let mut conditional = Vec::with_capacity(w * h);
        let mut marginal = Vec::with_capacity(h);
        let (mut total, mut weight) = (0.0f64, 0.0f64);
        for y in 0..h {
            let sin = ((y as f32 + 0.5) / h as f32 * PI).sin();
            let mut row = 0.0f64;
            for x in 0..w {
                let p = image.get(x as i64, y as i64, true);
                let l = luminance(Vec3A::new(p[0], p[1], p[2]));
                row += f64::from(if l.is_finite() { (l - subtract).max(0.0) } else { 0.0 });
                conditional.push(row as f32);
            }
            total += row * f64::from(sin);
            weight += w as f64 * f64::from(sin);
            marginal.push(total as f32);
            if row > 0.0 {
                for c in &mut conditional[y * w..] {
                    *c /= row as f32;
                }
            }
        }
        if total <= 0.0 || !total.is_finite() {
            return None;
        }
        for m in &mut marginal {
            *m /= total as f32;
        }
        let turn = if rotation.is_finite() { (rotation / (2.0 * PI)).rem_euclid(1.0) } else { 0.0 };
        Some(EnvMap { image, turn, conditional, marginal, mean_luminance: (total / weight) as f32 })
    }

    #[inline]
    fn uv(&self, dir: Vec3A) -> Vec2 {
        let u = dir.z.atan2(dir.x) / (2.0 * PI) + 0.5 - self.turn;
        Vec2::new(u.rem_euclid(1.0), dir.y.clamp(-1.0, 1.0).acos() / PI)
    }

    /// Radiance arriving from `dir`.
    #[inline]
    pub fn radiance(&self, dir: Vec3A) -> Rgb {
        // The pixel under the direction, unfiltered: exactly the pixels the sampling table knows,
        // so no light falls where the table has no probability.
        let uv = self.uv(dir);
        let (w, h) = (self.image.width(), self.image.height());
        let p = self.image.get(((uv.x * w as f32) as i64).min(i64::from(w) - 1), ((uv.y * h as f32) as i64).min(i64::from(h) - 1), false);
        let c = Vec3A::new(p[0], p[1], p[2]);
        if c.is_finite() { c.max(Vec3A::ZERO) } else { Vec3A::ZERO }
    }

    /// Probability of the pixel under `uv`, as a density over the unit square.
    #[inline]
    fn pdf_uv(&self, uv: Vec2) -> f32 {
        let (w, h) = (self.image.width() as usize, self.image.height() as usize);
        let (x, y) = (((uv.x * w as f32) as usize).min(w - 1), ((uv.y * h as f32) as usize).min(h - 1));
        let step = |table: &[f32], i: usize, first: usize| {
            table.get(i).copied().unwrap_or(0.0) - if i > first { table.get(i - 1).copied().unwrap_or(0.0) } else { 0.0 }
        };
        step(&self.marginal, y, 0) * step(&self.conditional, y * w + x, y * w) * (w * h) as f32
    }

    /// Density over solid angle with which [`EnvMap::sample`] returns `dir`.
    #[inline]
    pub fn pdf(&self, dir: Vec3A) -> f32 {
        let sin = (1.0 - dir.y * dir.y).max(0.0).sqrt();
        if sin <= 1e-6 { 0.0 } else { self.pdf_uv(self.uv(dir)) / (2.0 * PI * PI * sin) }
    }

    /// A direction drawn in proportion to brightness, and its density over solid angle.
    #[inline]
    pub fn sample(&self, u: Vec2) -> (Vec3A, f32) {
        let (w, h) = (self.image.width() as usize, self.image.height() as usize);
        let pick = |table: &[f32], r: f32| -> (usize, f32) {
            let i = table.partition_point(|c| *c <= r).min(table.len().saturating_sub(1));
            let lo = if i > 0 { table.get(i - 1).copied().unwrap_or(0.0) } else { 0.0 };
            let span = table.get(i).copied().unwrap_or(1.0) - lo;
            (i, if span > 0.0 { ((r - lo) / span).clamp(0.0, 0.999_999) } else { 0.5 })
        };
        let (y, fy) = pick(&self.marginal, u.y);
        let (x, fx) = pick(self.conditional.get(y * w..(y + 1) * w).unwrap_or(&[]), u.x);
        let uv = Vec2::new((x as f32 + fx) / w as f32, (y as f32 + fy) / h as f32);
        let (theta, phi) = (uv.y * PI, (uv.x + self.turn - 0.5) * 2.0 * PI);
        let dir = Vec3A::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
        (dir, self.pdf(dir))
    }
}

#[derive(Clone, Copy, Debug)]
struct TreeNode {
    centre: Vec3A,
    /// Squared radius of the sphere around the node's lights.
    radius2: f32,
    power: f32,
    /// Interior: the two children. Leaf (`right == u32::MAX`): `left` is the light.
    left: u32,
    right: u32,
}

/// The hierarchy over lights that have a position.
#[derive(Clone, Debug, Default)]
struct LightTree {
    nodes: Vec<TreeNode>,
    /// Light → the left/right turns from the root to its leaf (bit `i` set = right at depth `i`).
    paths: Vec<u64>,
}

struct TreeLight {
    lo: Vec3A,
    hi: Vec3A,
    power: f32,
}

impl LightTree {
    fn build(lights: &[TreeLight]) -> LightTree {
        let mut tree = LightTree { nodes: Vec::with_capacity(lights.len() * 2), paths: vec![0; lights.len()] };
        if lights.is_empty() {
            return tree;
        }
        let mut order: Vec<u32> = (0..lights.len() as u32).collect();
        tree.split(lights, &mut order, 0, 0);
        tree
    }

    fn split(&mut self, lights: &[TreeLight], order: &mut [u32], path: u64, depth: u32) -> u32 {
        let (mut lo, mut hi, mut power) = (Vec3A::splat(f32::INFINITY), Vec3A::splat(f32::NEG_INFINITY), 0.0f32);
        for i in order.iter() {
            if let Some(l) = lights.get(*i as usize) {
                lo = lo.min(l.lo);
                hi = hi.max(l.hi);
                power += l.power.max(0.0);
            }
        }
        let me = self.nodes.len() as u32;
        self.nodes.push(TreeNode { centre: (lo + hi) * 0.5, radius2: (hi - lo).length_squared() * 0.25, power, left: 0, right: u32::MAX });
        if order.len() <= 1 || depth >= 63 {
            let light = order.first().copied().unwrap_or(0);
            if let Some(n) = self.nodes.get_mut(me as usize) {
                n.left = light;
            }
            if let Some(p) = self.paths.get_mut(light as usize) {
                *p = path;
            }
            return me;
        }
        let extent = hi - lo;
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        let mid = order.len() / 2;
        let centre = |i: &u32| lights.get(*i as usize).map_or(0.0, |l| (l.lo[axis] + l.hi[axis]) * 0.5);
        order.select_nth_unstable_by(mid, |a, b| centre(a).total_cmp(&centre(b)));
        let (a, b) = order.split_at_mut(mid);
        let left = self.split(lights, a, path, depth + 1);
        let right = self.split(lights, b, path | (1 << depth), depth + 1);
        if let Some(n) = self.nodes.get_mut(me as usize) {
            n.left = left;
            n.right = right;
        }
        me
    }

    #[inline]
    fn importance(&self, node: u32, p: Vec3A) -> f32 {
        let Some(n) = self.nodes.get(node as usize) else { return 0.0 };
        // Inside the node's sphere distance says nothing; treat the lights as one radius away.
        n.power / (n.centre - p).length_squared().max(n.radius2).max(1e-12)
    }

    /// Probability of going left at `node` when shading `p`.
    #[inline]
    fn left_probability(&self, node: &TreeNode, p: Vec3A) -> f32 {
        let (a, b) = (self.importance(node.left, p), self.importance(node.right, p));
        if a + b > 0.0 && (a + b).is_finite() { a / (a + b) } else { 0.5 }
    }

    /// A light and the probability with which it was chosen.
    #[inline]
    fn sample(&self, p: Vec3A, mut u: f32) -> Option<(u32, f32)> {
        let mut node = self.nodes.first()?;
        let mut pdf = 1.0;
        for _ in 0..64 {
            if node.right == u32::MAX {
                return Some((node.left, pdf));
            }
            let pl = self.left_probability(node, p);
            let next = if u < pl {
                u /= pl;
                pdf *= pl;
                node.left
            } else {
                u = (u - pl) / (1.0 - pl);
                pdf *= 1.0 - pl;
                node.right
            };
            node = self.nodes.get(next as usize)?;
        }
        None
    }

    /// Probability with which [`LightTree::sample`] returns `light` when shading `p`.
    #[inline]
    fn pdf(&self, light: u32, p: Vec3A) -> f32 {
        let Some(mut path) = self.paths.get(light as usize).copied() else { return 0.0 };
        let Some(mut node) = self.nodes.first() else { return 0.0 };
        let mut pdf = 1.0;
        for _ in 0..64 {
            if node.right == u32::MAX {
                return if node.left == light { pdf } else { 0.0 };
            }
            let pl = self.left_probability(node, p);
            let next = if path & 1 == 0 {
                pdf *= pl;
                node.left
            } else {
                pdf *= 1.0 - pl;
                node.right
            };
            path >>= 1;
            match self.nodes.get(next as usize) {
                Some(n) => node = n,
                None => return 0.0,
            }
        }
        0.0
    }
}

/// Everything that emits, with the tables used to choose one light per shading point.
#[derive(Clone, Debug, Default)]
pub struct Lights {
    pub emissive: Vec<EmissiveTri>,
    /// Point and spot lights first, then directional lights and suns.
    pub analytic: Vec<Light>,
    /// Constant radiance arriving from every direction that leaves the scene.
    pub environment: Vec3,
    /// An image of the surroundings; replaces `environment` when present.
    pub env_map: Option<EnvMap>,
    /// Triangle index → index into `emissive` (`u32::MAX` for the rest).
    tri_slot: Vec<u32>,
    /// Running sum of selection probabilities by power: emissive triangles, analytic lights,
    /// environment.
    cdf: Vec<f32>,
    /// How many entries of `cdf` have a position (and so are in the tree).
    positional: usize,
    tree: LightTree,
}

const NO_SLOT: u32 = u32::MAX;

impl Lights {
    pub fn is_empty(&self) -> bool {
        self.cdf.is_empty()
    }

    pub fn has_environment(&self) -> bool {
        self.env_map.is_some() || self.environment.max_element() > 0.0
    }

    pub(crate) fn build(&mut self, positions: &[[Vec3; 3]], scene_radius: f32) {
        // Lights with a position come first, so one range of the table belongs to the tree.
        self.analytic.sort_by_key(|l| l.positional().is_none());
        let mut power = Vec::with_capacity(self.emissive.len() + self.analytic.len() + 1);
        let mut tree_lights = Vec::new();
        self.tri_slot = vec![NO_SLOT; positions.len()];
        for (slot, e) in self.emissive.iter().enumerate() {
            if let Some(s) = self.tri_slot.get_mut(e.tri as usize) {
                *s = slot as u32;
            }
            let sides = if e.two_sided { 2.0 } else { 1.0 };
            let p = luminance(e.radiance.into()) * e.area * PI * sides;
            power.push(p);
            let t = positions.get(e.tri as usize).copied().unwrap_or([Vec3::ZERO; 3]);
            tree_lights.push(TreeLight { lo: Vec3A::from(t[0].min(t[1]).min(t[2])), hi: Vec3A::from(t[0].max(t[1]).max(t[2])), power: p });
        }
        let r2 = scene_radius * scene_radius;
        for l in &self.analytic {
            let p = match *l {
                Light::Point { intensity, .. } => luminance(intensity.into()) * 4.0 * PI,
                Light::Spot { intensity, cone_angle, .. } => luminance(intensity.into()) * 2.0 * PI * (1.0 - cone_angle.clamp(0.0, PI).cos()),
                Light::Directional { irradiance, .. } | Light::Sun { irradiance, .. } => luminance(irradiance.into()) * PI * r2,
            };
            power.push(p);
            if let Some(at) = l.positional() {
                tree_lights.push(TreeLight { lo: at.into(), hi: at.into(), power: p });
            }
        }
        self.positional = tree_lights.len();
        self.tree = LightTree::build(&tree_lights);
        let env = self.env_map.as_ref().map_or(luminance(self.environment.into()), |m| m.mean_luminance);
        if env > 0.0 {
            power.push(env * 4.0 * PI * PI * r2);
        }
        let total: f64 = power.iter().map(|p| f64::from(p.max(0.0))).sum();
        self.cdf.clear();
        if total <= 0.0 || !total.is_finite() {
            // Lights that carry no measurable power are chosen uniformly.
            let n = power.len();
            self.cdf.extend((1..=n).map(|i| i as f32 / n as f32));
            return;
        }
        let mut acc = 0.0f64;
        for p in &power {
            acc += f64::from(p.max(0.0));
            self.cdf.push((acc / total) as f32);
        }
        if let Some(last) = self.cdf.last_mut() {
            *last = 1.0;
        }
    }

    /// The running sum of selection probabilities by power, in the order emissive triangles,
    /// analytic lights, environment.
    pub fn selection_cdf(&self) -> &[f32] {
        &self.cdf
    }

    /// The emissive slot of a triangle, if it emits.
    pub fn emissive_slot(&self, tri: u32) -> Option<u32> {
        self.tri_slot.get(tri as usize).copied().filter(|s| *s != NO_SLOT)
    }

    /// Probability with which entry `entry` of [`Lights::selection_cdf`] is chosen by power.
    #[inline]
    pub fn pmf(&self, entry: usize) -> f32 {
        let hi = self.cdf.get(entry).copied().unwrap_or(0.0);
        let lo = if entry == 0 { 0.0 } else { self.cdf.get(entry - 1).copied().unwrap_or(0.0) };
        hi - lo
    }

    /// Probability that a light with a position is chosen at all.
    #[inline]
    fn positional_mass(&self) -> f32 {
        if self.positional == 0 { 0.0 } else { self.cdf.get(self.positional - 1).copied().unwrap_or(0.0) }
    }

    /// Probability of choosing positional light `entry` when shading `p`.
    #[inline]
    fn select_pdf(&self, entry: usize, p: Vec3A, mode: LightSampling) -> f32 {
        match mode {
            LightSampling::Power => self.pmf(entry),
            LightSampling::Tree => self.positional_mass() * self.tree.pdf(entry as u32, p),
        }
    }

    /// Density over solid angle with which [`Lights::sample`] reaches a point on emissive
    /// triangle `tri` from `p`, at `distance`, under the angle whose cosine is `cos_light`.
    #[inline]
    pub fn pdf_triangle(&self, tri: u32, p: Vec3A, distance: f32, cos_light: f32, mode: LightSampling) -> f32 {
        let Some(slot) = self.emissive_slot(tri) else { return 0.0 };
        let Some(e) = self.emissive.get(slot as usize) else { return 0.0 };
        let cos = cos_light.abs();
        if cos <= 0.0 || e.area <= 0.0 {
            return 0.0;
        }
        self.select_pdf(slot as usize, p, mode) * distance * distance / (e.area * cos)
    }

    /// Density with which [`Lights::sample`] picks direction `dir` leaving the scene.
    #[inline]
    pub fn pdf_environment(&self, dir: Vec3A) -> f32 {
        if !self.has_environment() {
            return 0.0;
        }
        let pmf = self.pmf(self.cdf.len().saturating_sub(1));
        pmf * self.env_map.as_ref().map_or(1.0 / (4.0 * PI), |m| m.pdf(dir))
    }

    /// Radiance arriving from `dir` from outside the scene.
    #[inline]
    pub fn environment_radiance(&self, dir: Vec3A) -> Rgb {
        self.env_map.as_ref().map_or(self.environment.into(), |m| m.radiance(dir))
    }

    /// Radiance an emissive triangle sends towards a viewer on its front (`front`) or back side.
    #[inline]
    pub fn emitted(&self, tri: u32, front: bool) -> Option<Rgb> {
        let e = self.emissive.get(self.emissive_slot(tri)? as usize)?;
        (front || e.two_sided).then(|| e.radiance.into())
    }

    /// Choose a light with `pick` and a point on it with `u`, as seen from `p`.
    #[inline]
    pub fn sample(&self, positions: &[[Vec3; 3]], p: Vec3A, pick: f32, u: Vec2, mode: LightSampling) -> Option<LightSample> {
        if self.cdf.is_empty() {
            return None;
        }
        let mass = self.positional_mass();
        let (entry, pmf) = if mode == LightSampling::Tree && pick < mass {
            let (light, pdf) = self.tree.sample(p, pick / mass)?;
            (light as usize, pdf * mass)
        } else {
            let entry = self.cdf.partition_point(|c| *c <= pick).min(self.cdf.len() - 1);
            (entry, self.pmf(entry))
        };
        if pmf <= 0.0 {
            return None;
        }
        if let Some(e) = self.emissive.get(entry) {
            let t = positions.get(e.tri as usize)?;
            let (p0, p1, p2) = (Vec3A::from(t[0]), Vec3A::from(t[1]), Vec3A::from(t[2]));
            let (b1, b2) = uniform_triangle(u);
            let q = p0 + (p1 - p0) * b1 + (p2 - p0) * b2;
            let d = q - p;
            let dist2 = d.length_squared();
            if dist2 <= 0.0 {
                return None;
            }
            let distance = dist2.sqrt();
            let wi = d / distance;
            let n = (p1 - p0).cross(p2 - p0).normalize_or_zero();
            let cos = -wi.dot(n);
            if cos == 0.0 || (cos < 0.0 && !e.two_sided) || e.area <= 0.0 {
                return None;
            }
            return Some(LightSample { wi, distance, radiance: e.radiance.into(), pdf: pmf * dist2 / (e.area * cos.abs()), delta: false });
        }
        if let Some(l) = self.analytic.get(entry - self.emissive.len()) {
            return match *l {
                Light::Point { position, intensity } => {
                    let d = Vec3A::from(position) - p;
                    let dist2 = d.length_squared();
                    (dist2 > 0.0).then(|| LightSample {
                        wi: d / dist2.sqrt(),
                        distance: dist2.sqrt(),
                        radiance: Vec3A::from(intensity) / dist2,
                        pdf: pmf,
                        delta: true,
                    })
                }
                Light::Spot { position, direction, intensity, cone_angle, blend } => {
                    let d = Vec3A::from(position) - p;
                    let dist2 = d.length_squared();
                    if dist2 <= 0.0 {
                        return None;
                    }
                    let wi = d / dist2.sqrt();
                    let falloff = spot_falloff(-wi.dot(Vec3A::from(direction).normalize_or_zero()), cone_angle, blend);
                    (falloff > 0.0).then(|| LightSample {
                        wi,
                        distance: dist2.sqrt(),
                        radiance: Vec3A::from(intensity) * (falloff / dist2),
                        pdf: pmf,
                        delta: true,
                    })
                }
                Light::Directional { direction, irradiance } => {
                    let wi = Vec3A::from(direction).normalize_or_zero();
                    (wi != Vec3A::ZERO).then(|| LightSample { wi, distance: f32::INFINITY, radiance: irradiance.into(), pdf: pmf, delta: true })
                }
                Light::Sun { direction, irradiance, angular_radius } => {
                    let axis = Vec3A::from(direction).normalize_or_zero();
                    if axis == Vec3A::ZERO {
                        return None;
                    }
                    // A point on the sun's disk; the irradiance is spread evenly over it.
                    let d = concentric_disk(u) * angular_radius.clamp(0.0, 0.5).tan();
                    let wi = Frame::new(axis).to_world(Vec3A::new(d.x, d.y, 1.0).normalize());
                    Some(LightSample { wi, distance: f32::INFINITY, radiance: irradiance.into(), pdf: pmf, delta: true })
                }
            };
        }
        match &self.env_map {
            Some(map) => {
                let (wi, pdf) = map.sample(u);
                (pdf > 0.0).then(|| LightSample { wi, distance: f32::INFINITY, radiance: map.radiance(wi), pdf: pmf * pdf, delta: false })
            }
            None => {
                Some(LightSample { wi: uniform_sphere(u), distance: f32::INFINITY, radiance: self.environment.into(), pdf: pmf / (4.0 * PI), delta: false })
            }
        }
    }
}

/// How much of a spot light reaches a direction whose cosine to the axis is `cos`.
#[inline]
pub fn spot_falloff(cos: f32, cone_angle: f32, blend: f32) -> f32 {
    let outer = cone_angle.clamp(1e-3, PI);
    let inner = outer * (1.0 - blend.clamp(0.0, 1.0));
    let (co, ci) = (outer.cos(), inner.cos());
    if cos <= co {
        0.0
    } else if cos >= ci || ci - co < 1e-6 {
        1.0
    } else {
        let t = (cos - co) / (ci - co);
        t * t * (3.0 - 2.0 * t)
    }
}
