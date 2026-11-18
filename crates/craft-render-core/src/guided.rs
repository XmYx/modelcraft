//! The path tracer of [`crate::integrator`] with learned sampling: the same estimator and the
//! same expected image, with two decisions taken from what earlier paths found (stored in a
//! [`Guide`], a hash grid over the scene).
//!
//! - **Path guiding.** At a surface with a wide lobe, part of the continuing directions are
//!   drawn from the cell's learned distribution of incoming light times reflectance instead of
//!   from the BSDF, and the two are combined as one mixture density (one-sample MIS, Veach 1997).
//!   Learning proceeds in passes that double in length, each sampling from what the passes
//!   before it recorded (Müller, Gross and Novák, "Practical Path Guiding for Efficient
//!   Light-Transport Simulation", 2017).
//! - **Russian roulette and splitting by expected contribution.** A path arriving at a surface
//!   is ended at random, kept, or split into several, by how much it is expected to add to its
//!   pixel compared with what the pixel's samples have been (Vorba and Křivánek, "Adjoint-Driven
//!   Russian Roulette and Splitting in Light Transport Simulation", 2016). As in Rath et al.,
//!   "EARS: Efficiency-Aware Russian Roulette and Splitting" (2022), both sides are root second
//!   moments rather than means (throughput × the root second moment of the cell's cached
//!   outgoing radiance, over the root second moment of the pixel's samples), so paths go where
//!   the noise is; their cost term is left out (every continuation counts the same here).
//!
//! Neither changes the expected value: a mixture density that is positive wherever the BSDF is,
//! and roulette/splitting with weights 1/q and 1/n, are unbiased for any guide contents, and a
//! pass only reads what earlier passes recorded.

use craft_render_bvh::Ray;
use craft_render_guide::{Guide, LEVELS};
use glam::Vec3A;

use crate::bsdf::Bsdf;
use crate::integrator::{Features, IntegratorSettings, MAX_DEPTH, Strategy, shading_frame};
use crate::math::{Rgb, luminance, power_heuristic, sanitize};
use crate::sampler::Sampler;
use crate::scene::Scene;

/// Most branches one camera path may have.
pub const MAX_BRANCHES: usize = 16;
/// Most copies one split makes.
const MAX_SPLIT: f32 = 4.0;
/// Lowest survival probability of the adaptive roulette.
const MIN_SURVIVAL: f32 = 0.1;
/// The adaptive roulette leaves paths alone whose expected contribution is within this factor
/// of what their pixel's samples have been (the weight window of Vorba and Křivánek).
const WINDOW: f32 = 2.0;
/// Largest share of the wide lobes' samples that the learned distribution draws.
const GUIDE_SHARE: f32 = 0.5;

/// Share of this surface's continuing directions that come from the learned distribution. A
/// direction the BSDF draws weighs `1 / (1 − share)` times what it would without guiding; keeping
/// the share below `1 − albedo` keeps that weight below 1, so a long path through bright walls
/// cannot grow heavier at every bounce when the learned distribution misses its directions.
#[inline]
fn guide_share(bsdf: &Bsdf) -> f32 {
    GUIDE_SHARE.min(1.0 - bsdf.wide_albedo()) * bsdf.wide_probability()
}
/// A direction record counts for at most this many times the cell's mean outgoing radiance.
const OUTLIER: f32 = 50.0;
/// Vertices of one branch that teach the guide (later ones are still rendered).
const MAX_RECORDS: usize = 24;

/// What the guided integrator reads besides the scene.
#[derive(Clone, Copy)]
pub struct GuideContext<'a> {
    pub guide: &'a Guide,
    /// The root of the mean squared luminance of the pixel's samples in earlier passes; 0 when
    /// unknown, which turns the adaptive roulette and splitting off for this path.
    pub reference: f32,
    /// Whether this path may teach the guide.
    pub learn: bool,
}

#[derive(Clone, Copy)]
struct Branch {
    ray: Ray,
    throughput: Rgb,
    prev_pdf: f32,
    medium: Option<Vec3A>,
    depth: u32,
    /// Product of the split factors above this branch: contributions times this are what the
    /// unsplit path would have carried.
    scale: f32,
    /// A copy made by a split: its first hit is the surface the split happened at, where the
    /// original already took the emission and the cut-out and split decisions.
    copy: bool,
    sampler: Sampler,
}

#[derive(Clone, Copy)]
struct Record {
    cells: [u32; LEVELS],
    /// Unsplit throughput arriving at the vertex.
    throughput: f32,
    /// Unsplit contributions before the vertex's direct light, and after it.
    before: f32,
    after: f32,
    dir: Option<Vec3A>,
}

