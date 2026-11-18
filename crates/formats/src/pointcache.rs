//! Point cache: the positions of one mesh's vertices at a run of frames, for handing deformed
//! or animated geometry to a renderer or another tool without the scene that made it.
//!
//! ```text
//! "MCPC" u32:version=1 u32:vertices u32:frames f64:fps f64:start f64:step
//!        frames × vertices × f32[3]
//! ```
//!
//! Little-endian. Frame `i` is at scene frame `start + i·step`. Vertex order is the order of the
//! mesh the cache was baked from. Readers check the counts against the bytes present before
//! allocating, and refuse non-finite header values.

use glam::Vec3;

use crate::{FormatError, Result};

pub const EXTENSION: &str = "mcpc";
const MAGIC: &[u8; 4] = b"MCPC";
const VERSION: u32 = 1;
const HEADER: usize = 16 + 24;
/// Most positions (vertices × frames) one cache holds: 2³¹ bytes of floats.
pub const MAX_POSITIONS: u64 = (1 << 31) / 12;

#[derive(Clone, Debug, PartialEq)]
pub struct PointCache {
    pub fps: f64,
    /// Scene frame of the first sample.
    pub start: f64,
    /// Scene frames between samples.
    pub step: f64,
    pub vertices: usize,
    /// `frames × vertices` positions, frame after frame.
    pub positions: Vec<Vec3>,
}

fn bad(m: &str) -> FormatError {
    FormatError::Malformed(format!("point cache: {m}"))
}

impl PointCache {
    pub fn new(fps: f64, start: f64, step: f64, vertices: usize) -> PointCache {
        PointCache { fps, start, step, vertices, positions: Vec::new() }
    }

    pub fn frames(&self) -> usize {
        self.positions.len().checked_div(self.vertices).unwrap_or(0)
    }

    /// Add one frame; its length must be the cache's vertex count.
    pub fn push(&mut self, frame: &[Vec3]) -> Result<()> {
        if frame.len() != self.vertices {
            return Err(bad(&format!("a frame has {} points, the cache has {}", frame.len(), self.vertices)));
        }
        if (self.positions.len() + frame.len()) as u64 > MAX_POSITIONS {
            return Err(bad("too large"));
        }
        self.positions.extend_from_slice(frame);
        Ok(())
    }

    /// The positions of sample `i`.
    pub fn frame(&self, i: usize) -> Option<&[Vec3]> {
        let a = i.checked_mul(self.vertices)?;
        self.positions.get(a..a.checked_add(self.vertices)?)
    }

    /// The cache of a simulation, for writing it to a file or a project blob.
    pub fn from_frame_cache(c: &modelcraft_sim::FrameCache) -> PointCache {
        PointCache { fps: c.fps, start: c.start, step: c.step, vertices: c.points(), positions: c.positions().to_vec() }
    }

