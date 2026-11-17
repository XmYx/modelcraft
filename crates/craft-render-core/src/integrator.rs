//! The light transport estimator: unidirectional path tracing (Kajiya 1986) with next-event
//! estimation and multiple importance sampling (Veach and Guibas 1995), and Russian roulette
//! (Arvo and Kirk 1990).

use craft_render_bvh::Ray;
use glam::Vec3A;

use crate::bsdf::Bsdf;
use crate::light::LightSampling;
use crate::material::Material;
use crate::math::Frame;
use crate::math::{Rgb, power_heuristic, sanitize};
use crate::sampler::Sampler;
use crate::scene::{Scene, Surface};

/// Longest path the integrator follows, whatever the settings ask for.
pub const MAX_DEPTH: u32 = 1024;

/// Which estimators contribute direct light. `Mis` is the one to use; the other two exist so
/// tests can check that all three converge to the same image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Strategy {
    /// Light sampling and BSDF sampling, weighted by the power heuristic.
    #[default]
    Mis,
    /// Light sampling only.
    LightOnly,
    /// BSDF sampling only (lights without area are never found).
    BsdfOnly,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IntegratorSettings {
    /// Number of bounces after the camera ray (0 shows emitters only).
    pub max_depth: u32,
    /// Bounce from which paths are ended at random in proportion to their throughput.
    pub roulette_start: u32,
    /// Upper bound on one indirect sample's contribution; 0 disables it. Bounding trades a
    /// little energy for the removal of rare, very bright samples.
    pub clamp_indirect: f32,
    pub strategy: Strategy,
    /// How lights with a position are chosen for direct lighting.
    pub light_sampling: LightSampling,
}

impl Default for IntegratorSettings {
    fn default() -> Self {
        IntegratorSettings { max_depth: 12, roulette_start: 3, clamp_indirect: 0.0, strategy: Strategy::Mis, light_sampling: LightSampling::Tree }
    }
}

/// Radiance arriving along `ray` (one path).
pub fn radiance(scene: &Scene, mut ray: Ray, sampler: &mut Sampler, cfg: &IntegratorSettings) -> Rgb {
    let mut total = Vec3A::ZERO;
    let mut throughput = Vec3A::ONE;
    // Density of the direction that led here; infinite for the camera ray.
    let mut prev_pdf = f32::INFINITY;
    let max_depth = cfg.max_depth.min(MAX_DEPTH);
    let clamp = if cfg.clamp_indirect.is_finite() && cfg.clamp_indirect > 0.0 { cfg.clamp_indirect } else { f32::INFINITY };
    let mut add = |c: Rgb, depth: u32| {
        let c = sanitize(c);
        total += if depth > 0 { c.min(Vec3A::splat(clamp)) } else { c };
    };
    // Absorption of the object the path is inside, if any.
    let mut medium: Option<Vec3A> = None;
    // Cut-outs passed without spending a bounce (bounded, so a stack of them cannot loop forever).
    let mut passes = 0u32;
    let mut depth = 0u32;
    while depth <= max_depth {
        let Some(hit) = scene.accel.intersect(&ray) else {
            if scene.lights.has_environment() {
                let env = scene.lights.environment_radiance(ray.dir);
                let w = if prev_pdf.is_infinite() {
                    1.0
                } else {
                    match cfg.strategy {
                        Strategy::Mis => power_heuristic(prev_pdf, scene.lights.pdf_environment(ray.dir)),
                        Strategy::BsdfOnly => 1.0,
                        Strategy::LightOnly => 0.0,
                    }
                };
                add(throughput * env * w, depth);
            }
            break;
        };
        let Some(mut sf) = scene.surface(&hit, &ray) else { break };
        let Some(material) = scene.material(sf.material) else { break };
        if let Some(sigma) = medium {
            throughput *= Vec3A::new((-sigma.x * hit.t).exp(), (-sigma.y * hit.t).exp(), (-sigma.z * hit.t).exp());
        }
        if material.has_cutouts() && passes < 256 {
            let opacity = material.opacity(sf.uv, &scene.textures);
            if opacity < 1.0 && sampler.next1() >= opacity {
                passes += 1;
                ray = Scene::spawn(sf.p, sf.ng, ray.dir, f32::INFINITY);
                continue;
            }
        }
        if let Some(le) = scene.lights.emitted(hit.prim, sf.front) {
            let w = if prev_pdf.is_infinite() {
                1.0
            } else {
                match cfg.strategy {
                    Strategy::Mis => power_heuristic(prev_pdf, scene.lights.pdf_triangle(hit.prim, ray.origin, hit.t, sf.ng.dot(ray.dir), cfg.light_sampling)),
                    Strategy::BsdfOnly => 1.0,
                    Strategy::LightOnly => 0.0,
                }
            };
            add(throughput * le * w, depth);
        }
        if depth == max_depth {
            break;
        }
        shading_frame(scene, material, hit.prim, &mut sf);
        let transmissive = material.is_transmissive();
        let inside = transmissive && !sf.front;
        let bsdf = Bsdf::new(&material.params(sf.uv, &scene.textures), sf.frame.to_local(-ray.dir), inside);

        // Direct light from one chosen light.
        if cfg.strategy != Strategy::BsdfOnly && !scene.lights.is_empty() {
            let pick = sampler.next1();
            let u = sampler.next2();
            if let Some(ls) = scene.lights.sample(&scene.positions, sf.p, pick, u, cfg.light_sampling)
                && ls.pdf > 0.0
            {
                let wi = sf.frame.to_local(ls.wi);
                // The shading and the real surface must agree on which side the light is.
                let side = ls.wi.dot(sf.ng);
                if side * wi.z > 0.0 && (side > 0.0 || transmissive) {
                    let (f, pdf_bsdf) = bsdf.eval(wi);
                    if f.max_element() > 0.0 && !scene.occluded(sf.p, sf.ng, ls.wi, ls.distance) {
                        let w = if ls.delta || cfg.strategy == Strategy::LightOnly { 1.0 } else { power_heuristic(ls.pdf, pdf_bsdf) };
                        add(throughput * f * ls.radiance * (w / ls.pdf), depth + 1);
                    }
                }
            }
        }

        // Continue the path.
        let lobe = sampler.next1();
        let u = sampler.next2();
        let Some(bs) = bsdf.sample(lobe, u) else { break };
        let wi = sf.frame.to_world(bs.wi);
        let side = wi.dot(sf.ng);
        if side * bs.wi.z <= 0.0 {
            // The shading normal sent the direction to the other side of the real surface.
            break;
        }
        if side < 0.0 {
            // Crossed the surface: entering sets the object's absorption, leaving clears it.
            medium = if sf.front { Some(material.absorption()).filter(|s| s.max_element() > 0.0) } else { None };
        }
        throughput *= bs.weight;
        prev_pdf = bs.pdf;
        if depth >= cfg.roulette_start {
            let q = throughput.max_element().min(0.95);
            if q <= 0.0 || sampler.next1() >= q {
                break;
            }
            throughput /= q;
        }
        if !throughput.is_finite() {
            break;
        }
        ray = Scene::spawn(sf.p, sf.ng, wi, f32::INFINITY);
        depth += 1;
    }
    total
}

