//! Resampled direct lighting at the first surface (ReSTIR DI).
//!
//! Bitterli, Wyman, Pharr, Shirley, Lefohn and Jarosz, "Spatiotemporal Reservoir Resampling for
//! Real-Time Ray Tracing with Dynamic Direct Lighting" (SIGGRAPH 2020), on top of the light tree.
//! Every pixel of a pass (one sample per pixel) does the following:
//!
//! 1. **Candidates.** `M` points on lights are drawn exactly as next-event estimation draws them
//!    (the light tree chooses the light, a uniform point is taken on it) and one of them is kept
//!    in a *reservoir*, chosen in proportion to `target / source density`, where the target is
//!    the unshadowed contribution `luminance(BSDF × radiance × geometry)` (resampled importance
//!    sampling, Talbot et al. 2005). Only the kept point gets a shadow ray (an occluded one is
//!    dropped and cannot spread).
//! 2. **Reuse.** The reservoirs of other pixels are merged into this pixel's, each weighted by
//!    the target of *this* pixel at *its* sample, so one pixel's lucky candidates serve its
//!    neighbours:
//!    - [`RestirMode::Unbiased`]: one pass over neighbours' initial reservoirs, normalised with
//!      the count of pixels that could have produced the kept point (`Z`, found with a shadow ray
//!      from each). This is exactly unbiased: the expected image equals the plain estimator's
//!      (a test holds a long render's mean to that of the light tree). Passes are independent,
//!      so the film's error still falls as 1/N.
//!    - [`RestirMode::Viewport`]: the same pass normalised by the sum of the counts (biased, no
//!      extra shadow rays) plus **temporal reuse**: the previous pass's final reservoir at the
//!      same pixel (the progressive viewport is static between edits, so the motion is zero)
//!      joins the merge, its count capped. It is faster to clean but correlates consecutive
//!      passes and carries a small bias near geometric edges; it belongs to the interactive
//!      view, not to final frames.
//! 3. **Shading.** The kept point's contribution is `BSDF × radiance × geometry × W`, weighted
//!    against BSDF sampling by the power heuristic with the light tree's density at this pixel.
//!    Any such weight is valid because the two techniques' weights sum to 1 at every point.
//!    Indirect bounces use ordinary next-event estimation.
//!
//! The expected value of every mode other than `Viewport` is the plain estimator's. Memory is
//! about 230 bytes per pixel, so the technique is off above [`MAX_PIXELS`] pixels.

use glam::{Vec2, Vec3A};
use rayon::prelude::*;

use craft_render_bvh::{Hit, Ray};

use crate::bsdf::Bsdf;
use crate::camera::PreparedCamera;
use crate::integrator::{IntegratorSettings, Strategy, radiance_with, shading_frame};
use crate::light::LightPoint;
use crate::math::{Rgb, concentric_disk, luminance, power_heuristic, sanitize};
use crate::render::{PixelOrder, PixelSums, RenderSettings, add_sample};
use crate::sampler::Sampler;
use crate::scene::{Scene, Surface};

/// Largest image (in pixels) the technique runs on; above it a render falls back to plain
/// next-event estimation.
pub const MAX_PIXELS: usize = 1 << 22;
/// Most candidates per pixel and pass.
pub const MAX_CANDIDATES: u32 = 64;
/// Most neighbours merged per pixel and pass.
pub const MAX_NEIGHBORS: u32 = 16;

/// Seeds of the random streams of the three stages (each stage's draws are independent of how
/// many another stage takes).
pub const SALT_CANDIDATES: u32 = 0x9e37_79b1;
pub const SALT_SPATIAL: u32 = 0x7f4a_7c15;
pub const SALT_TEMPORAL: u32 = 0x2545_f491;

/// Neighbours whose first surface is further than this fraction of the distance from the
/// camera, or whose normal turns by more than the angle with this cosine, are not merged.
pub const DEPTH_TOLERANCE: f32 = 0.1;
pub const NORMAL_COSINE: f32 = 0.9;

const NONE: u32 = u32::MAX;

