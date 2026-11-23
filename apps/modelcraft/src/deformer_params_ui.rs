//! The Properties section of a parametric deformer: its numbers, the box it works over (how far
//! it reaches, and whether that box fits the mesh) and, for a shrink-wrap, the object it pulls
//! onto. Every control has a name; pickers are combo boxes and so work from the keyboard.

use egui::RichText;
use glam::Vec3;
use modelcraft_engine::Command;
use modelcraft_engine::ops::deform::{Along, DisplaceSource, LIMIT_MODES, LimitMode, Limits, MAX_OCTAVES, MAX_SMOOTH_ITERATIONS, WrapMode};
use modelcraft_engine::scene::{Deformer, ObjectId, ObjectKind};

use crate::app::ModelApp;
use crate::generator_ui::pick;
use crate::panels::{commit_edit, drag_k, set_name, vec3_row};
use crate::theme;

fn limit_label(m: &str) -> String {
    match m {
        "limited" => tl!("Limited"),
        "unlimited" => tl!("Unlimited"),
        "within_box" => tl!("Within box"),
        _ => m.to_string(),
    }
}

fn source_label(s: DisplaceSource) -> String {
    match s {
        DisplaceSource::Noise => tl!("Noise"),
        DisplaceSource::Field => tl!("Falloff fields"),
    }
}

fn wrap_label(m: WrapMode) -> String {
    match m {
        WrapMode::Closest => tl!("Closest point"),
        WrapMode::Project => tl!("Along Y"),
    }
}

fn along_label(a: Along) -> String {
    match a {
        Along::Normal => tl!("Normal"),
        Along::X => tl!("X axis"),
        Along::Y => tl!("Y axis"),
        Along::Z => tl!("Z axis"),
        Along::Radial => tl!("Away from the Y axis"),
    }
}

fn int(ui: &mut egui::Ui, v: &mut u32, range: std::ops::RangeInclusive<u32>, name: String) -> egui::Response {
    ui.label(name.clone());
    let r = ui.add(egui::DragValue::new(v).range(range));
    set_name(ui, &r, name);
    r
}

fn set(id: ObjectId, deformer: Deformer, limits: Limits, target: Option<ObjectId>) -> Command {
    Command::SetDeformer { id, deformer, limits: Some(limits), target, clear_target: false }
}

