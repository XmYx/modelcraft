//! The rendered layers of a material in the Properties panel: emission, coat, sheen,
//! transmission and index of refraction. The Render View and `modelcraft-cli render` show them;
//! the viewport shows the base colour, metallic and roughness only.

use modelcraft_engine::Command;
use modelcraft_engine::scene::{Material, Object, ObjectId};

use crate::app::ModelApp;
use crate::panels::{commit_edit, drag};

type Field = fn(&mut Material) -> &mut f32;

pub fn rows(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, obj: &Object) {
    let sliders: [(String, Field, std::ops::RangeInclusive<f32>); 6] = [
        (tl!("Coat"), |m| &mut m.coat, 0.0..=1.0),
        (tl!("Coat roughness"), |m| &mut m.coat_roughness, 0.0..=1.0),
        (tl!("Sheen"), |m| &mut m.sheen, 0.0..=1.0),
        (tl!("Transmission"), |m| &mut m.transmission, 0.0..=1.0),
        (tl!("Index of refraction"), |m| &mut m.ior, 1.0..=3.0),
        (tl!("Emission strength"), |m| &mut m.emission_strength, 0.0..=1000.0),
    ];
    for (label, field, range) in sliders {
        let mut m = obj.material.clone();
        ui.horizontal(|ui| {
            let r = drag(ui, field(&mut m), 0.01, range, label);
            let mm = m.clone();
            commit_edit(app, &r, |o| o.material = mm.clone(), || Command::SetMaterial { id, material: mm.clone() }, id);
        });
    }
    let mut m = obj.material.clone();
    ui.horizontal(|ui| {
        ui.label(tl!("Emission colour"));
        let r = ui.color_edit_button_rgb(&mut m.emission);
        ui.ctx().accesskit_node_builder(r.id, |n| n.set_label(tl!("Emission colour")));
        let mm = m.clone();
        commit_edit(app, &r, |o| o.material = mm.clone(), || Command::SetMaterial { id, material: mm.clone() }, id);
    });
}

#[cfg(test)]
mod tests {
    use crate::app::ModelApp;
    use crate::args::Cli;
    use egui::{Key, Modifiers};
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use modelcraft_engine::Command;

    #[test]
    fn material_layers_are_named_and_editable_from_the_keyboard() {
        let ctx = egui::Context::default();
        let mut app = ModelApp::with_context(&ctx, Cli::default());
        let id = app.doc.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
        app.select_only(id);
        let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app);
        h.run();
        h.get_by_label("Material").click();
        h.run();
        for label in ["Coat", "Coat roughness", "Sheen", "Transmission", "Index of refraction", "Emission strength", "Emission colour"] {
            assert!(h.get_all_by_label(label).count() >= 1, "{label} is named");
        }
        // Focus the Transmission field and raise it with the arrow key: each step is a command.
        let field = h
            .get_all_by_label("Transmission")
            .find(|n| n.accesskit_node().role() == egui::accesskit::Role::SpinButton)
            .expect("a spin button named Transmission");
        field.focus();
        h.run();
        for _ in 0..5 {
            h.key_press(Key::ArrowUp);
            h.run();
        }
        let after = h.state().doc.scene.get(id).unwrap().material.transmission;
        assert!(after > 0.0 && after <= 1.0, "transmission after five steps: {after}");
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run();
        assert!(h.state().doc.scene.get(id).unwrap().material.transmission < after, "undo steps back");
    }
}
