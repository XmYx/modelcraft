//! The material manager and node editor.
//!
//! The window has a **material strip** (every surface of the scene, with its colour; click one to
//! work on it, or copy the active one to the selection), and below it the **node editor** for the
//! active object's material graph ([`modelcraft_matgraph`]): a breadcrumb, a node picker with
//! search, and one card per node with a thumbnail of what it makes, its settings, and for each
//! input a list to choose which node feeds it. Linking is by those lists, so it needs no dragging
//! and every part is in the accessibility tree.
//!
//! Every edit is a `SetMaterial` command carrying the new graph, so it is undoable and saved with
//! the project; the render engine reads the graph as maps ([`modelcraft_render::trace`]).

use std::sync::Arc;

use egui::{Key, RichText};
use modelcraft_engine::Command;
use modelcraft_engine::scene::matgraph::{Blend, Graph, GraphError, MathOp, NodeId, NodeKind};
use modelcraft_engine::scene::{Material, ObjectId, ObjectKind};
use modelcraft_render::trace::FileImages;

use crate::app::ModelApp;
use crate::panels::set_name;
use crate::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatAction {
    /// The Material Nodes window.
    Window,
    /// Give the selected object a material graph that starts from its material.
    AddGraph,
    /// Take the graph off the selected object (its flat material stays).
    RemoveGraph,
    /// Copy the active object's material, graph included, onto the other selected objects.
    CopyToSelection,
}

impl MatAction {
    pub const ALL: [MatAction; 4] = [MatAction::Window, MatAction::AddGraph, MatAction::RemoveGraph, MatAction::CopyToSelection];

    pub fn label(self) -> String {
        match self {
            MatAction::Window => tl!("Material Nodes…"),
            MatAction::AddGraph => tl!("Add Material Graph"),
            MatAction::RemoveGraph => tl!("Remove Material Graph"),
            MatAction::CopyToSelection => tl!("Copy Material to Selection"),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            MatAction::Window => "render.material_nodes",
            MatAction::AddGraph => "render.add_material_graph",
            MatAction::RemoveGraph => "render.remove_material_graph",
            MatAction::CopyToSelection => "render.copy_material",
        }
    }
}

pub fn act(app: &mut ModelApp, a: MatAction) {
    match a {
        MatAction::Window => {
            app.materials.open = !app.materials.open;
            app.materials.focus_pending = app.materials.open;
        }
        MatAction::AddGraph => add_graph(app),
        MatAction::RemoveGraph => match app.active() {
            Some(id) => set_graph(app, id, None),
            None => app.status = tl!("Select an object first"),
        },
        MatAction::CopyToSelection => copy_to_selection(app),
    }
}

/// The window's state.
#[derive(Default)]
pub struct MaterialUi {
    pub open: bool,
    pub focus_pending: bool,
    had_focus: bool,
    /// The node whose card is shown selected (and named last in the breadcrumb).
    pub selected: Option<NodeId>,
    /// Whether the node picker is open, and what it is searching for.
    pub picker: bool,
    pub query: String,
    thumbs: Option<Thumbs>,
}

/// The thumbnails of one graph, kept while the graph does not change.
struct Thumbs {
    object: ObjectId,
    graph: Graph,
    images: Vec<(NodeId, egui::TextureHandle)>,
}

const THUMB: u32 = 40;

fn mesh_objects(app: &ModelApp) -> Vec<ObjectId> {
    app.doc.scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. })).map(|o| o.id).collect()
}

fn set_graph(app: &mut ModelApp, id: ObjectId, graph: Option<Graph>) {
    let Some(o) = app.doc.scene.get(id) else { return };
    let material = Material { graph: graph.map(Arc::new), ..o.material.clone() };
    app.run(Command::SetMaterial { id, material });
}

