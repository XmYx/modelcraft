//! Hair and fibre scattering, and the curve sets a scene carries.
//!
//! The model is the one of Chiang, Bitterli, Tappan and Burley, "A Practical and Controllable Hair
//! and Fur Model for Production Path Tracing" (EGSR 2016): a fibre is a smooth, rough-surfaced
//! cylinder, and light leaves it in lobes by the number of times it crossed the fibre.
//!
//! - **R**: reflection off the outside. **TT**: transmitted in and out. **TRT**: one internal
//!   reflection. A fourth lobe stands for every longer path (TRRT and beyond), summed as a geometric
//!   series and spread evenly around the fibre. Together they carry exactly the light that was not
//!   absorbed: with no absorption the four add up to one, which a test checks.
//! - **Longitudinal** scattering `M_p(θi, θo)` is the von Mises–Fisher based form of d'Eon, Francois,
//!   Hill, Letteri and Aubry, "An Energy-Conserving Hair Reflectance Model" (EGSR 2011), around the
//!   direction the lobe's tilt (the cuticle scale angle) points to. Its variance follows the
//!   longitudinal roughness `beta_m` (Chiang's mapping; TT is half and TRT twice the R variance).
//! - **Azimuthal** scattering is a logistic distribution (trimmed to one turn) around the angle a
//!   ray at offset `h` across the fibre leaves at, `Φ_p(h) = 2pγt − 2γo + pπ`, with width from the
//!   azimuthal roughness `beta_n`.
//! - **Attenuation** `A_p(h)` is Fresnel at the outside and `exp(−σa · path)` inside, with the
//!   modified index of refraction `η′ = √(η² − sin²θo) / cosθo` that turns the refraction of an
//!   oblique ray into a 2-D one in the plane across the fibre.
//!
//! A fibre hit gives the BSDF its frame directly: x along the fibre, y along the surface normal at
//! the hit (so the offset across the fibre is `h = sin φo`), z across. Directions point away from
//! the fibre. `eval` returns `f · cosθi` (so a path's contribution is `eval · L / pdf`, as for
//! surfaces), and `pdf` is a density over solid angle.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use craft_render_bvh::curves::{CurveBvh, CurveHit, HitFrame, Segment};
use craft_render_bvh::{Aabb, Ray};
use glam::{Vec2, Vec3, Vec3A};

use crate::bsdf::fresnel_dielectric;
use crate::math::{Rgb, luminance};

/// How a set of fibres scatters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HairParams {
    /// Absorption per fibre radius (linear RGB): `exp(−σa · 2r)` is the transmittance straight
    /// through the middle. [`HairParams::from_color`] derives it from a colour.
    pub sigma_a: Rgb,
    /// Index of refraction of the fibre (1.55 for keratin).
    pub eta: f32,
    /// Longitudinal roughness, 0 to 1.
    pub beta_m: f32,
    /// Azimuthal roughness, 0 to 1.
    pub beta_n: f32,
    /// Tilt of the cuticle scales, radians (2° is typical).
    pub alpha: f32,
}

impl Default for HairParams {
    fn default() -> HairParams {
        HairParams::from_color(Vec3A::new(0.45, 0.30, 0.18), 0.3)
    }
}

impl HairParams {
    /// Absorption that makes a fibre look `color` (the multiply-scattered reflectance, Chiang's
    /// fit): `σa = (ln c / (5.969 − 0.215β + 2.532β² − 10.73β³ + 5.574β⁴ + 0.245β⁵))²`.
    pub fn from_color(color: Rgb, beta_n: f32) -> HairParams {
        let b = beta_n.clamp(0.0, 1.0);
        let denom = 5.969 - 0.215 * b + 2.532 * b * b - 10.73 * b.powi(3) + 5.574 * b.powi(4) + 0.245 * b.powi(5);
        let sigma = |c: f32| {
            let l = c.clamp(1e-4, 1.0).ln() / denom;
            l * l
        };
        HairParams { sigma_a: Vec3A::new(sigma(color.x), sigma(color.y), sigma(color.z)), eta: 1.55, beta_m: 0.3, beta_n: b, alpha: 2.0f32.to_radians() }
    }

