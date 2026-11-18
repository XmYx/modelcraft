//! The Properties section of a clone modifier: its mode, what it does to the clones (move, turn,
//! grow, tint) and, for the target mode, the object it points at. Every number has an accessible
//! name and every choice is a combo box, so the section works from the keyboard like the rest of
//! the panel. Falloff fields added inside the modifier limit where it acts.

use egui::RichText;
use modelcraft_engine::Command;
use modelcraft_engine::clone::{Axis, EASES, Ease, MODES, Mode, Modifier};
use modelcraft_engine::scene::{ObjectId, ObjectKind};

use crate::app::ModelApp;
use crate::panels::{commit_edit, drag, set_name, vec3_row};
use crate::theme;

/// The label of a mode (our own names, `docs/feature-names.md`).
pub fn mode_label(kind: &str) -> String {
    match kind {
        "plain" => tl!("Plain"),
        "random" => tl!("Random"),
        "noise" => tl!("Noise"),
        "step" => tl!("Step"),
        "delay" => tl!("Delay"),
        "target" => tl!("Target"),
        _ => kind.to_string(),
    }
}

fn ease_label(e: &str) -> String {
    match e {
        "linear" => tl!("Even"),
        "smooth" => tl!("Smooth"),
        "in" => tl!("Slow start"),
        "out" => tl!("Slow finish"),
        _ => e.to_string(),
    }
}

fn axis_label(a: Axis) -> String {
    match a {
        Axis::X => tl!("X axis"),
        Axis::Y => tl!("Y axis"),
        Axis::Z => tl!("Z axis"),
    }
}

/// A combo box over `options` (label per entry); returns the entry the user picked. Focus goes
/// back to the combo after a pick, so keyboard users stay where they were.
fn pick(ui: &mut egui::Ui, salt: (&str, u64), name: String, shown: String, selected: usize, options: &[String]) -> Option<usize> {
    let mut chosen = None;
    ui.horizontal(|ui| {
        ui.label(name.clone());
        let r = egui::ComboBox::from_id_salt(salt).selected_text(shown).show_ui(ui, |ui| {
            for (i, o) in options.iter().enumerate() {
                if ui.selectable_label(i == selected, o).clicked() && i != selected {
                    chosen = Some(i);
                }
            }
        });
        set_name(ui, &r.response, name);
        if chosen.is_some() {
            r.response.request_focus();
        }
    });
    chosen
}