fn add_graph(app: &mut ModelApp) {
    let Some(id) = app.active().filter(|id| app.doc.scene.get(*id).is_some_and(|o| matches!(o.kind, ObjectKind::Mesh { .. } | ObjectKind::SmoothCage { .. })))
    else {
        app.status = tl!("Select a mesh first");
        return;
    };
    let Some(o) = app.doc.scene.get(id) else { return };
    if o.material.graph.is_some() {
        app.status = tl!("That material already has a graph");
        return;
    }
    let m = &o.material;
    let mut g = Graph::new();
    let kind = NodeKind::Surface {
        base_color: [m.base_color[0], m.base_color[1], m.base_color[2]],
        metallic: m.metallic,
        roughness: m.roughness,
        emission: m.emission.map(|c| c * m.emission_strength),
    };
    if g.add(kind).is_ok() {
        set_graph(app, id, Some(g));
    }
}

fn copy_to_selection(app: &mut ModelApp) {
    let Some(from) = app.active() else {
        app.status = tl!("Select an object first");
        return;
    };
    let Some(material) = app.doc.scene.get(from).map(|o| o.material.clone()) else { return };
    let targets: Vec<ObjectId> = app.selection.iter().copied().filter(|t| *t != from).collect();
    if targets.is_empty() {
        app.status = tl!("Select the objects to copy onto, with the one to copy from last");
        return;
    }
    let commands: Vec<Command> = targets.into_iter().map(|id| Command::SetMaterial { id, material: material.clone() }).collect();
    app.run(Command::Batch { label: Some("Copy material".into()), commands });
}

/// The link choices for one input: every other node whose output would not loop back.
fn sources(g: &Graph, node: NodeId, port: usize) -> Vec<(NodeId, String)> {
    g.nodes
        .iter()
        .filter(|n| !matches!(n.kind, NodeKind::Surface { .. }) && g.can_link(node, port, n.id))
        .map(|n| (n.id, format!("{} {}", n.name, n.id)))
        .collect()
}

fn kind_title(k: &NodeKind) -> String {
    tl!(k.title())
}

fn input_title(name: &str) -> String {
    match name {
        "fac" => tl!("Factor"),
        "a" => tl!("First"),
        "b" => tl!("Second"),
        "colour" => tl!("Colour"),
        "base_color" => tl!("Base colour"),
        "metallic" => tl!("Metallic"),
        "roughness" => tl!("Roughness"),
        "emission" => tl!("Emission"),
        "normal" => tl!("Normal"),
        other => other.to_string(),
    }
}

fn blend_title(b: Blend) -> String {
    match b {
        Blend::Mix => tl!("Mix"),
        Blend::Add => tl!("Add"),
        Blend::Multiply => tl!("Multiply"),
        Blend::Screen => tl!("Screen"),
        Blend::Difference => tl!("Difference"),
    }
}

fn op_title(o: MathOp) -> String {
    match o {
        MathOp::Add => tl!("Add"),
        MathOp::Subtract => tl!("Subtract"),
        MathOp::Multiply => tl!("Multiply"),
        MathOp::Divide => tl!("Divide"),
        MathOp::Power => tl!("Power"),
        MathOp::Minimum => tl!("Smallest"),
        MathOp::Maximum => tl!("Largest"),
        MathOp::Absolute => tl!("Absolute value"),
        MathOp::Invert => tl!("Invert"),
    }
}

/// A drag value with an accessible name; true when it changed.
fn number(ui: &mut egui::Ui, v: &mut f32, name: String, range: std::ops::RangeInclusive<f32>, speed: f32, drags: &mut Vec<egui::Response>) {
    ui.horizontal(|ui| {
        ui.label(name.clone());
        let r = ui.add(egui::DragValue::new(v).speed(speed).range(range).max_decimals(3));
        set_name(ui, &r, name);
        drags.push(r);
    });
}

fn colour(ui: &mut egui::Ui, c: &mut [f32; 3], name: String, drags: &mut Vec<egui::Response>) {
    ui.horizontal(|ui| {
        ui.label(name.clone());
        let r = ui.color_edit_button_rgb(c);
        set_name(ui, &r, name);
        drags.push(r);
    });
}