/// Radiance arriving along `ray` (one path, possibly branching) with the first surface's
/// features.
pub fn radiance_guided(scene: &Scene, ray: Ray, sampler: &mut Sampler, cfg: &IntegratorSettings, ctx: &GuideContext) -> (Rgb, Features) {
    let mut features = Features::default();
    let mut total = Vec3A::ZERO;
    let max_depth = cfg.max_depth.min(MAX_DEPTH);
    let clamp = if cfg.clamp_indirect.is_finite() && cfg.clamp_indirect > 0.0 { cfg.clamp_indirect } else { f32::INFINITY };
    let guide = ctx.guide;
    let adaptive = cfg.adaptive_splitting && ctx.reference > 0.0;
    let mut stack = [None::<Branch>; MAX_BRANCHES];
    let mut waiting = 1usize;
    let mut made = 1u32;
    stack[0] = Some(Branch { ray, throughput: Vec3A::ONE, prev_pdf: f32::INFINITY, medium: None, depth: 0, scale: 1.0, copy: false, sampler: *sampler });
    let mut primary = true;
    while waiting > 0 {
        waiting -= 1;
        let Some(Branch { mut ray, mut throughput, mut prev_pdf, mut medium, mut depth, mut scale, copy, sampler: mut rng }) =
            stack.get_mut(waiting).and_then(Option::take)
        else {
            break;
        };
        let mut forced = copy;
        let mut first = primary;
        primary = false;
        let mut records = [None::<Record>; MAX_RECORDS];
        let mut recorded = 0usize;
        // Contributions of this branch, as the unsplit path would have carried them.
        let mut unsplit = 0.0f32;
        let mut passes = 0u32;
        // The state the current ray was spawned with (what a copy made at its hit starts from).
        let mut entry = (throughput, medium);
        while depth <= max_depth {
            let mut add = |c: Rgb, at_depth: u32, scale: f32, unsplit: &mut f32| {
                let c = sanitize(c);
                let c = if at_depth > 0 { c.min(Vec3A::splat(clamp)) } else { c };
                total += c;
                *unsplit += luminance(c) * scale;
            };
            let Some(hit) = scene.accel.intersect(&ray) else {
                if first {
                    features.albedo = scene.lights.environment_radiance(ray.dir).min(Vec3A::ONE);
                }
                let (sun, sun_pdf) = scene.lights.sun_radiance(ray.dir);
                if sun_pdf > 0.0 && !forced {
                    let w = if prev_pdf.is_infinite() {
                        1.0
                    } else {
                        match cfg.strategy {
                            Strategy::Mis => power_heuristic(prev_pdf, sun_pdf),
                            Strategy::BsdfOnly => 1.0,
                            Strategy::LightOnly => 0.0,
                        }
                    };
                    add(throughput * sun * w, depth, scale, &mut unsplit);
                }
                if scene.lights.has_environment() && !forced {
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
                    add(throughput * env * w, depth, scale, &mut unsplit);
                }
                break;
            };
            let Some(mut sf) = scene.surface(&hit, &ray) else { break };
            let Some(material) = scene.material(sf.material) else { break };
            if let Some(sigma) = medium {
                throughput *= Vec3A::new((-sigma.x * hit.t).exp(), (-sigma.y * hit.t).exp(), (-sigma.z * hit.t).exp());
            }
            if !forced && material.has_cutouts() && passes < 256 {
                let opacity = material.opacity(sf.uv, &scene.textures);
                if opacity < 1.0 && rng.next1() >= opacity {
                    passes += 1;
                    ray = Scene::spawn(sf.p, sf.ng, ray.dir, f32::INFINITY);
                    entry = (throughput, medium);
                    continue;
                }
            }
            if !forced && let Some(le) = scene.lights.emitted(hit.prim, sf.front) {
                let w = if prev_pdf.is_infinite() {
                    1.0
                } else {
                    match cfg.strategy {
                        Strategy::Mis => {
                            power_heuristic(prev_pdf, scene.lights.pdf_triangle(hit.prim, ray.origin, hit.t, sf.ng.dot(ray.dir), cfg.light_sampling))
                        }
                        Strategy::BsdfOnly => 1.0,
                        Strategy::LightOnly => 0.0,
                    }
                };
                add(throughput * le * w, depth, scale, &mut unsplit);
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
            let cells = guide.cells(sf.p, sf.ng);
            // The distribution this vertex samples from, if its surroundings have one yet.
            let learned = if cfg.guiding { guide.finest_trained(&cells) } else { None };

            // End, keep or split the path by what it is expected to add to its pixel.
            let mut decided = false;
            if adaptive
                && !forced
                && let Some(cache) = guide.finest_radiance(&cells)
            {
                decided = true;
                let q = luminance(throughput) * cache.second_moment.max(0.0).sqrt() / ctx.reference;
                let q = if q.is_finite() { q } else { 1.0 };
                if q < 1.0 / WINDOW {
                    if depth > 0 {
                        let survive = (q * WINDOW).max(MIN_SURVIVAL);
                        if rng.next1() >= survive {
                            break;
                        }
                        throughput /= survive;
                    }
                } else if q > WINDOW {
                    let room = (MAX_BRANCHES as u32).saturating_sub(made) as usize + 1;
                    let n = (((q / WINDOW).min(MAX_SPLIT) + rng.next1()).floor() as usize).min(room).max(1);
                    if n > 1 {
                        let share = 1.0 / n as f32;
                        throughput *= share;
                        scale *= n as f32;
                        for _ in 1..n {
                            if let Some(slot) = stack.get_mut(waiting) {
                                *slot = Some(Branch {
                                    ray,
                                    throughput: entry.0 * share,
                                    prev_pdf,
                                    medium: entry.1,
                                    depth,
                                    scale,
                                    copy: true,
                                    sampler: rng.fork(made),
                                });
                                waiting += 1;
                                made += 1;
                            }
                        }
                    }
                }
            }
            forced = false;
            let arriving = luminance(throughput) * scale;
            let before = unsplit;

            // Direct light from one chosen light.
            if cfg.strategy != Strategy::BsdfOnly && !scene.lights.is_empty() {
                let pick = rng.next1();
                let u = rng.next2();
                if let Some(ls) = scene.lights.sample(&scene.positions, sf.p, pick, u, cfg.light_sampling)
                    && ls.pdf > 0.0
                {
                    let wi = sf.frame.to_local(ls.wi);
                    let side = ls.wi.dot(sf.ng);
                    if side * wi.z > 0.0 && (side > 0.0 || transmissive) {
                        let (f, pdf_bsdf) = bsdf.eval(wi);
                        if f.max_element() > 0.0 && !scene.occluded(sf.p, sf.ng, ls.wi, ls.distance) {
                            // BSDF sampling here is the mixture with the guide; its density is what MIS weighs.
                            let pdf_scatter = match learned {
                                Some(cell) => {
                                    let share = guide_share(&bsdf);
                                    (1.0 - share) * pdf_bsdf + share * guide.pdf(cell, ls.wi)
                                }
                                None => pdf_bsdf,
                            };
                            let w = if ls.delta || cfg.strategy == Strategy::LightOnly { 1.0 } else { power_heuristic(ls.pdf, pdf_scatter) };
                            add(throughput * f * ls.radiance * (w / ls.pdf), depth + 1, scale, &mut unsplit);
                        }
                    }
                }
            }
            let mut record = Record { cells, throughput: arriving, before, after: unsplit, dir: None };

            // Continue the path: from the learned distribution or from the BSDF, as one mixture.
            let share = if learned.is_some() { guide_share(&bsdf) } else { 0.0 };
            let cell = learned.unwrap_or(0);
            let lobe = rng.next1();
            let u = rng.next2();
            let local = if lobe < share {
                guide.sample(cell, u).map(|(dir, _)| sf.frame.to_local(dir))
            } else {
                bsdf.sample(((lobe - share) / (1.0 - share)).clamp(0.0, 0.999_999), u).map(|s| s.wi)
            };
            let mut alive = false;
            if let Some(local) = local {
                let wi = sf.frame.to_world(local);
                let (f, pdf_bsdf) = bsdf.eval(local);
                let pdf = (1.0 - share) * pdf_bsdf + if share > 0.0 { share * guide.pdf(cell, wi) } else { 0.0 };
                let side = wi.dot(sf.ng);
                // The shading normal may not send the direction to the other side of the real surface.
                if pdf > 0.0 && f.is_finite() && f.max_element() > 0.0 && side * local.z > 0.0 {
                    if side < 0.0 {
                        medium = if sf.front { Some(material.absorption()).filter(|s| s.max_element() > 0.0) } else { None };
                    }
                    throughput *= f / pdf;
                    prev_pdf = pdf;
                    record.dir = Some(wi);
                    alive = throughput.is_finite();
                    if alive && !decided && depth >= cfg.roulette_start {
                        let q = throughput.max_element().min(0.95);
                        alive = q > 0.0 && rng.next1() < q;
                        throughput /= q;
                    }
                    ray = Scene::spawn(sf.p, sf.ng, wi, f32::INFINITY);
                    entry = (throughput, medium);
                }
            }
            if let Some(slot) = records.get_mut(recorded) {
                *slot = Some(record);
                recorded += 1;
            }
            if !alive {
                break;
            }
            depth += 1;
        }
        if ctx.learn {
            for r in records.iter().take(recorded).flatten() {
                if r.throughput > 0.0 && r.throughput.is_finite() {
                    // Directions are learned once the surroundings' cache knows what is ordinary
                    // there: one rare, very bright path may not claim a whole distribution.
                    let known = guide.finest_radiance(&r.cells);
                    for cell in r.cells {
                        guide.record_radiance(cell, (unsplit - r.before) / r.throughput);
                        if let (Some(dir), Some(known)) = (r.dir, known) {
                            // What the path gathered beyond this vertex, per unit of what arrived:
                            // one estimate of ∫ f · cos · L over the bin the direction fell in.
                            let gathered = (unsplit - r.after) / r.throughput;
                            guide.record_direction(cell, dir, gathered.min(OUTLIER * known.mean.max(1e-3)));
                        }
                    }
                }
            }
        }
    }
    (total, features)
}
