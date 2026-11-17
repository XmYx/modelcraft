//! Numeric attributes of an object, addressed by a dotted path.
//!
//! This is what makes "any number can be keyed" true: every number (and switch) an object stores
//! is reachable by name, so curves, the Key Sheet and scripts need no per-feature code.
//!
//! | Path | Meaning |
//! |---|---|
//! | `translate.x|y|z`, `scale.x|y|z` | the transform |
//! | `rotate.x|y|z` | rotation as XYZ Euler angles in degrees |
//! | `visible` | 1 shown, 0 hidden |
//! | `material.metallic`, `material.base_color.0` … | the material, as it is saved |
//! | `primitive.radius` … | a primitive's parameters |
//! | `step.<step id>.distance` …, `step.<step id>.enabled` | a build-history step |
//! | `dual_quaternion` | a skin's blending switch |
//! | `levels`, `deformer.angle`, `radius` … | every other kind, as it is saved |
//!
//! Paths other than the transform and `visible` follow the saved JSON of the part they name:
//! object fields by name, list items by index. Whole numbers stay whole and switches flip at 0.5.
//! Meshes and sculpt layers are never walked (they are large and not numbers to key).

use glam::Vec3;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{Object, ObjectKind, SceneError, Source, StepOp};

/// Lists longer than this are not attributes (component selections, point lists).
const MAX_LIST: usize = 4;
/// Deepest nesting walked inside one part.
const MAX_NESTING: usize = 6;
/// Longest path accepted.
pub const MAX_PATH: usize = 128;

const AXES: [&str; 3] = ["x", "y", "z"];

fn axis(name: &str) -> Option<usize> {
    AXES.iter().position(|a| *a == name)
}

fn invalid(path: &str) -> SceneError {
    SceneError::Invalid(format!("no numeric attribute {path:?}"))
}

/// Every numeric attribute of an object with its current value, transform first.
pub fn list(o: &Object) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let e = o.transform.euler_degrees();
    for (name, v) in [("translate", o.transform.translation), ("rotate", e), ("scale", o.transform.scale)] {
        for (a, x) in AXES.iter().zip(v.to_array()) {
            out.push((format!("{name}.{a}"), x as f64));
        }
    }
    out.push(("visible".into(), if o.visible { 1.0 } else { 0.0 }));
    collect(&o.material, "material", &mut out);
    match &o.kind {
        ObjectKind::Mesh { source, steps } => {
            if let Source::Primitive { primitive } = source {
                collect(primitive, "primitive", &mut out);
            }
            for s in steps {
                out.push((format!("step.{}.enabled", s.id), if s.enabled { 1.0 } else { 0.0 }));
                if !matches!(s.op, StepOp::Sculpt { .. }) {
                    collect(&s.op, &format!("step.{}", s.id), &mut out);
                }
            }
        }
        // A skin's weights are data, not attributes; its one switch is.
        ObjectKind::Skin { skin } => out.push(("dual_quaternion".into(), if skin.dual_quaternion { 1.0 } else { 0.0 })),
        kind => collect(kind, "", &mut out),
    }
    out
}

/// The value of one attribute.
pub fn get(o: &Object, path: &str) -> Option<f64> {
    let (head, rest) = path.split_once('.').unwrap_or((path, ""));
    match head {
        "translate" => axis(rest).map(|i| o.transform.translation.to_array()[i] as f64),
        "scale" => axis(rest).map(|i| o.transform.scale.to_array()[i] as f64),
        "rotate" => axis(rest).map(|i| o.transform.euler_degrees().to_array()[i] as f64),
        "visible" if rest.is_empty() => Some(if o.visible { 1.0 } else { 0.0 }),
        "material" => read(&o.material, rest),
        _ => match &o.kind {
            ObjectKind::Mesh { source, steps } => match (head, source) {
                ("primitive", Source::Primitive { primitive }) => read(primitive, rest),
                ("step", _) => {
                    let (id, field) = rest.split_once('.')?;
                    let id: u64 = id.parse().ok()?;
                    let s = steps.iter().find(|s| s.id == id)?;
                    match field {
                        "enabled" => Some(if s.enabled { 1.0 } else { 0.0 }),
                        _ if matches!(s.op, StepOp::Sculpt { .. }) => None,
                        _ => read(&s.op, field),
                    }
                }
                _ => None,
            },
            ObjectKind::Skin { skin } => (path == "dual_quaternion").then_some(if skin.dual_quaternion { 1.0 } else { 0.0 }),
            kind => read(kind, path),
        },
    }
}

