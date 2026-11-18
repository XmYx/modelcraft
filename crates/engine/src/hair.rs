//! Hair commands. One command, `hair`, with the operation in `op`:
//! `{"cmd":"hair","op":"add","mesh":3}`.
//!
//! Hair is an object under the mesh it grows from. Grooming strokes, numeric brush applications and
//! map edits all go through here, so the app, the CLI, scripts and the crash journal run the same
//! code, and every step is one undo.

use std::sync::Arc;

use glam::Vec3;
use modelcraft_hair::{Amount, Brush, Dab, GrowSpec, InterpSpec, Limits, Stroke, Surface, SurfaceMaps, Symmetry, apply_stroke, grow_guides, paint_map};
use modelcraft_scene::hair::HairData;
use modelcraft_scene::{ObjectId, ObjectKind, SceneError};
use serde::{Deserialize, Serialize};

use crate::command::Outcome;
use crate::document::{Document, EngineError, invalid};

/// Which per-vertex map of the emitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapKind {
    /// Brushes act in proportion to it; 0 protects.
    Mask,
    /// Where regrowing puts hair; 0 grows none.
    Density,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "edit", rename_all = "snake_case")]
pub enum MapEdit {
    /// Set every vertex to `value`.
    Fill { value: f32 },
    /// Replace each value `v` by `1 - v`.
    Invert,
    /// Drop the map (1 everywhere).
    Clear,
    /// One round dab: values near `center` move toward `value`.
    Paint {
        center: [f32; 3],
        radius: f32,
        value: f32,
        strength: f32,
        #[serde(default = "half")]
        falloff: f32,
    },
}