/// Apply the material's normal map, bump map and anisotropy direction to the shading frame.
#[inline]
fn shading_frame(scene: &Scene, m: &Material, tri: u32, sf: &mut Surface) {
    let needs_axes = m.normal_texture.is_some() || m.bump_texture.is_some() || m.specular_roughness_anisotropy > 0.0;
    if !needs_axes {
        return;
    }
    let Some((dpdu, dpdv)) = scene.uv_axes(tri) else { return };
    let mut n = sf.frame.n;
    let tangent = (dpdu - n * n.dot(dpdu)).normalize_or_zero();
    if tangent == Vec3A::ZERO {
        return;
    }
    let bitangent = n.cross(tangent) * if n.cross(tangent).dot(dpdv) < 0.0 { -1.0 } else { 1.0 };
    if let Some(img) = m.normal_texture.and_then(|t| scene.textures.get(t as usize)) {
        let s = img.sample(sf.uv.x, sf.uv.y, true);
        let (x, y, z) = ((s[0] * 2.0 - 1.0) * m.normal_scale, (s[1] * 2.0 - 1.0) * m.normal_scale, (s[2] * 2.0 - 1.0).max(1e-3));
        n = (tangent * x + bitangent * y + n * z).normalize_or(n);
    }
    if let Some(img) = m.bump_texture.and_then(|t| scene.textures.get(t as usize)) {
        // Finite differences of the height, one texel apart (Blinn 1978).
        let (du, dv) = (1.0 / img.width().max(1) as f32, 1.0 / img.height().max(1) as f32);
        let h = |u: f32, v: f32| img.sample_gray(u, v, true);
        let h0 = h(sf.uv.x, sf.uv.y);
        let dhdu = (h(sf.uv.x + du, sf.uv.y) - h0) / du * m.bump_scale;
        let dhdv = (h(sf.uv.x, sf.uv.y + dv) - h0) / dv * m.bump_scale;
        n = (n - dpdu * (dhdu / dpdu.length_squared()) - dpdv * (dhdv / dpdv.length_squared())).normalize_or(n);
    }
    if n.dot(sf.ng) <= 0.0 {
        // A map may not turn the surface away from the side it is seen from.
        n = (n - sf.ng * (n.dot(sf.ng) - 1e-3)).normalize_or(sf.ng);
    }
    let t = (tangent - n * n.dot(tangent)).normalize_or_zero();
    sf.frame = if t == Vec3A::ZERO { Frame::new(n) } else { Frame { t, b: n.cross(t), n } };
}
