//! Hair in the app: the Hair menu, the Properties section of a hair object, grooming with the pointer
//! and the guide curves drawn in the viewport.
//!
//! Every action is a [`HairCommand`]. The pointer draws a stroke of dabs; while it moves, the guides
//! on screen are a preview made by applying the dabs one at a time, and on release the whole stroke
//! is run as one command (one undo step) that gives the same guides. Every brush has a numeric path:
//! the Properties section applies the current brush to the whole groom (or to a point) by typing.

use egui::{Pos2, Rect, RichText};
use glam::Vec3;
use modelcraft_engine::Command;
use modelcraft_engine::hair::{HairCommand, MapEdit, MapKind};
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_hair::{Amount, Blend, Brush, BrushKind, Dab, DynamicsSettings, GrowSpec, Guides, InterpSpec, Stroke, Surface, Symmetry, apply_dabs};
use modelcraft_sim::SimTiming;

use crate::app::ModelApp;
use crate::panels::set_name;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HairAction {
    Add,
    Remove,
    Regrow,
    ToggleGroom,
    Brush(BrushKind),
    SymmetryX,
    SymmetryY,
    SymmetryZ,
    ApplyAll,
    ToggleStrands,
    ToggleGuides,
    Bake,
    ClearBake,
    ExportGlb,
    ExportCache,
    BrushBigger,
    BrushSmaller,
    MaskAll,
    MaskInvert,
    MaskClear,
}

impl HairAction {
    pub fn all() -> Vec<HairAction> {
        use HairAction::*;
        let mut v = vec![Add, Remove, Regrow, ToggleGroom];
        v.extend(BrushKind::ALL.map(Brush));
        v.extend([
            SymmetryX,
            SymmetryY,
            SymmetryZ,
            ApplyAll,
            ToggleStrands,
            ToggleGuides,
            Bake,
            ClearBake,
            ExportGlb,
            ExportCache,
            BrushBigger,
            BrushSmaller,
            MaskAll,
            MaskInvert,
            MaskClear,
        ]);
        v
    }

    pub fn label(self) -> String {
        match self {
            HairAction::Add => tl!("Add Hair"),
            HairAction::Remove => tl!("Remove Hair"),
            HairAction::Regrow => tl!("Regrow Guides"),
            HairAction::ToggleGroom => tl!("Groom with the Pointer"),
            HairAction::Brush(k) => crate::tlf!("{brush} Hair Brush", brush = brush_label(k)),
            HairAction::SymmetryX => tl!("Hair Symmetry X"),
            HairAction::SymmetryY => tl!("Hair Symmetry Y"),
            HairAction::SymmetryZ => tl!("Hair Symmetry Z"),
            HairAction::ApplyAll => tl!("Apply Hair Brush to All Guides"),
            HairAction::ToggleStrands => tl!("Show Hair Strands"),
            HairAction::ToggleGuides => tl!("Show Hair Guides"),
            HairAction::Bake => tl!("Bake Hair Dynamics"),
            HairAction::ClearBake => tl!("Clear Hair Bake"),
            HairAction::ExportGlb => tl!("Export Hair as glTF…"),
            HairAction::ExportCache => tl!("Export Hair Point Cache…"),
            HairAction::BrushBigger => tl!("Larger Hair Brush"),
            HairAction::BrushSmaller => tl!("Smaller Hair Brush"),
            HairAction::MaskAll => tl!("Hair Mask All"),
            HairAction::MaskInvert => tl!("Hair Mask Invert"),
            HairAction::MaskClear => tl!("Hair Mask Clear"),
        }
    }

    pub fn id(self) -> String {
        match self {
            HairAction::Add => "hair.add".into(),
            HairAction::Remove => "hair.remove".into(),
            HairAction::Regrow => "hair.regrow".into(),
            HairAction::ToggleGroom => "hair.groom".into(),
            HairAction::Brush(k) => format!("hair.brush.{}", k.id()),
            HairAction::SymmetryX => "hair.symmetry_x".into(),
            HairAction::SymmetryY => "hair.symmetry_y".into(),
            HairAction::SymmetryZ => "hair.symmetry_z".into(),
            HairAction::ApplyAll => "hair.apply_all".into(),
            HairAction::ToggleStrands => "hair.show_strands".into(),
            HairAction::ToggleGuides => "hair.show_guides".into(),
            HairAction::Bake => "hair.bake".into(),
            HairAction::ClearBake => "hair.clear_bake".into(),
            HairAction::ExportGlb => "hair.export_glb".into(),
            HairAction::ExportCache => "hair.export_cache".into(),
            HairAction::BrushBigger => "hair.brush_bigger".into(),
            HairAction::BrushSmaller => "hair.brush_smaller".into(),
            HairAction::MaskAll => "hair.mask_all".into(),
            HairAction::MaskInvert => "hair.mask_invert".into(),
            HairAction::MaskClear => "hair.mask_clear".into(),
        }
    }