/// `x > 0`, false for NaN.
#[inline]
fn positive(x: f32) -> bool {
    x > 0.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RestirMode {
    #[default]
    Off,
    /// Spatial reuse with exact normalisation: same expected image as plain sampling.
    Unbiased,
    /// Temporal and spatial reuse with count normalisation, for the interactive view.
    Viewport,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RestirSettings {
    pub mode: RestirMode,
    /// Light samples drawn per pixel and pass.
    pub candidates: u32,
    /// Neighbouring pixels merged per pass.
    pub neighbors: u32,
    /// Radius in pixels in which neighbours are taken.
    pub radius: f32,
    /// A reservoir carried over from the previous pass counts for at most this many times the
    /// candidates of one pass (`Viewport` only).
    pub history: f32,
}

impl Default for RestirSettings {
    fn default() -> Self {
        RestirSettings { mode: RestirMode::Off, candidates: 8, neighbors: 4, radius: 12.0, history: 20.0 }
    }
}

impl RestirSettings {
    /// The settings with every number inside what the implementation supports.
    pub fn clamped(self) -> RestirSettings {
        RestirSettings {
            mode: self.mode,
            candidates: self.candidates.clamp(1, MAX_CANDIDATES),
            neighbors: self.neighbors.min(MAX_NEIGHBORS),
            radius: if self.radius.is_finite() { self.radius.clamp(1.0, 256.0) } else { 12.0 },
            history: if self.history.is_finite() { self.history.clamp(1.0, 64.0) } else { 20.0 },
        }
    }

    /// Whether a render with these settings uses the technique (it needs light sampling).
    pub fn active(&self, integrator: &IntegratorSettings, pixels: usize) -> bool {
        self.mode != RestirMode::Off && integrator.strategy != Strategy::BsdfOnly && integrator.max_depth > 0 && pixels <= MAX_PIXELS
    }
}

/// The first surface a camera sample sees.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Gbuf {
    prim: u32,
    u: f32,
    v: f32,
    /// Distance from the camera.
    t: f32,
    dir: Vec3A,
    /// Shading normal.
    n: Vec3A,
}

impl Gbuf {
    const EMPTY: Gbuf = Gbuf { prim: NONE, u: 0.0, v: 0.0, t: 0.0, dir: Vec3A::ZERO, n: Vec3A::ZERO };
}

/// A reservoir: the kept point of a light and its unbiased contribution weight.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Resv {
    /// Entry of the light table, `NONE` for an empty reservoir.
    entry: u32,
    /// The point on the light.
    u: Vec2,
    /// Unbiased contribution weight `W`: the estimate is `f(point) × W`.
    w: f32,
    /// How many candidates the reservoir stands for.
    m: f32,
}

impl Resv {
    const EMPTY: Resv = Resv { entry: NONE, u: Vec2::ZERO, w: 0.0, m: 0.0 };
}

/// The per-pixel state of a ReSTIR render.
pub struct RestirState {
    width: usize,
    height: usize,
    gbuf: Vec<Gbuf>,
    init: Vec<Resv>,
    mid: Vec<Resv>,
    out: Vec<Resv>,
    hist: Vec<Resv>,
    hist_gbuf: Vec<Gbuf>,
    have_hist: bool,
}

impl RestirState {
    pub fn new(width: u32, height: u32) -> RestirState {
        let n = width as usize * height as usize;
        RestirState {
            width: width as usize,
            height: height as usize,
            gbuf: vec![Gbuf::EMPTY; n],
            init: vec![Resv::EMPTY; n],
            mid: vec![Resv::EMPTY; n],
            out: vec![Resv::EMPTY; n],
            hist: vec![Resv::EMPTY; n],
            hist_gbuf: vec![Gbuf::EMPTY; n],
            have_hist: false,
        }
    }
}

/// A first surface ready to be lit.
struct Shade {
    sf: Surface,
    bsdf: Bsdf,
    transmissive: bool,
}

fn shade(scene: &Scene, g: &Gbuf) -> Option<Shade> {
    if g.prim == NONE {
        return None;
    }
    let hit = Hit { t: g.t, u: g.u, v: g.v, prim: g.prim };
    let ray = Ray { origin: Vec3A::ZERO, dir: g.dir, tmin: 0.0, tmax: f32::INFINITY };
    let mut sf = scene.surface(&hit, &ray)?;
    let m = scene.material(sf.material)?;
    shading_frame(scene, m, hit.prim, &mut sf);
    let transmissive = m.is_transmissive();
    let inside = transmissive && !sf.front;
    let params = m.params(sf.uv, &scene.textures);
    let bsdf = Bsdf::new(&params, sf.frame.to_local(-g.dir), inside);
    Some(Shade { sf, bsdf, transmissive })
}