/// The settings of one node. Returns the node's changed kind when the user changed it.
fn settings(ui: &mut egui::Ui, kind: &NodeKind, drags: &mut Vec<egui::Response>) -> Option<NodeKind> {
    let mut k = kind.clone();
    match &mut k {
        NodeKind::Value { value } => number(ui, value, tl!("Number"), -1e6..=1e6, 0.01, drags),
        NodeKind::Colour { color } => colour(ui, color, tl!("Colour"), drags),
        NodeKind::Noise { scale, detail, roughness, seed } => {
            number(ui, scale, tl!("Scale"), 1.0..=4096.0, 0.1, drags);
            let mut d = *detail as f32;
            number(ui, &mut d, tl!("Detail"), 1.0..=12.0, 0.05, drags);
            *detail = d.round() as u32;
            number(ui, roughness, tl!("Roughness"), 0.0..=1.0, 0.01, drags);
            let mut s = *seed as f32;
            number(ui, &mut s, tl!("Seed"), 0.0..=1e6, 1.0, drags);
            *seed = s.round() as u32;
        }
        NodeKind::Ramp { stops } => {
            for (i, (p, c)) in stops.iter_mut().enumerate() {
                number(ui, p, crate::tlf!("Stop {n} position", n = i + 1), 0.0..=1.0, 0.01, drags);
                colour(ui, c, crate::tlf!("Stop {n} colour", n = i + 1), drags);
            }
        }
        NodeKind::Mix { blend } => {
            let before = *blend;
            let r = egui::ComboBox::from_id_salt("mat-blend").selected_text(blend_title(*blend)).show_ui(ui, |ui| {
                for b in [Blend::Mix, Blend::Add, Blend::Multiply, Blend::Screen, Blend::Difference] {
                    ui.selectable_value(blend, b, blend_title(b));
                }
            });
            set_name(ui, &r.response, tl!("Blend"));
            if *blend != before {
                r.response.request_focus();
            }
        }
        NodeKind::Math { op } => {
            let before = *op;
            let r = egui::ComboBox::from_id_salt("mat-op").selected_text(op_title(*op)).show_ui(ui, |ui| {
                for o in [
                    MathOp::Add,
                    MathOp::Subtract,
                    MathOp::Multiply,
                    MathOp::Divide,
                    MathOp::Power,
                    MathOp::Minimum,
                    MathOp::Maximum,
                    MathOp::Absolute,
                    MathOp::Invert,
                ] {
                    ui.selectable_value(op, o, op_title(o));
                }
            });
            set_name(ui, &r.response, tl!("Operation"));
            if *op != before {
                r.response.request_focus();
            }
        }
        NodeKind::Image { path } => {
            ui.horizontal(|ui| {
                ui.label(tl!("Picture"));
                let r = ui.add(egui::TextEdit::singleline(path).desired_width(150.0));
                set_name(ui, &r, tl!("Picture file"));
                if ui.button(tl!("Choose…")).clicked()
                    && let Some(p) = rfd::FileDialog::new().add_filter(tl!("Pictures"), &["png", "jpg", "jpeg", "tga", "bmp"]).pick_file()
                {
                    *path = p.display().to_string();
                }
            });
        }
        NodeKind::NormalMap { strength } => number(ui, strength, tl!("Strength"), 0.0..=64.0, 0.05, drags),
        NodeKind::Surface { base_color, metallic, roughness, emission } => {
            colour(ui, base_color, tl!("Base colour"), drags);
            number(ui, metallic, tl!("Metallic"), 0.0..=1.0, 0.01, drags);
            number(ui, roughness, tl!("Roughness"), 0.0..=1.0, 0.01, drags);
            colour(ui, emission, tl!("Emission"), drags);
        }
    }
    (k != *kind).then_some(k)
}

