//! Island packing: place UV islands in the unit square without overlap, with a margin, an
//! optional rotation and an even texel density.
//!
//! - *Density.* An island's texel density is `√(UV area / surface area)`. With
//!   [`Density::Uniform`] every island is first scaled so all densities match, so a small part
//!   gets as many texels per metre as a large one; [`Density::Keep`] keeps the islands' relative
//!   sizes.
//! - *Rotation.* The best angle of an island is the one that makes its bounding rectangle
//!   smallest, tried for up to 90 edges of its convex hull (the minimum-area rectangle of a convex
//!   polygon has a side on a hull edge, Freeman and Shapira 1975). The packer may also turn a
//!   rectangle by 90 degrees.
//! - *Packing.* Rectangles go into the square by MaxRects with best-short-side-fit (Jylänki, "A
//!   Thousand Ways to Pack the Bin", 2010): a list of the maximal free rectangles is kept, the
//!   rectangle goes into the one it fits tightest, and every free rectangle it overlaps is split.
//!   All islands share one scale, found by bisection as the largest that still fits.
//! - *Bounded time.* MaxRects is quadratic, so it runs only for up to [`MAX_RECTS_ISLANDS`] islands
//!   and within a work budget ([`MAX_WORK`]); beyond either, a linear next-fit shelf packer does
//!   the job (looser, but O(n) per scale).

use glam::{Mat2, Vec2};
use serde::{Deserialize, Serialize};

use crate::PackError;

/// Most islands one pack accepts.
pub const MAX_ISLANDS: usize = 100_000;
/// Most points (triangle corners) one pack accepts in total.
pub const MAX_POINTS: usize = 30_000_000;
/// Most islands MaxRects packs (one heuristic above 350, all four up to there); above it the
/// packer uses linear-time shelves (see `shelves`).
pub const MAX_RECTS_ISLANDS: usize = 500;
/// Free rectangles MaxRects may examine in one pack, across all scales tried: about a second on
/// a loaded desktop processor (`modelcraft-cli uv-pack-bench` shows 0.3 to 1.5 million per
/// millisecond, the work being the same on every run).
pub const MAX_WORK: u64 = 300_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    /// Islands keep their orientation.
    #[default]
    None,
    /// Islands may turn by 90 degrees.
    Quarter,
    /// Islands are first turned to their smallest bounding rectangle, and may then turn by 90 degrees.
    Fit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    /// Keep the islands' relative sizes.
    #[default]
    Keep,
    /// Scale every island to the same texel density.
    Uniform,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackOptions {
    /// Gap between islands, and half of it to the border, as a fraction of the square (0 to 0.1).
    pub margin: f32,
    pub rotation: Rotation,
    pub density: Density,
}

impl Default for PackOptions {
    fn default() -> Self {
        PackOptions { margin: 0.01, rotation: Rotation::Fit, density: Density::Uniform }
    }
}

/// One island to pack: its triangles in UV space and the surface area they cover.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackIsland {
    pub triangles: Vec<[Vec2; 3]>,
    /// Surface area of the island (for [`Density::Uniform`]); 0 when unknown.
    pub area_3d: f32,
}

impl PackIsland {
    /// Total area of the UV triangles.
    pub fn uv_area(&self) -> f32 {
        self.triangles.iter().map(|t| 0.5 * (t[1] - t[0]).perp_dot(t[2] - t[0]).abs()).sum()
    }
}

/// The affine map of one island: `uv' = matrix · uv + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub matrix: Mat2,
    pub offset: Vec2,
}

impl Placement {
    pub fn apply(&self, p: Vec2) -> Vec2 {
        self.matrix * p + self.offset
    }
}