/// What the surface sends back along the camera ray per unit of the point's measure, and the
/// density with which the BSDF would have drawn the direction; `None` when none.
#[inline]
fn eval(s: &Shade, lp: &LightPoint) -> Option<(Rgb, f32)> {
    let wi = s.sf.frame.to_local(lp.wi);
    // The shading and the real surface must agree on which side the light is.
    let side = lp.wi.dot(s.sf.ng);
    if side * wi.z > 0.0 && (side > 0.0 || s.transmissive) {
        let (f, pdf) = s.bsdf.eval(wi);
        (f.max_element() > 0.0).then_some((f, pdf))
    } else {
        None
    }
}

/// The target function: luminance of the unshadowed contribution of a point.
#[inline]
fn target(s: &Shade, lp: &LightPoint) -> f32 {
    match eval(s, lp) {
        Some((f, _)) => {
            let t = luminance(sanitize(f * lp.contrib));
            if t.is_finite() { t } else { 0.0 }
        }
        None => 0.0,
    }
}

#[inline]
fn visible(scene: &Scene, s: &Shade, lp: &LightPoint) -> bool {
    !scene.occluded(s.sf.p, s.sf.ng, lp.wi, lp.distance)
}

/// The sampler of one phase of one camera sample: the render's seed is salted so that the
/// phases draw independent numbers.
#[inline]
pub(crate) fn phase_sampler(settings: &RenderSettings, z_bits: u32, x: u32, y: u32, s: u32, salt: u32) -> Sampler {
    let seed = settings.seed ^ salt;
    let ordered = if settings.pixel_order == PixelOrder::ZCurve { Sampler::z_ordered(x, y, s, z_bits, seed) } else { None };
    ordered.unwrap_or_else(|| Sampler::new(Sampler::pixel_seed(x, y, seed), s))
}

/// The first surface of a camera sample, passing through cut-outs exactly as the integrator
/// does (the sampler is advanced the same way).
fn primary_surface(scene: &Scene, mut ray: Ray, sampler: &mut Sampler) -> Gbuf {
    let mut passes = 0u32;
    loop {
        let Some(hit) = scene.accel.intersect(&ray) else { return Gbuf::EMPTY };
        let Some(sf) = scene.surface(&hit, &ray) else { return Gbuf::EMPTY };
        let Some(material) = scene.material(sf.material) else { return Gbuf::EMPTY };
        if material.has_cutouts() && passes < 256 {
            let opacity = material.opacity(sf.uv, &scene.textures);
            if opacity < 1.0 && sampler.next1() >= opacity {
                passes += 1;
                ray = Scene::spawn(sf.p, sf.ng, ray.dir, f32::INFINITY);
                continue;
            }
        }
        return Gbuf { prim: hit.prim, u: hit.u, v: hit.v, t: hit.t, dir: ray.dir, n: sf.frame.n };
    }
}

/// Merges reservoirs for one pixel, streaming (weighted reservoir sampling).
struct Merger {
    w_sum: f32,
    entry: u32,
    u: Vec2,
    target: f32,
    m: f32,
    /// Which of the merged reservoirs the kept point came from.
    from: u32,
}

impl Merger {
    fn new() -> Merger {
        Merger { w_sum: 0.0, entry: NONE, u: Vec2::ZERO, target: 0.0, m: 0.0, from: 0 }
    }

    /// Merge `r`, which stands for `m` candidates, evaluating the target at the pixel being
    /// shaded. `rand` is consumed whether or not the reservoir can contribute.
    fn add(&mut self, scene: &Scene, at: &Shade, r: &Resv, m: f32, rand: f32, id: u32) {
        self.m += m;
        if r.entry == NONE || !positive(r.w) {
            return;
        }
        let Some(lp) = scene.lights.point(&scene.positions, r.entry, at.sf.p, r.u) else { return };
        let t = target(at, &lp);
        if !positive(t) {
            return;
        }
        let w = t * r.w * m;
        if !w.is_finite() || w <= 0.0 {
            return;
        }
        self.w_sum += w;
        if rand * self.w_sum < w {
            self.entry = r.entry;
            self.u = r.u;
            self.target = t;
            self.from = id;
        }
    }
}

