//! The surface scattering model: the layered surface of the OpenPBR Surface specification
//! (v1.1), evaluated as a sum of lobes with albedo-scaled layering.
//!
//! From the top: a fuzz lobe, a coat, then the base, which mixes a metal with a dielectric whose
//! interface reflects and whose interior is either diffuse or transmissive.
//!
//! - Microfacets: GGX (Walter et al., "Microfacet Models for Refraction through Rough Surfaces",
//!   2007; Trowbridge and Reitz 1975), anisotropic, with the height-correlated Smith term (Heitz,
//!   JCGT 2014) and sampling of visible normals (Heitz, JCGT 2018). Refraction uses Walter's
//!   half-vector formulation.
//! - Fresnel: exact for dielectrics; Schlick with the "F82-tint" edge colour for metals (Kutz,
//!   Hasan and Edmondson, "Novel aspects of the Adobe Standard Material", 2021, as adopted by
//!   OpenPBR); the Airy formula of thin-film interference (Born and Wolf, *Principles of
//!   Optics*) at a few wavelengths per colour channel.
//! - Diffuse: Lambert, flattened with roughness by the Oren–Nayar model in Fujii's
//!   reformulation ("A tiny improvement of Oren–Nayar reflectance model"), the basis of OpenPBR's
//!   diffuse lobe.
//! - Fuzz: the inverted-Gaussian sheen distribution of Conty and Kulla ("Production Friendly
//!   Microfacet Sheen BRDF", 2017) with the visibility term of Neubelt and Pettineo (2013).
//! - Energy: reflective GGX lobes are scaled by `1 + F0 (1 − E) / E` (Turquin 2019, after Kulla
//!   and Conty 2017) to restore the energy single scattering loses; each layer passes
//!   `1 − E_layer(ωo)` to the layers below (albedo scaling, as the OpenPBR specification
//!   describes). The directional albedos `E` are tabulated once by numerical integration.
//!
//! Directions are in the shading frame (z is the normal, on the viewer's side) and point away
//! from the surface.

use std::f32::consts::PI;
use std::sync::OnceLock;

use glam::{Vec2, Vec3A};

use crate::material::SurfaceParams;
use crate::math::{INV_PI, Rgb, cosine_hemisphere, luminance, uniform_sphere};

/// Smallest microfacet width; smoother surfaces are treated as this sharp.
pub const ALPHA_MIN: f32 = 1e-3;
/// Resolution of the albedo tables (roughness × cosine).
pub const TABLE_SIZE: usize = 32;
/// Cosine at which the F82-tint model places the metal's edge colour (about 82°).
const MU_82: f32 = 1.0 / 7.0;

#[inline]
fn ggx_d(h: Vec3A, ax: f32, ay: f32) -> f32 {
    let d = h.x * h.x / (ax * ax) + h.y * h.y / (ay * ay) + h.z * h.z;
    1.0 / (PI * ax * ay * d * d)
}

#[inline]
fn ggx_lambda(w: Vec3A, ax: f32, ay: f32) -> f32 {
    let z2 = w.z * w.z;
    if z2 <= 0.0 {
        return f32::INFINITY;
    }
    let t2 = (ax * ax * w.x * w.x + ay * ay * w.y * w.y) / z2;
    ((1.0 + t2).sqrt() - 1.0) * 0.5
}

/// A microfacet normal from the distribution of normals visible from `wo`.
#[inline]
fn sample_visible_normal(wo: Vec3A, ax: f32, ay: f32, u: Vec2) -> Vec3A {
    let vh = Vec3A::new(ax * wo.x, ay * wo.y, wo.z).normalize();
    let len2 = vh.x * vh.x + vh.y * vh.y;
    let t1 = if len2 > 0.0 { Vec3A::new(-vh.y, vh.x, 0.0) / len2.sqrt() } else { Vec3A::X };
    let t2 = vh.cross(t1);
    let r = u.x.sqrt();
    let phi = 2.0 * PI * u.y;
    let p1 = r * phi.cos();
    let s = 0.5 * (1.0 + vh.z);
    let p2 = (1.0 - s) * (1.0 - p1 * p1).max(0.0).sqrt() + s * r * phi.sin();
    let nh = t1 * p1 + t2 * p2 + vh * (1.0 - p1 * p1 - p2 * p2).max(0.0).sqrt();
    Vec3A::new(ax * nh.x, ay * nh.y, nh.z.max(1e-6)).normalize()
}

