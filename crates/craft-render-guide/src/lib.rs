//! Learned sampling for path tracing.
//!
//! A [`Guide`] is a hash grid over space. Every cell holds two things a renderer fills while it
//! renders and reads in its next pass:
//!
//! - a **radiance cache**: the mean and the second moment of the light leaving surfaces in the
//!   cell, as estimated by the paths that passed through it (in the spirit of path-space
//!   filtering in a hashed grid: Binder, Fricke and Keller, "Massively Parallel Path Space
//!   Filtering", 2018). The cache is only ever used to *decide* (how many paths to continue), not
//!   as a value in the picture, so it adds no bias;
//! - a **directional distribution** of the light arriving in the cell, a small equal-area image
//!   over the sphere of directions that paths sample from in proportion to what earlier paths
//!   found there (path guiding: Vorba et al., "On-line Learning of Parametric Mixture Models for
//!   Light Transport Simulation", 2014; Müller, Gross and Novák, "Practical Path Guiding for
//!   Efficient Light-Transport Simulation", 2017, whose iterative learn-then-sample scheme this
//!   follows with a fixed grid instead of their adaptive trees).
//!
//! The grid has [`LEVELS`] resolutions, each four times coarser than the one before. A record
//! goes into every level; a lookup uses the finest level that has seen enough records, so a short
//! render learns coarsely at once and a long one refines (the role adaptive spatial trees play in
//! the papers above).
//!
//! Recording is lock-free and **deterministic across threads**: sums are kept in fixed point in
//! atomic integers, and integer addition does not depend on the order in which threads add.
//!
//! Hostile input: positions, directions and values that are not finite are ignored or clamped;
//! memory is fixed at construction; nothing panics.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

use std::f32::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};

use glam::{Vec2, Vec3A};
use rayon::prelude::*;

/// Resolutions of the grid.
pub const LEVELS: usize = 3;
/// How much coarser each level is than the one before it.
const LEVEL_STEP: f32 = 4.0;
/// Largest table: 2^20 cells.
pub const MAX_CELLS_LOG2: u32 = 20;
/// Largest directional image: 64 × 64 bins.
pub const MAX_RESOLUTION: u32 = 64;
/// Largest radiance the cache keeps of one estimate (brighter ones count as this).
const CACHE_CLAMP: f32 = 1e3;
/// Largest value one direction record may carry.
const RECORD_CLAMP: f32 = 1e7;
const RECORD_SCALE: f32 = 1024.0;
const MEAN_SCALE: f32 = 65_536.0;
const MOMENT_SCALE: f32 = 1_048_576.0;
/// Share of every directional distribution that stays uniform, so no direction is ever lost.
const UNIFORM_SHARE: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuideConfig {
    /// The table has `2^cells_log2` cells (clamped to 8..=[`MAX_CELLS_LOG2`]).
    pub cells_log2: u32,
    /// Edge of a cell of the finest level as a fraction of the scene's diagonal.
    pub cell_fraction: f32,
    /// Bins along each side of a cell's directional image (clamped to 2..=[`MAX_RESOLUTION`]).
    pub resolution: u32,
    /// Records a cell needs before its distribution or its cache is used.
    pub min_records: u32,
}

impl Default for GuideConfig {
    fn default() -> Self {
        GuideConfig { cells_log2: 14, cell_fraction: 1.0 / 48.0, resolution: 16, min_records: 128 }
    }
}

/// What the cache knows about the light leaving a cell.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CachedRadiance {
    pub mean: f32,
    /// Mean of the squared estimates (their spread, with `mean`).
    pub second_moment: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct CellState {
    cache: Option<CachedRadiance>,
    trained: bool,
}

/// Per-cell counters: direction records, cache records, Σ radiance, Σ radiance².
const STATS: usize = 4;