/// Stage 1: the first surface and the initial reservoir of every pixel of sample `s`.
fn initial_stage(
    state: &mut RestirState,
    scene: &Scene,
    camera: &PreparedCamera,
    settings: &RenderSettings,
    z_bits: u32,
    s: u32,
    cancel: &std::sync::atomic::AtomicBool,
) {
    let (w, h) = (state.width, state.height);
    let cfg = &settings.integrator;
    let r = cfg.restir.clamped();
    state.gbuf.par_chunks_mut(w).zip(state.init.par_chunks_mut(w)).enumerate().for_each(|(y, (grow, irow))| {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        for x in 0..w {
            let mut main = phase_sampler(settings, z_bits, x as u32, y as u32, s, 0);
            let jitter = main.next2();
            let lens = main.next2();
            let ndc = Vec2::new((x as f32 + jitter.x) / w as f32 * 2.0 - 1.0, 1.0 - (y as f32 + jitter.y) / h as f32 * 2.0);
            let mut g = primary_surface(scene, camera.ray(ndc, lens), &mut main);
            let mut resv = Resv::EMPTY;
            if let Some(sh) = shade(scene, &g) {
                g.n = sh.sf.frame.n;
                let mut rng = phase_sampler(settings, z_bits, x as u32, y as u32, s, SALT_CANDIDATES);
                resv = initial_reservoir(scene, cfg, &r, &sh, &mut rng);
            } else {
                g = Gbuf::EMPTY;
            }
            if let (Some(a), Some(b)) = (grow.get_mut(x), irow.get_mut(x)) {
                *a = g;
                *b = resv;
            }
        }
    });
}

/// `candidates` points drawn like next-event estimation draws them, one kept, checked for
/// visibility.
fn initial_reservoir(scene: &Scene, cfg: &IntegratorSettings, r: &RestirSettings, at: &Shade, rng: &mut Sampler) -> Resv {
    let (mut w_sum, mut entry, mut keep_u, mut keep_target) = (0.0f32, NONE, Vec2::ZERO, 0.0f32);
    for _ in 0..r.candidates {
        let pick = rng.next1();
        let u = rng.next2();
        let rand = rng.next1();
        let Some((e, pmf)) = scene.lights.pick(at.sf.p, pick, cfg.light_sampling) else { continue };
        let Some(lp) = scene.lights.point(&scene.positions, e, at.sf.p, u) else { continue };
        let t = target(at, &lp);
        let src = pmf * lp.density;
        if !(t > 0.0 && src > 0.0) {
            continue;
        }
        let w = t / src;
        if !w.is_finite() {
            continue;
        }
        w_sum += w;
        if rand * w_sum < w {
            entry = e;
            keep_u = u;
            keep_target = t;
        }
    }
    let mut resv = Resv { entry, u: keep_u, w: 0.0, m: r.candidates as f32 };
    if entry != NONE && keep_target > 0.0 {
        resv.w = w_sum / (r.candidates as f32 * keep_target);
        // Visibility reuse: a point the pixel cannot see must not spread.
        match scene.lights.point(&scene.positions, entry, at.sf.p, keep_u) {
            Some(lp) if visible(scene, at, &lp) => {}
            _ => resv.w = 0.0,
        }
    }
    resv
}

#[inline]
fn similar(g: &Gbuf, o: &Gbuf) -> bool {
    o.prim != NONE && (o.t - g.t).abs() <= DEPTH_TOLERANCE * g.t.max(o.t) && g.n.dot(o.n) >= NORMAL_COSINE
}

/// Stage 2 (viewport only): merge the previous pass's final reservoir at the same pixel.
fn temporal_stage(state: &mut RestirState, scene: &Scene, settings: &RenderSettings, z_bits: u32, s: u32) {
    let w = state.width;
    let cfg = &settings.integrator;
    let r = cfg.restir.clamped();
    let (gbuf, init, hist, hist_gbuf, have) = (&state.gbuf, &state.init, &state.hist, &state.hist_gbuf, state.have_hist);
    state.mid.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, out) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let (g, own) = (gbuf[i], init[i]);
            *out = own;
            let mut rng = phase_sampler(settings, z_bits, x as u32, y as u32, s, SALT_TEMPORAL);
            let (r_own, r_hist) = (rng.next1(), rng.next1());
            if !have || g.prim == NONE {
                continue;
            }
            let Some(at) = shade(scene, &g) else { continue };
            let past = hist[i];
            let mut merger = Merger::new();
            merger.add(scene, &at, &own, own.m, r_own, 0);
            if similar(&g, &hist_gbuf[i]) {
                let m = past.m.min(r.history * r.candidates as f32);
                merger.add(scene, &at, &past, m, r_hist, 1);
            }
            *out = finish_biased(&merger);
        }
    });
}

