//! The Coordinates panel: position, rotation and size (or scale) of the selection as numbers.
//!
//! For objects it shows the active object's values and applies a typed change to every selected
//! object, as one history step (the same batch the Properties fields use). For components
//! (vertices, edges, faces) it shows the centre and the extent of the selection and applies the
//! change as one "Move components" step. The change goes through [`NumericTransform`], the same
//! numbers Transform by Numbers applies, so the two cannot disagree.
//!
//! Switches: Object or World for the position; Size or Scale for the third column. Rotation is the
//! object's own angles for an object and a turn to apply for components. Nothing is applied until
//! Apply (or Enter in a field), so a number can be typed over several fields first.

use egui::RichText;
use glam::{Mat4, Quat, Vec3};
use modelcraft_engine::scene::ObjectId;

use crate::app::{ModelApp, NumericTransform, SelectMode};
use crate::panels::set_name;
use crate::theme;

/// The three numbers of one kind for the selection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub position: Vec3,
    /// XYZ Euler angles in degrees.
    pub rotation: Vec3,
    /// The size of the bounding box, or the scale.
    pub size: Vec3,
}

/// The change that turns `now` into `wanted`, as the numbers Transform by Numbers applies: a
/// move, a turn (the rotation that takes the old angles to the new) and a scale factor per axis.
/// An axis with no extent to scale (size 0) is left alone.
pub fn change(now: &Reading, wanted: &Reading) -> NumericTransform {
    let q = |d: Vec3| {
        let r = d * (std::f32::consts::PI / 180.0);
        Quat::from_euler(glam::EulerRot::XYZ, r.x, r.y, r.z)
    };
    let turn = (q(wanted.rotation) * q(now.rotation).inverse()).normalize();
    let (x, y, z) = turn.to_euler(glam::EulerRot::XYZ);
    let ratio = |w: f32, n: f32| if n.abs() > 1e-6 && w.is_finite() && w.abs() > 1e-6 { w / n } else { 1.0 };
    NumericTransform {
        translate: wanted.position - now.position,
        rotate_deg: Vec3::new(x, y, z) * (180.0 / std::f32::consts::PI),
        scale: Vec3::new(ratio(wanted.size.x, now.size.x), ratio(wanted.size.y, now.size.y), ratio(wanted.size.z, now.size.z)),
    }
}

#[derive(Default)]
pub struct State {
    /// Show and edit the position in world space instead of the object's own space.
    pub world: bool,
    /// Show the scale instead of the size.
    pub scale: bool,
    /// What the fields hold while they differ from the selection.
    draft: Option<Reading>,
    /// The selection the draft was typed for.
    sig: Vec<u64>,
}

/// What the panel acts on.
enum Target {
    Nothing,
    Object(ObjectId),
    Components,
}

fn target(app: &ModelApp) -> Target {
    if app.mode != SelectMode::Object && !app.components.is_empty() && app.edit_target().is_some() {
        Target::Components
    } else {
        app.active().map_or(Target::Nothing, Target::Object)
    }
}

fn signature(app: &ModelApp, t: &Target) -> Vec<u64> {
    match t {
        Target::Nothing => Vec::new(),
        Target::Object(id) => app.selection.iter().map(|i| i.0).chain([id.0]).collect(),
        Target::Components => app.components.iter().map(|c| u64::from(*c) | (1 << 40)).collect(),
    }
}

fn parent_world(app: &ModelApp, id: ObjectId) -> Mat4 {
    app.doc.scene.get(id).and_then(|o| o.parent).map_or(Mat4::IDENTITY, |p| app.doc.scene.world_matrix(p))
}

