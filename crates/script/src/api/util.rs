//! Small helpers: a deterministic random generator.

use std::rc::Rc;

use rhai::{Dynamic, Engine, INT};

use super::conv::num;
use crate::error::{R, rt};
use crate::host::Shared;

/// SplitMix64: small, fast and the same on every machine, so a scatter with a seed is repeatable.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// In [0, 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

pub(crate) fn register(engine: &mut Engine, _s: &Rc<Shared>) {
    engine.register_type_with_name::<Rng>("Rng");
    engine.register_fn("rng", |seed: INT| Rng::new(seed as u64));
    engine.register_fn("next", |r: &mut Rng| r.unit());
    engine.register_fn("range", |r: &mut Rng, lo: Dynamic, hi: Dynamic| -> R<f64> {
        let (lo, hi) = (num(&lo)?, num(&hi)?);
        Ok(lo + (hi - lo) * r.unit())
    });
    engine.register_fn("int_range", |r: &mut Rng, lo: INT, hi: INT| -> R<INT> {
        if hi < lo {
            return Err(rt("int_range needs lo <= hi"));
        }
        let span = (hi as i128 - lo as i128 + 1) as u128;
        Ok((lo as i128 + (r.next_u64() as u128 % span) as i128) as INT)
    });
}