fn finish_biased(m: &Merger) -> Resv {
    if m.entry == NONE || !positive(m.target) || !positive(m.m) {
        return Resv { entry: NONE, u: Vec2::ZERO, w: 0.0, m: m.m };
    }
    Resv { entry: m.entry, u: m.u, w: m.w_sum / (m.m * m.target), m: m.m }
}

/// The neighbours of pixel `(x, y)` for sample `s`: up to `neighbors` pixels in a disk, each
/// with the same similarity test every time (a pixel can be drawn twice, as with any sampling
/// with replacement).
fn neighbor_pixels(rng: &mut Sampler, r: &RestirSettings, x: usize, y: usize, w: usize, h: usize) -> ([Option<usize>; MAX_NEIGHBORS as usize], usize) {
    let mut out = [None; MAX_NEIGHBORS as usize];
    let k = r.neighbors as usize;
    for slot in out.iter_mut().take(k) {
        let d = concentric_disk(rng.next2()) * r.radius;
        let nx = (x as f32 + d.x + 0.5).floor();
        let ny = (y as f32 + d.y + 0.5).floor();
        if nx >= 0.0 && ny >= 0.0 && (nx as usize) < w && (ny as usize) < h && !(nx as usize == x && ny as usize == y) {
            *slot = Some(ny as usize * w + nx as usize);
        }
    }
    (out, k)
}

/// Stage 3: merge neighbours' reservoirs into each pixel's.
fn spatial_stage(state: &mut RestirState, scene: &Scene, settings: &RenderSettings, z_bits: u32, s: u32, from_mid: bool) {
    let (w, h) = (state.width, state.height);
    let cfg = &settings.integrator;
    let r = cfg.restir.clamped();
    let unbiased = r.mode == RestirMode::Unbiased;
    let (gbuf, src) = (&state.gbuf, if from_mid { &state.mid } else { &state.init });
    state.out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, out) in row.iter_mut().enumerate() {
            let i = y * w + x;
            let (g, own) = (gbuf[i], src[i]);
            *out = own;
            let mut rng = phase_sampler(settings, z_bits, x as u32, y as u32, s, SALT_SPATIAL);
            let (picks, k) = neighbor_pixels(&mut rng, &r, x, y, w, h);
            let mut rands = [0.0f32; MAX_NEIGHBORS as usize + 1];
            for slot in rands.iter_mut().take(k + 1) {
                *slot = rng.next1();
            }
            if g.prim == NONE || k == 0 {
                continue;
            }
            let Some(at) = shade(scene, &g) else { continue };
            let mut merger = Merger::new();
            merger.add(scene, &at, &own, own.m, rands[0], 0);
            // The pixels merged (index 0 is this pixel).
            let mut used = [usize::MAX; MAX_NEIGHBORS as usize + 1];
            used[0] = i;
            let mut count = 1usize;
            for (j, p) in picks.iter().take(k).enumerate() {
                let Some(ni) = *p else { continue };
                if !similar(&g, &gbuf[ni]) {
                    continue;
                }
                merger.add(scene, &at, &src[ni], src[ni].m, rands[j + 1], count as u32);
                used[count] = ni;
                count += 1;
            }
            if count == 1 {
                continue;
            }
            *out = if unbiased { finish_unbiased(scene, &at, &merger, &used[..count], src, gbuf, i) } else { finish_biased(&merger) };
        }
    });
}

/// The exact normalisation: `Z` counts the candidates of every merged pixel that could have
/// produced the kept point (it has a target there and the pixel sees it). The point must also be
/// visible from this pixel, which the shading then needs no ray for.
fn finish_unbiased(scene: &Scene, at: &Shade, m: &Merger, used: &[usize], src: &[Resv], gbuf: &[Gbuf], me: usize) -> Resv {
    let empty = Resv { entry: NONE, u: Vec2::ZERO, w: 0.0, m: m.m };
    if m.entry == NONE || !positive(m.target) {
        return empty;
    }
    let origin = used.get(m.from as usize).copied().unwrap_or(me);
    let mut z = 0.0f32;
    for &pi in used {
        let other;
        let sh = if pi == me {
            at
        } else {
            let Some(s) = gbuf.get(pi).and_then(|g| shade(scene, g)) else { continue };
            other = s;
            &other
        };
        let Some(lp) = scene.lights.point(&scene.positions, m.entry, sh.sf.p, m.u) else { continue };
        if !positive(target(sh, &lp)) {
            continue;
        }
        // A reservoir's own kept point was found visible already.
        if pi == origin || visible(scene, sh, &lp) {
            z += src.get(pi).map_or(0.0, |r| r.m);
        } else if pi == me {
            // Not visible from this pixel: nothing to shade.
            return empty;
        }
    }
    if !positive(z) {
        return empty;
    }
    Resv { entry: m.entry, u: m.u, w: m.w_sum / (z * m.target), m: m.m }
}