    /// The same parameters with every number made usable.
    pub fn sanitized(self) -> HairParams {
        let fin = |v: f32, lo: f32, hi: f32, d: f32| if v.is_finite() { v.clamp(lo, hi) } else { d };
        let s = if self.sigma_a.is_finite() { self.sigma_a.clamp(Vec3A::ZERO, Vec3A::splat(1000.0)) } else { Vec3A::splat(1.0) };
        HairParams {
            sigma_a: s,
            eta: fin(self.eta, 1.05, 3.0, 1.55),
            beta_m: fin(self.beta_m, 0.02, 1.0, 0.3),
            beta_n: fin(self.beta_n, 0.02, 1.0, 0.3),
            alpha: fin(self.alpha, 0.0, 0.5, 0.035),
        }
    }
}

const LOBES: usize = 4;

/// A sampled direction (same meaning as [`crate::bsdf::BsdfSample`]).
#[derive(Clone, Copy, Debug)]
pub struct HairSample {
    pub wi: Vec3A,
    /// `f · cosθi / pdf`.
    pub weight: Rgb,
    pub pdf: f32,
}

/// A fibre resolved at one hit for one outgoing direction.
#[derive(Clone, Copy, Debug)]
pub struct HairBsdf {
    sin_o: f32,
    phi_o: f32,
    /// Per lobe: tilted `sinθo`, `cosθo`; variance of the longitudinal lobe; azimuth centre.
    sin_t: [f32; LOBES],
    cos_t: [f32; LOBES],
    v: [f32; LOBES],
    phi_c: [f32; LOBES],
    a: [Rgb; LOBES],
    /// Probability of choosing each lobe.
    p: [f32; LOBES],
    s: f32,
}

#[inline]
fn log_i0(x: f32) -> f32 {
    let x = x.abs();
    if x < 12.0 {
        // Power series: Σ (x²/4)^k / (k!)².
        let (q, mut term, mut sum) = (x * x * 0.25, 1.0f32, 1.0f32);
        for k in 1..40 {
            term *= q / (k * k) as f32;
            sum += term;
            if term < 1e-9 * sum {
                break;
            }
        }
        sum.ln()
    } else {
        // Asymptotic expansion.
        x - 0.5 * (TAU * x).ln() + (1.0 + 1.0 / (8.0 * x) + 9.0 / (128.0 * x * x)).ln()
    }
}

/// `ln(2 v sinh(1/v))`, stable for small `v`.
#[inline]
fn log_norm(v: f32) -> f32 {
    let x = 1.0 / v;
    // sinh x = e^x (1 − e^{−2x}) / 2
    (2.0 * v).ln() + x + (1.0 - (-2.0 * x).exp()).max(1e-30).ln() - 2.0f32.ln()
}

#[inline]
fn logistic_pdf(x: f32, s: f32) -> f32 {
    let e = (-x.abs() / s).exp();
    e / (s * (1.0 + e) * (1.0 + e))
}

#[inline]
fn logistic_cdf(x: f32, s: f32) -> f32 {
    1.0 / (1.0 + (-x / s).exp())
}

/// The logistic density trimmed to `[−π, π]` and renormalised.
#[inline]
fn trimmed_logistic(x: f32, s: f32) -> f32 {
    if x.abs() > PI {
        return 0.0;
    }
    logistic_pdf(x, s) / (logistic_cdf(PI, s) - logistic_cdf(-PI, s)).max(1e-12)
}

#[inline]
fn sample_trimmed_logistic(u: f32, s: f32) -> f32 {
    let (lo, hi) = (logistic_cdf(-PI, s), logistic_cdf(PI, s));
    let p = (lo + u.clamp(0.0, 1.0) * (hi - lo)).clamp(1e-7, 1.0 - 1e-7);
    (-s * (1.0 / p - 1.0).ln()).clamp(-PI, PI)
}

#[inline]
fn wrap(mut x: f32) -> f32 {
    x = (x + PI).rem_euclid(TAU) - PI;
    x
}