pub fn fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, modifier: Modifier, target: Option<ObjectId>) {
    let mut q = modifier;
    let mut t = target;
    let mut drags: Vec<egui::Response> = Vec::new();
    // Direct changes (combos, checkboxes) commit at once; drags commit when released.
    let mut discrete = false;

    let labels: Vec<String> = MODES.iter().map(|k| mode_label(k)).collect();
    let current = MODES.iter().position(|k| *k == q.kind()).unwrap_or(0);
    if let Some(i) = pick(ui, ("modifier-mode", id.0), tl!("Mode"), labels[current].clone(), current, &labels)
        && let Some(mode) = MODES.get(i).and_then(|k| Mode::default_of(k))
    {
        q.mode = mode;
        discrete = true;
    }
    drags.push(drag(ui, &mut q.strength, 0.01, -10.0..=10.0, tl!("Strength")));

    let ease_names: Vec<String> = EASES.iter().map(|e| ease_label(e)).collect();
    let mut ease_pick = |ui: &mut egui::Ui, ease: &mut Ease, salt: u64, discrete: &mut bool| {
        let cur = EASES.iter().position(|e| *e == ease.name()).unwrap_or(0);
        if let Some(i) = pick(ui, ("modifier-ease", salt), tl!("Easing"), ease_names[cur].clone(), cur, &ease_names)
            && let Some(e) = EASES.get(i).and_then(|n| Ease::from_name(n))
        {
            *ease = e;
            *discrete = true;
        }
    };
    match &mut q.mode {
        Mode::Plain => {}
        Mode::Random { seed, signed } => {
            ui.horizontal(|ui| {
                ui.label(tl!("Seed"));
                let r = ui.add(egui::DragValue::new(seed).speed(0.2));
                set_name(ui, &r, tl!("Seed"));
                drags.push(r);
            });
            discrete |= ui.checkbox(signed, tl!("Both directions")).changed();
        }
        Mode::Noise { seed, frequency, octaves, speed, signed } => {
            ui.horizontal(|ui| {
                ui.label(tl!("Seed"));
                let r = ui.add(egui::DragValue::new(seed).speed(0.2));
                set_name(ui, &r, tl!("Seed"));
                drags.push(r);
            });
            ui.horizontal(|ui| drags.push(drag(ui, frequency, 0.01, 0.0..=1000.0, tl!("Detail per unit"))));
            ui.horizontal(|ui| {
                ui.label(tl!("Layers"));
                let r = ui.add(egui::DragValue::new(octaves).speed(0.1).range(1..=4));
                set_name(ui, &r, tl!("Layers"));
                drags.push(r);
            });
            drags.extend(vec3_row(ui, &tl!("Drift per second"), speed, 0.01));
            discrete |= ui.checkbox(signed, tl!("Both directions")).changed();
        }
        Mode::Step { steps, ease, reverse } => {
            ui.horizontal(|ui| {
                ui.label(tl!("Levels (0 for smooth)"));
                let r = ui.add(egui::DragValue::new(steps).speed(0.2).range(0..=10_000));
                set_name(ui, &r, tl!("Levels (0 for smooth)"));
                drags.push(r);
            });
            ease_pick(ui, ease, id.0, &mut discrete);
            discrete |= ui.checkbox(reverse, tl!("From the last clone")).changed();
        }
        Mode::Delay { start, stagger, duration, ease, reverse } => {
            ui.horizontal(|ui| drags.push(drag(ui, start, 0.01, -3600.0..=3600.0, tl!("Start (seconds)"))));
            ui.horizontal(|ui| drags.push(drag(ui, stagger, 0.005, 0.0..=3600.0, tl!("Delay per clone (seconds)"))));
            ui.horizontal(|ui| drags.push(drag(ui, duration, 0.01, 0.0..=3600.0, tl!("Duration (seconds)"))));
            ease_pick(ui, ease, id.0, &mut discrete);
            discrete |= ui.checkbox(reverse, tl!("From the last clone")).changed();
        }
        Mode::Target { pull, aim, aim_axis, reach } => {
            ui.horizontal(|ui| drags.push(drag(ui, pull, 0.01, 0.0..=1.0, tl!("Pull toward target"))));
            ui.horizontal(|ui| drags.push(drag(ui, reach, 0.05, 0.0..=100_000.0, tl!("Reach (0 for everywhere)"))));
            discrete |= ui.checkbox(aim, tl!("Turn toward target")).changed();
            let axes = [Axis::X, Axis::Y, Axis::Z];
            let names: Vec<String> = axes.iter().map(|a| axis_label(*a)).collect();
            let cur = axes.iter().position(|a| a == aim_axis).unwrap_or(0);
            if let Some(i) = pick(ui, ("modifier-aim", id.0), tl!("Aim axis"), names[cur].clone(), cur, &names)
                && let Some(a) = axes.get(i)
            {
                *aim_axis = *a;
                discrete = true;
            }
        }
    }

    ui.separator();
    drags.extend(vec3_row(ui, &tl!("Move"), &mut q.position, 0.01));
    drags.extend(vec3_row(ui, &tl!("Turn"), &mut q.rotation, 0.5));
    drags.extend(vec3_row(ui, &tl!("Grow"), &mut q.scale, 0.01));
    discrete |= ui.checkbox(&mut q.uniform_scale, tl!("Grow every axis alike")).changed();
    ui.horizontal(|ui| {
        for (name, c) in [(tl!("Tint red"), 0), (tl!("Tint green"), 1), (tl!("Tint blue"), 2)] {
            drags.push(drag(ui, &mut q.tint[c], 0.01, 0.0..=1.0, name));
        }
    });
    ui.horizontal(|ui| drags.push(drag(ui, &mut q.tint_mix, 0.01, 0.0..=1.0, tl!("Tint amount"))));

    if q.needs_target() {
        let candidates: Vec<(ObjectId, String)> = {
            let scene = &app.doc.scene;
            let own_generator = scene.get(id).and_then(|o| o.parent);
            let excluded = own_generator.map(|g| scene.subtree(g)).unwrap_or_default();
            scene
                .objects()
                .iter()
                .filter(|o| o.id != id && !excluded.contains(&o.id) && matches!(o.kind, ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. } | ObjectKind::Group))
                .map(|o| (o.id, o.name.clone()))
                .collect()
        };
        let mut names = vec![tl!("None")];
        names.extend(candidates.iter().map(|(_, n)| n.clone()));
        let cur = t.and_then(|x| candidates.iter().position(|(i, _)| *i == x)).map_or(0, |p| p + 1);
        if let Some(i) = pick(ui, ("modifier-target", id.0), tl!("Target object"), names[cur].clone(), cur, &names) {
            t = if i == 0 { None } else { candidates.get(i - 1).map(|(c, _)| *c) };
            discrete = true;
        }
    }

    // Commit: a combo or checkbox change at once, number fields as one undo step per edit.
    let changed = q != modifier || t != target;
    if changed && discrete {
        app.run(Command::SetCloneModifier { id, modifier: q, target: t });
    } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        commit_edit(
            app,
            r,
            |o| o.kind = ObjectKind::CloneModifier { modifier: q, target: t },
            || Command::SetCloneModifier { id, modifier: q, target: t },
            id,
        );
    }

    let fields = app.doc.scene.field_stack(id).len();
    ui.label(RichText::new(crate::tlf!("{count} falloff fields limit where it acts", count = fields)).color(theme::TEXT_DIM).small());
    if modifier.is_time_dependent() {
        ui.label(RichText::new(tl!("Follows the timeline: move the current frame to see it change.")).color(theme::TEXT_DIM).small());
    }
    if let Some(parent) = app.doc.scene.get(id).and_then(|o| o.parent) {
        let e = app.doc.eval.evaluate(&app.doc.scene, parent);
        for err in e.errors.iter().filter(|e| e.message.contains("target")) {
            ui.label(RichText::new(&err.message).color(theme::TEXT_DIM).small());
        }
    }
    ui.label(
        RichText::new(tl!("Acts on the clones of the generator it sits in. Add a falloff field inside it to limit where. Modifiers apply in order."))
            .color(theme::TEXT_DIM)
            .small(),
    );
}