/// Set one attribute. Returns whether the object changed. The caller bumps the revision.
pub fn set(o: &mut Object, path: &str, value: f64) -> Result<bool, SceneError> {
    if !value.is_finite() || path.len() > MAX_PATH {
        return Err(invalid(path));
    }
    let (head, rest) = path.split_once('.').unwrap_or((path, ""));
    let v = value as f32;
    match head {
        "translate" | "scale" => {
            let i = axis(rest).ok_or_else(|| invalid(path))?;
            let mut t = o.transform;
            let target = if head == "translate" { &mut t.translation } else { &mut t.scale };
            let mut a = target.to_array();
            a[i] = v;
            *target = Vec3::from_array(a);
            let t = t.sanitized();
            Ok(std::mem::replace(&mut o.transform, t) != t)
        }
        "rotate" => {
            let i = axis(rest).ok_or_else(|| invalid(path))?;
            let mut e = o.transform.euler_degrees().to_array();
            if (e[i] - v).abs() < 1e-5 {
                return Ok(false);
            }
            e[i] = v;
            o.transform.set_euler_degrees(Vec3::from_array(e));
            Ok(true)
        }
        "visible" if rest.is_empty() => Ok(std::mem::replace(&mut o.visible, value >= 0.5) != (value >= 0.5)),
        "material" => write(&mut o.material, rest, value).ok_or_else(|| invalid(path)),
        _ => match &mut o.kind {
            ObjectKind::Mesh { source, steps } => match (head, source) {
                ("primitive", Source::Primitive { primitive }) => write(primitive, rest, value).ok_or_else(|| invalid(path)),
                ("step", _) => {
                    let (id, field) = rest.split_once('.').ok_or_else(|| invalid(path))?;
                    let id: u64 = id.parse().map_err(|_| invalid(path))?;
                    let s = steps.iter_mut().find(|s| s.id == id).ok_or_else(|| invalid(path))?;
                    match field {
                        "enabled" => Ok(std::mem::replace(&mut s.enabled, value >= 0.5) != (value >= 0.5)),
                        _ if matches!(s.op, StepOp::Sculpt { .. }) => Err(invalid(path)),
                        _ => write(&mut s.op, field, value).ok_or_else(|| invalid(path)),
                    }
                }
                _ => Err(invalid(path)),
            },
            ObjectKind::Skin { skin } if path == "dual_quaternion" => {
                let on = value >= 0.5;
                let changed = skin.dual_quaternion != on;
                if changed {
                    std::sync::Arc::make_mut(skin).dual_quaternion = on;
                }
                Ok(changed)
            }
            ObjectKind::Skin { .. } => Err(invalid(path)),
            kind => write(kind, path, value).ok_or_else(|| invalid(path)),
        },
    }
}

/// Set all three rotation angles at once (degrees). Keyed rotations go through here so that the
/// three curves are never mixed with angles read back from the quaternion.
pub fn set_rotation(o: &mut Object, degrees: Vec3) -> bool {
    if !degrees.is_finite() {
        return false;
    }
    let mut t = o.transform;
    t.set_euler_degrees(degrees);
    let t = t.sanitized();
    let changed = t.rotation.dot(o.transform.rotation).abs() < 1.0 - 1e-7;
    if changed {
        o.transform = t;
    }
    changed
}

fn collect<T: Serialize>(part: &T, prefix: &str, out: &mut Vec<(String, f64)>) {
    if let Ok(v) = serde_json::to_value(part) {
        walk(&v, prefix, 0, out);
    }
}

fn walk(v: &Value, path: &str, depth: usize, out: &mut Vec<(String, f64)>) {
    let join = |k: &str| if path.is_empty() { k.to_string() } else { format!("{path}.{k}") };
    match v {
        Value::Number(n) => {
            if let (Some(f), false) = (n.as_f64(), path.is_empty()) {
                out.push((path.to_string(), f));
            }
        }
        Value::Bool(b) if !path.is_empty() => out.push((path.to_string(), if *b { 1.0 } else { 0.0 })),
        Value::Object(m) if depth < MAX_NESTING => {
            for (k, x) in m {
                walk(x, &join(k), depth + 1, out);
            }
        }
        Value::Array(a) if depth < MAX_NESTING && a.len() <= MAX_LIST => {
            for (i, x) in a.iter().enumerate() {
                walk(x, &join(&i.to_string()), depth + 1, out);
            }
        }
        _ => {}
    }
}

fn leaf<'a>(root: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    let mut v = root;
    for (n, seg) in path.split('.').enumerate() {
        if n >= MAX_NESTING {
            return None;
        }
        v = match v {
            Value::Object(m) => m.get_mut(seg)?,
            Value::Array(a) if a.len() <= MAX_LIST => a.get_mut(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(v)
}

fn read<T: Serialize>(part: &T, path: &str) -> Option<f64> {
    let mut root = serde_json::to_value(part).ok()?;
    match leaf(&mut root, path)? {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Write `value` at `path` inside `part`; `Some(changed)` when the path names a number or switch
/// and the part still reads back.
fn write<T: Serialize + DeserializeOwned + PartialEq>(part: &mut T, path: &str, value: f64) -> Option<bool> {
    let mut root = serde_json::to_value(&*part).ok()?;
    let slot = leaf(&mut root, path)?;
    *slot = match slot {
        Value::Bool(_) => Value::Bool(value >= 0.5),
        Value::Number(n) if n.is_u64() => Value::from(value.round().clamp(0.0, u32::MAX as f64) as u64),
        Value::Number(n) if n.is_i64() => Value::from(value.round().clamp(i32::MIN as f64, i32::MAX as f64) as i64),
        Value::Number(_) => Value::from(value as f32),
        _ => return None,
    };
    let new: T = serde_json::from_value(root).ok()?;
    Some(std::mem::replace(part, new) != *part)
}
