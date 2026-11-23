//! The Properties section of the procedural generators (connect, thicken, reduce, boolean), and
//! their menu labels. Every control has an accessible name; the pickers are combo
//! boxes, so they work from the keyboard.

use egui::RichText;
use modelcraft_engine::ops::boolean::{BoolOp, OPS};
use modelcraft_engine::scene::generator::Generator;
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::{Command, ProceduralCommand};

use crate::app::ModelApp;
use crate::generator_ui::pick;
use crate::panels::{commit_edit, drag};
use crate::theme;

pub fn generator_label(kind: &str) -> String {
    match kind {
        "connect" => tl!("Connect"),
        "thicken" => tl!("Thicken"),
        "reduce" => tl!("Reduce"),
        "boolean" => tl!("Boolean"),
        _ => kind.to_string(),
    }
}

fn direction_label(d: &str) -> String {
    match d {
        "both" => tl!("Both sides"),
        "outward" => tl!("Outward"),
        "inward" => tl!("Inward"),
        _ => d.to_string(),
    }
}

fn op_label(o: &str) -> String {
    match o {
        "union" => tl!("Union"),
        "subtract" => tl!("Subtract"),
        "intersect" => tl!("Intersect"),
        _ => o.to_string(),
    }
}

fn note(ui: &mut egui::Ui, text: String) {
    ui.label(RichText::new(text).color(theme::TEXT_DIM).small());
}

pub fn fields(app: &mut ModelApp, ui: &mut egui::Ui, id: ObjectId, generator: &Generator) {
    let mut q = generator.clone();
    let mut drags: Vec<egui::Response> = Vec::new();
    let mut discrete = false;
    match &mut q {
        Generator::Connect { weld } => {
            ui.horizontal(|ui| drags.push(drag(ui, weld, 0.001, 0.0..=1000.0, tl!("Join distance"))));
            note(ui, tl!("Joins every mesh inside into one mesh. A join distance merges points that lie closer than that."));
        }
        Generator::Thicken { amount, direction } => {
            ui.horizontal(|ui| drags.push(drag(ui, amount, 0.005, 0.00001..=100_000.0, tl!("Thickness"))));
            let names = modelcraft_engine::ops::thicken::DIRECTIONS;
            let labels: Vec<String> = names.iter().map(|d| direction_label(d)).collect();
            let cur = names.iter().position(|d| *d == direction.name()).unwrap_or(0);
            if let Some(i) = pick(ui, ("thicken-direction", id.0), tl!("Grow"), cur, &labels)
                && let Some(d) = names.get(i).and_then(|n| modelcraft_engine::ops::thicken::Direction::from_name(n))
            {
                *direction = d;
                discrete = true;
            }
            note(ui, tl!("Gives the first mesh inside a wall. An open sheet becomes a closed solid."));
        }
        Generator::Reduce { ratio } => {
            ui.horizontal(|ui| drags.push(drag(ui, ratio, 0.005, 0.0..=1.0, tl!("Keep"))));
            note(ui, tl!("Keeps about this share of the triangles of the first mesh inside, and follows it when it changes."));
        }
        Generator::Boolean { op } => {
            let labels: Vec<String> = OPS.iter().map(|o| op_label(o)).collect();
            let cur = OPS.iter().position(|o| *o == op.name()).unwrap_or(0);
            if let Some(i) = pick(ui, ("boolean-op", id.0), tl!("Operation"), cur, &labels)
                && let Some(o) = OPS.get(i).and_then(|n| BoolOp::from_name(n))
            {
                *op = o;
                discrete = true;
            }
            note(ui, tl!("Combines the first mesh inside with each of the others. The meshes must be closed. Move one and the result follows."));
        }
    }
    if q != *generator && discrete {
        app.run(Command::Procedural(ProceduralCommand::SetGenerator { id, generator: q }));
    } else if let Some(r) = drags.iter().find(|r| r.changed() || r.drag_stopped()) {
        let g = q.clone();
        commit_edit(
            app,
            r,
            |o| o.kind = ObjectKind::Generator { generator: g.clone() },
            || Command::Procedural(ProceduralCommand::SetGenerator { id, generator: q.clone() }),
            id,
        );
    }
    let e = app.doc.eval.evaluate(&app.doc.scene, id);
    ui.label(crate::tlf!("{count} faces", count = e.mesh.face_count()));
    for err in e.errors.iter().chain(&e.warnings) {
        note(ui, err.message.clone());
    }
}