    pub fn default_binding(self) -> Option<&'static str> {
        match self {
            HairAction::ToggleGroom => Some("Model: Alt+H"),
            HairAction::ApplyAll => Some("Model: Alt+Enter"),
            _ => None,
        }
    }
}

pub fn brush_label(k: BrushKind) -> String {
    match k {
        BrushKind::Comb => tl!("Comb"),
        BrushKind::Cut => tl!("Cut"),
        BrushKind::Length => tl!("Length"),
        BrushKind::Puff => tl!("Puff"),
        BrushKind::Smooth => tl!("Smooth"),
        BrushKind::Clump => tl!("Clump"),
        BrushKind::Frizz => tl!("Frizz"),
    }
}

/// A stroke being drawn.
pub struct StrokeState {
    view: usize,
    hair: ObjectId,
    dabs: Vec<Dab>,
    /// Where the pointer touched the surface last (emitter space).
    last: Option<Vec3>,
    /// Pointer travel since the last dab, for combing.
    travel: Vec3,
    /// Where the last dab went down.
    last_dab: Option<Vec3>,
}

/// Most strands drawn while editing when "limit" is on, whatever the percentage.
pub const EDIT_STRAND_LIMIT: usize = 100_000;

/// What the viewport shows of the hair.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplaySettings {
    /// Draw render hairs (on the GPU path).
    pub strands: bool,
    /// Draw the guide curves of selected hair (always when strands are off).
    pub guides: bool,
    /// Share of the strands drawn, 0.1 to 100.
    pub percent: f32,
    /// Never draw more than [`EDIT_STRAND_LIMIT`] strands, whatever the percentage.
    pub limit: bool,
}

impl Default for DisplaySettings {
    fn default() -> DisplaySettings {
        DisplaySettings { strands: true, guides: true, percent: 100.0, limit: true }
    }
}

/// How many of `total` strands the viewport draws.
pub fn strands_to_draw(total: usize, d: &DisplaySettings) -> usize {
    if total == 0 || !d.strands {
        return 0;
    }
    let share = if d.percent.is_finite() { f64::from(d.percent).clamp(0.1, 100.0) } else { 100.0 };
    // The small subtraction keeps 0.1 % of 1,000 at one strand, not two (an f32 0.1 is a little over).
    let mut n = (total as f64 * share / 100.0 - 1e-4).ceil() as usize;
    if d.limit {
        n = n.min(EDIT_STRAND_LIMIT);
    }
    n.clamp(1, total)
}

/// Grooming state of the window.
pub struct GroomUi {
    pub display: DisplaySettings,
    /// The pointer grooms (rather than selects) while a hair object is selected.
    pub active: bool,
    pub brush: Brush,
    pub symmetry: Symmetry,
    /// The comb movement used by the numeric "apply to all", in emitter units.
    pub direction: [f32; 3],
    pub stroke: Option<StrokeState>,
    /// The guides as the stroke so far would leave them.
    pub preview: Option<(ObjectId, Guides)>,
    /// Where the brush circle is drawn: screen position and radius in pixels.
    hover: Option<(Pos2, f32)>,
    /// Growth and render settings being edited for one hair object.
    draft: Option<(ObjectId, GrowSpec, InterpSpec)>,
    /// Dynamics settings being edited for one hair object.
    dyn_draft: Option<(ObjectId, DynamicsSettings, SimTiming)>,
}

impl Default for GroomUi {
    fn default() -> GroomUi {
        GroomUi {
            display: DisplaySettings::default(),
            active: false,
            brush: Brush::new(BrushKind::Comb),
            symmetry: Symmetry::default(),
            direction: [0.0, 0.0, 0.0],
            stroke: None,
            preview: None,
            hover: None,
            draft: None,
            dyn_draft: None,
        }
    }
}

fn is_hair(app: &ModelApp, id: ObjectId) -> bool {
    matches!(app.doc.scene.get(id).map(|o| &o.kind), Some(ObjectKind::Hair { .. }))
}

/// The hair the actions work on: the selected hair, else the first hair of the selected mesh.
pub fn target_hair(app: &ModelApp) -> Option<ObjectId> {
    let a = app.active()?;
    if is_hair(app, a) {
        return Some(a);
    }
    app.doc.scene.hair_of(a).first().copied()
}

fn emitter_of(app: &ModelApp, hair: ObjectId) -> Option<ObjectId> {
    app.doc.scene.get(hair).and_then(|o| o.parent)
}