/// The selection's numbers, as the panel shows them (length units applied).
fn read(app: &mut ModelApp, t: &Target) -> Option<Reading> {
    let per = app.coord.units_per_metre(app.unit.per_metre());
    match t {
        Target::Nothing => None,
        Target::Object(id) => {
            let o = app.doc.scene.get(*id)?;
            let world = app.doc.scene.world_matrix(*id);
            let position = if app.coord.world { world.w_axis.truncate() } else { o.transform.translation };
            let size = if app.coord.scale {
                o.transform.scale
            } else {
                let mesh = app.doc.eval.evaluate(&app.doc.scene, *id).mesh;
                modelcraft_engine::scene::organize::bounds(mesh.positions()).map_or(Vec3::ZERO, |(lo, hi)| (hi - lo) * o.transform.scale.abs())
            };
            Some(Reading { position: position * per, rotation: o.transform.euler_degrees(), size: if app.coord.scale { size } else { size * per } })
        }
        Target::Components => {
            let id = app.edit_target()?;
            let verts = app.component_vertices();
            let mesh = app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh;
            let m = if app.coord.world { app.doc.scene.world_matrix(id) } else { Mat4::IDENTITY };
            let pts: Vec<Vec3> = verts.iter().filter_map(|v| mesh.positions().get(*v as usize)).map(|p| m.transform_point3(*p)).collect();
            let (lo, hi) = modelcraft_engine::scene::organize::bounds(&pts)?;
            let size = if app.coord.scale { Vec3::ONE } else { (hi - lo) * per };
            Some(Reading { position: (lo + hi) * 0.5 * per, rotation: Vec3::ZERO, size })
        }
    }
}

impl State {
    fn units_per_metre(&self, per: f32) -> f32 {
        if per.is_finite() && per > 0.0 { per } else { 1.0 }
    }
}

/// Apply a typed reading: one history step.
pub fn apply(app: &mut ModelApp, wanted: Reading) -> bool {
    let t = target(app);
    let Some(now) = read(app, &t) else { return false };
    let per = app.coord.units_per_metre(app.unit.per_metre());
    // Back to metres.
    let unit = |r: Reading, scale: bool| Reading { position: r.position / per, size: if scale { r.size } else { r.size / per }, ..r };
    let (now, wanted) = (unit(now, app.coord.scale), unit(wanted, app.coord.scale));
    let mut n = change(&now, &wanted);
    if n.translate == Vec3::ZERO && n.rotate_deg.abs().max_element() < 1e-4 && (n.scale - Vec3::ONE).abs().max_element() < 1e-6 {
        app.status = tl!("Coordinates already match");
        return false;
    }
    match t {
        Target::Nothing => false,
        Target::Object(id) => {
            let Some(before) = app.doc.scene.get(id).map(|o| o.transform) else { return false };
            if app.coord.world {
                n.translate = parent_world(app, id).inverse().transform_vector3(n.translate);
            }
            let after = n.applied_to(before);
            let cmd = crate::object_manager::transform_batch(app, id, before, after);
            app.run(cmd)
        }
        Target::Components => {
            if app.coord.world
                && let Some(id) = app.edit_target()
            {
                n.translate = app.doc.scene.world_matrix(id).inverse().transform_vector3(n.translate);
            }
            let steps = app.doc.undo.len();
            app.apply_numeric(n);
            app.doc.undo.len() > steps
        }
    }
}

