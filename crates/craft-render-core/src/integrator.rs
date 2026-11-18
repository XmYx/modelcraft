//! The light transport estimator: unidirectional path tracing (Kajiya 1986) with next-event
//! estimation and multiple importance sampling (Veach and Guibas 1995), and Russian roulette
//! (Arvo and Kirk 1990).

use craft_render_bvh::Ray;
use glam::Vec3A;

use crate::bsdf::Bsdf;
use crate::hair::{CurveSet, FibreHit, HairBsdf, fibre_frame, leave_fibre};
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
    /// Learn where light comes from while rendering and send part of the paths that way (path
    /// guiding; see [`crate::guided`]). Off by default; the GPU path does not have it.
    pub guiding: bool,
    /// End or multiply paths by what they are expected to add to their pixel, from a radiance
    /// cache filled while rendering, instead of by throughput alone (see [`crate::guided`]).
    pub adaptive_splitting: bool,
    /// Resampled direct lighting at the first surface (see [`crate::restir`]).
    pub restir: crate::restir::RestirSettings,
}

impl Default for IntegratorSettings {
    fn default() -> Self {
        IntegratorSettings {
            max_depth: 12,
            roulette_start: 3,
            clamp_indirect: 0.0,
            strategy: Strategy::Mis,
            light_sampling: LightSampling::Tree,
            guiding: false,
            adaptive_splitting: false,
            restir: crate::restir::RestirSettings::default(),
        }
    }
}

/// What one path saw at its first surface, for the feature images a denoiser reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct Features {
    /// Base colour of the first surface (the environment's colour where nothing was hit).
    pub albedo: Rgb,
    /// Shading normal of the first surface in world space (zero where nothing was hit).
    pub normal: Vec3A,
    /// Distance from the camera to the first surface (zero where nothing was hit).
    pub depth: f32,
    /// World position of the first surface, if any.
    pub position: Option<Vec3A>,
}

/// Radiance arriving along `ray` (one path).
pub fn radiance(scene: &Scene, ray: Ray, sampler: &mut Sampler, cfg: &IntegratorSettings) -> Rgb {
    radiance_and_features(scene, ray, sampler, cfg).0
}

/// [`radiance`] together with the features of the first surface the path met.
pub fn radiance_and_features(scene: &Scene, ray: Ray, sampler: &mut Sampler, cfg: &IntegratorSettings) -> (Rgb, Features) {
    radiance_with(scene, ray, sampler, cfg, None)
}

/// Direct light at the first surface, supplied by the caller instead of next-event estimation:
/// it receives the surface, its BSDF and whether the object is transmissive, and returns the
/// light the surface sends back (multiple importance weight included).
pub type PrimaryDirect<'a> = &'a mut dyn FnMut(&Surface, &Bsdf, bool) -> Rgb;