#[inline]
fn schlick_weight(cos: f32) -> f32 {
    let m = (1.0 - cos).clamp(0.0, 1.0);
    let m2 = m * m;
    m2 * m2 * m
}

/// The F82-tint edge term `μ (1 − μ)^6`.
#[inline]
fn edge_weight(cos: f32) -> f32 {
    let c = cos.clamp(0.0, 1.0);
    let m = 1.0 - c;
    let m2 = m * m;
    c * m2 * m2 * m2
}

/// Unpolarized Fresnel reflectance of a dielectric interface; `eta` is the index on the far
/// side over the index on the near side. 1 under total internal reflection.
#[inline]
pub fn fresnel_dielectric(cos_i: f32, eta: f32) -> f32 {
    let c = cos_i.clamp(0.0, 1.0);
    let s2 = (1.0 - c * c) / (eta * eta);
    if s2 >= 1.0 {
        return 1.0;
    }
    let ct = (1.0 - s2).sqrt();
    let rs = (c - eta * ct) / (c + eta * ct);
    let rp = (eta * c - ct) / (eta * c + ct);
    0.5 * (rs * rs + rp * rp)
}

/// Reflectance of a base of index `n_base` under a film of index `n_film` and `thickness`
/// micrometres, seen from air: the Airy formula, averaged over polarization and over three
/// wavelengths per channel.
pub fn thin_film_reflectance(cos_i: f32, n_film: f32, n_base: f32, thickness: f32) -> Rgb {
    const WAVELENGTHS: [[f32; 3]; 3] = [[0.580, 0.610, 0.640], [0.520, 0.545, 0.570], [0.440, 0.460, 0.480]];
    let c1 = cos_i.clamp(1e-4, 1.0);
    let s1 = (1.0 - c1 * c1).max(0.0);
    let (s2, s3) = (s1 / (n_film * n_film), s1 / (n_base * n_base));
    if s2 >= 1.0 || s3 >= 1.0 {
        return Vec3A::ONE;
    }
    let (c2, c3) = ((1.0 - s2).sqrt(), (1.0 - s3).sqrt());
    let amp = |na: f32, ca: f32, nb: f32, cb: f32| ((na * ca - nb * cb) / (na * ca + nb * cb), (nb * ca - na * cb) / (nb * ca + na * cb));
    let (r12s, r12p) = amp(1.0, c1, n_film, c2);
    let (r23s, r23p) = amp(n_film, c2, n_base, c3);
    let path = 4.0 * PI * n_film * thickness * c2;
    let mut out = [0.0f32; 3];
    for (o, lambdas) in out.iter_mut().zip(WAVELENGTHS) {
        for lambda in lambdas {
            let cp = (path / lambda).cos();
            for (a, b) in [(r12s, r23s), (r12p, r23p)] {
                *o += (a * a + b * b + 2.0 * a * b * cp) / (1.0 + a * a * b * b + 2.0 * a * b * cp);
            }
        }
        *o /= 6.0;
    }
    Vec3A::from_array(out).clamp(Vec3A::ZERO, Vec3A::ONE)
}

#[inline]
fn fuzz_alpha(roughness: f32) -> f32 {
    roughness.clamp(0.05, 1.0)
}

/// The fuzz lobe without colour or weight: `D · V`.
#[inline]
fn fuzz_dv(wo: Vec3A, wi: Vec3A, alpha: f32) -> f32 {
    let h = (wo + wi).normalize_or_zero();
    let sin2 = (1.0 - h.z * h.z).max(0.0);
    let inv = 1.0 / alpha;
    let d = (2.0 + inv) * sin2.powf(0.5 * inv) / (2.0 * PI);
    d / (4.0 * (wi.z + wo.z - wi.z * wo.z)).max(1e-4)
}

