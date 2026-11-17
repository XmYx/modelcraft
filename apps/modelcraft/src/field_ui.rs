//! The Properties section of a falloff field and its outline in the viewport. Every control has a
//! name; shape, blend and curve picker are combo boxes, so they work from the keyboard.

use egui::RichText;
use glam::{Mat4, Vec3};
use modelcraft_engine::Command;
use modelcraft_engine::field::{BLENDS, Blend, CURVE_PRESETS, Curve, Field, MAX_OCTAVES, SHAPES, Shape};
use modelcraft_engine::scene::{ObjectId, ObjectKind, Scene};

use crate::app::ModelApp;
use crate::panels::{commit_edit, drag, set_name, vec3_row};
use crate::theme;

pub fn shape_label(kind: &str) -> String {
    match kind {
        "sphere" => tl!("Sphere"),
        "box" => tl!("Box"),
        "linear" => tl!("Linear"),
        "noise" => tl!("Noise"),
        _ => kind.to_string(),
    }
}

fn blend_label(b: &str) -> String {
    match b {
        "multiply" => tl!("Multiply"),
        "add" => tl!("Add"),
        "subtract" => tl!("Subtract"),
        "min" => tl!("Smallest"),
        "max" => tl!("Largest"),
        _ => b.to_string(),
    }
}

fn curve_label(preset: Option<&str>) -> String {
    match preset {
        Some("linear") => tl!("Straight"),
        Some("smooth") => tl!("Smooth"),
        Some("ease_in") => tl!("Slow start"),
        Some("ease_out") => tl!("Slow finish"),
        Some("steep") => tl!("Steep"),
        _ => tl!("Custom"),
    }
}

fn labelled_combo<R>(ui: &mut egui::Ui, salt: impl std::hash::Hash + std::fmt::Debug, name: String, shown: String, add: impl FnOnce(&mut egui::Ui) -> R) {
    ui.horizontal(|ui| {
        ui.label(name.clone());
        let r = egui::ComboBox::from_id_salt(salt).selected_text(shown).show_ui(ui, add);
        set_name(ui, &r.response, name);
    });
}

