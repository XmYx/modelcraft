//! The surface scattering model.
//!
//! One GGX microfacet lobe (Walter et al. 2007; Trowbridge and Reitz 1975) carries the metal and
//! the dielectric specular reflection; a Lambert lobe sits under the dielectric interface.
//!
//! - Masking and shadowing: the height-correlated Smith term (Heitz, "Understanding the
//!   Masking-Shadowing Function in Microfacet-Based BRDFs", JCGT 2014).
//! - Sampling: the distribution of visible normals (Heitz, "Sampling the GGX Distribution of
//!   Visible Normals", JCGT 2018).
//! - Energy: the single-scattering lobe loses energy at high roughness; it is scaled by
//!   `1 + F0 (1 − E) / E` (Turquin, "Practical multiple scattering compensation for microfacet
//!   models", 2019; after Kulla and Conty 2017), where `E` is the lobe's directional albedo.
//! - Layering: the diffuse base receives what the specular interface does not reflect,
//!   `1 − E_spec(ωo)` (albedo scaling, as the OpenPBR specification describes).
//!
//! Directions are in the shading frame (z is the normal) and point away from the surface.

use std::f32::consts::PI;
use std::sync::OnceLock;

use glam::{Vec2, Vec3A};

use crate::material::Material;
use crate::math::{INV_PI, Rgb, cosine_hemisphere, luminance};

/// Smallest microfacet width; smoother surfaces are treated as this sharp.
pub const ALPHA_MIN: f32 = 1e-3;
/// Resolution of the albedo tables (roughness × cosine).
pub const TABLE_SIZE: usize = 32;

#[inline]
fn ggx_d(h: Vec3A, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let d = (h.x * h.x + h.y * h.y) / a2 + h.z * h.z;
    1.0 / (PI * a2 * d * d)
}

#[inline]
fn ggx_lambda(w: Vec3A, alpha: f32) -> f32 {
    let z2 = w.z * w.z;
    if z2 <= 0.0 {
        return f32::INFINITY;
    }
    let t2 = alpha * alpha * (w.x * w.x + w.y * w.y) / z2;
    ((1.0 + t2).sqrt() - 1.0) * 0.5
}

/// A microfacet normal from the distribution of normals visible from `wo`.
#[inline]
fn sample_visible_normal(wo: Vec3A, alpha: f32, u: Vec2) -> Vec3A {
    let vh = Vec3A::new(alpha * wo.x, alpha * wo.y, wo.z).normalize();
    let len2 = vh.x * vh.x + vh.y * vh.y;
    let t1 = if len2 > 0.0 { Vec3A::new(-vh.y, vh.x, 0.0) / len2.sqrt() } else { Vec3A::X };
    let t2 = vh.cross(t1);
    let r = u.x.sqrt();
    let phi = 2.0 * PI * u.y;
    let p1 = r * phi.cos();
    let s = 0.5 * (1.0 + vh.z);
    let p2 = (1.0 - s) * (1.0 - p1 * p1).max(0.0).sqrt() + s * r * phi.sin();
    let nh = t1 * p1 + t2 * p2 + vh * (1.0 - p1 * p1 - p2 * p2).max(0.0).sqrt();
    Vec3A::new(alpha * nh.x, alpha * nh.y, nh.z.max(1e-6)).normalize()
}

#[inline]
fn schlick_weight(cos: f32) -> f32 {
    let m = (1.0 - cos).clamp(0.0, 1.0);
    let m2 = m * m;
    m2 * m2 * m
}

/// Directional albedo of the single-scattering GGX lobe, split by the Schlick weight:
/// `E(F0, F90) = F0·a + F90·b`.
pub struct AlbedoTable {
    /// `[roughness][cosine]` → `(a, b)`.
    pub ab: Vec<[f32; 2]>,
}

