//! The light transport estimator: unidirectional path tracing (Kajiya 1986) with next-event
//! estimation and multiple importance sampling (Veach and Guibas 1995), and Russian roulette
//! (Arvo and Kirk 1990).

use craft_render_bvh::Ray;
use glam::Vec3A;

use crate::bsdf::Bsdf;
use crate::math::{Rgb, power_heuristic, sanitize};
use crate::sampler::Sampler;
use crate::scene::Scene;

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
}

impl Default for IntegratorSettings {
    fn default() -> Self {
        IntegratorSettings { max_depth: 12, roulette_start: 3, clamp_indirect: 0.0, strategy: Strategy::Mis }
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
    for depth in 0..=max_depth {
        let Some(hit) = scene.accel.intersect(&ray) else {
            let env = Vec3A::from(scene.lights.environment);
            if env.max_element() > 0.0 {
                let w = if prev_pdf.is_infinite() {
                    1.0
                } else {
                    match cfg.strategy {
                        Strategy::Mis => power_heuristic(prev_pdf, scene.lights.pdf_environment()),
                        Strategy::BsdfOnly => 1.0,
                        Strategy::LightOnly => 0.0,
                    }
                };
                add(throughput * env * w, depth);
            }
            break;
        };
        let Some(sf) = scene.surface(&hit, &ray) else { break };
        if let Some(le) = scene.lights.emitted(hit.prim, sf.front) {
            let w = if prev_pdf.is_infinite() {
                1.0
            } else {
                match cfg.strategy {
                    Strategy::Mis => power_heuristic(prev_pdf, scene.lights.pdf_triangle(hit.prim, hit.t, sf.ng.dot(ray.dir))),
                    Strategy::BsdfOnly => 1.0,
                    Strategy::LightOnly => 0.0,
                }
            };
            add(throughput * le * w, depth);
        }
        if depth == max_depth {
            break;
        }
        let Some(material) = scene.material(sf.material) else { break };
        let bsdf = Bsdf::new(material, sf.frame.to_local(-ray.dir));

        // Direct light from one chosen light.
        if cfg.strategy != Strategy::BsdfOnly && !scene.lights.is_empty() {
            let pick = sampler.next1();
            let u = sampler.next2();
            if let Some(ls) = scene.lights.sample(&scene.positions, sf.p, pick, u)
                && ls.wi.dot(sf.ng) > 0.0
                && ls.pdf > 0.0
            {
                let (f, pdf_bsdf) = bsdf.eval(sf.frame.to_local(ls.wi));
                if f.max_element() > 0.0 && !scene.occluded(sf.p, sf.ng, ls.wi, ls.distance) {
                    let w = if ls.delta || cfg.strategy == Strategy::LightOnly { 1.0 } else { power_heuristic(ls.pdf, pdf_bsdf) };
                    add(throughput * f * ls.radiance * (w / ls.pdf), depth + 1);
                }
            }
        }

        // Continue the path.
        let lobe = sampler.next1();
        let u = sampler.next2();
        let Some(bs) = bsdf.sample(lobe, u) else { break };
        let wi = sf.frame.to_world(bs.wi);
        if wi.dot(sf.ng) <= 0.0 {
            // The shading normal sent the direction below the real surface.
            break;
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
    }
    total
}