pub fn fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, d: Deformer, limits: Limits, target: Option<ObjectId>) {
    let (mut q, mut lim, mut tgt) = (d.clone(), limits, target);
    let mut cleared = false;
    let mut discrete = false;
    let responses: Vec<egui::Response> = ui
        .horizontal_wrapped(|ui| match &mut q {
            Deformer::Bend { angle } | Deformer::Twist { angle } => vec![drag_k(ui, "deformer.angle", angle, 0.5, -3600.0..=3600.0, tl!("Angle"))],
            Deformer::Taper { amount } => vec![drag_k(ui, "deformer.amount", amount, 0.01, -10.0..=10.0, tl!("Amount"))],
            Deformer::Ripple { amplitude, frequency } => {
                vec![
                    drag_k(ui, "deformer.amplitude", amplitude, 0.005, -10.0..=10.0, tl!("Amplitude")),
                    drag_k(ui, "deformer.frequency", frequency, 0.01, 0.0..=100.0, tl!("Frequency")),
                ]
            }
            Deformer::Round { amount } => vec![drag_k(ui, "deformer.amount", amount, 0.01, 0.0..=1.0, tl!("Amount"))],
            Deformer::Shear { x, z } => {
                vec![drag_k(ui, "deformer.x", x, 0.01, -1000.0..=1000.0, tl!("Slide X")), drag_k(ui, "deformer.z", z, 0.01, -1000.0..=1000.0, tl!("Slide Z"))]
            }
            Deformer::Bulge { amount } => vec![drag_k(ui, "deformer.amount", amount, 0.01, -10.0..=10.0, tl!("Amount"))],
            Deformer::Squash { amount } => vec![drag_k(ui, "deformer.amount", amount, 0.01, -0.95..=10.0, tl!("Amount"))],
            Deformer::Smooth { iterations, strength } | Deformer::DeltaSmooth { iterations, strength } => {
                vec![int(ui, iterations, 0..=MAX_SMOOTH_ITERATIONS, tl!("Passes")), drag_k(ui, "deformer.strength", strength, 0.01, 0.0..=1.0, tl!("Strength"))]
            }
            Deformer::Displace { amplitude, scale, octaves, seed, .. } => {
                let mut v = vec![
                    drag_k(ui, "deformer.amplitude", amplitude, 0.005, -1000.0..=1000.0, tl!("Amplitude")),
                    drag_k(ui, "deformer.scale", scale, 0.01, -1000.0..=1000.0, tl!("Feature scale")),
                    int(ui, octaves, 1..=MAX_OCTAVES, tl!("Octaves")),
                ];
                ui.label(tl!("Seed"));
                let r = ui.add(egui::DragValue::new(seed).speed(0.2));
                set_name(ui, &r, tl!("Seed"));
                v.push(r);
                v
            }
            Deformer::ShrinkWrap { offset, .. } => vec![drag_k(ui, "deformer.offset", offset, 0.005, -1000.0..=1000.0, tl!("Offset"))],
            Deformer::Formula { amount, .. } => vec![drag_k(ui, "deformer.amount", amount, 0.01, -1000.0..=1000.0, tl!("Amount"))],
        })
        .inner;

    // Pickers: how a displacement reads its height, how a shrink-wrap finds its place.
    match &mut q {
        Deformer::Displace { source, .. } => {
            let opts = [DisplaceSource::Noise, DisplaceSource::Field];
            let labels: Vec<String> = opts.iter().map(|s| source_label(*s)).collect();
            let cur = opts.iter().position(|s| s == source).unwrap_or(0);
            if let Some(i) = pick(ui, ("displace-source", id.0), tl!("Height from"), cur, &labels) {
                *source = opts[i];
                discrete = true;
            }
        }
        Deformer::ShrinkWrap { mode, .. } => {
            let opts = [WrapMode::Closest, WrapMode::Project];
            let labels: Vec<String> = opts.iter().map(|m| wrap_label(*m)).collect();
            let cur = opts.iter().position(|m| m == mode).unwrap_or(0);
            if let Some(i) = pick(ui, ("wrap-mode", id.0), tl!("Find the surface"), cur, &labels) {
                *mode = opts[i];
                discrete = true;
            }
            // The target: any other mesh that is not inside this one.
            let mesh = app.doc.scene.get(id).and_then(|o| o.parent);
            let candidates: Vec<(ObjectId, String)> = app
                .doc
                .scene
                .objects()
                .iter()
                .filter(|o| {
                    o.id != id
                        && Some(o.id) != mesh
                        && matches!(o.kind, ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. } | ObjectKind::Array { .. } | ObjectKind::Symmetry { .. })
                        && !mesh.is_some_and(|m| app.doc.scene.subtree(m).contains(&o.id))
                })
                .map(|o| (o.id, o.name.clone()))
                .collect();
            let mut names = vec![tl!("None")];
            names.extend(candidates.iter().map(|(_, n)| n.clone()));
            let cur = tgt.and_then(|t| candidates.iter().position(|(i, _)| *i == t)).map_or(0, |p| p + 1);
            if let Some(i) = pick(ui, ("wrap-target", id.0), tl!("Target object"), cur, &names) {
                if i == 0 {
                    tgt = None;
                    cleared = true;
                } else {
                    tgt = candidates.get(i - 1).map(|(c, _)| *c);
                }
                discrete = true;
            }
            if tgt.is_none() {
                ui.label(RichText::new(tl!("Choose the object to wrap onto.")).color(theme::TEXT_DIM).small());
            }
        }
        Deformer::Formula { expr, along, .. } => {
            let labels: Vec<String> = Along::ALL.iter().map(|a| along_label(*a)).collect();
            let cur = Along::ALL.iter().position(|a| a == along).unwrap_or(0);
            if let Some(i) = pick(ui, ("formula-along", id.0), tl!("Move along"), cur, &labels)
                && let Some(a) = Along::ALL.get(i)
            {
                *along = *a;
                discrete = true;
            }
            let points = app.doc.scene.get(id).and_then(|o| o.parent).map_or(0, |m| app.doc.eval.evaluate(&app.doc.scene, m).mesh.vertex_count());
            let problem = d.problem(points);
            let hint = tl!("x, y, z, r, t, i, n, time, frame");
            if let Some(text) = crate::formula_modes_ui::formula_text(
                ui,
                ("deformer-formula", id.0),
                &tl!("Formula"),
                expr,
                problem,
                &hint,
                modelcraft_engine::ops::deform::FORMULA_NAMES,
            ) {
                *expr = text;
                discrete = true;
            }
        }
        _ => {}
    }

    // The box and how far the deformer reaches.
    let mut box_drags: Vec<egui::Response> = Vec::new();
    egui::CollapsingHeader::new(tl!("Reach")).id_salt(("deformer-reach", id.0)).default_open(false).show(ui, |ui| {
        let modes: Vec<String> = LIMIT_MODES.iter().map(|m| limit_label(m)).collect();
        let cur = LIMIT_MODES.iter().position(|m| *m == lim.mode.name()).unwrap_or(0);
        if let Some(i) = pick(ui, ("deformer-mode", id.0), tl!("Mode"), cur, &modes)
            && let Some(m) = LIMIT_MODES.get(i).and_then(|n| LimitMode::from_name(n))
        {
            lim.mode = m;
            discrete = true;
        }
        let r = ui.checkbox(&mut lim.fit_to_parent, tl!("Fit to parent"));
        discrete |= r.changed();
        if !lim.fit_to_parent {
            let mut size: Vec3 = lim.size;
            box_drags.extend(vec3_row(ui, &tl!("Box size"), &mut size, 0.01));
            lim.size = size;
        }
    });

    if discrete && (q != d || lim != limits || tgt != target || cleared) {
        let mut c = set(id, q.clone(), lim, tgt);
        if let Command::SetDeformer { clear_target, .. } = &mut c {
            *clear_target = cleared;
        }
        app.run(c);
    } else if let Some(r) = responses.iter().chain(box_drags.iter()).find(|r| r.changed() || r.drag_stopped()) {
        let (q2, lim2) = (q.clone(), lim);
        commit_edit(
            app,
            r,
            |o| {
                if let ObjectKind::Deformer { deformer, limits, .. } = &mut o.kind {
                    *deformer = q2.clone();
                    *limits = lim2;
                }
            },
            || set(id, q.clone(), lim, tgt),
            id,
        );
    }
}
