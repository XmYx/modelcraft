//! The frame cache: the points a simulation produced at each frame, bounded in size.
//!
//! It has the layout of a `.mcpc` point cache (the file reader and writer live in
//! `modelcraft-formats`, which converts to and from this type): `points` positions per frame,
//! frame after frame, sample `i` at scene frame `start + i·step`. A point that does not exist at a
//! frame (an unborn or dead particle) is stored as NaN, see [`is_live`].

use glam::Vec3;

use crate::SimError;

/// Most bytes of positions one cache holds (the point-cache file limit).
pub const MAX_CACHE_BYTES: usize = 1 << 31;
/// Most frames one cache holds.
pub const MAX_FRAMES: usize = 100_000;
/// Most points one frame holds.
pub const MAX_POINTS: usize = 4_000_000;

/// A point that exists at the frame (not the NaN a particle that is not alive is stored as).
pub fn is_live(p: Vec3) -> bool {
    p.is_finite()
}

/// The marker for a point that does not exist at a frame.
pub const DEAD: Vec3 = Vec3::NAN;

#[derive(Clone, Debug, PartialEq)]
pub struct FrameCache {
    pub fps: f64,
    /// Scene frame of the first sample.
    pub start: f64,
    /// Scene frames between samples.
    pub step: f64,
    points: usize,
    data: Vec<Vec3>,
}

impl FrameCache {
    /// An empty cache for `points` positions per frame. Non-finite or non-positive `fps` and
    /// `step` become 24 and 1.
    pub fn new(fps: f64, start: f64, step: f64, points: usize) -> FrameCache {
        let fps = if fps.is_finite() && fps > 0.0 { fps } else { 24.0 };
        let step = if step.is_finite() && step > 0.0 { step } else { 1.0 };
        let start = if start.is_finite() { start } else { 0.0 };
        FrameCache { fps, start, step, points: points.min(MAX_POINTS), data: Vec::new() }
    }

    /// Build a cache from `frames × points` positions already in hand (a file read).
    pub fn from_positions(fps: f64, start: f64, step: f64, points: usize, data: Vec<Vec3>) -> Result<FrameCache, SimError> {
        let mut c = FrameCache::new(fps, start, step, points);
        if c.points != points {
            return Err(SimError::Invalid(format!("{points} points per frame is more than the {MAX_POINTS} a cache holds")));
        }
        let frames = data.len().checked_div(points).unwrap_or(0);
        if data.len() != frames * points {
            return Err(SimError::Invalid("the positions are not a whole number of frames".into()));
        }
        if frames > MAX_FRAMES || data.len().saturating_mul(12) > MAX_CACHE_BYTES {
            return Err(SimError::CacheFull { limit: MAX_CACHE_BYTES });
        }
        c.data = data;
        Ok(c)
    }

    /// Points per frame.
    pub fn points(&self) -> usize {
        self.points
    }

