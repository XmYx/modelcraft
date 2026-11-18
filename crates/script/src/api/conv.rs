//! Converting between script values and engine values. Every function checks its input: a script
//! may pass anything.

use glam::Vec3;
use modelcraft_engine::scene::ObjectId;
use rhai::{Array, Dynamic, INT, Map};

use crate::error::{R, rt};

/// A number (an integer or a float) as a float; it must be finite.
pub fn num(d: &Dynamic) -> R<f64> {
    let v = if let Ok(i) = d.as_int() {
        i as f64
    } else if let Ok(f) = d.as_float() {
        f
    } else {
        return Err(rt(format!("expected a number, found {}", d.type_name())));
    };
    if v.is_finite() { Ok(v) } else { Err(rt("a number must be finite")) }
}

pub fn f32_of(d: &Dynamic) -> R<f32> {
    Ok(num(d)? as f32)
}

pub fn vec3(d: &Dynamic) -> R<Vec3> {
    let a = d.clone().into_array().map_err(|_| rt(format!("expected [x, y, z], found {}", d.type_name())))?;
    match a.as_slice() {
        [x, y, z] => Ok(Vec3::new(f32_of(x)?, f32_of(y)?, f32_of(z)?)),
        _ => Err(rt(format!("expected [x, y, z], found {} values", a.len()))),
    }
}

pub fn xyz(x: &Dynamic, y: &Dynamic, z: &Dynamic) -> R<Vec3> {
    Ok(Vec3::new(f32_of(x)?, f32_of(y)?, f32_of(z)?))
}

pub fn to_array(v: Vec3) -> Dynamic {
    Dynamic::from_array(vec![Dynamic::from_float(v.x as f64), Dynamic::from_float(v.y as f64), Dynamic::from_float(v.z as f64)])
}

pub fn id_of(i: INT) -> R<ObjectId> {
    u64::try_from(i).map(ObjectId).map_err(|_| rt(format!("{i} is not an object id")))
}

pub fn id_dyn(id: ObjectId) -> Dynamic {
    Dynamic::from_int(id.0.min(i64::MAX as u64) as INT)
}

pub fn ids_array(ids: impl IntoIterator<Item = ObjectId>) -> Array {
    ids.into_iter().map(id_dyn).collect()
}

/// One id or an array of ids.
pub fn ids_from(d: &Dynamic) -> R<Vec<ObjectId>> {
    if let Ok(i) = d.as_int() {
        return Ok(vec![id_of(i)?]);
    }
    let a = d.clone().into_array().map_err(|_| rt(format!("expected an object id or an array of ids, found {}", d.type_name())))?;
    a.iter().map(|x| x.as_int().map_err(|_| rt("expected an object id")).and_then(id_of)).collect()
}

pub fn to_json(d: &Dynamic) -> R<serde_json::Value> {
    rhai::serde::from_dynamic::<serde_json::Value>(d).map_err(|e| rt(format!("cannot read this value: {e}")))
}

pub fn from_json(v: &serde_json::Value) -> R<Dynamic> {
    rhai::serde::to_dynamic(v).map_err(|e| rt(format!("cannot convert this value: {e}")))
}

pub fn map_of(pairs: Vec<(&str, Dynamic)>) -> Map {
    pairs.into_iter().map(|(k, v)| (k.into(), v)).collect()
}

pub fn int(n: usize) -> Dynamic {
    Dynamic::from_int(n.min(i64::MAX as usize) as INT)
}

/// `a.get(i)` with a clear error.
pub fn index(i: INT, len: usize, what: &str) -> R<u32> {
    match u32::try_from(i) {
        Ok(v) if (v as usize) < len => Ok(v),
        _ => Err(rt(format!("{what} {i} is out of range (there are {len}, numbered from 0)"))),
    }
}