impl HairBsdf {
    /// `wo` is the direction to the viewer in the fibre frame (x along the fibre, y along the
    /// normal at the hit, z across) with `wo.y > 0` for a hit from outside.
    pub fn new(params: &HairParams, wo: Vec3A) -> HairBsdf {
        let p = params.sanitized();
        let wo = wo.normalize_or(Vec3A::Y);
        let sin_o = wo.x.clamp(-1.0, 1.0);
        let cos_o = (1.0 - sin_o * sin_o).max(1e-8).sqrt();
        let phi_o = wo.z.atan2(wo.y);
        let gamma_o = phi_o;
        let h = gamma_o.sin();
        let eta_p = (p.eta * p.eta - sin_o * sin_o).max(1e-6).sqrt() / cos_o;
        let gamma_t = (h / eta_p).clamp(-1.0, 1.0).asin();
        let sin_theta_t = sin_o / p.eta;
        let cos_theta_t = (1.0 - sin_theta_t * sin_theta_t).max(1e-8).sqrt();
        let path = 2.0 * gamma_t.cos() / cos_theta_t;
        let t = Vec3A::new((-p.sigma_a.x * path).exp(), (-p.sigma_a.y * path).exp(), (-p.sigma_a.z * path).exp());
        let f = fresnel_dielectric(cos_o * gamma_o.cos().abs(), p.eta);
        let a1 = t * ((1.0 - f) * (1.0 - f));
        let a2 = a1 * t * f;
        let tail = a2 * t * f / (Vec3A::ONE - t * f).max(Vec3A::splat(1e-4));
        let a = [Vec3A::splat(f), a1, a2, tail];

        // Variances, tilts and azimuth centres per lobe.
        let bm = p.beta_m;
        let v0 = (0.726 * bm + 0.812 * bm * bm + 3.7 * bm.powi(20)).powi(2).max(1e-4);
        let v = [v0, v0 * 0.5, v0 * 2.0, v0 * 2.0];
        let tilt = [-2.0 * p.alpha, p.alpha, 4.0 * p.alpha, 4.0 * p.alpha];
        let mut sin_t = [0.0; LOBES];
        let mut cos_t = [0.0; LOBES];
        for k in 0..LOBES {
            let (st, ct) = tilt[k].sin_cos();
            // θo rotated by the lobe's tilt.
            sin_t[k] = sin_o * ct.abs() + cos_o * st;
            cos_t[k] = cos_o * ct.abs() - sin_o * st;
        }
        let phi_c = [
            -2.0 * gamma_o,
            2.0 * gamma_t - 2.0 * gamma_o + PI,
            4.0 * gamma_t - 2.0 * gamma_o + 2.0 * PI,
            0.0, // spread evenly
        ];
        let lum: [f32; LOBES] = std::array::from_fn(|k| luminance(a[k]).max(0.0));
        let total: f32 = lum.iter().sum();
        let probs = if total > 1e-12 { std::array::from_fn(|k| lum[k] / total) } else { [0.0; LOBES] };
        let s = (PI / 8.0).sqrt() * (0.265 * p.beta_n + 1.194 * p.beta_n * p.beta_n + 5.372 * p.beta_n.powi(22));
        HairBsdf { sin_o, phi_o, sin_t, cos_t, v, phi_c, a, p: probs, s: s.max(0.01) }
    }

    /// The longitudinal term of lobe `k` for incident `sinθi`.
    #[inline]
    fn m(&self, k: usize, sin_i: f32, cos_i: f32) -> f32 {
        let v = self.v[k];
        let a = cos_i * self.cos_t[k] / v;
        let b = sin_i * self.sin_t[k] / v;
        (log_i0(a) - b - log_norm(v)).exp()
    }

    /// The azimuthal density of lobe `k` at relative azimuth `dphi = φi − φo`.
    #[inline]
    fn d(&self, k: usize, dphi: f32) -> f32 {
        if k == LOBES - 1 { 1.0 / TAU } else { trimmed_logistic(wrap(dphi - self.phi_c[k]), self.s) }
    }

    /// `(f · cosθi, pdf)` for the incident direction `wi` (same frame as `wo`).
    pub fn eval(&self, wi: Vec3A) -> (Rgb, f32) {
        let wi = wi.normalize_or(Vec3A::Y);
        let sin_i = wi.x.clamp(-1.0, 1.0);
        let cos_i = (1.0 - sin_i * sin_i).max(0.0).sqrt();
        let dphi = wi.z.atan2(wi.y) - self.phi_o;
        let (mut f, mut pdf) = (Vec3A::ZERO, 0.0);
        for k in 0..LOBES {
            let md = self.m(k, sin_i, cos_i) * self.d(k, dphi);
            f += self.a[k] * md;
            pdf += self.p[k] * md;
        }
        if f.is_finite() && pdf.is_finite() { (f, pdf) } else { (Vec3A::ZERO, 0.0) }
    }