/// The grid. Record into it from any number of threads, then call [`Guide::update`] between
/// passes to make what was recorded available to [`Guide::sample`], [`Guide::pdf`] and
/// [`Guide::radiance`].
#[derive(Debug)]
pub struct Guide {
    origin: Vec3A,
    inv_cell: f32,
    mask: u32,
    res: usize,
    min_records: u64,
    sums: Vec<AtomicU64>,
    stats: Vec<AtomicU64>,
    /// Running sums of every cell's bins, normalized to end at 1 (zeros while untrained).
    cdf: Vec<f32>,
    state: Vec<CellState>,
}

fn mix(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

impl Guide {
    /// A grid for a scene inside the box `lo..hi`. Degenerate boxes and settings are repaired.
    pub fn new(lo: Vec3A, hi: Vec3A, cfg: GuideConfig) -> Guide {
        let diagonal = if lo.is_finite() && hi.is_finite() { (hi - lo).length() } else { 0.0 };
        let diagonal = if diagonal.is_finite() && diagonal > 1e-6 { diagonal } else { 1.0 };
        let fraction = if cfg.cell_fraction.is_finite() { cfg.cell_fraction.clamp(1e-4, 1.0) } else { 1.0 / 48.0 };
        let cells = 1usize << cfg.cells_log2.clamp(8, MAX_CELLS_LOG2);
        let res = cfg.resolution.clamp(2, MAX_RESOLUTION) as usize;
        Guide {
            origin: if lo.is_finite() { lo } else { Vec3A::ZERO },
            inv_cell: 1.0 / (diagonal * fraction),
            mask: cells as u32 - 1,
            res,
            min_records: u64::from(cfg.min_records.max(1)),
            sums: (0..cells * res * res).map(|_| AtomicU64::new(0)).collect(),
            stats: (0..cells * STATS).map(|_| AtomicU64::new(0)).collect(),
            cdf: vec![0.0; cells * res * res],
            state: vec![CellState::default(); cells],
        }
    }

    pub fn cell_count(&self) -> usize {
        self.state.len()
    }

    /// Bins of one cell's directional image.
    pub fn bins(&self) -> usize {
        self.res * self.res
    }

    /// The cells of a surface point with normal `n`, finest level first. Surfaces facing
    /// different ways (the two sides of a wall, a floor and the wall above it) get different cells.
    #[inline]
    pub fn cells(&self, p: Vec3A, n: Vec3A) -> [u32; LEVELS] {
        let a = n.abs();
        let face = if a.x >= a.y && a.x >= a.z {
            u32::from(n.x < 0.0)
        } else if a.y >= a.z {
            2 + u32::from(n.y < 0.0)
        } else {
            4 + u32::from(n.z < 0.0)
        };
        let mut scale = self.inv_cell;
        std::array::from_fn(|level| {
            let q = (p - self.origin) * scale;
            scale /= LEVEL_STEP;
            let q = if q.is_finite() { q.clamp(Vec3A::splat(-1e9), Vec3A::splat(1e9)) } else { Vec3A::ZERO };
            let (x, y, z) = (q.x.floor() as i32 as u32, q.y.floor() as i32 as u32, q.z.floor() as i32 as u32);
            mix(x ^ mix(y ^ mix(z ^ mix((face + 8 * level as u32).wrapping_add(0x9e37_79b9))))) & self.mask
        })
    }

    /// The finest cell of a point ([`Guide::cells`]`[0]`).
    #[inline]
    pub fn cell(&self, p: Vec3A, n: Vec3A) -> u32 {
        self.cells(p, n)[0]
    }

    /// The finest of `cells` that has a directional distribution.
    #[inline]
    pub fn finest_trained(&self, cells: &[u32; LEVELS]) -> Option<u32> {
        cells.iter().copied().find(|c| self.is_trained(*c))
    }

    /// The cached light leaving the finest of `cells` that has a cache.
    #[inline]
    pub fn finest_radiance(&self, cells: &[u32; LEVELS]) -> Option<CachedRadiance> {
        cells.iter().find_map(|c| self.radiance(*c))
    }

    #[inline]
    fn bin(&self, dir: Vec3A) -> usize {
        let res = self.res as f32;
        let u = ((dir.y.clamp(-1.0, 1.0) + 1.0) * 0.5 * res) as usize;
        let v = ((dir.z.atan2(dir.x) / (2.0 * PI) + 0.5) * res) as usize;
        u.min(self.res - 1) * self.res + v.min(self.res - 1)
    }

    /// Record that light of `value` (radiance over the density the direction was drawn with)
    /// arrived in `cell` from the unit direction `dir`.
    #[inline]
    pub fn record_direction(&self, cell: u32, dir: Vec3A, value: f32) {
        if !dir.is_finite() || !value.is_finite() {
            return;
        }
        let cell = (cell & self.mask) as usize;
        if let Some(s) = self.stats.get(cell * STATS) {
            s.fetch_add(1, Ordering::Relaxed);
        }
        let v = (value.clamp(0.0, RECORD_CLAMP) * RECORD_SCALE) as u64;
        if v > 0
            && let Some(s) = self.sums.get(cell * self.bins() + self.bin(dir))
        {
            s.fetch_add(v, Ordering::Relaxed);
        }
    }

    /// Record one estimate of the light leaving `cell`.
    #[inline]
    pub fn record_radiance(&self, cell: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        let v = value.clamp(0.0, CACHE_CLAMP);
        let base = (cell & self.mask) as usize * STATS;
        if let Some([_, n, sum, sq]) = self.stats.get(base..base + STATS) {
            n.fetch_add(1, Ordering::Relaxed);
            sum.fetch_add((v * MEAN_SCALE) as u64, Ordering::Relaxed);
            sq.fetch_add((v * v * MOMENT_SCALE) as u64, Ordering::Relaxed);
        }
    }

    /// Make everything recorded so far available to sampling and to the cache.
    pub fn update(&mut self) {
        let bins = self.bins();
        let (sums, stats, min) = (&self.sums, &self.stats, self.min_records);
        self.cdf.par_chunks_mut(bins).zip(self.state.par_iter_mut()).enumerate().for_each(|(cell, (cdf, state))| {
            let stat = |k: usize| stats.get(cell * STATS + k).map_or(0, |a| a.load(Ordering::Relaxed));
            let n = stat(1);
            state.cache = (n >= min).then(|| CachedRadiance {
                mean: (stat(2) as f64 / n as f64 / f64::from(MEAN_SCALE)) as f32,
                second_moment: (stat(3) as f64 / n as f64 / f64::from(MOMENT_SCALE)) as f32,
            });
            let cell_sums = sums.get(cell * bins..(cell + 1) * bins).unwrap_or(&[]);
            let total: f64 = cell_sums.iter().map(|a| a.load(Ordering::Relaxed) as f64).sum();
            state.trained = stat(0) >= min && total > 0.0 && cell_sums.len() == bins;
            if !state.trained {
                cdf.fill(0.0);
                return;
            }
            let uniform = f64::from(UNIFORM_SHARE) / bins as f64;
            let mut acc = 0.0f64;
            for (c, s) in cdf.iter_mut().zip(cell_sums) {
                acc += s.load(Ordering::Relaxed) as f64 / total * f64::from(1.0 - UNIFORM_SHARE) + uniform;
                *c = acc as f32;
            }
            if let Some(last) = cdf.last_mut() {
                *last = 1.0;
            }
        });
    }

    /// Forget everything.
    pub fn reset(&mut self) {
        for a in self.sums.iter().chain(&self.stats) {
            a.store(0, Ordering::Relaxed);
        }
        self.cdf.fill(0.0);
        self.state.fill(CellState::default());
    }

    /// Whether [`Guide::sample`] has a distribution for `cell`.
    #[inline]
    pub fn is_trained(&self, cell: u32) -> bool {
        self.state.get((cell & self.mask) as usize).is_some_and(|s| s.trained)
    }

    /// How many cells have a distribution.
    pub fn trained_cells(&self) -> usize {
        self.state.iter().filter(|s| s.trained).count()
    }

    /// The cached light leaving `cell`, once enough paths have told.
    #[inline]
    pub fn radiance(&self, cell: u32) -> Option<CachedRadiance> {
        self.state.get((cell & self.mask) as usize).and_then(|s| s.cache)
    }

    #[inline]
    fn table(&self, cell: u32) -> Option<&[f32]> {
        let cell = (cell & self.mask) as usize;
        if !self.state.get(cell)?.trained {
            return None;
        }
        self.cdf.get(cell * self.bins()..(cell + 1) * self.bins())
    }

    /// Density over solid angle with which [`Guide::sample`] returns `dir` in `cell` (zero for
    /// a cell without a distribution).
    #[inline]
    pub fn pdf(&self, cell: u32, dir: Vec3A) -> f32 {
        let Some(table) = self.table(cell) else { return 0.0 };
        if !dir.is_finite() {
            return 0.0;
        }
        let b = self.bin(dir);
        let hi = table.get(b).copied().unwrap_or(0.0);
        let lo = if b > 0 { table.get(b - 1).copied().unwrap_or(0.0) } else { 0.0 };
        (hi - lo).max(0.0) * self.bins() as f32 / (4.0 * PI)
    }

    /// A unit direction drawn from the distribution of `cell`, with its density over solid
    /// angle; `None` for a cell without a distribution.
    #[inline]
    pub fn sample(&self, cell: u32, u: Vec2) -> Option<(Vec3A, f32)> {
        let table = self.table(cell)?;
        let b = table.partition_point(|c| *c <= u.x).min(table.len().saturating_sub(1));
        let lo = if b > 0 { table.get(b - 1).copied().unwrap_or(0.0) } else { 0.0 };
        let span = table.get(b).copied().unwrap_or(1.0) - lo;
        if span <= 0.0 {
            return None;
        }
        let fx = ((u.x - lo) / span).clamp(0.0, 0.999_999);
        let res = self.res as f32;
        let (iu, iv) = ((b / self.res) as f32, (b % self.res) as f32);
        let y = ((iu + fx) / res * 2.0 - 1.0).clamp(-1.0, 1.0);
        let phi = ((iv + u.y.clamp(0.0, 0.999_999)) / res - 0.5) * 2.0 * PI;
        let r = (1.0 - y * y).max(0.0).sqrt();
        let dir = Vec3A::new(r * phi.cos(), y, r * phi.sin());
        // The density of the bin the direction is found in again (rounding at a bin's edge may
        // name its neighbour), so that sampling and `pdf` always agree.
        let pdf = self.pdf(cell, dir);
        (pdf > 0.0).then_some((dir, pdf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn guide() -> Guide {
        Guide::new(Vec3A::splat(-1.0), Vec3A::splat(1.0), GuideConfig { cells_log2: 8, min_records: 4, ..GuideConfig::default() })
    }

    fn halton(i: u32) -> Vec2 {
        let radical = |mut n: u32, base: u32| {
            let (mut f, mut r) = (1.0f32, 0.0f32);
            while n > 0 {
                f /= base as f32;
                r += f * (n % base) as f32;
                n /= base;
            }
            r
        };
        Vec2::new(radical(i + 1, 2), radical(i + 1, 3))
    }

    #[test]
    fn an_untrained_cell_has_no_distribution_and_no_cache() {
        let mut g = guide();
        let c = g.cell(Vec3A::ZERO, Vec3A::Y);
        assert!(!g.is_trained(c) && g.sample(c, Vec2::splat(0.5)).is_none() && g.pdf(c, Vec3A::Y) == 0.0 && g.radiance(c).is_none());
        // Too few records, or records without light, train nothing.
        g.record_direction(c, Vec3A::Y, 1.0);
        for _ in 0..10 {
            g.record_direction(c ^ 1, Vec3A::Y, 0.0);
        }
        g.update();
        assert!(!g.is_trained(c) && !g.is_trained(c ^ 1) && g.trained_cells() == 0);
    }

    #[test]
    fn a_cell_learns_where_light_comes_from() {
        let mut g = guide();
        let c = g.cell(Vec3A::new(0.2, 0.0, 0.3), Vec3A::Y);
        let bright = Vec3A::new(0.3, 0.8, -0.52).normalize();
        for i in 0..64 {
            g.record_direction(c, bright, 5.0);
            g.record_direction(c, Vec3A::new(-0.5, 0.2, 0.84).normalize(), if i % 8 == 0 { 0.1 } else { 0.0 });
            g.record_radiance(c, 2.0 + (i % 2) as f32);
        }
        g.update();
        assert!(g.is_trained(c) && g.trained_cells() == 1);
        // Most samples land in the bright bin, whose density is far above uniform (1 / 4π).
        let near = (0..1024).filter(|i| g.sample(c, halton(*i)).is_some_and(|(d, _)| d.dot(bright) > 0.9)).count();
        assert!(near > 800, "{near} of 1024 samples near the light");
        assert!(g.pdf(c, bright) > 10.0 / (4.0 * PI));
        // The uniform share keeps every direction possible.
        assert!(g.pdf(c, -bright) > 0.05 / (4.0 * PI));
        // The cache holds the mean and the second moment of what was recorded: 2 and 3 in turn.
        let cached = g.radiance(c).unwrap();
        assert!((cached.mean - 2.5).abs() < 1e-3 && (cached.second_moment - 6.5).abs() < 1e-3, "{cached:?}");
        g.reset();
        assert!(!g.is_trained(c) && g.radiance(c).is_none());
    }

    #[test]
    fn a_coarse_level_answers_until_the_fine_one_has_seen_enough() {
        let mut g = guide();
        // Sixteen points, each in a fine cell of its own, share coarser cells: one record is
        // too few for a fine cell (this guide asks for four), sixteen are enough for a coarse one.
        let points: Vec<Vec3A> = (0..16).map(|i| Vec3A::new(0.01 + (i % 4) as f32 * 0.08, 0.3, 0.01 + (i / 4) as f32 * 0.08)).collect();
        for p in &points {
            let cells = g.cells(*p, Vec3A::Y);
            for c in cells {
                g.record_direction(c, Vec3A::Y, 1.0);
                g.record_radiance(c, 1.0);
            }
        }
        g.update();
        let cells = g.cells(points[5], Vec3A::Y);
        assert!(!g.is_trained(cells[0]), "one record does not train a fine cell");
        let found = g.finest_trained(&cells).unwrap();
        assert!(found == cells[1] || found == cells[2]);
        assert!(g.finest_radiance(&cells).is_some_and(|c| (c.mean - 1.0).abs() < 1e-3));
        // The other side of the surface is another cell at every level.
        let below = g.cells(points[5], -Vec3A::Y);
        assert!(g.finest_trained(&below).is_none() && cells.iter().zip(&below).all(|(a, b)| a != b));
    }

    #[test]
    fn the_density_matches_the_samples_and_integrates_to_one() {
        let mut g = guide();
        let c = 7;
        for i in 0..4096 {
            let u = halton(i);
            let y = 1.0 - 2.0 * u.x;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let d = Vec3A::new(r * (u.y * std::f32::consts::TAU).cos(), y, r * (u.y * std::f32::consts::TAU).sin());
            g.record_direction(c, d, (d.y + 0.3).max(0.0) * (2.0 + d.x));
        }
        g.update();
        // The density sample() reports is the density pdf() reports for that direction.
        for i in 0..512 {
            let (d, p) = g.sample(c, halton(i)).unwrap();
            assert!((d.length() - 1.0).abs() < 1e-4 && (p - g.pdf(c, d)).abs() < 1e-4 * p.max(1.0), "{d} {p} {}", g.pdf(c, d));
        }
        // ∫ pdf dω = 1 (uniform directions, each worth 4π / n).
        let n = 16_384;
        let total: f64 = (0..n)
            .map(|i| {
                let u = halton(i);
                let y = 1.0 - 2.0 * u.x;
                let r = (1.0 - y * y).max(0.0).sqrt();
                f64::from(g.pdf(c, Vec3A::new(r * (u.y * std::f32::consts::TAU).cos(), y, r * (u.y * std::f32::consts::TAU).sin())))
            })
            .sum::<f64>()
            * 4.0
            * std::f64::consts::PI
            / f64::from(n);
        assert!((total - 1.0).abs() < 0.02, "{total}");
        // An estimator that samples the guide integrates a function over the sphere correctly:
        // ∫ max(y, 0) dω = π.
        let estimate: f64 = (0..n).map(|i| g.sample(c, halton(i)).map_or(0.0, |(d, p)| f64::from(d.y.max(0.0) / p))).sum::<f64>() / f64::from(n);
        assert!((estimate - std::f64::consts::PI).abs() < 0.05, "{estimate}");
    }

    #[test]
    fn recording_from_many_threads_gives_the_same_tables() {
        let record = |g: &Guide, i: u32| {
            let u = halton(i);
            let d = Vec3A::new(u.x - 0.5, u.y - 0.2, 0.4).normalize();
            g.record_direction(i % 5, d, 0.1 + u.x * 3.7);
            g.record_radiance(i % 5, u.y * 9.3);
        };
        let (mut a, mut b) = (guide(), guide());
        (0..20_000u32).for_each(|i| record(&a, i));
        (0..20_000u32).into_par_iter().for_each(|i| record(&b, i));
        a.update();
        b.update();
        assert_eq!(a.cdf, b.cdf);
        assert_eq!((0..5).map(|c| a.radiance(c)).collect::<Vec<_>>(), (0..5).map(|c| b.radiance(c)).collect::<Vec<_>>());
    }

    #[test]
    fn hostile_input_is_ignored() {
        let mut g = Guide::new(
            Vec3A::splat(f32::NAN),
            Vec3A::splat(f32::INFINITY),
            GuideConfig { cells_log2: 99, cell_fraction: f32::NAN, resolution: 0, min_records: 0 },
        );
        assert_eq!((g.cell_count(), g.bins()), (1 << MAX_CELLS_LOG2, 4));
        let c = g.cell(Vec3A::splat(f32::NAN), Vec3A::ZERO);
        g.record_direction(c, Vec3A::splat(f32::NAN), 1.0);
        g.record_direction(c, Vec3A::Y, f32::INFINITY);
        g.record_direction(u32::MAX, Vec3A::Y, f32::MAX);
        g.record_radiance(u32::MAX, f32::NAN);
        g.record_radiance(u32::MAX, -5.0);
        g.update();
        assert!(g.pdf(u32::MAX, Vec3A::splat(f32::NAN)) == 0.0);
        assert!(g.sample(u32::MAX, Vec2::new(f32::NAN, 7.0)).is_none_or(|(d, p)| d.is_finite() && p.is_finite()));
        assert_eq!(g.radiance(u32::MAX), Some(CachedRadiance { mean: 0.0, second_moment: 0.0 }));
    }

    proptest! {
        #[test]
        fn cells_and_samples_stay_in_range(x in -1e12f32..1e12, y in -5.0f32..5.0, z in -5.0f32..5.0, nx in -1.0f32..1.0, ny in -1.0f32..1.0, a in 0.0f32..1.0, b in 0.0f32..1.0) {
            let mut g = guide();
            let c = g.cell(Vec3A::new(x, y, z), Vec3A::new(nx, ny, 0.3));
            prop_assert!((c as usize) < g.cell_count());
            for i in 0..8 {
                g.record_direction(c, Vec3A::new(nx, ny, 0.3).normalize(), 1.0 + i as f32);
            }
            g.update();
            let (d, p) = g.sample(c, Vec2::new(a, b)).unwrap();
            prop_assert!((d.length() - 1.0).abs() < 1e-3 && p > 0.0 && p.is_finite());
            prop_assert!((g.pdf(c, d) - p).abs() <= 1e-3 * p.max(1.0));
        }
    }
}