    pub fn frames(&self) -> usize {
        self.data.len().checked_div(self.points).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Bytes of positions held.
    pub fn bytes(&self) -> usize {
        self.data.len() * 12
    }

    /// Every position, frame after frame.
    pub fn positions(&self) -> &[Vec3] {
        &self.data
    }

    /// The scene frame of the last sample (`start` when empty).
    pub fn end_frame(&self) -> f64 {
        self.start + self.frames().saturating_sub(1) as f64 * self.step
    }

    /// Add one frame. Its length must be the cache's point count, and the cache must stay within
    /// `limit_bytes` (at most [`MAX_CACHE_BYTES`]) and [`MAX_FRAMES`].
    pub fn push(&mut self, frame: &[Vec3], limit_bytes: usize) -> Result<(), SimError> {
        if frame.len() != self.points {
            return Err(SimError::Invalid(format!("a frame has {} points, the cache has {}", frame.len(), self.points)));
        }
        let limit = limit_bytes.min(MAX_CACHE_BYTES);
        if (self.data.len() + frame.len()) * 12 > limit || self.frames() >= MAX_FRAMES {
            return Err(SimError::CacheFull { limit });
        }
        self.data.extend_from_slice(frame);
        Ok(())
    }

    /// The positions of sample `i`.
    pub fn frame(&self, i: usize) -> Option<&[Vec3]> {
        let a = i.checked_mul(self.points)?;
        self.data.get(a..a.checked_add(self.points)?)
    }

    /// Keep only the first `frames` samples.
    pub fn truncate(&mut self, frames: usize) {
        self.data.truncate(frames.saturating_mul(self.points));
    }

    /// The positions at scene frame `frame`, linearly interpolated between the two samples around
    /// it; before the first sample the first one holds, after the last the last one. Returns
    /// `false` (leaving `out` empty) when the cache is empty. A point that is dead at either
    /// neighbouring sample is dead in between.
    pub fn sample(&self, frame: f64, out: &mut Vec<Vec3>) -> bool {
        out.clear();
        let n = self.frames();
        if n == 0 {
            return false;
        }
        let t = if frame.is_finite() { ((frame - self.start) / self.step).clamp(0.0, (n - 1) as f64) } else { 0.0 };
        let i = t.floor() as usize;
        let f = (t - i as f64) as f32;
        let (Some(a), Some(b)) = (self.frame(i), self.frame((i + 1).min(n - 1))) else { return false };
        if f <= 0.0 {
            out.extend_from_slice(a);
        } else {
            out.extend(a.iter().zip(b).map(|(p, q)| p.lerp(*q, f)));
        }
        true
    }

    /// A hash of every position's bits and the header, for comparing runs.
    pub fn checksum(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |x: u64| h = (h ^ x).wrapping_mul(0x0100_0000_01b3);
        mix(self.points as u64);
        mix(self.start.to_bits());
        mix(self.step.to_bits());
        for p in &self.data {
            for c in p.to_array() {
                mix(u64::from(c.to_bits()));
            }
        }
        h
    }

    /// Smallest and largest live position over every frame, or `None` without any.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.data.iter().copied().filter(|p| is_live(*p));
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_frames() -> FrameCache {
        let mut c = FrameCache::new(24.0, 10.0, 2.0, 2);
        c.push(&[Vec3::ZERO, Vec3::X], MAX_CACHE_BYTES).unwrap();
        c.push(&[Vec3::new(2.0, 0.0, 0.0), DEAD], MAX_CACHE_BYTES).unwrap();
        c
    }

    #[test]
    fn push_checks_the_frame_and_the_budget() {
        let mut c = FrameCache::new(24.0, 0.0, 1.0, 2);
        assert!(c.push(&[Vec3::ZERO], 1000).is_err(), "a frame of another size");
        c.push(&[Vec3::ZERO; 2], 48).unwrap();
        c.push(&[Vec3::ZERO; 2], 48).unwrap();
        assert!(matches!(c.push(&[Vec3::ZERO; 2], 48), Err(SimError::CacheFull { limit: 48 })));
        assert_eq!((c.frames(), c.bytes()), (2, 48));
    }

    #[test]
    fn sampling_interpolates_and_holds_at_both_ends() {
        let c = two_frames();
        let mut out = Vec::new();
        assert!(c.sample(10.0, &mut out));
        assert_eq!(out, vec![Vec3::ZERO, Vec3::X]);
        assert!(c.sample(11.0, &mut out));
        assert_eq!(out[0], Vec3::new(1.0, 0.0, 0.0), "halfway between the samples");
        assert!(!is_live(out[1]), "dead at one neighbour: dead between");
        c.sample(-5.0, &mut out);
        assert_eq!(out[0], Vec3::ZERO, "before the start the first frame holds");
        c.sample(99.0, &mut out);
        assert_eq!(out[0], Vec3::new(2.0, 0.0, 0.0), "after the end the last frame holds");
        c.sample(f64::NAN, &mut out);
        assert_eq!(out[0], Vec3::ZERO);
        assert!(!FrameCache::new(24.0, 0.0, 1.0, 3).sample(0.0, &mut out) && out.is_empty());
        assert_eq!(c.end_frame(), 12.0);
    }

    #[test]
    fn bounds_skip_dead_points_and_the_checksum_sees_every_bit() {
        let c = two_frames();
        assert_eq!(c.bounds(), Some((Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0))));
        let mut d = c.clone();
        assert_eq!(c.checksum(), d.checksum());
        d.data[0] = Vec3::new(f32::MIN_POSITIVE, 0.0, 0.0);
        assert_ne!(c.checksum(), d.checksum());
    }

    #[test]
    fn building_from_positions_refuses_ragged_data() {
        assert!(FrameCache::from_positions(24.0, 0.0, 1.0, 2, vec![Vec3::ZERO; 3]).is_err());
        assert!(FrameCache::from_positions(24.0, 0.0, 1.0, MAX_POINTS + 1, vec![]).is_err());
        let c = FrameCache::from_positions(f64::NAN, f64::NAN, -1.0, 2, vec![Vec3::ONE; 4]).unwrap();
        assert_eq!((c.fps, c.start, c.step, c.frames()), (24.0, 0.0, 1.0, 2));
        let mut c = c;
        c.truncate(1);
        assert_eq!(c.frames(), 1);
    }
}