/// Stage 4: shade every pixel with its final reservoir and continue the path. The final
/// reservoirs are left in `out` (with an occluded point zeroed) for the next pass's temporal reuse.
fn shade_stage(
    state: &mut RestirState,
    scene: &Scene,
    camera: &PreparedCamera,
    previous: &Option<PreparedCamera>,
    settings: &RenderSettings,
    z_bits: u32,
    s: u32,
    scratch: &mut [PixelSums],
    cancel: &std::sync::atomic::AtomicBool,
) {
    let (w, h) = (state.width, state.height);
    let cfg = &settings.integrator;
    let unbiased = cfg.restir.mode == RestirMode::Unbiased;
    let light_only = cfg.strategy == Strategy::LightOnly;
    state.out.par_chunks_mut(w).zip(scratch.par_chunks_mut(w)).enumerate().for_each(|(y, (orow, srow))| {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        for (x, (resv, px)) in orow.iter_mut().zip(srow.iter_mut()).enumerate() {
            let mut sampler = phase_sampler(settings, z_bits, x as u32, y as u32, s, 0);
            let jitter = sampler.next2();
            let lens = sampler.next2();
            let ndc = Vec2::new((x as f32 + jitter.x) / w as f32 * 2.0 - 1.0, 1.0 - (y as f32 + jitter.y) / h as f32 * 2.0);
            let r = *resv;
            let mut killed = false;
            let mut hook = |sf: &Surface, bsdf: &Bsdf, transmissive: bool| -> Rgb {
                if r.entry == NONE || !positive(r.w) {
                    return Vec3A::ZERO;
                }
                let Some(lp) = scene.lights.point(&scene.positions, r.entry, sf.p, r.u) else { return Vec3A::ZERO };
                let wi = sf.frame.to_local(lp.wi);
                let side = lp.wi.dot(sf.ng);
                if !(side * wi.z > 0.0 && (side > 0.0 || transmissive)) {
                    return Vec3A::ZERO;
                }
                let (f, pdf_bsdf) = bsdf.eval(wi);
                if !positive(f.max_element()) {
                    return Vec3A::ZERO;
                }
                if !unbiased && scene.occluded(sf.p, sf.ng, lp.wi, lp.distance) {
                    killed = true;
                    return Vec3A::ZERO;
                }
                let source = scene.lights.selection_pdf(r.entry, sf.p, cfg.light_sampling) * lp.density * lp.to_solid;
                let weight = if lp.delta || light_only { 1.0 } else { power_heuristic(source, pdf_bsdf) };
                f * lp.contrib * (r.w * weight)
            };
            let (l, f) = radiance_with(scene, camera.ray(ndc, lens), &mut sampler, cfg, Some(&mut hook));
            if killed {
                // The kept point is hidden from this pixel: nothing is known about it, so the
                // history it would leave counts for nothing.
                *resv = Resv::EMPTY;
            }
            add_sample(px, s, l, &f, ndc, previous, w, h);
        }
    });
}

/// Add one sample per pixel with the sample number `s`.
pub(crate) fn run_sample(
    state: &mut RestirState,
    scene: &Scene,
    camera: &PreparedCamera,
    previous: &Option<PreparedCamera>,
    settings: &RenderSettings,
    s: u32,
    scratch: &mut [PixelSums],
    cancel: &std::sync::atomic::AtomicBool,
) {
    let z_bits = crate::sampler::z_order_bits(state.width as u32, state.height as u32);
    let viewport = settings.integrator.restir.mode == RestirMode::Viewport;
    initial_stage(state, scene, camera, settings, z_bits, s, cancel);
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    if viewport {
        temporal_stage(state, scene, settings, z_bits, s);
    }
    spatial_stage(state, scene, settings, z_bits, s, viewport);
    shade_stage(state, scene, camera, previous, settings, z_bits, s, scratch, cancel);
    if viewport {
        std::mem::swap(&mut state.hist, &mut state.out);
        std::mem::swap(&mut state.hist_gbuf, &mut state.gbuf);
        state.have_hist = true;
    }
}
