//! File ▸ Exchange Settings: the up axis and unit of the files ModelCraft imports and exports.
//! "From the file" uses what an FBX file says (others are read as Y up in metres); the choice
//! applies to the next import and export and is part of the journaled import command.

use modelcraft_engine::formats::exchange::Up;

use crate::app::ModelApp;

/// Units offered, as metres per file unit (`None`: from the file).
pub const UNITS: &[(Option<f32>, &str)] =
    &[(None, "From the file"), (Some(1.0), "Metres"), (Some(0.01), "Centimetres"), (Some(0.001), "Millimetres"), (Some(0.0254), "Inches")];

pub fn up_label(u: Up) -> String {
    match u {
        Up::Auto => tl!("From the file"),
        Up::Y => tl!("Y up"),
        Up::Z => tl!("Z up"),
    }
}

pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new(tl!("Up axis")).color(crate::theme::TEXT_DIM).small());
    for u in [Up::Auto, Up::Y, Up::Z] {
        ui.radio_value(&mut app.exchange.up, u, up_label(u));
    }
    ui.separator();
    ui.label(egui::RichText::new(tl!("File unit")).color(crate::theme::TEXT_DIM).small());
    for (s, label) in UNITS {
        ui.radio_value(&mut app.exchange.scale, *s, tl!(label));
    }
}
