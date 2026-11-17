//! Mesh access. `Mesh` is a read-only view of an evaluated mesh; `MeshBuilder` makes a new mesh
//! object from positions and faces; `PositionEdit` changes the vertices of a stored mesh. Every
//! index is checked against the mesh, and what a script builds or copies is charged to the run's
//! geometry budget.

use std::rc::Rc;
use std::sync::Arc;

use glam::Vec3;
use modelcraft_engine::Command;
use modelcraft_engine::mesh::{MAX_POLYGON_CORNERS, PolyMesh};
use modelcraft_engine::scene::{ObjectKind, Source};
use rhai::{Array, Dynamic, Engine, INT, Map};

use super::commands::created;
use super::conv::*;
use crate::error::{R, rt};
use crate::host::Shared;

/// A read-only view of an evaluated mesh (cheap to copy: the geometry is shared).
#[derive(Clone)]
pub struct MeshHandle(pub Arc<PolyMesh>);

/// Positions and faces being collected for a new mesh object.
#[derive(Clone, Default)]
pub struct MeshBuilder {
    pos: Vec<Vec3>,
    faces: Vec<Vec<u32>>,
}

/// The positions of a stored mesh, being edited.
#[derive(Clone)]
pub struct PositionEdit {
    id: modelcraft_engine::scene::ObjectId,
    pos: Vec<Vec3>,
}

/// Refuse an array that would hold more than a script array may (nested arrays count too, so a
/// list of positions is four entries per vertex).
fn capped_len(s: &Shared, entries: usize, what: &str) -> R<()> {
    if entries > s.limits.max_array_len {
        return Err(rt(format!(
            "{what} would be an array of {entries} entries (nested ones count), more than a script array holds ({}); read it in pieces with {what}_range or use the counts",
            s.limits.max_array_len
        )));
    }
    Ok(())
}

fn range(start: INT, count: INT, len: usize, per_item: usize, name: &str, s: &Shared) -> R<std::ops::Range<usize>> {
    let (Ok(a), Ok(n)) = (usize::try_from(start), usize::try_from(count)) else { return Err(rt("start and count cannot be negative")) };
    if a > len {
        return Err(rt(format!("start {a} is past the end ({len})")));
    }
    let end = a.saturating_add(n).min(len);
    capped_len(s, (end - a).saturating_mul(per_item), name)?;
    Ok(a..end)
}

/// Entries one face may add to an array (itself and its corners) when a range is checked up
/// front; quads are the usual case, and the engine's own limit still applies to the real size.
const MAX_FACE_WEIGHT: usize = 4;

fn tri_area(a: Vec3, b: Vec3, c: Vec3) -> f64 {
    0.5 * (b - a).cross(c - a).as_dvec3().length()
}

fn volume(m: &PolyMesh) -> f64 {
    let (tris, _) = m.triangulate();
    let p = m.positions();
    let mut v = 0.0f64;
    for t in &tris {
        if let (Some(a), Some(b), Some(c)) = (p.get(t[0] as usize), p.get(t[1] as usize), p.get(t[2] as usize)) {
            v += a.as_dvec3().dot(b.as_dvec3().cross(c.as_dvec3())) / 6.0;
        }
    }
    v.abs()
}

fn closed(m: &PolyMesh) -> bool {
    m.face_count() > 0 && (0..m.edge_count() as u32).all(|e| !m.is_boundary_edge(e))
}

fn face_indices(m: &PolyMesh, f: u32) -> Array {
    m.face_vertices(f).map(|v| Dynamic::from_int(v as INT)).collect()
}

fn vertex(m: &MeshHandle, i: INT) -> R<u32> {
    index(i, m.0.vertex_count(), "vertex")
}

fn face(m: &MeshHandle, i: INT) -> R<u32> {
    index(i, m.0.face_count(), "face")
}

impl MeshBuilder {
    fn push_vertex(&mut self, s: &Shared, p: Vec3) -> R<INT> {
        s.charge(1)?;
        self.pos.push(p);
        Ok((self.pos.len() - 1) as INT)
    }