pub fn act(app: &mut ModelApp, a: HairAction) {
    match a {
        HairAction::Add => {
            let Some(mesh) = app.active().filter(|id| matches!(app.doc.scene.get(*id).map(|o| &o.kind), Some(ObjectKind::Mesh { .. }))) else {
                app.status = tl!("Select the mesh to grow hair on first");
                return;
            };
            if app.run(Command::Hair(HairCommand::Add { mesh, grow: None, interp: None, name: None })) {
                app.status = tl!("Hair added; choose a brush in Properties and turn on Groom with the Pointer");
            }
        }
        HairAction::Remove => match target_hair(app) {
            Some(id) => {
                app.groom.stroke = None;
                app.groom.preview = None;
                app.run(Command::Hair(HairCommand::Remove { id }));
            }
            None => app.status = tl!("Select hair first"),
        },
        HairAction::Regrow => match target_hair(app) {
            Some(id) => {
                let grow = app.groom.draft.as_ref().filter(|d| d.0 == id).map(|d| d.1.clone());
                app.run(Command::Hair(HairCommand::Regrow { id, grow }));
            }
            None => app.status = tl!("Select hair first"),
        },
        HairAction::ToggleGroom => {
            if app.groom.active {
                app.groom.active = false;
                app.status = tl!("Grooming off");
            } else if target_hair(app).is_some() {
                app.groom.active = true;
                app.status = tl!("Grooming: drag over the surface; Shift smooths, Ctrl inverts the brush");
            } else {
                app.status = tl!("Select hair first");
            }
        }
        HairAction::Brush(k) => {
            app.groom.brush.kind = k;
            if target_hair(app).is_some() {
                app.groom.active = true;
            }
            app.status = crate::tlf!("{brush} brush", brush = brush_label(k));
        }
        HairAction::SymmetryX | HairAction::SymmetryY | HairAction::SymmetryZ => {
            let s = &mut app.groom.symmetry;
            let flag = match a {
                HairAction::SymmetryX => &mut s.x,
                HairAction::SymmetryY => &mut s.y,
                _ => &mut s.z,
            };
            *flag = !*flag;
            app.status = if *flag { crate::tlf!("{action} on", action = a.label()) } else { crate::tlf!("{action} off", action = a.label()) };
        }
        HairAction::ToggleStrands => {
            app.groom.display.strands = !app.groom.display.strands;
            app.status = if app.groom.display.strands { tl!("Hair strands shown") } else { tl!("Hair strands hidden") };
        }
        HairAction::ToggleGuides => {
            app.groom.display.guides = !app.groom.display.guides;
            app.status = if app.groom.display.guides { tl!("Hair guides shown") } else { tl!("Hair guides hidden") };
        }
        HairAction::Bake => match target_hair(app) {
            Some(id) => {
                if app.run(Command::Hair(HairCommand::Bake { id })) {
                    app.status = tl!("Hair baked: scrub the timeline to play it");
                }
            }
            None => app.status = tl!("Select hair first"),
        },
        HairAction::ClearBake => match target_hair(app) {
            Some(id) => {
                app.run(Command::Hair(HairCommand::ClearBake { id }));
            }
            None => app.status = tl!("Select hair first"),
        },
        HairAction::ExportGlb | HairAction::ExportCache => {
            let Some(id) = target_hair(app) else {
                app.status = tl!("Select hair first");
                return;
            };
            let glb = a == HairAction::ExportGlb;
            let name = app.doc.scene.get(id).map_or_else(|| "Hair".to_string(), |o| o.name.clone());
            let dialog = rfd::FileDialog::new().set_file_name(format!("{name}.{}", if glb { "glb" } else { "mcpc" }));
            let dialog = if glb { dialog.add_filter("glTF binary", &["glb"]) } else { dialog.add_filter("Point cache", &["mcpc"]) };
            if let Some(path) = dialog.save_file() {
                export_to(app, id, &path, glb);
            }
        }
        HairAction::BrushBigger => scale_radius(app, 1.25),
        HairAction::BrushSmaller => scale_radius(app, 0.8),
        HairAction::ApplyAll => match target_hair(app) {
            Some(id) => {
                let (brush, symmetry, direction) = (app.groom.brush, app.groom.symmetry, app.groom.direction);
                if brush.kind == BrushKind::Comb && direction == [0.0; 3] {
                    app.status = tl!("Type a comb direction first");
                    return;
                }
                app.run(Command::Hair(HairCommand::Apply { id, brush, symmetry, center: None, direction }));
            }
            None => app.status = tl!("Select hair first"),
        },
        HairAction::MaskAll => map_edit(app, MapEdit::Fill { value: 1.0 }),
        HairAction::MaskInvert => map_edit(app, MapEdit::Invert),
        HairAction::MaskClear => map_edit(app, MapEdit::Clear),
    }
}