fn thumbnails(app: &mut ModelApp, ctx: &egui::Context, id: ObjectId, graph: &Graph) {
    let fresh = app.materials.thumbs.as_ref().is_some_and(|t| t.object == id && t.graph == *graph);
    if fresh {
        return;
    }
    let images = FileImages::default();
    let list = graph
        .nodes
        .iter()
        .map(|n| {
            let rgb = graph.thumbnail(n.id, THUMB, &images);
            let px: Vec<u8> = rgb
                .iter()
                .flat_map(|p| {
                    let e = |v: f32| (modelcraft_render::pathtrace::render::linear_to_srgb(v) * 255.0 + 0.5) as u8;
                    [e(p[0]), e(p[1]), e(p[2]), 255]
                })
                .collect();
            let img = egui::ColorImage::from_rgba_unmultiplied([THUMB as usize, THUMB as usize], &px);
            (n.id, ctx.load_texture(format!("mat-thumb-{}-{}", id.0, n.id), img, egui::TextureOptions::LINEAR))
        })
        .collect();
    app.materials.thumbs = Some(Thumbs { object: id, graph: graph.clone(), images: list });
}

pub fn escape(app: &mut ModelApp, ctx: &egui::Context) -> bool {
    let m = &mut app.materials;
    if m.open && m.had_focus && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
        if m.picker {
            m.picker = false;
        } else {
            m.open = false;
            m.had_focus = false;
        }
        return true;
    }
    false
}

pub fn show(app: &mut ModelApp, ctx: &egui::Context) {
    if !app.materials.open {
        return;
    }
    let mut open = true;
    egui::Window::new(tl!("Material Nodes")).id(egui::Id::unique("material-nodes")).open(&mut open).collapsible(false).default_size([720.0, 520.0]).show(
        ctx,
        |ui| {
            strip(app, ui);
            ui.separator();
            editor(app, ui, ctx);
            let layer = ui.layer_id();
            app.materials.had_focus = ui.memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id)).is_some_and(|r| r.layer_id == layer);
        },
    );
    if !open {
        app.materials.open = false;
    }
}

/// The material manager strip.
fn strip(app: &mut ModelApp, ui: &mut egui::Ui) {
    ui.label(RichText::new(tl!("Materials")).strong());
    egui::ScrollArea::horizontal().id_salt("material-strip").show(ui, |ui| {
        ui.horizontal(|ui| {
            for id in mesh_objects(app) {
                let Some(o) = app.doc.scene.get(id) else { continue };
                let c = o.material.base_color;
                let swatch = egui::Color32::from_rgb(
                    (modelcraft_render::pathtrace::render::linear_to_srgb(c[0]) * 255.0) as u8,
                    (modelcraft_render::pathtrace::render::linear_to_srgb(c[1]) * 255.0) as u8,
                    (modelcraft_render::pathtrace::render::linear_to_srgb(c[2]) * 255.0) as u8,
                );
                let graph = o.material.graph.is_some();
                let name = o.name.clone();
                let selected = app.active() == Some(id);
                ui.vertical(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 14.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, swatch);
                    let label = if graph { crate::tlf!("{name} (graph)", name = name) } else { name.clone() };
                    let r = ui.selectable_label(selected, label.clone());
                    set_name(ui, &r, crate::tlf!("Material of {name}", name = label));
                    if r.clicked() {
                        app.select_only(id);
                    }
                });
            }
        });
    });
    ui.horizontal(|ui| {
        let r = ui.button(tl!("Copy to Selection"));
        if r.clicked() {
            act(app, MatAction::CopyToSelection);
        }
    });
}

fn breadcrumb(app: &mut ModelApp, ui: &mut egui::Ui, object: &str, graph: &Graph) {
    ui.horizontal(|ui| {
        let r = ui.link(tl!("Scene"));
        set_name(ui, &r, tl!("Breadcrumb: Scene"));
        ui.label("›");
        let r = ui.link(object.to_string());
        set_name(ui, &r, crate::tlf!("Breadcrumb: {name}", name = object));
        ui.label("›");
        let r = ui.link(tl!("Material graph"));
        set_name(ui, &r, tl!("Breadcrumb: Material graph"));
        if r.clicked() {
            app.materials.selected = None;
        }
        if let Some(n) = app.materials.selected.and_then(|n| graph.get(n)) {
            ui.label("›");
            ui.label(RichText::new(&n.name).strong());
        }
    });
}

