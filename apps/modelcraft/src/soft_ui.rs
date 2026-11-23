//! Soft selection and symmetric component editing: the settings, their commands and the
//! Properties section. The settings go into every component move, rotation, scale and
//! Transform by Numbers as part of the `MoveVertices` step (`modelcraft_ops::soft`), so a step
//! stays editable and replays exactly. Vertices are drawn in colour by their weight while soft
//! selection is on.

use egui::Color32;
use modelcraft_engine::field::{CURVE_PRESETS, Curve};
use modelcraft_engine::ops::soft::{Soft, Symmetry};

use crate::app::{ModelApp, SelectMode};
use crate::panels::{drag, set_name};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Toggle,
    RadiusUp,
    RadiusDown,
    /// Off, then X, Y and Z.
    CycleSymmetry,
}

pub const ALL: [Op; 4] = [Op::Toggle, Op::RadiusUp, Op::RadiusDown, Op::CycleSymmetry];

impl Op {
    pub fn label(self) -> String {
        match self {
            Op::Toggle => tl!("Toggle Soft Selection"),
            Op::RadiusUp => tl!("Soft Radius Bigger"),
            Op::RadiusDown => tl!("Soft Radius Smaller"),
            Op::CycleSymmetry => tl!("Cycle Component Symmetry"),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Op::Toggle => "select.soft",
            Op::RadiusUp => "select.soft_radius_up",
            Op::RadiusDown => "select.soft_radius_down",
            Op::CycleSymmetry => "select.symmetry",
        }
    }
    pub fn default_binding(self) -> Option<&'static str> {
        Some(match self {
            Op::Toggle => "Model: O",
            Op::RadiusUp => "Model: Alt+]",
            Op::RadiusDown => "Model: Alt+[",
            Op::CycleSymmetry => "Model: Shift+Y",
        })
    }
}

/// The settings kept while modeling.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub enabled: bool,
    pub radius: f32,
    /// One of `modelcraft_field::CURVE_PRESETS`.
    pub curve: &'static str,
    /// The mirror axis (0 = X, 1 = Y, 2 = Z) when symmetric editing is on.
    pub symmetry: Option<u8>,
}

impl Default for State {
    fn default() -> Self {
        State { enabled: false, radius: 0.5, curve: "smooth", symmetry: None }
    }
}

impl State {
    /// The soft selection to record in a step.
    pub fn soft(&self) -> Option<Soft> {
        self.enabled.then(|| Soft { radius: self.radius, curve: Curve::preset(self.curve).unwrap_or_default() })
    }
    /// The symmetry to record in a step.
    pub fn sym(&self) -> Option<Symmetry> {
        self.symmetry.map(Symmetry::new)
    }
}

const MIN_RADIUS: f32 = 0.001;
const MAX_RADIUS: f32 = 1e5;

fn axis_name(a: u8) -> String {
    match a {
        0 => tl!("X"),
        1 => tl!("Y"),
        _ => tl!("Z"),
    }
}

/// Run a command of this module.
pub fn run(app: &mut ModelApp, op: Op) {
    let s = &mut app.modeling.soft;
    match op {
        Op::Toggle => {
            s.enabled = !s.enabled;
            app.status =
                if s.enabled { crate::tlf!("Soft selection on, radius {radius}", radius = format!("{:.3}", s.radius)) } else { tl!("Soft selection off") };
        }
        Op::RadiusUp | Op::RadiusDown => {
            s.radius = (if op == Op::RadiusUp { s.radius * 1.25 } else { s.radius / 1.25 }).clamp(MIN_RADIUS, MAX_RADIUS);
            app.status = crate::tlf!("Soft radius {radius}", radius = format!("{:.3}", s.radius));
        }
        Op::CycleSymmetry => {
            s.symmetry = match s.symmetry {
                None => Some(0),
                Some(a) if a < 2 => Some(a + 1),
                Some(_) => None,
            };
            app.status = match s.symmetry {
                Some(a) => crate::tlf!("Component symmetry across {axis}", axis = axis_name(a)),
                None => tl!("Component symmetry off"),
            };
        }
    }
}

/// The weights of the vertices to draw, when soft selection is on and there is a selection.
pub fn weights(app: &mut ModelApp) -> Option<Vec<f32>> {
    let soft = app.modeling.soft.soft()?;
    if app.mode == SelectMode::Object || app.components.is_empty() {
        return None;
    }
    let id = app.edit_target()?;
    let m = app.doc.eval.steps_output(&app.doc.scene, id, usize::MAX).mesh;
    if m.vertex_count() > 200_000 {
        return None;
    }
    let verts = app.component_vertices();
    Some(modelcraft_engine::ops::soft::weights(&m, &verts, &soft))
}

/// The colour of a vertex dot by weight: blue far out, orange to red near the selection.
pub fn weight_color(w: f32) -> Color32 {
    let t = w.clamp(0.0, 1.0);
    let (a, b) = ([70.0, 110.0, 230.0], [255.0, 80.0, 60.0]);
    let c = |i: usize| (a[i] + (b[i] - a[i]) * t) as u8;
    Color32::from_rgb(c(0), c(1), c(2))
}

/// The Properties section.
pub fn section(app: &mut ModelApp, ui: &mut egui::Ui) {
    if app.mode == SelectMode::Object {
        return;
    }
    egui::CollapsingHeader::new(tl!("Soft Selection and Symmetry")).id_salt("soft-selection").default_open(false).show(ui, |ui| {
        let mut s = app.modeling.soft.clone();
        let r = ui.checkbox(&mut s.enabled, tl!("Soft selection"));
        set_name(ui, &r, tl!("Soft selection"));
        ui.horizontal(|ui| {
            drag(ui, &mut s.radius, 0.01, MIN_RADIUS..=MAX_RADIUS, tl!("Soft radius"));
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(tl!("Falloff"));
            for preset in CURVE_PRESETS {
                let name = match *preset {
                    "linear" => tl!("Linear"),
                    "smooth" => tl!("Smooth"),
                    "ease_in" => tl!("Ease in"),
                    "ease_out" => tl!("Ease out"),
                    _ => tl!("Steep"),
                };
                let r = ui.radio_value(&mut s.curve, *preset, name.clone());
                set_name(ui, &r, crate::tlf!("Falloff: {shape}", shape = name));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(tl!("Symmetry"));
            let r = ui.radio_value(&mut s.symmetry, None, tl!("Off"));
            set_name(ui, &r, tl!("Symmetry: Off"));
            for a in 0..3u8 {
                let r = ui.radio_value(&mut s.symmetry, Some(a), axis_name(a));
                set_name(ui, &r, crate::tlf!("Symmetry: {axis}", axis = axis_name(a)));
            }
        });
        ui.label(
            egui::RichText::new(tl!("Applies to moving, turning and scaling components. Vertices show their weight in colour."))
                .color(crate::theme::TEXT_DIM)
                .small(),
        );
        s.radius = if s.radius.is_finite() { s.radius.clamp(MIN_RADIUS, MAX_RADIUS) } else { 0.5 };
        app.modeling.soft = s;
    });
}