    /// Sample an incident direction: `lobe` picks the lobe, `u` the longitudinal direction and `u_phi`
    /// the azimuth.
    pub fn sample(&self, lobe: f32, u: Vec2, u_phi: f32) -> Option<HairSample> {
        let mut k = 0;
        let mut acc = 0.0;
        for (i, p) in self.p.iter().enumerate() {
            acc += p;
            k = i;
            if lobe < acc {
                break;
            }
        }
        if self.p[k] <= 0.0 {
            return None;
        }
        // Longitudinal: a von Mises–Fisher direction around the mirrored, tilted outgoing direction.
        let v = self.v[k];
        let u0 = u.x.clamp(1e-5, 1.0);
        let cos_g = (1.0 + v * (u0 + (1.0 - u0) * (-2.0 / v).exp()).ln()).clamp(-1.0, 1.0);
        let sin_g = (1.0 - cos_g * cos_g).max(0.0).sqrt();
        let axis = Vec3A::new(-self.sin_t[k], self.cos_t[k], 0.0);
        let e1 = axis.any_orthonormal_vector();
        let e2 = axis.cross(e1);
        let (sp, cp) = (TAU * u.y).sin_cos();
        let dir = axis * cos_g + (e1 * cp + e2 * sp) * sin_g;
        let sin_i = dir.x.clamp(-1.0, 1.0);
        let cos_i = (1.0 - sin_i * sin_i).max(0.0).sqrt();
        // Azimuth.
        let dphi = if k == LOBES - 1 { (u_phi - 0.5) * TAU } else { self.phi_c[k] + sample_trimmed_logistic(u_phi, self.s) };
        let phi = self.phi_o + dphi;
        let wi = Vec3A::new(sin_i, cos_i * phi.cos(), cos_i * phi.sin());
        let (f, pdf) = self.eval(wi);
        if pdf <= 0.0 || f.max_element() <= 0.0 {
            return None;
        }
        Some(HairSample { wi, weight: f / pdf, pdf })
    }

    /// The probability of each lobe (R, TT, TRT, residual).
    pub fn lobe_probabilities(&self) -> [f32; LOBES] {
        self.p
    }

    /// The fraction of light each lobe carries at this offset: the sum is the albedo.
    pub fn attenuation(&self) -> [Rgb; LOBES] {
        self.a
    }

    pub fn wo_azimuth(&self) -> f32 {
        self.phi_o
    }

    pub fn wo_sin_theta(&self) -> f32 {
        self.sin_o
    }
}

/// Hair strands in a scene: a BVH of round cones and one scattering model for them.
#[derive(Clone, Debug, Default)]
pub struct CurveSet {
    pub bvh: CurveBvh,
    pub params: HairParams,
}

/// What the integrator needs at a hit of a curve.
#[derive(Clone, Copy, Debug)]
pub struct FibreHit {
    pub t: f32,
    pub p: Vec3A,
    pub frame: HitFrame,
}

impl CurveSet {
    /// Strands of `points_per_strand` points each, with the radius at every point.
    pub fn new(points: &[Vec3], radii: &[f32], points_per_strand: usize, params: HairParams) -> CurveSet {
        CurveSet { bvh: CurveBvh::from_strands(points, radii, points_per_strand), params: params.sanitized() }
    }

    pub fn is_empty(&self) -> bool {
        self.bvh.segment_count() == 0
    }

    pub fn bounds(&self) -> Aabb {
        self.bvh.bounds()
    }

    /// The nearest fibre the ray hits before `tmax`, with its frame.
    #[inline]
    pub fn intersect(&self, ray: &Ray, tmax: f32) -> Option<FibreHit> {
        let mut r = *ray;
        r.tmax = r.tmax.min(tmax);
        let hit: CurveHit = self.bvh.intersect(&r)?;
        let seg: &Segment = self.bvh.segment_of(&hit)?;
        let p = ray.at(hit.t);
        Some(FibreHit { t: hit.t, p, frame: seg.frame_at(p) })
    }