fn editor(app: &mut ModelApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let Some(id) = app.active() else {
        ui.label(RichText::new(tl!("Select a mesh to edit its material.")).color(theme::TEXT_DIM));
        return;
    };
    let Some(obj) = app.doc.scene.get(id).cloned() else { return };
    let Some(graph) = obj.material.graph.clone() else {
        ui.label(RichText::new(crate::tlf!("{name} has a plain material.", name = obj.name)).color(theme::TEXT_DIM));
        let r = ui.button(tl!("Add Material Graph"));
        if std::mem::take(&mut app.materials.focus_pending) {
            r.request_focus();
        }
        if r.clicked() {
            act(app, MatAction::AddGraph);
        }
        return;
    };
    breadcrumb(app, ui, &obj.name, &graph);
    thumbnails(app, ctx, id, &graph);
    let mut g = (*graph).clone();
    let mut error: Option<GraphError> = None;
    ui.horizontal(|ui| {
        let r = ui.button(tl!("Add Node…"));
        if std::mem::take(&mut app.materials.focus_pending) {
            r.request_focus();
        }
        if r.clicked() {
            app.materials.picker = !app.materials.picker;
            app.materials.query.clear();
        }
        if ui.button(tl!("Remove Material Graph")).clicked() {
            act(app, MatAction::RemoveGraph);
        }
    });
    if app.materials.picker {
        picker(app, ui, &mut g, &mut error);
    }
    let mut drags: Vec<egui::Response> = Vec::new();
    // Adding a node from the picker is a whole change, not a drag.
    let mut discrete = g != *graph;
    let mut remove = None;
    let ids: Vec<NodeId> = g.nodes.iter().map(|n| n.id).collect();
    egui::ScrollArea::both().id_salt("material-cards").show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for nid in ids {
                let Some(node) = g.get(nid).cloned() else { continue };
                let selected = app.materials.selected == Some(nid);
                egui::Frame::group(ui.style())
                    .stroke(egui::Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { theme::ACCENT } else { theme::TEXT_DIM }))
                    .show(ui, |ui| {
                        ui.set_width(210.0);
                        ui.horizontal(|ui| {
                            if let Some(t) = &app.materials.thumbs
                                && let Some((_, tex)) = t.images.iter().find(|(i, _)| *i == nid)
                            {
                                let r = ui.add(egui::Image::new(egui::load::SizedTexture::new(tex.id(), egui::vec2(THUMB as f32, THUMB as f32))));
                                ui.ctx().accesskit_node_builder(r.id, |n| {
                                    n.set_role(egui::accesskit::Role::Image);
                                    n.set_label(crate::tlf!("Preview of {name}", name = node.name));
                                });
                            }
                            ui.vertical(|ui| {
                                let r = ui.selectable_label(selected, RichText::new(&node.name).strong());
                                set_name(ui, &r, crate::tlf!("Node {name}, {kind}", name = node.name, kind = kind_title(&node.kind)));
                                if r.clicked() {
                                    app.materials.selected = Some(nid);
                                }
                                ui.label(RichText::new(kind_title(&node.kind)).color(theme::TEXT_DIM).small());
                            });
                        });
                        // Where each input comes from, chosen from a list.
                        for (port, input) in node.kind.inputs().iter().enumerate() {
                            let title = input_title(input);
                            let current = node.inputs.get(port).copied().flatten();
                            let shown = current.and_then(|c| g.get(c)).map_or_else(|| tl!("none"), |n| format!("{} {}", n.name, n.id));
                            ui.horizontal(|ui| {
                                ui.label(title.clone());
                                let r = egui::ComboBox::from_id_salt(("mat-link", nid, port)).selected_text(shown).show_ui(ui, |ui| {
                                    if ui.selectable_label(current.is_none(), tl!("none")).clicked() && current.is_some() {
                                        let _ = g.link(nid, port, None);
                                        discrete = true;
                                    }
                                    for (src, name) in sources(&g, nid, port) {
                                        if ui.selectable_label(current == Some(src), name).clicked() && current != Some(src) {
                                            match g.link(nid, port, Some(src)) {
                                                Ok(()) => discrete = true,
                                                Err(e) => error = Some(e),
                                            }
                                        }
                                    }
                                });
                                set_name(ui, &r.response, crate::tlf!("{input} of {name}", input = title, name = node.name));
                            });
                        }
                        if let Some(k) = settings(ui, &node.kind, &mut drags)
                            && let Err(e) = g.set(nid, k)
                        {
                            error = Some(e);
                        }
                        if !matches!(node.kind, NodeKind::Surface { .. }) {
                            let r = ui.button(tl!("Remove"));
                            set_name(ui, &r, crate::tlf!("Remove {name}", name = node.name));
                            if r.clicked() {
                                remove = Some(nid);
                            }
                        }
                    });
            }
        });
    });
    if let Some(nid) = remove
        && g.remove(nid).is_ok()
    {
        discrete = true;
        if app.materials.selected == Some(nid) {
            app.materials.selected = None;
        }
    }
    let changed = g != *graph;
    if changed {
        let material = Material { graph: Some(Arc::new(g)), ..obj.material.clone() };
        if discrete || drags.is_empty() {
            app.run(Command::SetMaterial { id, material });
        } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
            crate::panels::commit_edit(app, r, |o| o.material = material.clone(), || Command::SetMaterial { id, material: material.clone() }, id);
        }
    }
    let line = match error {
        Some(e) => crate::tlf!("Not changed: {error}", error = e),
        None => String::new(),
    };
    let r = ui.label(RichText::new(line).color(theme::TEXT));
    ui.ctx().accesskit_node_builder(r.id, |n| n.set_live(egui::accesskit::Live::Polite));
}

