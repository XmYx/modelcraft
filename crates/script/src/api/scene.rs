//! Scene queries and the everyday edits: objects, attributes, transforms, selection, the
//! dependency graph and the evaluation of history nodes. Edits run as commands, so they are
//! journaled and undoable like anything else.

use std::rc::Rc;

use glam::{Mat4, Vec3};
use modelcraft_engine::scene::{Material, Object, ObjectId, ObjectKind, Source, Transform};
use modelcraft_engine::{Command, Document};
use rhai::{Array, Dynamic, Engine, INT, Map};

use super::commands::created;
use super::conv::*;
use super::mesh::MeshHandle;
use crate::error::{R, rt};
use crate::host::Shared;

fn object(d: &Document, id: ObjectId) -> R<&Object> {
    d.scene.get(id).ok_or_else(|| rt(format!("there is no object {}", id.0)))
}

fn capped(s: &Shared, n: usize, what: &str) -> R<()> {
    if n > s.limits.max_array_len {
        return Err(rt(format!(
            "{what} has {n} entries, more than a script array holds ({}); ask for a subtree with children(id) instead",
            s.limits.max_array_len
        )));
    }
    Ok(())
}

/// `*` matches any run of characters, `?` one character. Case matters.
pub(crate) fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        match p.get(pi) {
            Some('*') => {
                star = Some(pi);
                mark = ti;
                pi += 1;
            }
            Some(c) if *c == '?' || Some(c) == t.get(ti) => {
                pi += 1;
                ti += 1;
            }
            _ => match star {
                Some(s) => {
                    pi = s + 1;
                    mark += 1;
                    ti = mark;
                }
                None => return false,
            },
        }
    }
    p.iter().skip(pi).all(|c| *c == '*')
}

fn attribute_names(o: &Object) -> Vec<&'static str> {
    let mut v = vec!["name", "visible", "translation", "rotation_euler", "scale", "material"];
    match &o.kind {
        ObjectKind::Mesh { source: Source::Primitive { .. }, .. } => v.push("primitive"),
        ObjectKind::SmoothCage { .. } => v.push("levels"),
        ObjectKind::Deformer { .. } => v.push("deformer"),
        _ => {}
    }
    v
}

fn json_of<T: serde::Serialize>(v: &T) -> R<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| rt(e.to_string()))
}

/// Merge the keys of `change` into `base` (both JSON objects).
fn merged(mut base: serde_json::Value, change: serde_json::Value) -> R<serde_json::Value> {
    match (base.as_object_mut(), change) {
        (Some(b), serde_json::Value::Object(c)) => {
            for (k, v) in c {
                b.insert(k, v);
            }
            Ok(base)
        }
        _ => Err(rt("expected a map of values")),
    }
}

fn get_attr(d: &Document, id: ObjectId, name: &str) -> R<Dynamic> {
    let o = object(d, id)?;
    Ok(match (name, &o.kind) {
        ("name", _) => Dynamic::from(o.name.clone()),
        ("visible", _) => Dynamic::from_bool(o.visible),
        ("translation", _) => to_array(o.transform.translation),
        ("rotation_euler", _) => to_array(o.transform.euler_degrees()),
        ("scale", _) => to_array(o.transform.scale),
        ("material", _) => from_json(&json_of(&o.material)?)?,
        ("primitive", ObjectKind::Mesh { source: Source::Primitive { primitive }, .. }) => from_json(&json_of(primitive)?)?,
        ("levels", ObjectKind::SmoothCage { levels }) => Dynamic::from_int(*levels as INT),
        ("deformer", ObjectKind::Deformer { deformer, .. }) => from_json(&json_of(deformer)?)?,
        _ => return Err(rt(format!("{} has no attribute {name:?} (it has: {})", o.name, attribute_names(o).join(", ")))),
    })
}

fn set_transform(s: &Shared, id: ObjectId, f: impl FnOnce(&mut Transform)) -> R<()> {
    let mut t = s.with_doc(|d| object(d, id).map(|o| o.transform))??;
    f(&mut t);
    s.exec(Command::SetTransform { id, transform: t })?;
    Ok(())
}

