//! Run-length forms for the per-face arrays of a sculpt layer, so a project's JSON stays small:
//! patches as `[value, count, value, count, …]`, hidden faces as the lengths of alternating
//! runs starting with a shown one (`[3, 2]`: three shown, two hidden).
//!
//! Reading is bounded: a file cannot make these expand past [`modelcraft_mesh::MAX_ELEMENTS`].

use std::sync::Arc;

use modelcraft_mesh::MAX_ELEMENTS;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

fn too_long<E: serde::de::Error>() -> E {
    E::custom(format!("a per-face array longer than {MAX_ELEMENTS} entries"))
}

pub mod patches {
    use super::*;

    pub fn serialize<S: Serializer>(v: &Arc<Vec<u32>>, s: S) -> Result<S::Ok, S::Error> {
        let mut out: Vec<u32> = Vec::new();
        let mut run: Option<(u32, u32)> = None;
        for x in v.iter() {
            run = match run {
                Some((value, n)) if value == *x && n < u32::MAX => Some((value, n + 1)),
                Some((value, n)) => {
                    out.extend([value, n]);
                    Some((*x, 1))
                }
                None => Some((*x, 1)),
            };
        }
        if let Some((value, n)) = run {
            out.extend([value, n]);
        }
        out.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Vec<u32>>, D::Error> {
        let runs = Vec::<u32>::deserialize(d)?;
        let total: usize = runs.chunks(2).map(|c| c.get(1).copied().unwrap_or(0) as usize).fold(0usize, usize::saturating_add);
        if total > MAX_ELEMENTS {
            return Err(too_long());
        }
        let mut out = Vec::with_capacity(total);
        for c in runs.chunks(2) {
            if let [value, n] = c {
                out.extend(std::iter::repeat_n(*value, *n as usize));
            } else {
                return Err(D::Error::custom("patch runs come in pairs"));
            }
        }
        Ok(Arc::new(out))
    }
}

pub mod hidden {
    use super::*;

    pub fn serialize<S: Serializer>(v: &Arc<Vec<bool>>, s: S) -> Result<S::Ok, S::Error> {
        let mut out: Vec<u32> = Vec::new();
        let (mut state, mut n) = (false, 0u32);
        for x in v.iter() {
            if *x != state || n == u32::MAX {
                out.push(n);
                if *x == state {
                    // A run longer than a u32: an empty run of the other kind keeps the order.
                    out.push(0);
                } else {
                    state = *x;
                }
                n = 0;
            }
            n += 1;
        }
        if n > 0 {
            out.push(n);
        }
        out.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Vec<bool>>, D::Error> {
        let runs = Vec::<u32>::deserialize(d)?;
        let total: usize = runs.iter().map(|n| *n as usize).fold(0usize, usize::saturating_add);
        if total > MAX_ELEMENTS {
            return Err(too_long());
        }
        let mut out = Vec::with_capacity(total);
        let mut state = false;
        for n in runs {
            out.extend(std::iter::repeat_n(state, n as usize));
            state = !state;
        }
        Ok(Arc::new(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Both {
        #[serde(with = "patches")]
        p: Arc<Vec<u32>>,
        #[serde(with = "hidden")]
        h: Arc<Vec<bool>>,
    }

    #[test]
    fn runs_round_trip_and_stay_short() {
        let cases: [(Vec<u32>, Vec<bool>); 4] = [
            (vec![], vec![]),
            (vec![7], vec![true]),
            (vec![1, 1, 1, 2, 2, 0, 0, 0, 0], vec![false, false, true, true, true, false]),
            ((0..1000).map(|i| i / 250).collect(), (0..1000).map(|i| i % 500 < 100).collect()),
        ];
        for (p, h) in cases {
            let v = Both { p: Arc::new(p), h: Arc::new(h) };
            let json = serde_json::to_string(&v).unwrap();
            assert!(json.len() < 120, "{json}");
            assert_eq!(serde_json::from_str::<Both>(&json).unwrap(), v);
        }
        assert_eq!(
            serde_json::to_string(&Both { p: Arc::new(vec![1, 1, 1, 2]), h: Arc::new(vec![true, true, false]) }).unwrap(),
            r#"{"p":[1,3,2,1],"h":[0,2,1]}"#
        );
    }

    #[test]
    fn a_file_cannot_ask_for_endless_runs() {
        let big = u32::MAX;
        assert!(serde_json::from_str::<Both>(&format!(r#"{{"p":[1,{big}],"h":[]}}"#)).is_err());
        assert!(serde_json::from_str::<Both>(&format!(r#"{{"p":[],"h":[{big},{big}]}}"#)).is_err());
        assert!(serde_json::from_str::<Both>(r#"{"p":[1],"h":[]}"#).is_err(), "half a pair");
    }
}