impl AlbedoTable {
    fn build() -> AlbedoTable {
        const SAMPLES: u32 = 1024;
        let mut ab = Vec::with_capacity(TABLE_SIZE * TABLE_SIZE);
        for j in 0..TABLE_SIZE {
            let rough = j as f32 / (TABLE_SIZE - 1) as f32;
            let alpha = (rough * rough).max(ALPHA_MIN);
            for i in 0..TABLE_SIZE {
                let mu = (i as f32 / (TABLE_SIZE - 1) as f32).max(1e-3);
                let wo = Vec3A::new((1.0 - mu * mu).max(0.0).sqrt(), 0.0, mu);
                let (mut a, mut b) = (0.0f64, 0.0f64);
                for k in 0..SAMPLES {
                    let u = Vec2::new((k as f32 + 0.5) / SAMPLES as f32, (k.reverse_bits() >> 8) as f32 / 16_777_216.0);
                    let h = sample_visible_normal(wo, alpha, u);
                    let wi = h * (2.0 * wo.dot(h)) - wo;
                    if wi.z <= 0.0 {
                        continue;
                    }
                    let w = (1.0 + ggx_lambda(wo, alpha)) / (1.0 + ggx_lambda(wo, alpha) + ggx_lambda(wi, alpha));
                    let fc = schlick_weight(wo.dot(h));
                    a += f64::from(w * (1.0 - fc));
                    b += f64::from(w * fc);
                }
                ab.push([(a / f64::from(SAMPLES)) as f32, (b / f64::from(SAMPLES)) as f32]);
            }
        }
        AlbedoTable { ab }
    }

    /// The shared table (built once, in a few milliseconds).
    pub fn get() -> &'static AlbedoTable {
        static TABLE: OnceLock<AlbedoTable> = OnceLock::new();
        TABLE.get_or_init(AlbedoTable::build)
    }

    /// `(a, b)` at a cosine and a roughness, bilinearly interpolated.
    #[inline]
    pub fn lookup(&self, cos: f32, roughness: f32) -> (f32, f32) {
        let m = (TABLE_SIZE - 1) as f32;
        let x = cos.clamp(0.0, 1.0) * m;
        let y = roughness.clamp(0.0, 1.0) * m;
        let (x0, y0) = ((x as usize).min(TABLE_SIZE - 2), (y as usize).min(TABLE_SIZE - 2));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |xi: usize, yi: usize| self.ab.get(yi * TABLE_SIZE + xi).copied().unwrap_or([0.0, 0.0]);
        let (p00, p10, p01, p11) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
        let lerp = |k: usize| (p00[k] * (1.0 - fx) + p10[k] * fx) * (1.0 - fy) + (p01[k] * (1.0 - fx) + p11[k] * fx) * fy;
        (lerp(0), lerp(1))
    }
}

/// A sampled direction.
#[derive(Clone, Copy, Debug)]
pub struct BsdfSample {
    pub wi: Vec3A,
    /// `f · |cos θi| / pdf`.
    pub weight: Rgb,
    pub pdf: f32,
}

/// A material resolved at one point for one outgoing direction.
#[derive(Clone, Copy, Debug)]
pub struct Bsdf {
    wo: Vec3A,
    alpha: f32,
    /// Lambert reflectance, already scaled by what the interface lets through.
    diffuse: Rgb,
    /// Schlick endpoints of the microfacet lobe, multiple-scattering factor included.
    f0: Rgb,
    f90: Rgb,
    /// Probability of sampling the microfacet lobe.
    p_spec: f32,
}

impl Bsdf {
    /// `wo` is the direction to the viewer in the shading frame, `wo.z > 0`. The material must be
    /// [`Material::sanitized`].
    pub fn new(m: &Material, wo: Vec3A) -> Bsdf {
        let wo = if wo.z > 1e-4 { wo } else { Vec3A::new(wo.x, wo.y, 1e-4).normalize() };
        let alpha = (m.specular_roughness * m.specular_roughness).max(ALPHA_MIN);
        let (a, b) = AlbedoTable::get().lookup(wo.z, m.specular_roughness);
        let e = (a + b).clamp(1e-4, 1.0);
        let k = (1.0 - e) / e;
        let base = Vec3A::from(m.base_color) * m.base_weight;
        // Metal: Schlick from the base colour to white.
        let metal_ms = Vec3A::ONE + base * k;
        let metal_e = (base * a + Vec3A::splat(b)) * metal_ms;
        // Dielectric interface.
        let r = (m.specular_ior - 1.0) / (m.specular_ior + 1.0);
        let d0 = r * r * m.specular_weight;
        let d90 = m.specular_weight;
        let diel_ms = 1.0 + d0 * k;
        let diel_e = ((d0 * a + d90 * b) * diel_ms).clamp(0.0, 1.0);
        let tint = Vec3A::from(m.specular_color);
        let (mt, dl) = (m.metalness, 1.0 - m.metalness);
        let f0 = base * metal_ms * mt + tint * (d0 * diel_ms * dl);
        let f90 = metal_ms * mt + tint * (d90 * diel_ms * dl);
        let diffuse = base * (dl * (1.0 - diel_e));
        let spec_albedo = luminance(metal_e * mt + tint * (diel_e * dl));
        let diff_albedo = luminance(diffuse);
        let total = spec_albedo + diff_albedo;
        let p_spec = if total > 0.0 { (spec_albedo / total).clamp(0.0, 1.0) } else { 0.0 };
        Bsdf { wo, alpha, diffuse, f0, f90, p_spec }
    }