fn set_attr(s: &Shared, id: ObjectId, name: &str, v: &Dynamic) -> R<()> {
    let names = s.with_doc(|d| object(d, id).map(attribute_names))??;
    if !names.contains(&name) {
        return Err(rt(format!("object {} has no attribute {name:?} (it has: {})", id.0, names.join(", "))));
    }
    match name {
        "name" => {
            let n = v.clone().into_string().map_err(|_| rt("name must be a string"))?;
            s.exec(Command::Rename { id, name: n })?;
        }
        "visible" => {
            let b = v.as_bool().map_err(|_| rt("visible must be true or false"))?;
            s.exec(Command::SetVisible { id, visible: b })?;
        }
        "translation" => {
            let p = vec3(v)?;
            set_transform(s, id, |t| t.translation = p)?;
        }
        "rotation_euler" => {
            let p = vec3(v)?;
            set_transform(s, id, |t| t.set_euler_degrees(p))?;
        }
        "scale" => {
            let p = vec3(v)?;
            set_transform(s, id, |t| t.scale = p)?;
        }
        "material" => {
            let cur = s.with_doc(|d| object(d, id).map(|o| o.material.clone()))??;
            let m: Material = serde_json::from_value(merged(json_of(&cur)?, to_json(v)?)?).map_err(|e| rt(format!("not a valid material: {e}")))?;
            s.exec(Command::SetMaterial { id, material: m })?;
        }
        "primitive" => {
            let cur = s.with_doc(|d| match object(d, id)?.kind.clone() {
                ObjectKind::Mesh { source: Source::Primitive { primitive }, .. } => Ok(primitive),
                _ => Err(rt("not a primitive")),
            })??;
            let p = serde_json::from_value(merged(json_of(&cur)?, to_json(v)?)?).map_err(|e| rt(format!("not valid primitive settings: {e}")))?;
            s.exec(Command::SetPrimitive { id, primitive: p })?;
        }
        "deformer" => {
            let cur = s.with_doc(|d| match object(d, id)?.kind.clone() {
                ObjectKind::Deformer { deformer, .. } => Ok(deformer),
                _ => Err(rt("not a deformer")),
            })??;
            let p = serde_json::from_value(merged(json_of(&cur)?, to_json(v)?)?).map_err(|e| rt(format!("not valid deformer settings: {e}")))?;
            s.exec(Command::SetDeformer { id, deformer: p, limits: None, target: None, clear_target: false })?;
        }
        "levels" => {
            let n = v.as_int().map_err(|_| rt("levels must be a whole number"))?;
            let levels = u32::try_from(n).map_err(|_| rt("levels cannot be negative"))?;
            s.exec(Command::SetCageLevels { id, levels })?;
        }
        _ => return Err(rt(format!("cannot set {name:?}"))),
    }
    Ok(())
}

/// Edges of the evaluation graph as (consumer, input, kind): a mesh reads its visible deformer
/// children, a smooth cage reads its first mesh child, and every child's world transform reads
/// its parent's.
pub(crate) fn graph(d: &Document) -> Vec<(ObjectId, ObjectId, &'static str)> {
    let mut e = Vec::new();
    for o in d.scene.objects() {
        if let Some(p) = o.parent {
            e.push((o.id, p, "transform"));
        }
        match &o.kind {
            ObjectKind::Mesh { .. } => {
                for c in d.scene.children(o.id) {
                    if d.scene.get(c).is_some_and(|c| c.visible && matches!(c.kind, ObjectKind::Deformer { .. })) {
                        e.push((o.id, c, "deformer"));
                    }
                }
            }
            ObjectKind::SmoothCage { .. } => {
                if let Some(c) = d.scene.children(o.id).into_iter().find(|c| d.scene.get(*c).is_some_and(|c| matches!(c.kind, ObjectKind::Mesh { .. }))) {
                    e.push((o.id, c, "input"));
                }
            }
            _ => {}
        }
    }
    e
}

fn mesh_of(d: &mut Document, id: ObjectId) -> R<MeshHandle> {
    object(d, id)?;
    d.sync_sculpt();
    let m = d.eval.evaluate(&d.scene, id).mesh;
    Ok(MeshHandle(m))
}

