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

/// A command-line argument as the most specific script value: a switch, a whole number, a
/// number or text.
fn parse_arg(v: &str) -> Dynamic {
    if v == "true" || v == "false" {
        Dynamic::from_bool(v == "true")
    } else if let Ok(i) = v.parse::<INT>() {
        Dynamic::from_int(i)
    } else if let Some(f) = v.parse::<f64>().ok().filter(|f| f.is_finite()) {
        Dynamic::from_float(f)
    } else {
        Dynamic::from(v.to_string())
    }
}

pub(crate) fn register(engine: &mut Engine, s: &Rc<Shared>) {
    let sh = s.clone();
    engine.register_fn("args", move || -> R<rhai::Map> {
        let a = sh.args.try_borrow().map_err(|_| rt("the arguments are busy"))?;
        Ok(a.iter().map(|(k, v)| (k.as_str().into(), parse_arg(v))).collect())
    });
    let sh = s.clone();
    engine.register_fn("arg", move |name: &str, default: Dynamic| -> R<Dynamic> {
        let a = sh.args.try_borrow().map_err(|_| rt("the arguments are busy"))?;
        Ok(a.get(name).map(|v| parse_arg(v)).unwrap_or(default))
    });
    let sh = s.clone();
    engine.register_fn("current_file", move || -> R<Dynamic> {
        let f = sh.file.try_borrow().map_err(|_| rt("the file name is busy"))?;
        Ok(f.clone().map(Dynamic::from).unwrap_or(Dynamic::UNIT))
    });
    engine.register_fn("round_to", |x: Dynamic, digits: INT| -> R<f64> {
        let x = num(&x)?;
        let k = 10f64.powi(digits.clamp(0, 12) as i32);
        Ok((x * k).round() / k)
    });

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