    /// The simulation cache this file holds, for playing it on a mesh. Fails when it is larger
    /// than a simulation cache may be.
    pub fn to_frame_cache(&self) -> Result<modelcraft_sim::FrameCache> {
        let frames = self.frames();
        let data = self.positions.get(..frames * self.vertices).unwrap_or_default().to_vec();
        modelcraft_sim::FrameCache::from_positions(self.fps, self.start, self.step, self.vertices, data).map_err(|e| bad(&e.to_string()))
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let (v, f) = (u32::try_from(self.vertices).map_err(|_| bad("too many vertices"))?, u32::try_from(self.frames()).map_err(|_| bad("too many frames"))?);
        if ![self.fps, self.start, self.step].iter().all(|x| x.is_finite()) {
            return Err(bad("the header has a non-finite number"));
        }
        let mut out = Vec::with_capacity(HEADER + self.positions.len() * 12);
        out.extend_from_slice(MAGIC);
        for x in [VERSION, v, f] {
            out.extend_from_slice(&x.to_le_bytes());
        }
        for x in [self.fps, self.start, self.step] {
            out.extend_from_slice(&x.to_le_bytes());
        }
        for p in self.positions.iter().take(self.frames() * self.vertices) {
            for c in p.to_array() {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        Ok(out)
    }

    pub fn from_bytes(b: &[u8]) -> Result<PointCache> {
        let head = b.get(..HEADER).ok_or_else(|| bad("truncated header"))?;
        if head.get(..4) != Some(MAGIC.as_slice()) {
            return Err(bad("not a point cache"));
        }
        let u = |at: usize| head.get(at..at + 4).and_then(|s| <[u8; 4]>::try_from(s).ok()).map(u32::from_le_bytes).unwrap_or(0);
        let d = |at: usize| head.get(at..at + 8).and_then(|s| <[u8; 8]>::try_from(s).ok()).map(f64::from_le_bytes).unwrap_or(f64::NAN);
        if u(4) != VERSION {
            return Err(FormatError::Unsupported(format!("point cache version {}", u(4))));
        }
        let (vertices, frames) = (u(8) as u64, u(12) as u64);
        let (fps, start, step) = (d(16), d(24), d(32));
        if ![fps, start, step].iter().all(|x| x.is_finite()) {
            return Err(bad("the header has a non-finite number"));
        }
        let count = vertices.checked_mul(frames).filter(|c| *c <= MAX_POSITIONS).ok_or_else(|| bad("too large"))?;
        let body = b.get(HEADER..).unwrap_or_default();
        if body.len() as u64 != count * 12 {
            return Err(bad("the data does not match the header"));
        }
        let (points, _) = body.as_chunks::<12>();
        let positions = points
            .iter()
            .map(|c| {
                let f = |i: usize| c.get(i..i + 4).and_then(|s| <[u8; 4]>::try_from(s).ok()).map(f32::from_le_bytes).unwrap_or(0.0);
                Vec3::new(f(0), f(4), f(8))
            })
            .collect();
        Ok(PointCache { fps, start, step, vertices: vertices as usize, positions })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn round_trip_and_frame_access() {
        let mut c = PointCache::new(24.0, 10.0, 0.5, 2);
        c.push(&[Vec3::ZERO, Vec3::ONE]).unwrap();
        c.push(&[Vec3::X, Vec3::Y]).unwrap();
        assert!(c.push(&[Vec3::X]).is_err(), "a frame of another size");
        let bytes = c.to_bytes().unwrap();
        assert_eq!(bytes.len(), 40 + 4 * 12);
        let back = PointCache::from_bytes(&bytes).unwrap();
        assert_eq!(back, c);
        assert_eq!((back.frames(), back.frame(1)), (2, Some(&[Vec3::X, Vec3::Y][..])));
        assert_eq!(back.frame(2), None);
        assert_eq!(PointCache::new(24.0, 0.0, 1.0, 0).frames(), 0);
    }

    #[test]
    fn hostile_files_are_refused() {
        let mut c = PointCache::new(24.0, 0.0, 1.0, 1);
        c.push(&[Vec3::ONE]).unwrap();
        let good = c.to_bytes().unwrap();
        assert!(PointCache::from_bytes(&good[..20]).is_err());
        assert!(PointCache::from_bytes(&good[..good.len() - 1]).is_err());
        let mut b = good.clone();
        b[0] = b'X';
        assert!(PointCache::from_bytes(&b).is_err());
        let mut b = good.clone();
        b[4] = 9;
        assert!(matches!(PointCache::from_bytes(&b), Err(FormatError::Unsupported(_))));
        // A header that promises 4 billion × 4 billion points.
        let mut b = good.clone();
        b[8..16].fill(0xFF);
        assert!(PointCache::from_bytes(&b).is_err());
        let mut b = good;
        b[16..24].copy_from_slice(&f64::NAN.to_le_bytes());
        assert!(PointCache::from_bytes(&b).is_err());
    }

    #[test]
    fn converts_to_and_from_a_simulation_cache() {
        let mut c = PointCache::new(30.0, 5.0, 1.0, 2);
        c.push(&[Vec3::ZERO, Vec3::X]).unwrap();
        c.push(&[Vec3::Y, Vec3::Z]).unwrap();
        let f = c.to_frame_cache().unwrap();
        assert_eq!((f.fps, f.start, f.step, f.points(), f.frames()), (30.0, 5.0, 1.0, 2, 2));
        assert_eq!(PointCache::from_frame_cache(&f), c);
        // A file with more frames than a simulation cache holds is refused, not truncated.
        let mut big = PointCache::new(24.0, 0.0, 1.0, 1);
        for _ in 0..=modelcraft_sim::MAX_FRAMES {
            big.push(&[Vec3::ZERO]).unwrap();
        }
        assert!(big.to_frame_cache().is_err());
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
            let _ = PointCache::from_bytes(&bytes);
            let mut b = b"MCPC\x01\0\0\0".to_vec();
            b.extend(bytes);
            let _ = PointCache::from_bytes(&b);
        }
    }
}