fn half() -> f32 {
    0.5
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HairCommand {
    /// Grow hair on a mesh object. Without `grow` and `interp` the hair is sized to the mesh:
    /// 1000 guides a tenth of its diagonal long and 20,000 hairs.
    Add {
        mesh: ObjectId,
        #[serde(default)]
        grow: Option<GrowSpec>,
        #[serde(default)]
        interp: Option<InterpSpec>,
        #[serde(default)]
        name: Option<String>,
    },
    Remove {
        id: ObjectId,
    },
    /// Grow the guides again (following the density map), discarding the grooming. `grow` replaces
    /// the growth settings when given.
    Regrow {
        id: ObjectId,
        #[serde(default)]
        grow: Option<GrowSpec>,
    },
    /// Change how render hairs are made from the guides.
    SetRender {
        id: ObjectId,
        interp: InterpSpec,
    },
    /// A brush stroke (the dabs the pointer made).
    Stroke {
        id: ObjectId,
        stroke: Stroke,
    },
    /// The numeric way to groom: one dab of `brush`, at `center` (emitter space) or, without it, over
    /// the whole groom at full reach. `direction` is the comb movement.
    Apply {
        id: ObjectId,
        brush: Brush,
        #[serde(default)]
        symmetry: Symmetry,
        #[serde(default)]
        center: Option<[f32; 3]>,
        #[serde(default)]
        direction: [f32; 3],
    },
    Map {
        id: ObjectId,
        map: MapKind,
        edit: MapEdit,
    },
}

impl HairCommand {
    pub fn label(&self) -> String {
        match self {
            HairCommand::Add { .. } => "Add hair".into(),
            HairCommand::Remove { .. } => "Remove hair".into(),
            HairCommand::Regrow { .. } => "Regrow guides".into(),
            HairCommand::SetRender { .. } => "Hair settings".into(),
            HairCommand::Stroke { stroke, .. } => format!("{} stroke", stroke.brush.kind.id()),
            HairCommand::Apply { brush, .. } => format!("{} all", brush.kind.id()),
            HairCommand::Map { map: MapKind::Mask, .. } => "Hair mask".into(),
            HairCommand::Map { map: MapKind::Density, .. } => "Hair density".into(),
        }
    }
}

/// The `op` names of the `hair` command with one-line descriptions.
pub const COMMANDS: &[(&str, &str)] = &[
    (
        "add",
        "Grow hair on a mesh object: mesh, grow? (amount, length, length_jitter, points_per_strand, seed, lean, bend), interp? (amount, blend, clump, frizz, width), name?",
    ),
    ("remove", "Remove hair: id"),
    ("regrow", "Grow the guides again (following the density map), discarding the grooming: id, grow?"),
    ("set_render", "Change how render hairs are made from the guides: id, interp"),
    (
        "stroke",
        "A grooming stroke: id, stroke {brush {kind (comb, cut, length, puff, smooth, clump, frizz), radius, strength, falloff, invert, cut_length, frequency, seed}, symmetry {x, y, z}, dabs [{center, normal, delta, pressure}]}",
    ),
    ("apply", "One numeric brush dab, over the whole groom without a center: id, brush, symmetry?, center? [x,y,z], direction? [x,y,z] (comb)"),
    (
        "map",
        "Edit the mask or the density on the emitter's vertices: id, map (mask, density), edit (fill + value | invert | clear | paint + center, radius, value, strength, falloff)",
    ),
];

fn hair_data(doc: &Document, id: ObjectId) -> Result<(Arc<HairData>, ObjectId), EngineError> {
    let o = doc.scene.get(id).ok_or(SceneError::NoObject(id))?;
    let ObjectKind::Hair { hair } = &o.kind else { return invalid(format!("object {} is not hair", id.0)) };
    let emitter = o.parent.ok_or_else(|| EngineError::Invalid("hair without an emitter".into()))?;
    Ok((hair.clone(), emitter))
}

fn emitter_surface(doc: &mut Document, emitter: ObjectId) -> Result<Surface, EngineError> {
    let mesh = doc.eval.evaluate(&doc.scene, emitter).mesh;
    Surface::from_mesh(&mesh).map_err(|e| EngineError::Invalid(e.to_string()))
}

fn err(e: modelcraft_hair::HairError) -> EngineError {
    EngineError::Invalid(e.to_string())
}

fn store(doc: &mut Document, id: ObjectId, data: HairData) -> Result<(), EngineError> {
    if let ObjectKind::Hair { hair } = &mut doc.scene.edit(id)?.kind {
        *hair = Arc::new(data);
    }
    Ok(())
}

pub fn apply(doc: &mut Document, cmd: &HairCommand, out: &mut Outcome) -> Result<(), EngineError> {
    match cmd {
        HairCommand::Add { mesh, grow, interp, name } => {
            if doc.scene.get(*mesh).is_none() {
                return Err(SceneError::NoObject(*mesh).into());
            }
            let evaluated = doc.eval.evaluate(&doc.scene, *mesh).mesh;
            if evaluated.face_count() == 0 {
                return invalid("hair needs a mesh with faces to grow from");
            }
            let (lo, hi) = evaluated.bounds();
            let diag = (hi - lo).length().max(1e-4);
            let grow = grow.clone().unwrap_or_else(|| GrowSpec { amount: Amount::Count(1000), length: diag * 0.1, length_jitter: 0.2, ..GrowSpec::default() });
            let interp = interp.clone().unwrap_or_else(|| {
                let mut i = InterpSpec { amount: Amount::Count(20_000), ..InterpSpec::default() };
                i.width.root = grow.length * 0.012;
                i.width.tip = grow.length * 0.003;
                i
            });
            let data = HairData::grow(&evaluated, grow, interp).map_err(err)?;
            let name = doc.scene.unique_name(name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Hair"));
            out.created.push(doc.scene.add(name, ObjectKind::Hair { hair: Arc::new(data) }, Some(*mesh))?);
        }
        HairCommand::Remove { id } => {
            hair_data(doc, *id)?;
            doc.scene.remove(*id);
        }
        HairCommand::Regrow { id, grow } => {
            let (hair, emitter) = hair_data(doc, *id)?;
            let surface = emitter_surface(doc, emitter)?;
            let mut data = (*hair).clone();
            if let Some(g) = grow {
                data.grow = g.clone();
            }
            let maps = SurfaceMaps { density: data.density.as_deref(), length: None };
            data.guides = grow_guides(&surface, &data.grow, &maps, &Limits::default()).map_err(err)?;
            store(doc, *id, data)?;
        }
        HairCommand::SetRender { id, interp } => {
            let (hair, _) = hair_data(doc, *id)?;
            let mut data = (*hair).clone();
            // Validate by making no strands: the settings must be usable.
            let probe = InterpSpec { amount: Amount::Count(0), ..interp.clone() };
            let surface = emitter_surface(doc, hair_data(doc, *id)?.1)?;
            modelcraft_hair::interpolate(&surface, &data.guides, &probe, &SurfaceMaps::default(), &Limits::default()).map_err(err)?;
            // And the amount must fit the caps.
            Limits::default()
                .check(
                    match interp.amount {
                        Amount::Count(n) => n,
                        Amount::PerUnitArea(_) => 0,
                    },
                    interp.points_per_strand.unwrap_or(2),
                    false,
                )
                .map_err(err)?;
            data.interp = interp.clone();
            store(doc, *id, data)?;
        }
        HairCommand::Stroke { id, stroke } => {
            let (hair, emitter) = hair_data(doc, *id)?;
            let surface = emitter_surface(doc, emitter)?;
            let mut data = (*hair).clone();
            apply_stroke(&mut data.guides, &surface, data.mask.as_deref(), stroke).map_err(err)?;
            store(doc, *id, data)?;
        }
        HairCommand::Apply { id, brush, symmetry, center, direction } => {
            // The radius is replaced below when the whole groom is asked for, so check it first.
            brush.validate().map_err(err)?;
            let (hair, emitter) = hair_data(doc, *id)?;
            let surface = emitter_surface(doc, emitter)?;
            let mut data = (*hair).clone();
            let delta = Vec3::from_array(*direction);
            let (dab_center, brush) = match center {
                Some(c) => (Vec3::from_array(*c), *brush),
                None => {
                    // Whole groom: a dab at the surface's centre reaching past everything.
                    let (lo, hi) = bounds(surface.positions());
                    ((lo + hi) * 0.5, Brush { radius: ((hi - lo).length() * 2.0).max(1e-3), falloff: 0.0, ..*brush })
                }
            };
            let stroke = Stroke { brush, symmetry: *symmetry, dabs: vec![Dab { center: dab_center, normal: Vec3::Y, delta, pressure: 1.0 }] };
            apply_stroke(&mut data.guides, &surface, data.mask.as_deref(), &stroke).map_err(err)?;
            store(doc, *id, data)?;
        }
        HairCommand::Map { id, map, edit } => {
            let (hair, emitter) = hair_data(doc, *id)?;
            let surface = emitter_surface(doc, emitter)?;
            let mut data = (*hair).clone();
            let n = surface.vertex_count();
            let slot = match map {
                MapKind::Mask => &mut data.mask,
                MapKind::Density => &mut data.density,
            };
            match edit {
                MapEdit::Clear => *slot = None,
                MapEdit::Fill { value } => {
                    if !value.is_finite() || !(0.0..=1.0).contains(value) {
                        return invalid("a map value is between 0 and 1");
                    }
                    *slot = Some(vec![*value; n]);
                }
                MapEdit::Invert => {
                    let m = slot.get_or_insert_with(|| vec![1.0; n]);
                    for v in m.iter_mut() {
                        *v = 1.0 - v.clamp(0.0, 1.0);
                    }
                }
                MapEdit::Paint { center, radius, value, strength, falloff } => {
                    let m = slot.get_or_insert_with(|| vec![1.0; n]);
                    paint_map(m, &surface, Vec3::from_array(*center), *radius, *value, *strength, *falloff).map_err(err)?;
                }
            }
            store(doc, *id, data)?;
        }
    }
    Ok(())
}

fn bounds(p: &[Vec3]) -> (Vec3, Vec3) {
    p.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)))
}