    /// `(f · cos θi, pdf)` for the incoming direction `wi`.
    #[inline]
    pub fn eval(&self, wi: Vec3A) -> (Rgb, f32) {
        if wi.z <= 0.0 {
            return (Vec3A::ZERO, 0.0);
        }
        let mut f = self.diffuse * (INV_PI * wi.z);
        let mut pdf = (1.0 - self.p_spec) * wi.z * INV_PI;
        if self.p_spec > 0.0 {
            let h = (self.wo + wi).normalize_or_zero();
            if h.z > 0.0 {
                let d = ggx_d(h, self.alpha);
                let (lo, li) = (ggx_lambda(self.wo, self.alpha), ggx_lambda(wi, self.alpha));
                let fr = self.f0 + (self.f90 - self.f0) * schlick_weight(self.wo.dot(h));
                let common = d / (4.0 * self.wo.z);
                if common.is_finite() {
                    f += fr * (common / (1.0 + lo + li));
                    pdf += self.p_spec * common / (1.0 + lo);
                }
            }
        }
        (f, pdf)
    }

    /// Draw an incoming direction. `lobe` chooses the lobe and `u` the direction within it.
    #[inline]
    pub fn sample(&self, lobe: f32, u: Vec2) -> Option<BsdfSample> {
        let wi = if lobe < self.p_spec {
            let h = sample_visible_normal(self.wo, self.alpha, u);
            h * (2.0 * self.wo.dot(h)) - self.wo
        } else {
            cosine_hemisphere(u)
        };
        let (f, pdf) = self.eval(wi);
        (pdf > 0.0 && f.is_finite()).then(|| BsdfSample { wi, weight: f / pdf, pdf })
    }