fn picker(app: &mut ModelApp, ui: &mut egui::Ui, g: &mut Graph, error: &mut Option<GraphError>) {
    ui.group(|ui| {
        let mut searched: Option<egui::Response> = None;
        ui.horizontal(|ui| {
            ui.label(tl!("Search nodes"));
            let r = ui.add(egui::TextEdit::singleline(&mut app.materials.query).desired_width(160.0));
            set_name(ui, &r, tl!("Search nodes"));
            if !r.has_focus() && app.materials.query.is_empty() {
                r.request_focus();
            }
            searched = Some(r);
        });
        let Some(searched) = searched else { return };
        let q = app.materials.query.trim().to_lowercase();
        let matches: Vec<&'static str> = NodeKind::IDS
            .iter()
            .copied()
            .filter(|id| *id != "surface")
            .filter(|id| {
                NodeKind::default_of(id).is_some_and(|k| q.is_empty() || tl!(k.title()).to_lowercase().contains(&q) || k.title().to_lowercase().contains(&q))
            })
            .collect();
        // Enter in the search box adds the first match (the key that opened the picker does not).
        let enter = searched.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        for (n, id) in matches.iter().enumerate() {
            let Some(kind) = NodeKind::default_of(id) else { continue };
            let r = ui.button(crate::tlf!("Add {kind}", kind = kind_title(&kind)));
            if r.clicked() || (enter && n == 0) {
                match g.add(kind) {
                    Ok(new) => {
                        app.materials.selected = Some(new);
                        app.materials.picker = false;
                    }
                    Err(e) => *error = Some(e),
                }
            }
        }
        if matches.is_empty() {
            ui.label(RichText::new(tl!("No node matches.")).color(theme::TEXT_DIM));
        }
    });
}