pub fn fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, field: &Field) {
    let mut q = field.clone();
    let mut drags: Vec<egui::Response> = Vec::new();
    let mut discrete = false;

    labelled_combo(ui, ("field-shape", id.0), tl!("Shape"), shape_label(q.kind()), |ui| {
        for k in SHAPES {
            if ui.selectable_label(q.kind() == *k, shape_label(k)).clicked()
                && q.kind() != *k
                && let Some(s) = Shape::default_of(k)
            {
                q.shape = s;
                discrete = true;
            }
        }
    });
    match &mut q.shape {
        Shape::Sphere { radius, inner } => {
            ui.horizontal(|ui| drags.push(drag(ui, radius, 0.01, 0.0001..=100_000.0, tl!("Radius"))));
            ui.horizontal(|ui| drags.push(drag(ui, inner, 0.01, 0.0..=0.999, tl!("Full strength inside"))));
        }
        Shape::Box { size, inner } => {
            drags.extend(vec3_row(ui, &tl!("Box size"), size, 0.01));
            ui.horizontal(|ui| drags.push(drag(ui, inner, 0.01, 0.0..=0.999, tl!("Full strength inside"))));
        }
        Shape::Linear { length } => {
            ui.horizontal(|ui| drags.push(drag(ui, length, 0.01, 0.0001..=100_000.0, tl!("Length"))));
        }
        Shape::Noise { scale, octaves, seed } => {
            ui.horizontal(|ui| drags.push(drag(ui, scale, 0.01, -100_000.0..=100_000.0, tl!("Scale"))));
            ui.horizontal(|ui| {
                ui.label(tl!("Octaves"));
                let r = ui.add(egui::DragValue::new(octaves).range(1..=MAX_OCTAVES));
                set_name(ui, &r, tl!("Octaves"));
                drags.push(r);
                ui.label(tl!("Seed"));
                let r = ui.add(egui::DragValue::new(seed).speed(0.2));
                set_name(ui, &r, tl!("Seed"));
                drags.push(r);
            });
        }
    }

    labelled_combo(ui, ("field-blend", id.0), tl!("Combine with earlier fields"), blend_label(q.blend.name()), |ui| {
        for b in BLENDS {
            let Some(blend) = Blend::from_name(b) else { continue };
            if ui.selectable_label(q.blend == blend, blend_label(b)).clicked() && q.blend != blend {
                q.blend = blend;
                discrete = true;
            }
        }
    });

    ui.label(RichText::new(tl!("Remap")).strong());
    labelled_combo(ui, ("field-curve", id.0), tl!("Curve"), curve_label(q.remap.curve.preset_name()), |ui| {
        for p in CURVE_PRESETS {
            if ui.selectable_label(q.remap.curve.preset_name() == Some(*p), curve_label(Some(p))).clicked()
                && let Some(c) = Curve::preset(p)
            {
                q.remap.curve = c;
                discrete = true;
            }
        }
    });
    let r = ui.checkbox(&mut q.remap.invert, tl!("Invert"));
    discrete |= r.changed();
    ui.horizontal(|ui| drags.push(drag(ui, &mut q.remap.strength, 0.01, 0.0..=1.0, tl!("Strength"))));

    egui::CollapsingHeader::new(tl!("Curve points")).default_open(false).show(ui, |ui| {
        let mut remove = None;
        let n = q.remap.curve.points.len();
        for (i, pt) in q.remap.curve.points.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(crate::tlf!("Point {n}", n = i + 1));
                let rx = ui.add(egui::DragValue::new(&mut pt[0]).speed(0.005).range(0.0..=1.0).max_decimals(3));
                set_name(ui, &rx, crate::tlf!("Point {n} input", n = i + 1));
                let ry = ui.add(egui::DragValue::new(&mut pt[1]).speed(0.005).range(0.0..=1.0).max_decimals(3));
                set_name(ui, &ry, crate::tlf!("Point {n} output", n = i + 1));
                drags.push(rx);
                drags.push(ry);
                let x = ui.add_enabled(n > 2, egui::Button::new("x").small());
                set_name(ui, &x, crate::tlf!("Remove point {n}", n = i + 1));
                if x.clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            q.remap.curve.points.remove(i);
            discrete = true;
        }
        let can_add = q.remap.curve.points.len() < Curve::MAX_POINTS;
        if ui.add_enabled(can_add, egui::Button::new(tl!("Add point"))).clicked() {
            // Halfway along the curve between the two points that leave the widest gap.
            let pts = &q.remap.curve.points;
            let gap = (1..pts.len()).max_by(|a, b| (pts[*a][0] - pts[a - 1][0]).total_cmp(&(pts[*b][0] - pts[b - 1][0])));
            if let Some(g) = gap {
                let x = (pts[g][0] + pts[g - 1][0]) * 0.5;
                let y = q.remap.curve.eval(x);
                q.remap.curve.points.insert(g, [x, y]);
                discrete = true;
            }
        }
    });

    if q != *field && discrete {
        app.run(Command::SetField { id, field: q });
    } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(app, r, |o| o.kind = ObjectKind::Field { field: q.clone() }, || Command::SetField { id, field: q.clone() }, id);
    }
    ui.label(
        RichText::new(tl!(
            "Inside a deformer: the deformer acts where the field is strong. Move and scale this object to place the field. Hide it to switch it off."
        ))
        .color(theme::TEXT_DIM)
        .small(),
    );
}

/// Outline segments (world space) of a field's shape, for the viewport.
pub fn outline(scene: &Scene, id: ObjectId) -> Vec<(Vec3, Vec3)> {
    let Some(ObjectKind::Field { field }) = scene.get(id).map(|o| &o.kind) else { return Vec::new() };
    let w: Mat4 = scene.world_matrix(id);
    let mut seg: Vec<(Vec3, Vec3)> = Vec::new();
    let circle = |seg: &mut Vec<(Vec3, Vec3)>, axis: usize, r: f32| {
        let n = 32;
        for k in 0..n {
            let (a, b) = (k as f32 / n as f32 * std::f32::consts::TAU, (k + 1) as f32 / n as f32 * std::f32::consts::TAU);
            let pt = |t: f32| match axis {
                0 => Vec3::new(0.0, t.cos() * r, t.sin() * r),
                1 => Vec3::new(t.cos() * r, 0.0, t.sin() * r),
                _ => Vec3::new(t.cos() * r, t.sin() * r, 0.0),
            };
            seg.push((pt(a), pt(b)));
        }
    };
    let cuboid = |seg: &mut Vec<(Vec3, Vec3)>, lo: Vec3, hi: Vec3| {
        let c = |i: usize| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        for i in 0..8usize {
            for bit in [1usize, 2, 4] {
                if i & bit == 0 {
                    seg.push((c(i), c(i | bit)));
                }
            }
        }
    };
    match field.sanitized().shape {
        Shape::Sphere { radius, .. } => (0..3).for_each(|a| circle(&mut seg, a, radius)),
        Shape::Box { size, .. } => cuboid(&mut seg, -size * 0.5, size * 0.5),
        Shape::Linear { length } => cuboid(&mut seg, Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, length, 1.0)),
        Shape::Noise { scale, .. } => {
            let h = (0.5 / scale.abs().max(1e-3)).min(5.0);
            cuboid(&mut seg, Vec3::splat(-h), Vec3::splat(h));
        }
    }
    seg.into_iter().map(|(a, b)| (w.transform_point3(a), w.transform_point3(b))).collect()
}