/// Write a hair object's guides to `path` as glTF lines or as a point cache, in world space, and say
/// what was written in the status bar. (The dialogs call this; so do the tests.)
pub fn export_to(app: &mut ModelApp, id: ObjectId, path: &std::path::Path, glb: bool) {
    use modelcraft_engine::{HairExportFormat, HairExportWhat};
    let format = if glb { HairExportFormat::Glb } else { HairExportFormat::Mcpc };
    // glTF carries the render hairs (capped); the point cache carries the guides, frame by frame.
    let what = if glb { HairExportWhat::Strands } else { HairExportWhat::Guides };
    app.status = match app.doc.export_hair(id, path, format, what, 100_000, true) {
        Ok(msg) => msg,
        Err(e) => crate::tlf!("{action} failed: {error}", action = if glb { tl!("Export Hair as glTF…") } else { tl!("Export Hair Point Cache…") }, error = e),
    };
}

fn scale_radius(app: &mut ModelApp, k: f32) {
    app.groom.brush.radius = (app.groom.brush.radius * k).clamp(1e-4, 1.0e3);
    app.status = crate::tlf!("Hair brush radius {r}", r = format!("{:.3}", app.groom.brush.radius));
}

fn map_edit(app: &mut ModelApp, edit: MapEdit) {
    match target_hair(app) {
        Some(id) => {
            app.run(Command::Hair(HairCommand::Map { id, map: MapKind::Mask, edit }));
        }
        None => app.status = tl!("Select hair first"),
    }
}

pub fn menu(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.menu_button(tl!("Hair"), |ui| {
        for a in HairAction::all() {
            if matches!(a, HairAction::Brush(BrushKind::Comb) | HairAction::SymmetryX | HairAction::MaskAll) {
                ui.separator();
            }
            let action = crate::actions::Action::Hair(a);
            let b = egui::Button::new(action.label()).shortcut_text(action.shortcut_text(app));
            if ui.add(b).clicked() {
                act(app, a);
                ui.close();
            }
        }
    });
}

// ---------------------------------------------------------------- properties

fn num(ui: &mut egui::Ui, v: &mut f32, speed: f64, range: std::ops::RangeInclusive<f32>, name: String) -> egui::Response {
    ui.label(name.clone());
    let r = ui.add(egui::DragValue::new(v).speed(speed).range(range).max_decimals(3));
    set_name(ui, &r, name);
    r
}

fn committed(r: &egui::Response) -> bool {
    r.drag_stopped() || (r.changed() && !r.dragged())
}

/// The Properties section of a hair object (nothing for other objects).
pub fn properties(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let Some(ObjectKind::Hair { hair }) = app.doc.scene.get(id).map(|o| o.kind.clone()) else { return };
    if app.groom.draft.as_ref().is_none_or(|d| d.0 != id) {
        app.groom.draft = Some((id, hair.grow.clone(), hair.interp.clone()));
    }
    let ctx = ui.ctx().clone();
    egui::CollapsingHeader::new(tl!("Hair")).default_open(true).show(ui, |ui| {
        ui.label(crate::tlf!("{guides} guides, {strands} hairs", guides = hair.guides.len(), strands = amount_text(&hair.interp.amount)));
        if ui.button(HairAction::Remove.label()).clicked() {
            act(app, HairAction::Remove);
            return;
        }
        let mut on = app.groom.active;
        let tip = crate::actions::Action::Hair(HairAction::ToggleGroom).shortcut_text(app);
        if ui.checkbox(&mut on, tl!("Groom with the Pointer")).on_hover_text(tip).changed() {
            app.act(&ctx, crate::actions::Action::Hair(HairAction::ToggleGroom));
        }
        ui.add_space(4.0);
        ui.label(RichText::new(tl!("Brush")).strong());
        ui.horizontal_wrapped(|ui| {
            for k in BrushKind::ALL {
                let a = HairAction::Brush(k);
                let r = ui.add(egui::Button::selectable(app.groom.brush.kind == k, brush_label(k)));
                set_name(ui, &r, a.label());
                if r.clicked() {
                    act(app, a);
                }
            }
        });
        let mut b = app.groom.brush;
        egui::Grid::new("hair-brush").num_columns(2).show(ui, |ui| {
            num(ui, &mut b.radius, 0.005, 0.0001..=1000.0, tl!("Hair brush radius"));
            ui.end_row();
            num(ui, &mut b.strength, 0.01, 0.0..=1.0, tl!("Hair brush strength"));
            ui.end_row();
            num(ui, &mut b.falloff, 0.01, 0.0..=1.0, tl!("Hair brush falloff"));
            ui.end_row();
            match b.kind {
                BrushKind::Cut => {
                    num(ui, &mut b.cut_length, 0.005, 0.0..=1000.0, tl!("Cut length"));
                    ui.end_row();
                }
                BrushKind::Frizz => {
                    num(ui, &mut b.frequency, 0.1, 0.0..=100.0, tl!("Frizz frequency"));
                    ui.end_row();
                }
                _ => {}
            }
        });
        let r =
            ui.checkbox(&mut b.invert, tl!("Invert Hair Brush")).on_hover_text(tl!("Shorter, flatter or spread apart instead of longer, lifted or gathered"));
        set_name(ui, &r, tl!("Invert Hair Brush"));
        app.groom.brush = b;
        ui.horizontal(|ui| {
            for (a, flag) in
                [(HairAction::SymmetryX, app.groom.symmetry.x), (HairAction::SymmetryY, app.groom.symmetry.y), (HairAction::SymmetryZ, app.groom.symmetry.z)]
            {
                let mut f = flag;
                let r = ui.checkbox(&mut f, a.label().replace("Hair Symmetry ", ""));
                set_name(ui, &r, a.label());
                if f != flag {
                    act(app, a);
                }
            }
        });
        ui.label(RichText::new(tl!("Type it instead of dragging")).color(theme::TEXT_DIM).small());
        if app.groom.brush.kind == BrushKind::Comb {
            ui.horizontal(|ui| {
                for (axis, name) in [(0, tl!("Comb direction X")), (1, tl!("Comb direction Y")), (2, tl!("Comb direction Z"))] {
                    let r = ui.add(egui::DragValue::new(&mut app.groom.direction[axis]).speed(0.005).max_decimals(3).prefix(["X ", "Y ", "Z "][axis]));
                    set_name(ui, &r, name);
                }
            });
        }
        let apply = HairAction::ApplyAll;
        let r = ui.button(apply.label()).on_hover_text(crate::actions::Action::Hair(apply).shortcut_text(app));
        if r.clicked() {
            act(app, apply);
        }
        ui.add_space(6.0);
        ui.label(RichText::new(tl!("Mask")).strong());
        let masked = if hair.mask.is_some() { tl!("Mask set: brushes act in proportion to it") } else { tl!("No mask: brushes act everywhere") };
        ui.label(RichText::new(masked).color(theme::TEXT_DIM).small());
        ui.horizontal_wrapped(|ui| {
            for a in [HairAction::MaskAll, HairAction::MaskInvert, HairAction::MaskClear] {
                if ui.button(a.label()).clicked() {
                    act(app, a);
                }
            }
        });
        guides_and_render(app, ui, id);
        dynamics(app, ui, id);
    });
}