    fn push_face(&mut self, s: &Shared, f: Vec<u32>) -> R<INT> {
        if f.len() < 3 {
            return Err(rt("a face needs at least three vertices"));
        }
        if f.len() > MAX_POLYGON_CORNERS {
            return Err(rt(format!("a face has at most {MAX_POLYGON_CORNERS} corners")));
        }
        if let Some(bad) = f.iter().find(|v| **v as usize >= self.pos.len()) {
            return Err(rt(format!("the face uses vertex {bad}, but only {} vertices exist so far (numbered from 0)", self.pos.len())));
        }
        s.charge(1 + f.len())?;
        self.faces.push(f);
        Ok((self.faces.len() - 1) as INT)
    }

    /// An axis-aligned box with outward quads.
    fn add_box(&mut self, s: &Shared, lo: Vec3, hi: Vec3) -> R<()> {
        let base = self.pos.len() as u32;
        let c = |i: u32| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        for i in 0..8 {
            self.push_vertex(s, c(i))?;
        }
        // Counter-clockwise seen from outside.
        for q in [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]] {
            self.push_face(s, q.iter().map(|i| base + *i).collect())?;
        }
        Ok(())
    }
}

pub(crate) fn register(engine: &mut Engine, s: &Rc<Shared>) {
    engine.register_type_with_name::<MeshHandle>("Mesh");
    engine.register_fn("vertex_count", |m: &mut MeshHandle| m.0.vertex_count() as INT);
    engine.register_fn("face_count", |m: &mut MeshHandle| m.0.face_count() as INT);
    engine.register_fn("edge_count", |m: &mut MeshHandle| m.0.edge_count() as INT);
    engine.register_fn("triangle_count", |m: &mut MeshHandle| m.0.triangle_count() as INT);
    engine.register_fn("position", |m: &mut MeshHandle, i: INT| -> R<Dynamic> { Ok(to_array(m.0.position(vertex(m, i)?))) });
    engine.register_fn("face", |m: &mut MeshHandle, i: INT| -> R<Array> { Ok(face_indices(&m.0, face(m, i)?)) });
    engine.register_fn("face_normal", |m: &mut MeshHandle, i: INT| -> R<Dynamic> { Ok(to_array(m.0.face_normal(face(m, i)?))) });
    engine.register_fn("face_center", |m: &mut MeshHandle, i: INT| -> R<Dynamic> { Ok(to_array(m.0.face_center(face(m, i)?))) });
    engine.register_fn("triangle", |m: &mut MeshHandle, i: INT| -> R<Array> {
        let (tris, _) = m.0.triangulate();
        let n = tris.len();
        let t = tris.get(index(i, n, "triangle")? as usize).ok_or_else(|| rt("triangle out of range"))?;
        Ok(t.iter().map(|v| Dynamic::from_int(*v as INT)).collect())
    });
    engine.register_fn("bounds", |m: &mut MeshHandle| -> Dynamic {
        if m.0.vertex_count() == 0 {
            return Dynamic::UNIT;
        }
        let (lo, hi) = m.0.bounds();
        Dynamic::from_map(map_of(vec![("min", to_array(lo)), ("max", to_array(hi))]))
    });
    engine.register_fn("surface_area", |m: &mut MeshHandle| -> f64 {
        let (tris, _) = m.0.triangulate();
        let p = m.0.positions();
        tris.iter().filter_map(|t| Some(tri_area(*p.get(t[0] as usize)?, *p.get(t[1] as usize)?, *p.get(t[2] as usize)?))).sum()
    });
    engine.register_fn("is_closed", |m: &mut MeshHandle| closed(&m.0));
    engine.register_fn("volume", |m: &mut MeshHandle| -> R<f64> {
        if !closed(&m.0) {
            return Err(rt("only a closed mesh has a volume"));
        }
        Ok(volume(&m.0))
    });
    engine.register_fn("stats", |m: &mut MeshHandle| -> Map {
        let area: f64 = {
            let (tris, _) = m.0.triangulate();
            let p = m.0.positions();
            tris.iter().filter_map(|t| Some(tri_area(*p.get(t[0] as usize)?, *p.get(t[1] as usize)?, *p.get(t[2] as usize)?))).sum()
        };
        let is_closed = closed(&m.0);
        map_of(vec![
            ("vertices", int(m.0.vertex_count())),
            ("faces", int(m.0.face_count())),
            ("edges", int(m.0.edge_count())),
            ("triangles", int(m.0.triangle_count())),
            ("closed", Dynamic::from_bool(is_closed)),
            ("area", Dynamic::from_float(area)),
            ("volume", if is_closed { Dynamic::from_float(volume(&m.0)) } else { Dynamic::UNIT }),
        ])
    });

    let sh = s.clone();
    engine.register_fn("positions", move |m: &mut MeshHandle| -> R<Array> {
        capped_len(&sh, m.0.vertex_count().saturating_mul(4), "positions")?;
        Ok(m.0.positions().iter().map(|p| to_array(*p)).collect())
    });
    let sh = s.clone();
    engine.register_fn("positions_range", move |m: &mut MeshHandle, start: INT, count: INT| -> R<Array> {
        let r = range(start, count, m.0.vertex_count(), 4, "positions", &sh)?;
        Ok(m.0.positions().get(r).unwrap_or(&[]).iter().map(|p| to_array(*p)).collect())
    });
    let sh = s.clone();
    engine.register_fn("faces", move |m: &mut MeshHandle| -> R<Array> {
        capped_len(&sh, m.0.face_count().saturating_add(m.0.half_edge_count()), "faces")?;
        Ok((0..m.0.face_count() as u32).map(|f| Dynamic::from_array(face_indices(&m.0, f))).collect())
    });
    let sh = s.clone();
    engine.register_fn("faces_range", move |m: &mut MeshHandle, start: INT, count: INT| -> R<Array> {
        let r = range(start, count, m.0.face_count(), 1 + MAX_FACE_WEIGHT, "faces", &sh)?;
        Ok(r.map(|f| Dynamic::from_array(face_indices(&m.0, f as u32))).collect())
    });
    let sh = s.clone();
    engine.register_fn("indices", move |m: &mut MeshHandle| -> R<Array> {
        capped_len(&sh, m.0.triangle_count() * 3, "indices")?;
        let (tris, _) = m.0.triangulate();
        Ok(tris.iter().flat_map(|t| t.iter().map(|v| Dynamic::from_int(*v as INT))).collect())
    });

    // ---- building ----
    engine.register_type_with_name::<MeshBuilder>("MeshBuilder");
    engine.register_fn("mesh_builder", MeshBuilder::default);
    let sh = s.clone();
    engine.register_fn("to_builder", move |m: &mut MeshHandle| -> R<MeshBuilder> {
        sh.charge(m.0.vertex_count() + m.0.face_count())?;
        Ok(MeshBuilder { pos: m.0.positions().to_vec(), faces: (0..m.0.face_count() as u32).map(|f| m.0.face_vertices(f).collect()).collect() })
    });
    engine.register_fn("vertex_count", |b: &mut MeshBuilder| b.pos.len() as INT);
    engine.register_fn("face_count", |b: &mut MeshBuilder| b.faces.len() as INT);
    let sh = s.clone();
    engine.register_fn("vertex", move |b: &mut MeshBuilder, x: Dynamic, y: Dynamic, z: Dynamic| -> R<INT> { b.push_vertex(&sh, xyz(&x, &y, &z)?) });
    let sh = s.clone();
    engine.register_fn("face", move |b: &mut MeshBuilder, vs: Array| -> R<INT> {
        let f = vs
            .iter()
            .map(|v| {
                v.as_int()
                    .map_err(|_| rt("a face is a list of vertex numbers"))
                    .and_then(|i| u32::try_from(i).map_err(|_| rt(format!("{i} is not a vertex number"))))
            })
            .collect::<R<Vec<u32>>>()?;
        b.push_face(&sh, f)
    });
    let sh = s.clone();
    engine.register_fn("triangle", move |b: &mut MeshBuilder, a: INT, c: INT, d: INT| -> R<INT> {
        let v = [a, c, d].map(u32::try_from);
        match v {
            [Ok(a), Ok(c), Ok(d)] => b.push_face(&sh, vec![a, c, d]),
            _ => Err(rt("vertex numbers cannot be negative")),
        }
    });
    let sh = s.clone();
    engine.register_fn("quad", move |b: &mut MeshBuilder, a: INT, c: INT, d: INT, e: INT| -> R<INT> {
        let v = [a, c, d, e].map(u32::try_from);
        match v {
            [Ok(a), Ok(c), Ok(d), Ok(e)] => b.push_face(&sh, vec![a, c, d, e]),
            _ => Err(rt("vertex numbers cannot be negative")),
        }
    });
    let sh = s.clone();
    engine.register_fn("add_box", move |b: &mut MeshBuilder, lo: Array, hi: Array| -> R<()> {
        let (lo, hi) = (vec3(&Dynamic::from_array(lo))?, vec3(&Dynamic::from_array(hi))?);
        b.add_box(&sh, lo.min(hi), lo.max(hi))
    });
    let sh = s.clone();
    engine.register_fn("build", move |b: &mut MeshBuilder, name: &str| -> R<INT> {
        let o = sh.exec(Command::AddMesh {
            name: Some(name.into()),
            parent: None,
            positions: b.pos.iter().map(|p| p.to_array()).collect(),
            faces: b.faces.clone(),
        })?;
        Ok(created(&o)?.0 as INT)
    });
    let sh = s.clone();
    engine.register_fn("build", move |b: &mut MeshBuilder, name: &str, parent: INT| -> R<INT> {
        let o = sh.exec(Command::AddMesh {
            name: Some(name.into()),
            parent: Some(id_of(parent)?),
            positions: b.pos.iter().map(|p| p.to_array()).collect(),
            faces: b.faces.clone(),
        })?;
        Ok(created(&o)?.0 as INT)
    });

    // ---- editing positions ----
    engine.register_type_with_name::<PositionEdit>("PositionEdit");
    let sh = s.clone();
    engine.register_fn("edit_positions", move |id: INT| -> R<PositionEdit> {
        let id = id_of(id)?;
        let mesh = sh.with_doc(|d| match d.scene.get(id).map(|o| &o.kind) {
            Some(ObjectKind::Mesh { source: Source::Mesh { mesh }, .. }) => Ok(mesh.clone()),
            Some(ObjectKind::Mesh { .. }) => Err(rt("this object is a primitive: collapse it first (cmd::collapse), then its positions can be edited")),
            Some(_) => Err(rt("only mesh objects have positions")),
            None => Err(rt(format!("there is no object {}", id.0))),
        })??;
        sh.charge(mesh.vertex_count())?;
        Ok(PositionEdit { id, pos: mesh.positions().to_vec() })
    });
    engine.register_fn("len", |e: &mut PositionEdit| e.pos.len() as INT);
    engine.register_fn("get", |e: &mut PositionEdit, i: INT| -> R<Dynamic> {
        Ok(to_array(e.pos.get(index(i, e.pos.len(), "vertex")? as usize).copied().unwrap_or(Vec3::ZERO)))
    });
    engine.register_fn("set", |e: &mut PositionEdit, i: INT, x: Dynamic, y: Dynamic, z: Dynamic| -> R<()> {
        let i = index(i, e.pos.len(), "vertex")? as usize;
        let p = xyz(&x, &y, &z)?;
        if let Some(slot) = e.pos.get_mut(i) {
            *slot = p;
        }
        Ok(())
    });
    engine.register_fn("translate_all", |e: &mut PositionEdit, x: Dynamic, y: Dynamic, z: Dynamic| -> R<()> {
        let d = xyz(&x, &y, &z)?;
        e.pos.iter_mut().for_each(|p| *p += d);
        Ok(())
    });
    let sh = s.clone();
    engine.register_fn("commit", move |e: &mut PositionEdit| -> R<()> {
        sh.exec(Command::SetPositions { id: e.id, positions: e.pos.iter().map(|p| p.to_array()).collect() })?;
        Ok(())
    });
}