fn info(d: &mut Document, id: ObjectId) -> R<Map> {
    let o = object(d, id)?.clone();
    let m = mesh_of(d, id)?;
    let parent = o.parent.map(id_dyn).unwrap_or(Dynamic::UNIT);
    Ok(map_of(vec![
        ("id", id_dyn(id)),
        ("name", Dynamic::from(o.name.clone())),
        ("kind", Dynamic::from(o.kind.type_name().to_string())),
        ("parent", parent),
        ("children", Dynamic::from_array(ids_array(d.scene.children(id)))),
        ("visible", Dynamic::from_bool(o.visible)),
        ("translation", to_array(o.transform.translation)),
        ("rotation_euler", to_array(o.transform.euler_degrees())),
        ("scale", to_array(o.transform.scale)),
        ("material", from_json(&json_of(&o.material)?)?),
        ("steps", int(o.steps().len())),
        ("vertices", int(m.0.vertex_count())),
        ("faces", int(m.0.face_count())),
    ]))
}

fn world_bounds(d: &mut Document, id: ObjectId) -> R<Dynamic> {
    object(d, id)?;
    d.sync_sculpt();
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let mut any = false;
    for sid in d.scene.subtree(id) {
        if d.scene.is_cage_input(sid) || !matches!(d.scene.get(sid).map(|o| &o.kind), Some(ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. })) {
            continue;
        }
        let mesh = d.eval.evaluate(&d.scene, sid).mesh;
        if mesh.vertex_count() == 0 {
            continue;
        }
        let w: Mat4 = d.scene.world_matrix(sid);
        let (a, b) = mesh.bounds();
        for i in 0..8u32 {
            let c = Vec3::new(if i & 1 == 0 { a.x } else { b.x }, if i & 2 == 0 { a.y } else { b.y }, if i & 4 == 0 { a.z } else { b.z });
            let p = w.transform_point3(c);
            lo = lo.min(p);
            hi = hi.max(p);
            any = true;
        }
    }
    Ok(if any { Dynamic::from_map(map_of(vec![("min", to_array(lo)), ("max", to_array(hi))])) } else { Dynamic::UNIT })
}