    #[inline]
    pub fn occluded(&self, ray: &Ray) -> bool {
        self.bvh.occluded(ray)
    }
}

/// The fibre frame at a hit: x along the fibre, y along the normal, z across; the viewer's
/// direction `-ray.dir` expressed in it, with the normal turned to face the viewer.
#[inline]
pub fn fibre_frame(frame: &HitFrame, to_viewer: Vec3A) -> (Vec3A, Vec3A, Vec3A) {
    let t = frame.tangent;
    let n = if frame.normal.dot(to_viewer) < 0.0 { -frame.normal } else { frame.normal };
    (t, n, t.cross(n))
}

/// A ray leaving the fibre at `hit` in direction `dir`: it starts where `dir` leaves the cylinder
/// (from the axis point) so the fibre it left cannot be hit again.
#[inline]
pub fn leave_fibre(hit: &FibreHit, dir: Vec3A, tmax: f32) -> Ray {
    let along = dir.dot(hit.frame.tangent);
    let across = (1.0 - along * along).max(1e-4).sqrt();
    let d = hit.frame.radius / across * 1.002 + 1e-6;
    Ray { origin: hit.frame.axis_point + dir * d, dir, tmin: 0.0, tmax }
}

/// Sharing a curve set between scene copies.
pub type SharedCurves = Arc<CurveSet>;

#[cfg(test)]
mod tests {
    use super::*;

    fn wo(theta: f32, phi: f32) -> Vec3A {
        Vec3A::new(theta.sin(), theta.cos() * phi.cos(), theta.cos() * phi.sin())
    }

    fn params(sigma: f32, bm: f32, bn: f32) -> HairParams {
        HairParams { sigma_a: Vec3A::splat(sigma), eta: 1.55, beta_m: bm, beta_n: bn, alpha: 2.0f32.to_radians() }
    }

    /// ∫ f cosθ dω by uniform sampling of the sphere.
    fn albedo_by_uniform_sampling(b: &HairBsdf, n: usize) -> Vec3A {
        let mut s = Vec3A::ZERO;
        for i in 0..n {
            let (u1, u2) = (((i as f32 + 0.5) / n as f32), (((i as u64 * 7919) % n as u64) as f32 + 0.5) / n as f32);
            let z = 1.0 - 2.0 * u1;
            let r = (1.0 - z * z).sqrt();
            let phi = TAU * u2 * 1.0;
            let w = Vec3A::new(z, r * phi.cos(), r * phi.sin());
            s += b.eval(w).0;
        }
        s * (4.0 * PI / n as f32)
    }

    #[test]
    fn the_longitudinal_lobe_integrates_to_one() {
        // ∫ M(θi, θo) cosθi dθi = 1 for any outgoing angle and variance.
        for (bm, theta_o) in [(0.1f32, 0.0f32), (0.3, 0.6), (0.6, -1.0), (0.9, 0.2)] {
            let b = HairBsdf::new(&params(0.0, bm, 0.3), wo(theta_o, 0.2));
            for k in 0..LOBES {
                let n = 20_000;
                let mut s = 0.0f64;
                for i in 0..n {
                    let th = -PI / 2.0 + PI * (i as f32 + 0.5) / n as f32;
                    s += f64::from(b.m(k, th.sin(), th.cos()) * th.cos());
                }
                s *= f64::from(PI) / n as f64;
                assert!((s - 1.0).abs() < 2e-3, "beta_m {bm} theta_o {theta_o} lobe {k}: {s}");
            }
        }
    }

    #[test]
    fn the_azimuthal_lobe_integrates_to_one() {
        for bn in [0.05f32, 0.3, 0.8] {
            let s = (PI / 8.0).sqrt() * (0.265 * bn + 1.194 * bn * bn + 5.372 * bn.powi(22));
            let n = 40_000;
            let sum: f32 = (0..n).map(|i| trimmed_logistic(-PI + TAU * (i as f32 + 0.5) / n as f32, s.max(0.01))).sum::<f32>() * TAU / n as f32;
            assert!((sum - 1.0).abs() < 1e-3, "{bn}: {sum}");
        }
    }