/// Dynamics: how the guides move when baked, the frames it covers, the bake and the exports.
fn dynamics(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let Some(ObjectKind::Hair { hair }) = app.doc.scene.get(id).map(|o| o.kind.clone()) else { return };
    if app.groom.dyn_draft.as_ref().is_none_or(|d| d.0 != id) {
        app.groom.dyn_draft = Some((id, hair.dynamics, hair.timing));
    }
    let Some((_, mut dynamics, mut timing)) = app.groom.dyn_draft else { return };
    let mut changed = false;
    ui.add_space(6.0);
    ui.label(RichText::new(tl!("Dynamics")).strong());
    egui::Grid::new("hair-dynamics").num_columns(2).show(ui, |ui| {
        for (axis, name) in [(0, tl!("Gravity X")), (1, tl!("Gravity Y")), (2, tl!("Gravity Z"))] {
            let r = num(ui, &mut dynamics.gravity[axis], 0.1, -1.0e4..=1.0e4, name);
            changed |= committed(&r);
            ui.end_row();
        }
        for (axis, name) in [(0, tl!("Wind X")), (1, tl!("Wind Y")), (2, tl!("Wind Z"))] {
            let r = num(ui, &mut dynamics.wind[axis], 0.1, -1.0e4..=1.0e4, name);
            changed |= committed(&r);
            ui.end_row();
        }
        let r = num(ui, &mut dynamics.damping, 0.01, 0.0..=1.0, tl!("Hair damping"));
        changed |= committed(&r);
        ui.end_row();
        let r = num(ui, &mut dynamics.stiffness, 0.01, 0.0..=1.0, tl!("Hair stiffness"));
        changed |= committed(&r);
        ui.end_row();
        let r = num(ui, &mut dynamics.clearance, 0.001, 0.0..=1000.0, tl!("Collision clearance"));
        changed |= committed(&r);
        ui.end_row();
        let (mut start, mut end) = (timing.start_frame as f32, timing.end_frame as f32);
        let r = num(ui, &mut start, 1.0, -100_000.0..=100_000.0, tl!("Bake start frame"));
        if r.changed() {
            timing.start_frame = f64::from(start);
        }
        changed |= committed(&r);
        ui.end_row();
        let r = num(ui, &mut end, 1.0, -100_000.0..=100_000.0, tl!("Bake end frame"));
        if r.changed() {
            timing.end_frame = f64::from(end);
        }
        changed |= committed(&r);
        ui.end_row();
    });
    let r = ui.checkbox(&mut dynamics.collide, tl!("Collide with the emitter"));
    set_name(ui, &r, tl!("Collide with the emitter"));
    changed |= r.changed();
    app.groom.dyn_draft = Some((id, dynamics, timing));
    if changed {
        app.run(Command::Hair(HairCommand::SetDynamics { id, dynamics: Some(dynamics), timing: Some(timing), seed: None }));
    }
    let baked = match &hair.baked {
        Some(c) => crate::tlf!("Baked: {frames} frames of {points} points", frames = c.frames(), points = c.points()),
        None => tl!("Not baked: the hair shows its groomed shape"),
    };
    ui.label(RichText::new(baked).color(theme::TEXT_DIM).small());
    ui.horizontal_wrapped(|ui| {
        for a in [HairAction::Bake, HairAction::ClearBake, HairAction::ExportGlb, HairAction::ExportCache] {
            if ui.button(a.label()).clicked() {
                act(app, a);
            }
        }
    });
}