    /// The outgoing direction this was resolved for.
    pub fn wo(&self) -> Vec3A {
        self.wo
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler::Sampler;
    use glam::Vec3;
    use proptest::prelude::*;

    fn albedo(m: &Material, cos: f32, n: u32) -> Rgb {
        let wo = Vec3A::new((1.0 - cos * cos).sqrt(), 0.0, cos);
        let bsdf = Bsdf::new(&m.sanitized(), wo);
        let mut acc = Vec3A::ZERO;
        for i in 0..n {
            let mut s = Sampler::new(11, i);
            let lobe = s.next1();
            if let Some(smp) = bsdf.sample(lobe, s.next2()) {
                acc += smp.weight;
            }
        }
        acc / n as f32
    }

    #[test]
    fn white_furnace_holds_for_metal_and_matte() {
        // A white metal reflects everything at any roughness once multiple scattering is restored.
        for rough in [0.05, 0.3, 0.6, 1.0] {
            for cos in [0.15, 0.5, 0.95] {
                let e = albedo(&Material::metal(Vec3::ONE, rough), cos, 16_384);
                assert!((e.x - 1.0).abs() < 0.03, "metal rough {rough} cos {cos}: {e}");
            }
        }
        let e = albedo(&Material::diffuse(Vec3::ONE), 0.7, 16_384);
        assert!((e.x - 1.0).abs() < 0.01, "{e}");
        // A white dielectric (specular over white diffuse) loses nothing to layering either.
        for rough in [0.1, 0.5, 1.0] {
            let e = albedo(&Material { base_color: Vec3::ONE, specular_roughness: rough, ..Material::default() }, 0.6, 16_384);
            assert!((e.x - 1.0).abs() < 0.03, "dielectric rough {rough}: {e}");
        }
    }

    #[test]
    fn sampled_weight_matches_eval_and_pdf_integrates_to_one() {
        let m = Material { base_color: Vec3::new(0.8, 0.4, 0.2), metalness: 0.3, specular_roughness: 0.4, ..Material::default() }.sanitized();
        let wo = Vec3A::new(0.6, 0.0, 0.8);
        let bsdf = Bsdf::new(&m, wo);
        for i in 0..256 {
            let mut s = Sampler::new(3, i);
            let lobe = s.next1();
            if let Some(smp) = bsdf.sample(lobe, s.next2()) {
                let (f, pdf) = bsdf.eval(smp.wi);
                assert!((pdf - smp.pdf).abs() <= 1e-4 * pdf.max(1.0));
                assert!((f / pdf - smp.weight).length() < 1e-3);
            }
        }
        // ∫ pdf dω over the hemisphere ≤ 1 (visible-normal reflections below the horizon are lost).
        let n = 256;
        let mut acc = 0.0f64;
        for a in 0..n {
            for b in 0..n {
                let u = Vec2::new((a as f32 + 0.5) / n as f32, (b as f32 + 0.5) / n as f32);
                let wi = crate::math::uniform_sphere(u);
                acc += f64::from(bsdf.eval(wi).1) * 4.0 * std::f64::consts::PI;
            }
        }
        let total = acc / f64::from(n * n);
        assert!(total > 0.9 && total < 1.01, "{total}");
    }

    #[test]
    fn reciprocity_of_the_microfacet_lobe() {
        // The specular lobe alone (a metal) is reciprocal: f(a→b)·cos_b / cos... compare f itself.
        let m = Material::metal(Vec3::new(0.9, 0.6, 0.3), 0.35).sanitized();
        let (a, b) = (Vec3A::new(0.3, 0.2, 0.93).normalize(), Vec3A::new(-0.5, 0.4, 0.77).normalize());
        // Compensation depends on the outgoing direction only; undo it to test the base lobe.
        let e = |w: Vec3A| {
            let (x, y) = AlbedoTable::get().lookup(w.z, 0.35);
            x + y
        };
        let ms = |w: Vec3A| Vec3A::ONE + Vec3A::new(0.9, 0.6, 0.3) * ((1.0 - e(w)) / e(w));
        let fab = Bsdf::new(&m, a).eval(b).0 / b.z / ms(a);
        let fba = Bsdf::new(&m, b).eval(a).0 / a.z / ms(b);
        assert!((fab - fba).length() < 1e-3 * fab.length().max(1.0), "{fab} {fba}");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn never_creates_energy(
            r in 0.0f32..1.0, g in 0.0f32..1.0, b in 0.0f32..1.0, metal in 0.0f32..1.0,
            rough in 0.0f32..1.0, ior in 1.0f32..3.0, sw in 0.0f32..1.0, cos in 0.05f32..1.0,
        ) {
            let m = Material { base_color: Vec3::new(r, g, b), metalness: metal, specular_roughness: rough, specular_ior: ior, specular_weight: sw, ..Material::default() };
            let e = albedo(&m, cos, 4096);
            prop_assert!(e.max_element() < 1.04, "{:?} cos {} → {}", m, cos, e);
            prop_assert!(e.min_element() >= 0.0);
        }
    }

    #[test]
    fn hostile_directions_do_not_produce_nan() {
        let m = Material::default().sanitized();
        for wo in [Vec3A::new(1.0, 0.0, 0.0), Vec3A::new(0.0, 0.0, -1.0), Vec3A::Z] {
            let bsdf = Bsdf::new(&m, wo);
            for wi in [Vec3A::Z, Vec3A::new(1.0, 0.0, 1e-9), -Vec3A::Z, Vec3A::new(-1.0, 0.0, 1e-7)] {
                let (f, pdf) = bsdf.eval(wi);
                assert!(f.is_finite() && pdf.is_finite(), "{wo} {wi}");
            }
            assert!(bsdf.sample(0.0, Vec2::ZERO).is_none_or(|s| s.weight.is_finite()));
            assert!(bsdf.sample(0.99, Vec2::splat(0.999)).is_none_or(|s| s.weight.is_finite()));
        }
    }
}