    #[test]
    fn no_absorption_conserves_all_the_light() {
        // The four lobes carry exactly 1 − 0 with nothing absorbed.
        for (theta, phi) in [(0.0f32, 0.0f32), (0.4, 0.7), (-0.9, -0.3), (1.2, 0.9)] {
            let b = HairBsdf::new(&params(0.0, 0.3, 0.3), wo(theta, phi));
            let total: Vec3A = b.attenuation().iter().copied().sum();
            assert!((total - Vec3A::ONE).abs().max_element() < 1e-4, "{theta} {phi}: {total:?}");
            let probs: f32 = b.lobe_probabilities().iter().sum();
            assert!((probs - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn the_integral_of_the_bsdf_equals_the_albedo_and_never_exceeds_one() {
        for sigma in [0.0f32, 0.3, 1.5] {
            for (bm, bn) in [(0.3f32, 0.3f32), (0.5, 0.6), (0.7, 0.9)] {
                for (theta, phi) in [(0.0f32, 0.0f32), (0.3, 0.8), (-0.7, -0.4)] {
                    let b = HairBsdf::new(&params(sigma, bm, bn), wo(theta, phi));
                    let albedo: Vec3A = b.attenuation().iter().copied().sum();
                    let integral = albedo_by_uniform_sampling(&b, 400_000);
                    // Uniform sampling of a peaky function is noisy: compare loosely, but exactly bound.
                    assert!((integral.x - albedo.x).abs() < 0.04 + 0.08 * albedo.x, "σ {sigma} β {bm},{bn} θ {theta}: {integral:?} vs {albedo:?}");
                    assert!(albedo.max_element() <= 1.0 + 1e-4, "{albedo:?}");
                }
            }
        }
    }

    #[test]
    fn absorption_only_removes_light() {
        let clear = HairBsdf::new(&params(0.0, 0.3, 0.3), wo(0.2, 0.3));
        let dark = HairBsdf::new(&params(2.0, 0.3, 0.3), wo(0.2, 0.3));
        let (ac, ad): (Vec3A, Vec3A) = (clear.attenuation().iter().copied().sum(), dark.attenuation().iter().copied().sum());
        assert!(ad.x < ac.x * 0.8 && ad.x > 0.0);
        // Reflection does not depend on absorption.
        assert!((clear.attenuation()[0] - dark.attenuation()[0]).abs().max_element() < 1e-6);
    }

    #[test]
    fn sampling_agrees_with_evaluation() {
        // The weight of a sample is f/pdf; its pdf is the one eval reports; the mean weight over
        // many samples is the albedo (the sampling densities match the lobes).
        for (theta, phi, sigma) in [(0.0f32, 0.0f32, 0.0f32), (0.5, 0.6, 0.4), (-0.8, -0.2, 1.0)] {
            let b = HairBsdf::new(&params(sigma, 0.4, 0.5), wo(theta, phi));
            let albedo: Vec3A = b.attenuation().iter().copied().sum();
            let n = 200_000;
            let mut sum = Vec3A::ZERO;
            for i in 0..n {
                let f = |k: u32| ((i as u32).wrapping_mul(2654435761).wrapping_add(k.wrapping_mul(40503)) >> 8) as f32 / 16_777_216.0;
                let (lobe, u0, u1, u2) = (f(1), f(2), f(3), f(4));
                if let Some(s) = b.sample(lobe, Vec2::new(u0, u1), u2) {
                    let (fv, pdf) = b.eval(s.wi);
                    assert!((pdf - s.pdf).abs() <= 1e-3 * pdf.max(1.0));
                    assert!((s.weight - fv / pdf).abs().max_element() < 1e-3 * (1.0 + s.weight.max_element()));
                    assert!((s.wi.length() - 1.0).abs() < 1e-4);
                    sum += s.weight;
                }
            }
            let mean = sum / n as f32;
            assert!((mean.x - albedo.x).abs() < 0.03, "{theta} {phi} {sigma}: mean {mean:?} albedo {albedo:?}");
        }
    }

    #[test]
    fn the_pdf_integrates_to_one() {
        let b = HairBsdf::new(&params(0.5, 0.5, 0.5), wo(0.3, 0.5));
        let n = 600_000;
        let mut s = 0.0f64;
        for i in 0..n {
            let (u1, u2) = ((i as f32 + 0.5) / n as f32, ((i as u64 * 104_729) % n as u64) as f32 / n as f32 + 0.5 / n as f32);
            let z = 1.0 - 2.0 * u1;
            let r = (1.0 - z * z).sqrt();
            let phi = TAU * u2;
            s += f64::from(b.eval(Vec3A::new(z, r * phi.cos(), r * phi.sin())).1);
        }
        s *= 4.0 * f64::from(PI) / n as f64;
        assert!((s - 1.0).abs() < 0.05, "{s}");
    }

    #[test]
    fn a_hit_at_the_edge_and_the_middle_scatter_differently() {
        // A ray through the middle (h = 0) transmits straight on; at the edge it mostly reflects.
        let mid = HairBsdf::new(&params(0.0, 0.3, 0.3), wo(0.0, 0.0));
        let edge = HairBsdf::new(&params(0.0, 0.3, 0.3), wo(0.0, 1.4));
        assert!(mid.attenuation()[1].x > 0.7, "TT through the middle: {:?}", mid.attenuation());
        assert!(edge.attenuation()[0].x > mid.attenuation()[0].x * 3.0, "R grows toward the edge");
        // Forward scatter is behind the fibre: the TT lobe's azimuth centre is π away from the viewer.
        assert!((wrap(mid.phi_c[1]) - PI).abs() < 1e-4 || (wrap(mid.phi_c[1]) + PI).abs() < 1e-4);
    }

    #[test]
    fn colour_sets_absorption_and_hostile_numbers_are_repaired() {
        let dark = HairParams::from_color(Vec3A::splat(0.05), 0.3);
        let light = HairParams::from_color(Vec3A::splat(0.8), 0.3);
        assert!(dark.sigma_a.x > light.sigma_a.x * 10.0);
        let bad = HairParams { sigma_a: Vec3A::NAN, eta: f32::INFINITY, beta_m: -3.0, beta_n: 9.0, alpha: f32::NAN }.sanitized();
        assert!(bad.sigma_a.is_finite() && bad.eta.is_finite() && (0.02..=1.0).contains(&bad.beta_m) && bad.beta_n <= 1.0 && bad.alpha.is_finite());
        // Grazing along the fibre, along the normal and NaN input do not produce NaN.
        for w in [Vec3A::X, -Vec3A::X, Vec3A::Y, Vec3A::NAN, Vec3A::ZERO, Vec3A::new(1.0, 1e-9, 0.0)] {
            let b = HairBsdf::new(&HairParams::default(), w);
            for wi in [Vec3A::X, Vec3A::Z, -Vec3A::Y, Vec3A::new(0.3, 0.4, 0.5), Vec3A::NAN] {
                let (f, p) = b.eval(wi);
                assert!(f.is_finite() && p.is_finite());
            }
            if let Some(s) = b.sample(0.3, Vec2::new(0.4, 0.9), 0.2) {
                assert!(s.wi.is_finite() && s.weight.is_finite());
            }
        }
    }

    #[test]
    fn a_ray_leaving_a_fibre_cannot_hit_it_again() {
        let strands = [Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)];
        let set = CurveSet::new(&strands, &[0.05, 0.05], 2, HairParams::default());
        let ray = Ray::new(Vec3::new(-1.0, 0.1, 0.0), Vec3::X);
        let hit = set.intersect(&ray, f32::INFINITY).unwrap();
        for dir in [Vec3A::X, Vec3A::new(0.3, 0.9, 0.1).normalize(), Vec3A::new(-1.0, 0.0, 0.2).normalize(), Vec3A::new(0.05, 1.0, 0.0).normalize()] {
            let out = leave_fibre(&hit, dir, f32::INFINITY);
            assert!(set.intersect(&out, f32::INFINITY).is_none(), "{dir:?} re-hits the fibre it left");
            assert!(!set.occluded(&out));
        }
        // A frame faces the viewer.
        let (t, n, w) = fibre_frame(&hit.frame, -ray.dir);
        assert!(n.dot(-ray.dir) > 0.0 && (t.cross(n) - w).length() < 1e-6);
    }
}