fn amount_text(a: &Amount) -> String {
    match a {
        Amount::Count(n) => n.to_string(),
        Amount::PerUnitArea(d) => crate::tlf!("{density} per unit area", density = d),
    }
}

/// Growth (applied by Regrow Guides) and render settings (applied as you edit them).
fn guides_and_render(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId) {
    let Some((_, mut grow, mut interp)) = app.groom.draft.clone() else { return };
    let mut render_changed = false;
    ui.add_space(6.0);
    ui.label(RichText::new(tl!("Display")).strong());
    let mut d = app.groom.display;
    for (on, action) in [(&mut d.strands, HairAction::ToggleStrands), (&mut d.guides, HairAction::ToggleGuides)] {
        let r = ui.checkbox(on, action.label());
        set_name(ui, &r, action.label());
    }
    let r = ui.checkbox(&mut d.limit, crate::tlf!("Limit to {count} strands while editing", count = EDIT_STRAND_LIMIT));
    set_name(ui, &r, tl!("Limit strands while editing"));
    ui.horizontal(|ui| {
        num(ui, &mut d.percent, 0.5, 0.1..=100.0, tl!("Hair display percent"));
    });
    app.groom.display = d;
    if let Some(ObjectKind::Hair { hair }) = app.doc.scene.get(id).map(|o| &o.kind)
        && let Some(emitter) = emitter_of(app, id)
    {
        let hair = hair.clone();
        let mesh = app.doc.eval.evaluate(&app.doc.scene, emitter).mesh;
        let total = hair.strand_count(&mesh);
        let shown = strands_to_draw(total, &app.groom.display);
        ui.label(RichText::new(crate::tlf!("Showing {shown} of {total} strands", shown = shown, total = total)).color(theme::TEXT_DIM).small());
    }
    ui.add_space(6.0);
    ui.label(RichText::new(tl!("Guides")).strong());
    egui::Grid::new("hair-grow").num_columns(2).show(ui, |ui| {
        let mut n = match grow.amount {
            Amount::Count(n) => n as f32,
            Amount::PerUnitArea(_) => 0.0,
        };
        ui.label(tl!("Guide count"));
        let r = ui.add(egui::DragValue::new(&mut n).speed(10.0).range(1.0..=200_000.0).max_decimals(0));
        set_name(ui, &r, tl!("Guide count"));
        if r.changed() {
            grow.amount = Amount::Count(n as usize);
        }
        ui.end_row();
        num(ui, &mut grow.length, 0.005, 0.0..=10_000.0, tl!("Guide length"));
        ui.end_row();
        num(ui, &mut grow.length_jitter, 0.01, 0.0..=1.0, tl!("Length variation"));
        ui.end_row();
    });
    ui.horizontal(|ui| {
        let r = ui.button(HairAction::Regrow.label()).on_hover_text(tl!("Grow the guides again; grooming is lost"));
        if r.clicked() {
            app.groom.draft = Some((id, grow.clone(), interp.clone()));
            act(app, HairAction::Regrow);
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new(tl!("Hairs")).strong());
    egui::Grid::new("hair-render").num_columns(2).show(ui, |ui| {
        let mut n = match interp.amount {
            Amount::Count(n) => n as f32,
            Amount::PerUnitArea(_) => 0.0,
        };
        ui.label(tl!("Hair count"));
        let r = ui.add(egui::DragValue::new(&mut n).speed(100.0).range(0.0..=5_000_000.0).max_decimals(0));
        set_name(ui, &r, tl!("Hair count"));
        if r.changed() {
            interp.amount = Amount::Count(n as usize);
        }
        render_changed |= committed(&r);
        ui.end_row();
        let r = num(ui, &mut interp.clump.amount, 0.01, 0.0..=1.0, tl!("Clump"));
        render_changed |= committed(&r);
        ui.end_row();
        let r = num(ui, &mut interp.frizz.amount, 0.005, 0.0..=10.0, tl!("Frizz"));
        render_changed |= committed(&r);
        ui.end_row();
        let r = num(ui, &mut interp.width.root, 0.0001, 0.0..=1000.0, tl!("Root width"));
        render_changed |= committed(&r);
        ui.end_row();
        let r = num(ui, &mut interp.width.tip, 0.0001, 0.0..=1000.0, tl!("Tip width"));
        render_changed |= committed(&r);
        ui.end_row();
    });
    ui.horizontal(|ui| {
        let bary = matches!(interp.blend, Blend::Barycentric);
        for (label, on, blend) in [(tl!("Barycentric blend"), bary, Blend::Barycentric), (tl!("Nearest guides blend"), !bary, Blend::Nearest { k: 4 })] {
            let r = ui.add(egui::Button::selectable(on, label.clone()));
            set_name(ui, &r, label);
            if r.clicked() && !on {
                interp.blend = blend;
                render_changed = true;
            }
        }
    });
    app.groom.draft = Some((id, grow, interp.clone()));
    if render_changed {
        app.run(Command::Hair(HairCommand::SetRender { id, interp }));
    }
}

// ---------------------------------------------------------------- viewport

/// World-space line segments for the guides of every visible hair object; a stroke in progress shows
/// its preview. Capped so a huge groom cannot stall the frame.
pub fn guide_lines(app: &ModelApp) -> Vec<([f32; 3], [f32; 3], [f32; 4])> {
    const MAX_SEGMENTS: usize = 400_000;
    let scene = &app.doc.scene;
    let mut out = Vec::new();
    for o in scene.objects().iter().filter(|o| scene.visible(o.id)) {
        let ObjectKind::Hair { hair } = &o.kind else { continue };
        let Some(emitter) = o.parent else { continue };
        let world = scene.world_matrix(emitter);
        let at = hair.at_frame(scene.anim.frame);
        let guides = app.groom.preview.as_ref().filter(|(id, _)| *id == o.id).map_or(&at.guides, |(_, g)| g);
        let selected = app.selection.contains(&o.id) || app.selection.contains(&emitter);
        let editing = app.groom.stroke.as_ref().is_some_and(|s| s.hair == o.id);
        if !editing && !(app.groom.display.guides && (selected || !app.groom.display.strands)) {
            continue;
        }
        let color = if selected { [0.95, 0.78, 0.42, 1.0] } else { [0.62, 0.55, 0.42, 0.9] };
        for i in 0..guides.len() {
            for w in guides.strand(i).windows(2) {
                if out.len() >= MAX_SEGMENTS {
                    return out;
                }
                out.push((world.transform_point3(w[0]).to_array(), world.transform_point3(w[1]).to_array(), color));
            }
        }
    }
    out
}

/// The hair to draw this frame on the GPU path: uploads (or reuses) each hair's strands, a prefix of
/// the groom sized by the display settings, and returns what to draw. A hair being groomed shows its
/// guides only until the stroke ends.
pub fn hair_specs(doc: &mut modelcraft_engine::Document, groom: &GroomUi, selection: &[ObjectId], g: &mut crate::gpu::GpuState) -> Vec<crate::gpu::HairSpec> {
    let _ = selection;
    let hairs: Vec<(ObjectId, ObjectId, u64, std::sync::Arc<modelcraft_engine::scene::hair::HairData>, [f32; 4])> = doc
        .scene
        .objects()
        .iter()
        .filter(|o| doc.scene.visible(o.id))
        .filter_map(|o| match (&o.kind, o.parent) {
            (ObjectKind::Hair { hair }, Some(emitter)) => Some((o.id, emitter, o.rev, hair.clone(), o.material.base_color)),
            _ => None,
        })
        .take(64)
        .collect();
    let mut out = Vec::new();
    for (id, emitter, rev, hair, color) in hairs {
        if groom.stroke.as_ref().is_some_and(|s| s.hair == id) {
            continue;
        }
        // A baked hair is drawn as it is at the current frame (the strands follow the cache).
        let frame = doc.scene.anim.frame;
        let hair_at = hair.at_frame(frame);
        let hair = &*hair_at;
        let mesh = doc.eval.evaluate(&doc.scene, emitter).mesh;
        let wanted = strands_to_draw(hair.strand_count(&mesh), &groom.display);
        if wanted == 0 {
            continue;
        }
        let emitter_rev = doc.scene.get(emitter).map_or(0, |e| e.rev);
        let frame_key = if hair.baked.is_some() { frame.to_bits() } else { 0 };
        let key_rev = rev.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ emitter_rev ^ frame_key.rotate_left(21);
        let build = |n: usize| hair.strands_with_amount(&mesh, Amount::Count(n)).ok();
        if let Some(fraction) = g.hair(id.0, key_rev, wanted, build) {
            out.push(crate::gpu::HairSpec { key: id.0, model: doc.scene.world_matrix(emitter), color: [color[0], color[1], color[2], 0.9], fraction });
        }
    }
    out
}

/// Pointer grooming on the viewport. Returns whether it took the pointer (so the view does not also
/// select or sculpt).
pub fn input(app: &mut ModelApp, ui: &egui::Ui, resp: &egui::Response, i: usize, rect: Rect) -> bool {
    app.groom.hover = None;
    if !app.groom.active {
        return false;
    }
    let Some(hair) = target_hair(app) else { return false };
    let Some(emitter) = emitter_of(app, hair) else { return false };
    let cam = app.views[i].camera;
    let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
    let (shift, ctrl) = ui.input(|inp| (inp.modifiers.shift, inp.modifiers.command));
    let world = app.doc.scene.world_matrix(emitter);
    let inv = world.inverse();
    let hit = |app: &mut ModelApp, at: Pos2| -> Option<(Vec3, Vec3)> {
        let mesh = app.doc.eval.evaluate(&app.doc.scene, emitter).mesh;
        let ray = crate::viewport::world_ray(&cam, rect, at).transformed(inv);
        let h = modelcraft_engine::scene::pick::pick_face(&mesh, &ray)?;
        Some((h.point, mesh.face_normal(h.face)))
    };
    if let Some(at) = pointer.filter(|_| resp.hovered() || app.groom.stroke.is_some())
        && let Some((p, _)) = hit(app, at)
    {
        let scale = world.to_scale_rotation_translation().0.abs().max_element().max(1e-6);
        let px = app.groom.brush.radius * scale / cam.world_per_pixel(world.transform_point3(p), rect.height()).max(1e-9);
        app.groom.hover = Some((at, px.clamp(3.0, 400.0)));
    }
    let mut brush = app.groom.brush;
    if shift {
        brush.kind = BrushKind::Smooth;
    }
    if ctrl {
        brush.invert = !brush.invert;
    }
    if resp.drag_started_by(egui::PointerButton::Primary)
        && !crate::viewport::is_nav(app, ui)
        && let Some(at) = pointer
        && hit(app, at - resp.drag_delta()).is_some()
    {
        let guides = match app.doc.scene.get(hair).map(|o| &o.kind) {
            Some(ObjectKind::Hair { hair }) => hair.guides.clone(),
            _ => return true,
        };
        app.groom.preview = Some((hair, guides));
        app.groom.stroke = Some(StrokeState { view: i, hair, dabs: Vec::new(), last: None, travel: Vec3::ZERO, last_dab: None });
    }
    let active = app.groom.stroke.as_ref().is_some_and(|s| s.view == i);
    if !active {
        return false;
    }
    if resp.dragged_by(egui::PointerButton::Primary)
        && let Some(at) = pointer
        && let Some((p, normal)) = hit(app, at)
    {
        let spacing = (brush.radius * 0.25).max(1e-5);
        let mut new_dab = None;
        if let Some(st) = &mut app.groom.stroke {
            if let Some(last) = st.last {
                st.travel += p - last;
            }
            st.last = Some(p);
            if st.last_dab.is_none_or(|d| d.distance(p) >= spacing) {
                new_dab = Some(Dab { center: p, normal, delta: st.travel, pressure: 1.0 });
                st.last_dab = Some(p);
                st.travel = Vec3::ZERO;
            }
        }
        if let Some(d) = new_dab {
            preview_dab(app, hair, emitter, &brush, d);
            if let Some(st) = &mut app.groom.stroke {
                st.dabs.push(d);
            }
        }
    }
    if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
        finish_stroke(app, brush);
    }
    true
}

fn preview_dab(app: &mut ModelApp, hair: ObjectId, emitter: ObjectId, brush: &Brush, dab: Dab) {
    let first = app.groom.stroke.as_ref().map_or(0, |s| s.dabs.len());
    let mesh = app.doc.eval.evaluate(&app.doc.scene, emitter).mesh;
    let Ok(surface) = Surface::from_mesh(&mesh) else { return };
    let mask = match app.doc.scene.get(hair).map(|o| &o.kind) {
        Some(ObjectKind::Hair { hair }) => hair.mask.clone(),
        _ => None,
    };
    let sym = app.groom.symmetry;
    if let Some((_, guides)) = &mut app.groom.preview {
        let _ = apply_dabs(guides, &surface, mask.as_deref(), brush, sym, &[dab], first);
    }
}

fn finish_stroke(app: &mut ModelApp, brush: Brush) {
    let Some(st) = app.groom.stroke.take() else { return };
    app.groom.preview = None;
    if st.dabs.is_empty() {
        return;
    }
    let stroke = Stroke { brush, symmetry: app.groom.symmetry, dabs: st.dabs };
    app.run(Command::Hair(HairCommand::Stroke { id: st.hair, stroke }));
}

/// The brush circle under the pointer.
pub fn overlay(app: &ModelApp, ui: &egui::Ui, rect: Rect) {
    if let Some((at, px)) = app.groom.hover {
        ui.painter_at(rect).circle_stroke(at, px, egui::Stroke::new(1.5, theme::ACCENT));
    }
}