pub fn panel(app: &mut ModelApp, ui: &mut egui::Ui) {
    let t = target(app);
    let sig = signature(app, &t);
    if app.coord.sig != sig {
        app.coord.sig = sig;
        app.coord.draft = None;
    }
    let Some(now) = read(app, &t) else {
        ui.label(RichText::new(tl!("Nothing selected.")).color(theme::TEXT_DIM));
        return;
    };
    let what = match t {
        Target::Components => tl!("selected components"),
        _ => app.active().and_then(|id| app.doc.scene.get(id)).map(|o| o.name.clone()).unwrap_or_default(),
    };
    ui.label(RichText::new(crate::tlf!("Applies to: {target}", target = what)).color(theme::TEXT_DIM));
    ui.horizontal(|ui| {
        for (value, label) in [(false, tl!("Object")), (true, tl!("World"))] {
            let r = ui.add(egui::Button::selectable(app.coord.world == value, label.clone()));
            set_name(ui, &r, crate::tlf!("{space} position", space = label));
            if r.clicked() && app.coord.world != value {
                app.coord.world = value;
                app.coord.draft = None;
            }
        }
        ui.separator();
        for (value, label) in [(false, tl!("Size")), (true, tl!("Scale"))] {
            let r = ui.add(egui::Button::selectable(app.coord.scale == value, label.clone()));
            set_name(ui, &r, crate::tlf!("{mode} numbers", mode = label));
            if r.clicked() && app.coord.scale != value {
                app.coord.scale = value;
                app.coord.draft = None;
            }
        }
    });
    let mut shown = app.coord.draft.unwrap_or(now);
    let mut apply_now = false;
    let mut edited = false;
    let third = if app.coord.scale { tl!("Scale") } else { tl!("Size") };
    egui::Grid::new("coordinates").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label("");
        for h in [tl!("Position"), tl!("Rotation"), third.clone()] {
            ui.label(RichText::new(h).color(theme::TEXT_DIM));
        }
        ui.end_row();
        for (i, (axis, c)) in [("X", theme::AXIS_X), ("Y", theme::AXIS_Y), ("Z", theme::AXIS_Z)].into_iter().enumerate() {
            ui.label(RichText::new(axis).color(c));
            let columns: [(String, &mut f32, f32); 3] = [
                (tl!("Coordinate position"), &mut shown.position[i], 0.01),
                (tl!("Coordinate rotation"), &mut shown.rotation[i], 0.5),
                (if app.coord.scale { tl!("Coordinate scale") } else { tl!("Coordinate size") }, &mut shown.size[i], 0.01),
            ];
            for (name, value, speed) in columns {
                let r = ui.add_sized([62.0, 18.0], egui::DragValue::new(value).speed(speed).max_decimals(3));
                set_name(ui, &r, format!("{name} {axis}"));
                edited |= r.changed();
                apply_now |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            }
            ui.end_row();
        }
    });
    if edited {
        app.coord.draft = Some(shown);
    }
    let dirty = app.coord.draft.is_some_and(|d| d != now);
    ui.horizontal(|ui| {
        let r = ui.add_enabled(dirty, egui::Button::new(tl!("Apply")));
        set_name(ui, &r, tl!("Apply coordinates"));
        apply_now |= r.clicked();
        let r = ui.add_enabled(dirty, egui::Button::new(tl!("Revert")));
        set_name(ui, &r, tl!("Revert coordinates"));
        if r.clicked() {
            app.coord.draft = None;
        }
    });
    if apply_now && let Some(w) = app.coord.draft {
        if apply(app, w) {
            app.status = tl!("Coordinates applied");
        }
        app.coord.draft = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(p: [f32; 3], r: [f32; 3], s: [f32; 3]) -> Reading {
        Reading { position: Vec3::from(p), rotation: Vec3::from(r), size: Vec3::from(s) }
    }

    #[test]
    fn the_change_turns_now_into_wanted_for_move_turn_and_scale() {
        let now = reading([1.0, 2.0, 3.0], [10.0, 20.0, 30.0], [2.0, 4.0, 1.0]);
        let wanted = reading([2.0, 2.0, 0.0], [45.0, 20.0, -10.0], [4.0, 2.0, 1.0]);
        let n = change(&now, &wanted);
        assert_eq!(n.translate, Vec3::new(1.0, 0.0, -3.0));
        assert_eq!(n.scale, Vec3::new(2.0, 0.5, 1.0));
        let mut t = modelcraft_engine::scene::Transform::default();
        t.set_euler_degrees(now.rotation);
        t.translation = now.position;
        let after = n.applied_to(t);
        let mut want = modelcraft_engine::scene::Transform::default();
        want.set_euler_degrees(wanted.rotation);
        assert!(after.rotation.dot(want.rotation).abs() > 0.99999, "the turn reaches the wanted rotation");
        assert!((after.translation - wanted.position).length() < 1e-5);
    }

    #[test]
    fn nothing_typed_means_no_change_and_a_flat_axis_is_not_scaled() {
        let now = reading([1.0, 1.0, 1.0], [0.0, 90.0, 0.0], [2.0, 0.0, 2.0]);
        let n = change(&now, &now);
        assert_eq!(n.translate, Vec3::ZERO);
        assert!(n.rotate_deg.abs().max_element() < 1e-3);
        assert_eq!(n.scale, Vec3::ONE);
        let wanted = Reading { size: Vec3::new(4.0, 5.0, 2.0), ..now };
        assert_eq!(change(&now, &wanted).scale, Vec3::new(2.0, 1.0, 1.0), "size 0 has no extent to scale");
        let nan = Reading { size: Vec3::splat(f32::NAN), position: Vec3::splat(f32::NAN), ..now };
        assert_eq!(change(&now, &nan).scale, Vec3::ONE);
    }
}