/// [`radiance_and_features`] with the direct light at the first surface taken from `primary`
/// when there is one (indirect bounces always use next-event estimation).
pub fn radiance_with(scene: &Scene, mut ray: Ray, sampler: &mut Sampler, cfg: &IntegratorSettings, mut primary: Option<PrimaryDirect<'_>>) -> (Rgb, Features) {
    let mut features = Features::default();
    let mut first = true;
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
        let tri = scene.accel.intersect(&ray);
        if !scene.curves.is_empty()
            && let Some((set, fh)) = scene.intersect_curves(&ray, tri.map_or(f32::INFINITY, |h| h.t))
        {
            let Some(curves) = scene.curves.get(set) else { break };
            if first {
                first = false;
                let albedo = curves.params.sigma_a.to_array().map(|s| (-s).exp());
                features = Features { albedo: Vec3A::from_array(albedo), normal: fh.frame.normal, depth: fh.t, position: Some(fh.p) };
            }
            if depth == max_depth {
                break;
            }
            let step = fibre_step(scene, curves, &fh, &ray, sampler, cfg);
            add(throughput * step.direct, depth + 1);
            let Some((next, weight, pdf)) = step.next else { break };
            throughput *= weight;
            prev_pdf = pdf;
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
            ray = next;
            depth += 1;
            continue;
        }
        let Some(hit) = tri else {
            if first {
                features.albedo = scene.lights.environment_radiance(ray.dir).min(Vec3A::ONE);
            }
            let (sun, sun_pdf) = scene.lights.sun_radiance(ray.dir);
            if sun_pdf > 0.0 {
                // The path found a sun's disk by itself; light sampling could have aimed at it too.
                let w = if prev_pdf.is_infinite() {
                    1.0
                } else {
                    match cfg.strategy {
                        Strategy::Mis => power_heuristic(prev_pdf, sun_pdf),
                        Strategy::BsdfOnly => 1.0,
                        Strategy::LightOnly => 0.0,
                    }
                };
                add(throughput * sun * w, depth);
            }
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
            if first {
                features = Features { albedo: Vec3A::from(material.base_color), normal: sf.frame.n, depth: hit.t, position: Some(sf.p) };
            }
            break;
        }
        shading_frame(scene, material, hit.prim, &mut sf);
        let transmissive = material.is_transmissive();
        let inside = transmissive && !sf.front;
        let params = material.params(sf.uv, &scene.textures);
        if first {
            first = false;
            features = Features { albedo: params.base, normal: sf.frame.n, depth: hit.t, position: Some(sf.p) };
        }
        let bsdf = Bsdf::new(&params, sf.frame.to_local(-ray.dir), inside);

        // Direct light from one chosen light.
        if let (0, Some(direct)) = (depth, primary.as_mut()) {
            let c = direct(&sf, &bsdf, transmissive);
            add(throughput * c, depth + 1);
        } else if cfg.strategy != Strategy::BsdfOnly && !scene.lights.is_empty() {
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
    (total, features)
}

/// What one fibre hit contributes: direct light already weighted, and the continuing ray with the
/// throughput factor and density of the direction chosen.
struct FibreStep {
    direct: Rgb,
    next: Option<(Ray, Rgb, f32)>,
}

/// One path vertex on a hair fibre: next-event estimation by the lights, then a sampled direction.
fn fibre_step(scene: &Scene, curves: &CurveSet, fh: &FibreHit, ray: &Ray, sampler: &mut Sampler, cfg: &IntegratorSettings) -> FibreStep {
    let to_viewer = -ray.dir;
    let (t, n, w) = fibre_frame(&fh.frame, to_viewer);
    let local = |v: Vec3A| Vec3A::new(v.dot(t), v.dot(n), v.dot(w));
    let bsdf = HairBsdf::new(&curves.params, local(to_viewer));
    let mut direct = Vec3A::ZERO;
    if cfg.strategy != Strategy::BsdfOnly && !scene.lights.is_empty() {
        let pick = sampler.next1();
        let u = sampler.next2();
        if let Some(ls) = scene.lights.sample(&scene.positions, fh.p, pick, u, cfg.light_sampling)
            && ls.pdf > 0.0
        {
            let (f, pdf_bsdf) = bsdf.eval(local(ls.wi));
            if f.max_element() > 0.0 && !scene.occluded_from_fibre(fh, ls.wi, ls.distance) {
                let weight = if ls.delta || cfg.strategy == Strategy::LightOnly { 1.0 } else { power_heuristic(ls.pdf, pdf_bsdf) };
                direct = sanitize(f * ls.radiance * (weight / ls.pdf));
            }
        }
    }
    let lobe = sampler.next1();
    let u = sampler.next2();
    let u_phi = sampler.next1();
    let next = bsdf.sample(lobe, u, u_phi).map(|s| {
        let wi = (t * s.wi.x + n * s.wi.y + w * s.wi.z).normalize_or(Vec3A::Y);
        (leave_fibre(fh, wi, f32::INFINITY), s.weight, s.pdf)
    });
    FibreStep { direct, next }
}

/// Apply the material's normal map, bump map and anisotropy direction to the shading frame.
#[inline]
pub(crate) fn shading_frame(scene: &Scene, m: &Material, tri: u32, sf: &mut Surface) {
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