pub(crate) fn register(engine: &mut Engine, s: &Rc<Shared>) {
    // ---- queries ----
    let sh = s.clone();
    engine.register_fn("object_count", move || -> R<INT> { sh.with_doc(|d| d.scene.len() as INT) });
    let sh = s.clone();
    engine.register_fn("object_ids", move || -> R<Array> {
        let ids: Vec<ObjectId> = sh.with_doc(|d| d.scene.tree_order().into_iter().map(|(i, _)| i).collect())?;
        capped(&sh, ids.len(), "the scene")?;
        Ok(ids_array(ids))
    });
    let sh = s.clone();
    engine.register_fn("roots", move || -> R<Array> {
        let ids = sh.with_doc(|d| d.scene.roots())?;
        capped(&sh, ids.len(), "the scene's top level")?;
        Ok(ids_array(ids))
    });
    let sh = s.clone();
    engine.register_fn("children", move |id: INT| -> R<Array> {
        let id = id_of(id)?;
        let ids = sh.with_doc(|d| object(d, id).map(|_| d.scene.children(id)))??;
        capped(&sh, ids.len(), "the object")?;
        Ok(ids_array(ids))
    });
    let sh = s.clone();
    engine.register_fn("parent", move |id: INT| -> R<Dynamic> {
        let id = id_of(id)?;
        Ok(sh.with_doc(|d| object(d, id).map(|o| o.parent))??.map(id_dyn).unwrap_or(Dynamic::UNIT))
    });
    let sh = s.clone();
    engine.register_fn("exists", move |id: INT| -> R<bool> { sh.with_doc(|d| u64::try_from(id).is_ok_and(|i| d.scene.get(ObjectId(i)).is_some())) });
    let sh = s.clone();
    engine.register_fn("find", move |name: &str| -> R<Dynamic> {
        Ok(sh.with_doc(|d| d.scene.objects().iter().find(|o| o.name == name).map(|o| id_dyn(o.id)))?.unwrap_or(Dynamic::UNIT))
    });
    let sh = s.clone();
    engine.register_fn("find_all", move |pattern: &str| -> R<Array> {
        let ids: Vec<ObjectId> =
            sh.with_doc(|d| d.scene.tree_order().into_iter().map(|(i, _)| i).filter(|i| d.scene.get(*i).is_some_and(|o| glob(pattern, &o.name))).collect())?;
        capped(&sh, ids.len(), "the result")?;
        Ok(ids_array(ids))
    });
    let sh = s.clone();
    engine.register_fn("name_of", move |id: INT| -> R<String> {
        let id = id_of(id)?;
        sh.with_doc(|d| object(d, id).map(|o| o.name.clone()))?
    });
    let sh = s.clone();
    engine.register_fn("kind_of", move |id: INT| -> R<String> {
        let id = id_of(id)?;
        sh.with_doc(|d| object(d, id).map(|o| o.kind.type_name().to_string()))?
    });
    let sh = s.clone();
    engine.register_fn("object_info", move |id: INT| -> R<Map> {
        let id = id_of(id)?;
        sh.with_doc(|d| info(d, id))?
    });
    let sh = s.clone();
    engine.register_fn("attributes", move |id: INT| -> R<Array> {
        let id = id_of(id)?;
        sh.with_doc(|d| object(d, id).map(|o| attribute_names(o).into_iter().map(Dynamic::from).collect()))?
    });
    let sh = s.clone();
    engine.register_fn("get_attr", move |id: INT, name: &str| -> R<Dynamic> {
        let id = id_of(id)?;
        sh.with_doc(|d| get_attr(d, id, name))?
    });
    let sh = s.clone();
    engine.register_fn("set_attr", move |id: INT, name: &str, v: Dynamic| -> R<()> { set_attr(&sh, id_of(id)?, name, &v) });

    // ---- creating and editing ----
    let sh = s.clone();
    engine.register_fn("add_primitive", move |kind: &str| -> R<INT> {
        Ok(created(&sh.exec(Command::AddPrimitive { kind: kind.into(), name: None, parent: None })?)?.0 as INT)
    });
    let sh = s.clone();
    engine.register_fn("add_primitive", move |kind: &str, name: &str| -> R<INT> {
        Ok(created(&sh.exec(Command::AddPrimitive { kind: kind.into(), name: Some(name.into()), parent: None })?)?.0 as INT)
    });
    let sh = s.clone();
    engine.register_fn("add_group", move || -> R<INT> { Ok(created(&sh.exec(Command::AddGroup { name: None })?)?.0 as INT) });
    let sh = s.clone();
    engine.register_fn("add_group", move |name: &str| -> R<INT> { Ok(created(&sh.exec(Command::AddGroup { name: Some(name.into()) })?)?.0 as INT) });
    let sh = s.clone();
    engine.register_fn("delete", move |ids: Dynamic| -> R<()> {
        sh.exec(Command::Delete { ids: ids_from(&ids)? })?;
        Ok(())
    });
    let sh = s.clone();
    engine.register_fn("duplicate", move |id: INT| -> R<INT> { Ok(created(&sh.exec(Command::Duplicate { id: id_of(id)? })?)?.0 as INT) });
    let sh = s.clone();
    engine.register_fn("rename", move |id: INT, name: &str| -> R<()> {
        sh.exec(Command::Rename { id: id_of(id)?, name: name.into() })?;
        Ok(())
    });
    let sh = s.clone();
    engine.register_fn("reparent", move |id: INT, parent: INT| -> R<()> {
        sh.exec(Command::Reparent { id: id_of(id)?, parent: Some(id_of(parent)?) })?;
        Ok(())
    });
    let sh = s.clone();
    engine.register_fn("unparent", move |id: INT| -> R<()> {
        sh.exec(Command::Reparent { id: id_of(id)?, parent: None })?;
        Ok(())
    });
    let sh = s.clone();
    engine.register_fn("set_visible", move |id: INT, visible: bool| -> R<()> {
        sh.exec(Command::SetVisible { id: id_of(id)?, visible })?;
        Ok(())
    });

    // ---- transforms ----
    let sh = s.clone();
    engine.register_fn("get_translation", move |id: INT| -> R<Dynamic> {
        let id = id_of(id)?;
        sh.with_doc(|d| get_attr(d, id, "translation"))?
    });
    let sh = s.clone();
    engine.register_fn("get_rotation_euler", move |id: INT| -> R<Dynamic> {
        let id = id_of(id)?;
        sh.with_doc(|d| get_attr(d, id, "rotation_euler"))?
    });
    let sh = s.clone();
    engine.register_fn("get_scale", move |id: INT| -> R<Dynamic> {
        let id = id_of(id)?;
        sh.with_doc(|d| get_attr(d, id, "scale"))?
    });
    let sh = s.clone();
    engine.register_fn("move_to", move |id: INT, x: Dynamic, y: Dynamic, z: Dynamic| -> R<()> {
        let p = xyz(&x, &y, &z)?;
        set_transform(&sh, id_of(id)?, |t| t.translation = p)
    });
    let sh = s.clone();
    engine.register_fn("move_by", move |id: INT, x: Dynamic, y: Dynamic, z: Dynamic| -> R<()> {
        let p = xyz(&x, &y, &z)?;
        set_transform(&sh, id_of(id)?, |t| t.translation += p)
    });
    let sh = s.clone();
    engine.register_fn("rotate_to", move |id: INT, x: Dynamic, y: Dynamic, z: Dynamic| -> R<()> {
        let p = xyz(&x, &y, &z)?;
        set_transform(&sh, id_of(id)?, |t| t.set_euler_degrees(p))
    });
    let sh = s.clone();
    engine.register_fn("scale_to", move |id: INT, x: Dynamic, y: Dynamic, z: Dynamic| -> R<()> {
        let p = xyz(&x, &y, &z)?;
        set_transform(&sh, id_of(id)?, |t| t.scale = p)
    });
    let sh = s.clone();
    engine.register_fn("world_position", move |id: INT| -> R<Dynamic> {
        let id = id_of(id)?;
        sh.with_doc(|d| object(d, id).map(|_| to_array(d.scene.world_matrix(id).transform_point3(Vec3::ZERO))))?
    });
    let sh = s.clone();
    engine.register_fn("world_bounds", move |id: INT| -> R<Dynamic> {
        let id = id_of(id)?;
        sh.with_doc(|d| world_bounds(d, id))?
    });

    // ---- selection ----
    let sh = s.clone();
    engine.register_fn("selection", move || -> R<Array> { sh.with_ctx(|c| ids_array(c.selection.iter().copied())) });
    let sh = s.clone();
    engine.register_fn("set_selection", move |ids: Dynamic| -> R<()> {
        let mut ids = ids_from(&ids)?;
        let mut seen = std::collections::HashSet::new();
        ids.retain(|i| seen.insert(*i));
        if let Some(bad) = sh.with_doc(|d| ids.iter().find(|i| d.scene.get(**i).is_none()).copied())? {
            return Err(rt(format!("there is no object {}", bad.0)));
        }
        sh.with_ctx(|c| {
            if c.selection != ids {
                c.selection = ids;
                sh.selection_changed.set(true);
            }
        })
    });
    let sh = s.clone();
    engine.register_fn("select", move |id: INT| -> R<()> {
        let id = id_of(id)?;
        sh.with_doc(|d| object(d, id).map(|_| ()))??;
        sh.with_ctx(|c| {
            if !c.selection.contains(&id) {
                c.selection.push(id);
                sh.selection_changed.set(true);
            }
        })
    });
    let sh = s.clone();
    engine.register_fn("deselect", move |id: INT| -> R<()> {
        let id = id_of(id)?;
        sh.with_ctx(|c| {
            let n = c.selection.len();
            c.selection.retain(|i| *i != id);
            if c.selection.len() != n {
                sh.selection_changed.set(true);
            }
        })
    });
    let sh = s.clone();
    engine.register_fn("clear_selection", move || -> R<()> {
        sh.with_ctx(|c| {
            if !c.selection.is_empty() {
                c.selection.clear();
                sh.selection_changed.set(true);
            }
        })
    });
    let sh = s.clone();
    engine.register_fn("is_selected", move |id: INT| -> R<bool> {
        let id = id_of(id)?;
        sh.with_ctx(|c| c.selection.contains(&id))
    });

    // ---- the dependency graph ----
    let sh = s.clone();
    engine.register_fn("dependencies", move |id: INT| -> R<Array> {
        let id = id_of(id)?;
        let mut v: Vec<ObjectId> = sh.with_doc(|d| object(d, id).map(|_| graph(d).into_iter().filter(|e| e.0 == id).map(|e| e.1).collect()))??;
        v.sort();
        v.dedup();
        Ok(ids_array(v))
    });
    let sh = s.clone();
    engine.register_fn("dependents", move |id: INT| -> R<Array> {
        let id = id_of(id)?;
        let mut v: Vec<ObjectId> = sh.with_doc(|d| object(d, id).map(|_| graph(d).into_iter().filter(|e| e.1 == id).map(|e| e.0).collect()))??;
        v.sort();
        v.dedup();
        capped(&sh, v.len(), "the result")?;
        Ok(ids_array(v))
    });
    let sh = s.clone();
    engine.register_fn("graph_edges", move || -> R<Array> {
        let e = sh.with_doc(|d| graph(d))?;
        capped(&sh, e.len(), "the graph")?;
        Ok(e.into_iter()
            .map(|(from, to, kind)| Dynamic::from_map(map_of(vec![("from", id_dyn(from)), ("to", id_dyn(to)), ("kind", Dynamic::from(kind.to_string()))])))
            .collect())
    });

    // ---- evaluation ----
    let sh = s.clone();
    engine.register_fn("evaluate", move |id: INT| -> R<MeshHandle> {
        let id = id_of(id)?;
        sh.with_doc(|d| mesh_of(d, id))?
    });
    let sh = s.clone();
    engine.register_fn("evaluate_step", move |id: INT, step: INT| -> R<MeshHandle> {
        let id = id_of(id)?;
        sh.with_doc(|d| {
            let n = object(d, id)?.steps().len();
            let i = index(step, n, "step")? as usize;
            d.sync_sculpt();
            Ok(MeshHandle(d.eval.steps_output(&d.scene, id, i + 1).mesh))
        })?
    });
    let sh = s.clone();
    engine.register_fn("steps", move |id: INT| -> R<Array> {
        let id = id_of(id)?;
        sh.with_doc(|d| {
            let o = object(d, id)?;
            Ok(o.steps()
                .iter()
                .enumerate()
                .map(|(i, st)| {
                    Dynamic::from_map(map_of(vec![
                        ("index", int(i)),
                        ("name", Dynamic::from(st.op.name().to_string())),
                        ("enabled", Dynamic::from_bool(st.enabled)),
                    ]))
                })
                .collect())
        })?
    });
    let sh = s.clone();
    engine.register_fn("evaluation_errors", move |id: INT| -> R<Array> {
        let id = id_of(id)?;
        sh.with_doc(|d| {
            object(d, id)?;
            d.sync_sculpt();
            let e = d.eval.evaluate(&d.scene, id);
            Ok(e.errors
                .iter()
                .map(|e| {
                    let step = if e.step == usize::MAX { Dynamic::UNIT } else { int(e.step) };
                    Dynamic::from_map(map_of(vec![("step", step), ("message", Dynamic::from(e.message.clone()))]))
                })
                .collect())
        })?
    });
}

#[cfg(test)]
mod tests {
    use super::glob;

    #[test]
    fn globs() {
        assert!(glob("Cube*", "Cube 3"));
        assert!(glob("*", ""));
        assert!(glob("C?be", "Cube"));
        assert!(!glob("C?be", "Cuube"));
        assert!(glob("*ube*", "A Cube B"));
        assert!(!glob("cube", "Cube"));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(!glob("a*b*c", "aXXbYY"));
    }
}