/// The outcome of [`pack`].
#[derive(Clone, Debug, PartialEq)]
pub struct PackResult {
    /// One per island, in the order given.
    pub placements: Vec<Placement>,
    /// The scale all islands share on top of their density scaling.
    pub scale: f32,
    /// Share of the unit square the islands' triangles cover.
    pub utilisation: f32,
    /// Work done by the packing search (free rectangles examined; at most [`MAX_WORK`] plus the
    /// linear shelf passes), a measure of time that does not depend on the machine.
    pub work: u64,
    /// The MaxRects search ran out of its work budget (or there were too many islands) and the
    /// shelf packer finished the job: the layout is valid but looser.
    pub shelf_fallback: bool,
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    fn contains(&self, o: &Rect) -> bool {
        o.x >= self.x - 1e-12 && o.y >= self.y - 1e-12 && o.x + o.w <= self.x + self.w + 1e-12 && o.y + o.h <= self.y + self.h + 1e-12
    }
    fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w - 1e-12 && o.x < self.x + self.w - 1e-12 && self.y < o.y + o.h - 1e-12 && o.y < self.y + self.h - 1e-12
    }
}

/// Where each rectangle (width, height) goes in a `size`×`size` bin, or `None` when they do not
/// fit. A placement is `(x, y, turned)`; `turned` swaps the width and height.
///
/// `work` counts the free rectangles examined; when it passes `limit` the search stops with
/// `Err(OutOfBudget)`, so the running time is bounded whatever the input.
fn max_rects(
    sizes: &[(f64, f64)],
    order: &[usize],
    size: f64,
    allow_turn: bool,
    area_fit: bool,
    work: &mut u64,
    limit: u64,
) -> Result<Option<Vec<(f64, f64, bool)>>, OutOfBudget> {
    let mut free = vec![Rect { x: 0.0, y: 0.0, w: size, h: size }];
    let mut out = vec![(0.0, 0.0, false); sizes.len()];
    for &i in order {
        let Some(&(w, h)) = sizes.get(i) else { return Ok(None) };
        *work += free.len() as u64;
        if *work > limit {
            return Err(OutOfBudget);
        }
        // Best short side fit over every free rectangle, in both orientations when allowed.
        let mut best: Option<(f64, f64, Rect, bool)> = None;
        for f in &free {
            for turned in [false, true] {
                if turned && !allow_turn {
                    continue;
                }
                let (rw, rh) = if turned { (h, w) } else { (w, h) };
                if rw <= f.w + 1e-12 && rh <= f.h + 1e-12 {
                    let (short, long) = if area_fit {
                        // Best area fit: the smallest free rectangle, ties by the short side.
                        (f.w * f.h - rw * rh, (f.w - rw).min(f.h - rh))
                    } else {
                        ((f.w - rw).min(f.h - rh), (f.w - rw).max(f.h - rh))
                    };
                    let better = best.as_ref().is_none_or(|b| (short, long) < (b.0, b.1));
                    if better {
                        best = Some((short, long, Rect { x: f.x, y: f.y, w: rw, h: rh }, turned));
                    }
                }
            }
        }
        let Some((_, _, placed, turned)) = best else { return Ok(None) };
        if let Some(o) = out.get_mut(i) {
            *o = (placed.x, placed.y, turned);
        }
        // Split every free rectangle the new one overlaps into its (up to four) maximal remainders.
        let mut next: Vec<Rect> = Vec::with_capacity(free.len() + 4);
        let mut pieces: Vec<Rect> = Vec::new();
        for f in &free {
            if !f.overlaps(&placed) {
                next.push(*f);
                continue;
            }
            if placed.x > f.x {
                pieces.push(Rect { x: f.x, y: f.y, w: placed.x - f.x, h: f.h });
            }
            if placed.x + placed.w < f.x + f.w {
                pieces.push(Rect { x: placed.x + placed.w, y: f.y, w: f.x + f.w - placed.x - placed.w, h: f.h });
            }
            if placed.y > f.y {
                pieces.push(Rect { x: f.x, y: f.y, w: f.w, h: placed.y - f.y });
            }
            if placed.y + placed.h < f.y + f.h {
                pieces.push(Rect { x: f.x, y: placed.y + placed.h, w: f.w, h: f.y + f.h - placed.y - placed.h });
            }
        }
        // A new piece stays unless an older free rectangle or an earlier piece contains it (an
        // older rectangle can never sit inside a piece: pieces are parts of older rectangles).
        pieces.retain(|r| r.w > 1e-12 && r.h > 1e-12);
        *work += (pieces.len() * (next.len() + 4)) as u64;
        if *work > limit {
            return Err(OutOfBudget);
        }
        let mut kept: Vec<Rect> = Vec::with_capacity(pieces.len());
        for r in &pieces {
            if !next.iter().chain(kept.iter()).any(|o| o.contains(r)) {
                kept.push(*r);
            }
        }
        next.extend(kept);
        free = next;
    }
    Ok(Some(out))
}

