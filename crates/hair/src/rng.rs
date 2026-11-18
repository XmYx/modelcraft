//! Counter-based randomness. Every random number is a pure function of `(seed, index, stream)`, so
//! strand `i` is the same whatever the strand count is, whatever the thread schedule is, and a prefix
//! of the strand list is a uniform random subset of the whole (the basis of percentage display).

#[inline]
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A 64-bit hash of `(seed, index, stream)`.
#[inline]
pub(crate) fn hash(seed: u64, index: u64, stream: u64) -> u64 {
    mix(mix(mix(seed ^ 0x9e37_79b9_7f4a_7c15).wrapping_add(index.wrapping_mul(0xd6e8_feb8_6659_fd93))).wrapping_add(stream.wrapping_mul(0xa076_1d64_78bd_642f)))
}

/// A uniform float in `[0, 1)` from a hash (24 random bits).
#[inline]
pub(crate) fn unit(h: u64) -> f32 {
    (h >> 40) as f32 * (1.0 / 16_777_216.0)
}

/// `unit(hash(seed, index, stream))`.
#[inline]
pub(crate) fn rand(seed: u64, index: u64, stream: u64) -> f32 {
    unit(hash(seed, index, stream))
}

/// Smooth one-dimensional value noise in `[-1, 1]`: random lattice values joined by a smoothstep.
pub(crate) fn noise1(seed: u64, channel: u64, x: f32) -> f32 {
    let x = if x.is_finite() { x.clamp(-1.0e6, 1.0e6) } else { 0.0 };
    let i = x.floor();
    let f = x - i;
    let i = i as i64 as u64;
    let a = rand(seed, i, channel) * 2.0 - 1.0;
    let b = rand(seed, i.wrapping_add(1), channel) * 2.0 - 1.0;
    let s = f * f * (3.0 - 2.0 * f);
    a + (b - a) * s
}

/// Stream ids, so two uses of one `(seed, index)` never share numbers.
pub(crate) mod stream {
    pub const TRI: u64 = 0x10;
    pub const BARY_A: u64 = 0x11;
    pub const BARY_B: u64 = 0x12;
    pub const ACCEPT: u64 = 0x13;
    pub const LENGTH: u64 = 0x20;
    pub const WIDTH: u64 = 0x21;
    pub const CLUMP: u64 = 0x22;
    pub const FRIZZ_PHASE: u64 = 0x23;
    pub const FRIZZ_AXIS: u64 = 0x30;
}