/// Directional albedos, tabulated over roughness and cosine.
pub struct AlbedoTable {
    /// `[roughness][cosine]` → `[a, b, c, fuzz]`: the single-scattering GGX lobe's albedo split
    /// by Fresnel weight (`E = F0·a + F90·b − edge·c`), and the albedo of the fuzz lobe.
    pub ab: Vec<[f32; 4]>,
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
                let (mut a, mut b, mut c, mut fuzz) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
                let lo = ggx_lambda(wo, alpha, alpha);
                for k in 0..SAMPLES {
                    let u = Vec2::new((k as f32 + 0.5) / SAMPLES as f32, (k.reverse_bits() >> 8) as f32 / 16_777_216.0);
                    let h = sample_visible_normal(wo, alpha, alpha, u);
                    let wi = h * (2.0 * wo.dot(h)) - wo;
                    if wi.z > 0.0 {
                        let w = (1.0 + lo) / (1.0 + lo + ggx_lambda(wi, alpha, alpha));
                        let fc = schlick_weight(wo.dot(h));
                        a += f64::from(w * (1.0 - fc));
                        b += f64::from(w * fc);
                        c += f64::from(w * edge_weight(wo.dot(h)));
                    }
                    let wf = uniform_sphere(u);
                    let wf = Vec3A::new(wf.x, wf.y, wf.z.abs());
                    fuzz += f64::from(fuzz_dv(wo, wf, fuzz_alpha(rough)) * wf.z * 2.0 * PI);
                }
                let n = f64::from(SAMPLES);
                ab.push([(a / n) as f32, (b / n) as f32, (c / n) as f32, (fuzz / n) as f32]);
            }
        }
        AlbedoTable { ab }
    }

    /// The shared table (built once, in a few milliseconds).
    pub fn get() -> &'static AlbedoTable {
        static TABLE: OnceLock<AlbedoTable> = OnceLock::new();
        TABLE.get_or_init(AlbedoTable::build)
    }

    /// `[a, b, c, fuzz]` at a cosine and a roughness, bilinearly interpolated.
    #[inline]
    pub fn lookup(&self, cos: f32, roughness: f32) -> [f32; 4] {
        let m = (TABLE_SIZE - 1) as f32;
        let x = cos.clamp(0.0, 1.0) * m;
        let y = roughness.clamp(0.0, 1.0) * m;
        let (x0, y0) = ((x as usize).min(TABLE_SIZE - 2), (y as usize).min(TABLE_SIZE - 2));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |xi: usize, yi: usize| self.ab.get(yi * TABLE_SIZE + xi).copied().unwrap_or([0.0; 4]);
        let (p00, p10, p01, p11) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
        std::array::from_fn(|k| (p00[k] * (1.0 - fx) + p10[k] * fx) * (1.0 - fy) + (p01[k] * (1.0 - fx) + p11[k] * fx) * fy)
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

const FUZZ: usize = 0;
const COAT: usize = 1;
const SPEC: usize = 2;
const DIFFUSE: usize = 3;
const TRANSMIT: usize = 4;

/// A material resolved at one point for one outgoing direction.
#[derive(Clone, Copy, Debug)]
pub struct Bsdf {
    wo: Vec3A,
    ax: f32,
    ay: f32,
    /// Metal: Schlick from `metal_f0` to white minus the edge term, times `metal_scale`.
    metal_f0: Rgb,
    metal_edge: Rgb,
    metal_scale: Rgb,
    /// Dielectric interface: exact Fresnel (or the thin film), times `diel_scale`.
    diel_scale: Rgb,
    /// Index on the far side of the interface over the index on the viewer's side.
    eta: f32,
    /// Thin film: weight, thickness, film index, base index.
    film: [f32; 4],
    diffuse: Rgb,
    sigma: f32,
    transmit: Rgb,
    coat: f32,
    coat_f0: f32,
    coat_alpha: f32,
    fuzz: Rgb,
    fuzz_alpha: f32,
    /// Probability of sampling each lobe.
    p: [f32; 5],
}

impl Bsdf {
    /// `wo` is the direction to the viewer in the shading frame, `wo.z > 0`. `inside` says the
    /// viewer is inside a transmissive object, looking at its surface from within: only the
    /// dielectric interface exists there.
    pub fn new(m: &SurfaceParams, wo: Vec3A, inside: bool) -> Bsdf {
        let wo = if wo.z > 1e-4 { wo } else { Vec3A::new(wo.x, wo.y, 1e-4).normalize() };
        let table = AlbedoTable::get();
        let alpha = (m.roughness * m.roughness).max(ALPHA_MIN);
        let stretch = (2.0 / (1.0 + (1.0 - m.anisotropy) * (1.0 - m.anisotropy))).sqrt();
        let ax = (alpha * stretch).min(1.0);
        let ay = ((1.0 - m.anisotropy) * ax).max(ALPHA_MIN);
        let mut eta = if inside { 1.0 / m.ior } else { m.ior };
        if (eta - 1.0).abs() < 1e-4 {
            // No interface bends nothing: keep the half-vector mapping well defined.
            eta = 1.0001;
        }
        let mut b = Bsdf {
            wo,
            ax,
            ay,
            metal_f0: Vec3A::ZERO,
            metal_edge: Vec3A::ZERO,
            metal_scale: Vec3A::ZERO,
            diel_scale: Vec3A::ONE,
            eta,
            film: [0.0; 4],
            diffuse: Vec3A::ZERO,
            sigma: 0.0,
            transmit: Vec3A::ONE,
            coat: 0.0,
            coat_f0: 0.0,
            coat_alpha: ALPHA_MIN,
            fuzz: Vec3A::ZERO,
            fuzz_alpha: 1.0,
            p: [0.0; 5],
        };
        if inside {
            let f = fresnel_dielectric(wo.z, eta).clamp(0.1, 0.9);
            b.p[SPEC] = f;
            b.p[TRANSMIT] = 1.0 - f;
            return b;
        }
        // The albedo tables are isotropic; a stretched lobe is looked up at the roughness whose
        // width is the geometric mean of its two widths.
        let [a, bb, c, _] = table.lookup(wo.z, (ax * ay).sqrt().sqrt());
        let e = (a + bb).clamp(1e-4, 1.0);
        let k = (1.0 - e) / e;

        // Fuzz on top.
        b.fuzz_alpha = fuzz_alpha(m.fuzz_roughness);
        let e_fuzz_lobe = table.lookup(wo.z, m.fuzz_roughness)[3];
        let fuzz_norm = 1.0 / e_fuzz_lobe.max(1.0);
        let e_fuzz = (m.fuzz * e_fuzz_lobe * fuzz_norm).clamp(0.0, 1.0);
        b.fuzz = m.fuzz_color * (m.fuzz * fuzz_norm);
        let under_fuzz = 1.0 - e_fuzz;

        // Coat.
        let rc = (m.coat_ior - 1.0) / (m.coat_ior + 1.0);
        b.coat_f0 = rc * rc;
        b.coat_alpha = (m.coat_roughness * m.coat_roughness).max(ALPHA_MIN);
        let [ca, cb, _, _] = table.lookup(wo.z, m.coat_roughness);
        let ce = (ca + cb).clamp(1e-4, 1.0);
        let coat_ms = 1.0 + b.coat_f0 * (1.0 - ce) / ce;
        let e_coat = (m.coat * (b.coat_f0 * ca + cb) * coat_ms).clamp(0.0, 1.0);
        b.coat = under_fuzz * m.coat * coat_ms;
        let base_scale = (Vec3A::ONE + (m.coat_color - Vec3A::ONE) * m.coat) * (under_fuzz * (1.0 - e_coat));

        // Metal.
        let (mt, dl) = (m.metalness, 1.0 - m.metalness);
        let metal_ms = Vec3A::ONE + m.base * k;
        let schlick_82 = m.base + (Vec3A::ONE - m.base) * schlick_weight(MU_82);
        let tint = m.specular_color * m.specular_weight + Vec3A::splat(1.0 - m.specular_weight);
        b.metal_f0 = m.base;
        b.metal_edge = schlick_82 * (Vec3A::ONE - tint) / edge_weight(MU_82);
        b.metal_scale = metal_ms * base_scale * mt;
        let metal_e = ((m.base * a + Vec3A::splat(bb) - b.metal_edge * c) * metal_ms).clamp(Vec3A::ZERO, Vec3A::ONE);

        // Dielectric interface.
        let r = (m.ior - 1.0) / (m.ior + 1.0);
        let mut d0 = r * r;
        if m.thin_film > 0.0 {
            b.film = [m.thin_film, m.thin_film_thickness, m.thin_film_ior, m.ior];
            let film0 = luminance(thin_film_reflectance(1.0, m.thin_film_ior, m.ior, m.thin_film_thickness));
            d0 += (film0 - d0) * m.thin_film;
        }
        let d0 = d0 * m.specular_weight;
        let diel_ms = 1.0 + d0 * k;
        let diel_e = ((d0 * a + m.specular_weight * bb) * diel_ms).clamp(0.0, 1.0);
        b.diel_scale = m.specular_color * base_scale * (m.specular_weight * diel_ms * dl);

        // Under the interface: diffuse or transmission.
        let below = base_scale * (dl * (1.0 - diel_e));
        b.diffuse = m.base * below * (1.0 - m.transmission);
        b.sigma = m.diffuse_roughness;
        b.transmit = m.transmission_tint * base_scale * (dl * m.transmission);

        let scale = luminance(base_scale);
        let mut w = [0.0f32; 5];
        w[FUZZ] = e_fuzz * luminance(m.fuzz_color);
        w[COAT] = under_fuzz * e_coat;
        w[SPEC] = scale * (mt * luminance(metal_e) + dl * luminance(m.specular_color) * diel_e);
        w[DIFFUSE] = luminance(b.diffuse);
        w[TRANSMIT] = luminance(b.transmit) * (1.0 - diel_e);
        if w[TRANSMIT] > 0.0 {
            // Glass seen at a grazing angle is mostly mirror; keep that lobe well sampled.
            w[SPEC] = w[SPEC].max(0.1 * w[TRANSMIT]);
        }
        let total: f32 = w.iter().sum();
        if total > 0.0 {
            b.p = w.map(|x| x / total);
        }
        b
    }

    #[inline]
    fn dielectric_fresnel(&self, cos: f32) -> Rgb {
        let exact = Vec3A::splat(fresnel_dielectric(cos, self.eta));
        if self.film[0] > 0.0 { exact + (thin_film_reflectance(cos, self.film[2], self.film[3], self.film[1]) - exact) * self.film[0] } else { exact }
    }

    /// `(f · |cos θi|, pdf)` for the incoming direction `wi` (below the surface for transmission).
    #[inline]
    pub fn eval(&self, wi: Vec3A) -> (Rgb, f32) {
        let wo = self.wo;
        let mut f = Vec3A::ZERO;
        let mut pdf = 0.0;
        if wi.z > 0.0 {
            let cos_pdf = wi.z * INV_PI;
            if self.fuzz.max_element() > 0.0 {
                f += self.fuzz * (fuzz_dv(wo, wi, self.fuzz_alpha) * wi.z);
                pdf += self.p[FUZZ] * cos_pdf;
            }
            if self.diffuse.max_element() > 0.0 {
                let mut shape = 1.0;
                if self.sigma > 0.0 {
                    let a = 1.0 / (1.0 + (0.5 - 2.0 / (3.0 * PI)) * self.sigma);
                    let s = wi.dot(wo) - wi.z * wo.z;
                    let t = if s > 0.0 { wi.z.max(wo.z) } else { 1.0 };
                    shape = a * (1.0 + self.sigma * s / t);
                }
                f += self.diffuse * (cos_pdf * shape);
                pdf += self.p[DIFFUSE] * cos_pdf;
            }
            let h = (wo + wi).normalize_or_zero();
            if h.z > 0.0 {
                let cos_h = wo.dot(h);
                if self.coat > 0.0 {
                    let (a, lo) = (self.coat_alpha, ggx_lambda(wo, self.coat_alpha, self.coat_alpha));
                    let dv = ggx_d(h, a, a) / (4.0 * wo.z);
                    if dv.is_finite() {
                        let fr = self.coat_f0 + (1.0 - self.coat_f0) * schlick_weight(cos_h);
                        f += Vec3A::splat(self.coat * fr * dv / (1.0 + lo + ggx_lambda(wi, a, a)));
                        pdf += self.p[COAT] * dv / (1.0 + lo);
                    }
                }
                if self.p[SPEC] > 0.0 {
                    let lo = ggx_lambda(wo, self.ax, self.ay);
                    let dv = ggx_d(h, self.ax, self.ay) / (4.0 * wo.z);
                    if dv.is_finite() {
                        let mut fr = self.diel_scale * self.dielectric_fresnel(cos_h);
                        if self.metal_scale.max_element() > 0.0 {
                            let metal = self.metal_f0 + (Vec3A::ONE - self.metal_f0) * schlick_weight(cos_h) - self.metal_edge * edge_weight(cos_h);
                            fr += self.metal_scale * metal.max(Vec3A::ZERO);
                        }
                        f += fr * (dv / (1.0 + lo + ggx_lambda(wi, self.ax, self.ay)));
                        pdf += self.p[SPEC] * dv / (1.0 + lo);
                    }
                }
            }
        } else if wi.z < 0.0 && self.p[TRANSMIT] > 0.0 {
            // Refraction: the half vector that bends `wo` into `wi` (Walter et al. 2007, eq. 16).
            let mut h = -(wo + wi * self.eta).normalize_or_zero();
            if h.z < 0.0 {
                h = -h;
            }
            let (co, ci) = (wo.dot(h), wi.dot(h));
            if h.z > 0.0 && co > 0.0 && ci < 0.0 {
                let denom = co + self.eta * ci;
                let jac = self.eta * self.eta * ci.abs() / (denom * denom);
                let lo = ggx_lambda(wo, self.ax, self.ay);
                let dvis = ggx_d(h, self.ax, self.ay) * co / (wo.z * (1.0 + lo));
                let g_ratio = (1.0 + lo) / (1.0 + lo + ggx_lambda(wi, self.ax, self.ay));
                let through = Vec3A::ONE - self.dielectric_fresnel(co);
                let v = dvis * jac;
                if v.is_finite() {
                    f += self.transmit * through * (v * g_ratio);
                    pdf += self.p[TRANSMIT] * v;
                }
            }
        }
        (f, pdf)
    }

    /// Draw an incoming direction. `lobe` chooses the lobe and `u` the direction within it.
    #[inline]
    pub fn sample(&self, lobe: f32, u: Vec2) -> Option<BsdfSample> {
        let mut acc = 0.0;
        let mut pick = DIFFUSE;
        for (i, p) in self.p.iter().enumerate() {
            acc += p;
            if *p > 0.0 {
                pick = i;
                if lobe < acc {
                    break;
                }
            }
        }
        let wo = self.wo;
        let wi = match pick {
            COAT => {
                let h = sample_visible_normal(wo, self.coat_alpha, self.coat_alpha, u);
                h * (2.0 * wo.dot(h)) - wo
            }
            SPEC => {
                let h = sample_visible_normal(wo, self.ax, self.ay, u);
                h * (2.0 * wo.dot(h)) - wo
            }
            TRANSMIT => {
                let h = sample_visible_normal(wo, self.ax, self.ay, u);
                let c = wo.dot(h);
                let k = 1.0 - (1.0 - c * c) / (self.eta * self.eta);
                if k <= 0.0 {
                    // Total internal reflection: this microfacet only reflects.
                    return None;
                }
                (-wo / self.eta + h * (c / self.eta - k.sqrt())).normalize()
            }
            _ => cosine_hemisphere(u),
        };
        let (f, pdf) = self.eval(wi);
        (pdf > 0.0 && f.is_finite()).then(|| BsdfSample { wi, weight: f / pdf, pdf })
    }

    /// Probability that [`Bsdf::sample`] draws from a wide lobe (diffuse or fuzz), where a
    /// learned distribution of incoming light can help.
    pub fn wide_probability(&self) -> f32 {
        self.p[DIFFUSE] + self.p[FUZZ]
    }

    /// The largest share of light the wide lobes send back (their albedo, at most 1).
    pub fn wide_albedo(&self) -> f32 {
        (self.diffuse + self.fuzz).max_element().clamp(0.0, 1.0)
    }

    /// The outgoing direction this was resolved for.
    pub fn wo(&self) -> Vec3A {
        self.wo
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::Material;
    use crate::sampler::Sampler;
    use glam::Vec3;
    use proptest::prelude::*;

    fn albedo(m: &Material, cos: f32, n: u32) -> Rgb {
        let wo = Vec3A::new((1.0 - cos * cos).sqrt(), 0.0, cos);
        let bsdf = Bsdf::new(&m.sanitized().params(Vec2::ZERO, &[]), wo, false);
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
        let m = Material { base_color: Vec3::new(0.8, 0.4, 0.2), base_metalness: 0.3, specular_roughness: 0.4, ..Material::default() }.sanitized();
        let wo = Vec3A::new(0.6, 0.0, 0.8);
        let bsdf = Bsdf::new(&m.params(Vec2::ZERO, &[]), wo, false);
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
            let t = AlbedoTable::get().lookup(w.z, 0.35);
            t[0] + t[1]
        };
        let ms = |w: Vec3A| Vec3A::ONE + Vec3A::new(0.9, 0.6, 0.3) * ((1.0 - e(w)) / e(w));
        let p = m.params(Vec2::ZERO, &[]);
        let fab = Bsdf::new(&p, a, false).eval(b).0 / b.z / ms(a);
        let fba = Bsdf::new(&p, b, false).eval(a).0 / a.z / ms(b);
        assert!((fab - fba).length() < 1e-3 * fab.length().max(1.0), "{fab} {fba}");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn never_creates_energy(
            r in 0.0f32..1.0, g in 0.0f32..1.0, b in 0.0f32..1.0, metal in 0.0f32..1.0,
            rough in 0.0f32..1.0, ior in 1.0f32..3.0, sw in 0.0f32..1.0, cos in 0.05f32..1.0,
            coat in 0.0f32..1.0, fuzz in 0.0f32..1.0, trans in 0.0f32..1.0, aniso in 0.0f32..1.0, film in 0.0f32..1.0,
        ) {
            let m = Material {
                base_color: Vec3::new(r, g, b), base_metalness: metal, specular_roughness: rough, specular_ior: ior, specular_weight: sw,
                coat_weight: coat, coat_roughness: rough * 0.5, fuzz_weight: fuzz, fuzz_roughness: rough, transmission_weight: trans,
                specular_roughness_anisotropy: aniso, base_diffuse_roughness: rough, thin_film_weight: film, specular_color: Vec3::new(b, r, g),
                ..Material::default()
            };
            let e = albedo(&m, cos, 4096);
            prop_assert!(e.max_element() < 1.06, "{:?} cos {} → {}", m, cos, e);
            prop_assert!(e.min_element() >= 0.0);
        }
    }

    #[test]
    fn hostile_directions_do_not_produce_nan() {
        let m = Material { coat_weight: 1.0, fuzz_weight: 1.0, transmission_weight: 0.5, thin_film_weight: 1.0, ..Material::default() }
            .sanitized()
            .params(Vec2::ZERO, &[]);
        for wo in [Vec3A::new(1.0, 0.0, 0.0), Vec3A::new(0.0, 0.0, -1.0), Vec3A::Z] {
            for inside in [false, true] {
                let bsdf = Bsdf::new(&m, wo, inside);
                for wi in [Vec3A::Z, Vec3A::new(1.0, 0.0, 1e-9), -Vec3A::Z, Vec3A::new(-1.0, 0.0, 1e-7), Vec3A::new(0.3, 0.0, -0.9)] {
                    let (f, pdf) = bsdf.eval(wi);
                    assert!(f.is_finite() && pdf.is_finite(), "{wo} {wi}");
                }
                assert!(bsdf.sample(0.0, Vec2::ZERO).is_none_or(|s| s.weight.is_finite()));
                assert!(bsdf.sample(0.99, Vec2::splat(0.999)).is_none_or(|s| s.weight.is_finite()));
            }
        }
    }

    #[test]
    fn layers_conserve_energy_and_glass_passes_light() {
        // Every layer on a white base, at several angles: nothing is created.
        for cos in [0.2, 0.6, 0.95] {
            for m in [
                Material { base_color: Vec3::ONE, coat_weight: 1.0, coat_roughness: 0.1, ..Material::default() },
                Material { base_color: Vec3::ONE, fuzz_weight: 1.0, fuzz_roughness: 0.3, ..Material::default() },
                Material { base_color: Vec3::ONE, thin_film_weight: 1.0, thin_film_thickness: 0.4, ..Material::default() },
                Material { base_color: Vec3::ONE, base_diffuse_roughness: 1.0, specular_weight: 0.0, ..Material::default() },
                Material { specular_roughness_anisotropy: 0.8, ..Material::metal(Vec3::ONE, 0.4) },
            ] {
                let e = albedo(&m, cos, 16_384);
                assert!(e.max_element() < 1.04 && e.min_element() > 0.5, "{m:?} at {cos}: {e}");
            }
            // Smooth clear glass: reflection plus refraction returns everything.
            let e = albedo(&Material::glass(1.5, 0.05), cos, 16_384);
            assert!((e.x - 1.0).abs() < 0.03, "glass at {cos}: {e}");
        }
        // From inside, beyond the critical angle, all light is reflected back.
        let p = Material::glass(1.5, 0.0).sanitized().params(Vec2::ZERO, &[]);
        let inside = Bsdf::new(&p, Vec3A::new(0.9, 0.0, (1.0f32 - 0.81).sqrt()), true);
        let mut reflected = 0.0;
        for i in 0..1024 {
            let mut s = Sampler::new(5, i);
            let lobe = s.next1();
            if let Some(smp) = inside.sample(lobe, s.next2()) {
                assert!(smp.wi.z > 0.0, "refracted beyond the critical angle");
                reflected += smp.weight.x;
            }
        }
        assert!((reflected / 1024.0 - 1.0).abs() < 0.05, "{}", reflected / 1024.0);
    }

    #[test]
    fn fresnel_and_thin_film_follow_optics() {
        // Normal incidence on glass: ((n−1)/(n+1))² = 4 %.
        assert!((fresnel_dielectric(1.0, 1.5) - 0.04).abs() < 1e-4);
        assert!((fresnel_dielectric(1e-4, 1.5) - 1.0).abs() < 1e-2);
        // Total internal reflection inside glass beyond asin(1/1.5) ≈ 41.8°.
        assert_eq!(fresnel_dielectric(0.7, 1.0 / 1.5), 1.0);
        assert!(fresnel_dielectric(0.8, 1.0 / 1.5) < 0.2);
        // A film of zero thickness is no film; a quarter-wave film of index √n is anti-reflective
        // at its design wavelength (green here), so green drops well below bare glass.
        let bare = thin_film_reflectance(1.0, 1.4, 1.5, 0.0);
        assert!((bare.y - 0.04).abs() < 2e-3, "{bare}");
        let n_film = 1.5f32.sqrt();
        let coated = thin_film_reflectance(1.0, n_film, 1.5, 0.545 / (4.0 * n_film));
        assert!(coated.y < 0.004 && coated.x > coated.y && coated.z > coated.y, "{coated}");
        // A thicker film of higher index colours the reflection: the channels differ.
        let soap = thin_film_reflectance(0.8, 2.0, 1.5, 0.3);
        assert!((soap.x - soap.z).abs() > 0.02, "{soap}");
    }
}