struct OutOfBudget;

/// Next-fit shelf packing, O(n): rectangles in the given order go left to right along a shelf, a
/// new shelf starts when one does not fit. With `allow_turn` each rectangle lies with its longer
/// side along the shelf. Much looser than MaxRects, but linear: it is used above
/// [`MAX_RECTS_ISLANDS`] islands and when the MaxRects work budget runs out.
fn shelves(sizes: &[(f64, f64)], order: &[usize], size: f64, allow_turn: bool) -> Option<Vec<(f64, f64, bool)>> {
    let mut out = vec![(0.0, 0.0, false); sizes.len()];
    let (mut x, mut y, mut row) = (0.0f64, 0.0f64, 0.0f64);
    for &i in order {
        let &(w0, h0) = sizes.get(i)?;
        let turned = allow_turn && h0 > w0;
        let (w, h) = if turned { (h0, w0) } else { (w0, h0) };
        if x + w > size + 1e-12 && x > 0.0 {
            x = 0.0;
            y += row;
            row = 0.0;
        }
        if x + w > size + 1e-12 || y + h > size + 1e-12 {
            return None;
        }
        *out.get_mut(i)? = (x, y, turned);
        x += w;
        row = row.max(h);
    }
    Some(out)
}

/// Convex hull of points (Andrew's monotone chain), counter-clockwise, without repeats.
fn hull(points: &mut Vec<Vec2>) -> Vec<Vec2> {
    points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    points.dedup();
    if points.len() < 3 {
        return points.clone();
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut h: Vec<Vec2> = Vec::with_capacity(points.len() * 2);
    for &p in points.iter() {
        while h.len() >= 2 && cross(h[h.len() - 2], h[h.len() - 1], p) <= 0.0 {
            h.pop();
        }
        h.push(p);
    }
    let lower = h.len() + 1;
    for &p in points.iter().rev().skip(1) {
        while h.len() >= lower && cross(h[h.len() - 2], h[h.len() - 1], p) <= 0.0 {
            h.pop();
        }
        h.push(p);
    }
    h.pop();
    h
}

fn bounds(points: &[Vec2], m: Mat2) -> (Vec2, Vec2) {
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in points {
        let q = m * *p;
        lo = lo.min(q);
        hi = hi.max(q);
    }
    (lo, hi)
}

/// Pack `islands` into the unit square.
pub fn pack(islands: &[PackIsland], opts: &PackOptions) -> Result<PackResult, PackError> {
    pack_with_limit(islands, opts, MAX_WORK)
}

/// [`pack`] with an explicit work budget for the MaxRects search (see [`PackResult::work`]).
pub fn pack_with_limit(islands: &[PackIsland], opts: &PackOptions, work_limit: u64) -> Result<PackResult, PackError> {
    if islands.len() > MAX_ISLANDS {
        return Err(PackError::TooLarge(islands.len()));
    }
    let total: usize = islands.iter().map(|i| i.triangles.len()).sum();
    if total.saturating_mul(3) > MAX_POINTS {
        return Err(PackError::TooLarge(total));
    }
    if islands.is_empty() {
        return Ok(PackResult { placements: Vec::new(), scale: 1.0, utilisation: 0.0, work: 0, shelf_fallback: false });
    }
    if islands.iter().any(|i| i.triangles.iter().flatten().any(|p| !p.is_finite())) {
        return Err(PackError::NonFinite);
    }
    let margin = if opts.margin.is_finite() { f64::from(opts.margin.clamp(0.0, 0.1)) } else { 0.0 };

    // 1. Density: one scale per island, so that UV area / surface area is the same everywhere.
    let uv_areas: Vec<f32> = islands.iter().map(PackIsland::uv_area).collect();
    let mut dens = vec![1.0f32; islands.len()];
    if opts.density == Density::Uniform {
        let (a3, auv): (f32, f32) =
            islands.iter().zip(&uv_areas).filter(|(i, a)| i.area_3d > 0.0 && **a > 0.0).fold((0.0, 0.0), |(s3, su), (i, a)| (s3 + i.area_3d, su + a));
        if a3 > 0.0 && auv > 0.0 {
            // Target: the density of the whole set, so the total UV area is unchanged.
            let target = (auv / a3).sqrt();
            for ((d, i), a) in dens.iter_mut().zip(islands).zip(&uv_areas) {
                if i.area_3d > 0.0 && *a > 0.0 {
                    *d = target * (i.area_3d / *a).sqrt();
                }
            }
        }
    }

    // 2. Rotation and bounding rectangle of each island.
    struct Prep {
        /// Maps the island's UVs to its rectangle at the origin (density and rotation applied).
        base: Mat2,
        shift: Vec2,
        w: f64,
        h: f64,
    }
    let mut preps: Vec<Prep> = Vec::with_capacity(islands.len());
    for (isl, d) in islands.iter().zip(&dens) {
        let mut pts: Vec<Vec2> = isl.triangles.iter().flatten().copied().collect();
        let ch = if pts.is_empty() { Vec::new() } else { hull(&mut pts) };
        let mut best = (Mat2::from_diagonal(Vec2::splat(*d)), f32::MAX);
        let area_of = |m: Mat2| {
            let (lo, hi) = bounds(&ch, m);
            (hi.x - lo.x) * (hi.y - lo.y)
        };
        best.1 = area_of(best.0);
        if opts.rotation == Rotation::Fit && ch.len() >= 3 {
            let step = ch.len().div_ceil(90).max(1);
            for k in (0..ch.len()).step_by(step) {
                let e = ch[(k + 1) % ch.len()] - ch[k];
                if e.length_squared() <= 0.0 {
                    continue;
                }
                let a = -e.y.atan2(e.x);
                let m = Mat2::from_angle(a) * *d;
                let area = area_of(m);
                if area < best.1 * 0.9999 {
                    best = (m, area);
                }
            }
        }
        let (lo, hi) = if ch.is_empty() { (Vec2::ZERO, Vec2::ZERO) } else { bounds(&ch, best.0) };
        preps.push(Prep { base: best.0, shift: -lo, w: f64::from((hi.x - lo.x).max(1e-6)), h: f64::from((hi.y - lo.y).max(1e-6)) });
    }

    // 3. The largest common scale that fits.
    let allow_turn = opts.rotation != Rotation::None;
    let mut by_side: Vec<usize> = (0..preps.len()).collect();
    by_side.sort_by(|a, b| (preps[*b].w.max(preps[*b].h)).total_cmp(&preps[*a].w.max(preps[*a].h)).then(a.cmp(b)));
    let mut by_area = by_side.clone();
    by_area.sort_by(|a, b| (preps[*b].w * preps[*b].h).total_cmp(&(preps[*a].w * preps[*a].h)).then(a.cmp(b)));
    // Several heuristics at each scale (each a different packing order and fit rule); the first
    // that fits wins. Large sets, and a search that has used up its work budget, use shelves.
    let all_tries: [(&[usize], bool); 4] = [(&by_side, false), (&by_area, false), (&by_side, true), (&by_area, true)];
    // Above 350 islands one heuristic is tried per scale, to stay within the work budget.
    let tries: &[(&[usize], bool)] = if preps.len() > 350 { &all_tries[..1] } else { &all_tries };
    // Shelves: tallest first, where an island's height is its shorter side when it may turn.
    let by_height: Vec<usize> = {
        let height = |p: &Prep| if allow_turn { p.h.min(p.w) } else { p.h };
        let mut o = by_side.clone();
        o.sort_by(|a, b| height(&preps[*b]).total_cmp(&height(&preps[*a])).then(a.cmp(b)));
        o
    };
    let work = std::cell::Cell::new(0u64);
    let fell_back = std::cell::Cell::new(preps.len() > MAX_RECTS_ISLANDS);
    let try_scale = |s: f64| -> Option<Vec<(f64, f64, bool)>> {
        let sizes: Vec<(f64, f64)> = preps.iter().map(|p| (p.w * s + margin, p.h * s + margin)).collect();
        if !fell_back.get() {
            for (order, area_fit) in tries {
                let mut w = work.get();
                let r = max_rects(&sizes, order, 1.0, allow_turn, *area_fit, &mut w, work_limit);
                work.set(w);
                match r {
                    Ok(Some(p)) => return Some(p),
                    Ok(None) => {}
                    Err(OutOfBudget) => {
                        fell_back.set(true);
                        break;
                    }
                }
            }
        }
        // The linear shelf layout is also a candidate at every scale: on some inputs it fits where
        // MaxRects' greedy choices do not, and it is the only one when the budget is gone.
        work.set(work.get() + sizes.len() as u64);
        shelves(&sizes, &by_height, 1.0, allow_turn)
    };
    let area_sum: f64 = preps.iter().map(|p| p.w * p.h).sum();
    let biggest = preps.iter().map(|p| p.w.max(p.h)).fold(1e-9, f64::max);
    // No scale above the one at which the largest side, or all the rectangle area, fills the square.
    let mut hi = ((1.0 - margin) / biggest).min(1.0 / area_sum.sqrt().max(1e-9)).max(1e-9);
    let mut lo = 0.0;
    let mut best_fit: Option<Vec<(f64, f64, bool)>> = None;
    // A tiny scale fits unless the margin alone leaves no room.
    let floor = hi * 1e-4;
    if let Some(p) = try_scale(floor) {
        best_fit = Some(p);
        lo = floor;
    }
    for _ in 0..20 {
        let mid = 0.5 * (lo + hi);
        if let Some(p) = try_scale(mid) {
            best_fit = Some(p);
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let places = best_fit.ok_or(PackError::NoRoom)?;
    let s = lo;

    // 4. Build the maps.
    let mut placements = Vec::with_capacity(preps.len());
    for (p, &(x, y, turned)) in preps.iter().zip(&places) {
        let mut m = p.base * (s as f32);
        let mut off = p.shift * (s as f32);
        let h = p.h * s;
        if turned {
            // Turn by 90 degrees about the rectangle's centre: (x, y) → (h − y, x), i.e. a width-w
            // rectangle becomes height-w.
            let r = Mat2::from_cols(Vec2::new(0.0, 1.0), Vec2::new(-1.0, 0.0));
            m = r * m;
            off = r * off + Vec2::new(h as f32, 0.0);
        }
        off += Vec2::new((x + margin * 0.5) as f32, (y + margin * 0.5) as f32);
        placements.push(Placement { matrix: m, offset: off });
    }
    let util: f64 = islands.iter().zip(&placements).map(|(i, pl)| f64::from(i.uv_area()) * f64::from(pl.matrix.determinant().abs())).sum();
    Ok(PackResult { placements, scale: s as f32, utilisation: util as f32, work: work.get(), shelf_fallback: fell_back.get() })
}
