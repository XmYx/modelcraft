//! kittest tests of the material manager and node editor: the palette, the node picker, links
//! chosen from lists (no dragging), thumbnails, undo and copying a material.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::Command;
use modelcraft_engine::scene::matgraph::{Graph, NodeKind};
use modelcraft_engine::scene::{Material, ObjectId};

use crate::app::ModelApp;
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let mut app = ModelApp::with_context(&ctx, Cli::default());
    let cube = app.doc.execute(Command::AddPrimitive { kind: "cube".into(), name: None, parent: None }).unwrap().created[0];
    app.selection = vec![cube];
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
}

fn graph_of(h: &Harness<'static, ModelApp>, id: ObjectId) -> Option<Graph> {
    h.state().doc.scene.get(id).and_then(|o| o.material.graph.as_ref().map(|g| (**g).clone()))
}

fn press(h: &mut Harness<'static, ModelApp>, role: Role, name: &str, key: Key) {
    h.get_by_role_and_label(role, name).focus();
    h.run();
    h.key_press(key);
    h.run();
    h.run();
}

#[test]
fn a_plain_material_gets_a_graph_from_the_keyboard_that_starts_from_it() {
    let mut h = harness();
    h.run();
    let cube = h.state().selection[0];
    h.state_mut()
        .doc
        .execute(Command::SetMaterial {
            id: cube,
            material: Material { base_color: [0.2, 0.4, 0.6, 1.0], metallic: 0.3, roughness: 0.7, ..Material::default() },
        })
        .unwrap();
    palette(&mut h, "Material Nodes");
    assert!(h.state().materials.open);
    h.get_by_label("Cube has a plain material.");
    press(&mut h, Role::Button, "Add Material Graph", Key::Enter);
    let g = graph_of(&h, cube).expect("the material has a graph");
    let surface = g.surface().unwrap();
    assert!(
        matches!(&surface.kind, NodeKind::Surface { base_color, metallic, roughness, .. } if *base_color == [0.2, 0.4, 0.6] && *metallic == 0.3 && *roughness == 0.7)
    );
    // The breadcrumb names where we are, and the card has a preview.
    h.get_by_role_and_label(Role::Link, "Breadcrumb: Cube");
    h.get_by_label("Preview of Surface");
    // Removing the graph keeps the flat material.
    press(&mut h, Role::Button, "Remove Material Graph", Key::Enter);
    assert!(graph_of(&h, cube).is_none());
    assert_eq!(h.state().doc.scene.get(cube).unwrap().material.roughness, 0.7);
}

#[test]
fn nodes_come_from_the_picker_and_links_are_chosen_from_lists_without_dragging() {
    let mut h = harness();
    h.run();
    let cube = h.state().selection[0];
    palette(&mut h, "Add Material Graph");
    palette(&mut h, "Material Nodes");
    // The picker: open it with the keyboard, type, Enter adds the first match.
    press(&mut h, Role::Button, "Add Node…", Key::Enter);
    h.event(egui::Event::Text("noi".into()));
    h.run();
    assert_eq!(h.state().materials.query, "noi", "the search box has the focus and takes the text");
    h.key_press(Key::Enter);
    h.run();
    h.run();
    let g = graph_of(&h, cube).unwrap();
    let noise = g.nodes.iter().find(|n| matches!(n.kind, NodeKind::Noise { .. })).expect("a noise node was added");
    assert_eq!(h.state().materials.selected, Some(noise.id));
    h.get_by_label("Preview of Noise");
    // Roughness of the surface is fed from the noise: open its list, walk to the noise, pick it.
    let undo = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::ComboBox, "Roughness of Surface").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    h.key_press(Key::Tab);
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    let g = graph_of(&h, cube).unwrap();
    assert_eq!(g.surface().unwrap().inputs[2], Some(noise.id), "linked by choosing from a list");
    assert_eq!(h.state().doc.undo.len(), undo + 1, "one undo step");
    // Undo takes the link away again.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    h.run();
    assert_eq!(graph_of(&h, cube).unwrap().surface().unwrap().inputs[2], None);
}

#[test]
fn a_node_card_edits_its_settings_and_removing_it_unlinks_what_used_it() {
    let mut h = harness();
    h.run();
    let cube = h.state().selection[0];
    palette(&mut h, "Add Material Graph");
    // Build a graph with a linked noise node directly, then open the editor on it.
    {
        let mut g = graph_of(&h, cube).unwrap();
        let surface = g.surface().unwrap().id;
        let noise = g.add(NodeKind::default_of("noise").unwrap()).unwrap();
        g.link(surface, 2, Some(noise)).unwrap();
        let material = Material { graph: Some(std::sync::Arc::new(g)), ..h.state().doc.scene.get(cube).unwrap().material.clone() };
        h.state_mut().doc.execute(Command::SetMaterial { id: cube, material }).unwrap();
    }
    palette(&mut h, "Material Nodes");
    h.get_by_role_and_label(Role::SpinButton, "Scale").set_value_accesskit(12.0);
    h.run();
    h.run();
    let g = graph_of(&h, cube).unwrap();
    assert!(g.nodes.iter().any(|n| matches!(n.kind, NodeKind::Noise { scale, .. } if scale == 12.0)));
    // A value past the end is held in range.
    h.get_by_role_and_label(Role::SpinButton, "Detail").set_value_accesskit(900.0);
    h.run();
    h.run();
    assert!(graph_of(&h, cube).unwrap().nodes.iter().any(|n| matches!(n.kind, NodeKind::Noise { detail, .. } if detail == 12)));
    // Removing the noise drops the link to it.
    press(&mut h, Role::Button, "Remove Noise", Key::Enter);
    let g = graph_of(&h, cube).unwrap();
    assert_eq!(g.nodes.len(), 1);
    assert_eq!(g.surface().unwrap().inputs[2], None);
}

#[test]
fn the_strip_lists_materials_and_copies_one_onto_the_selection() {
    let mut h = harness();
    h.run();
    let a = h.state().selection[0];
    let b = h.state_mut().doc.execute(Command::AddPrimitive { kind: "sphere".into(), name: Some("Ball".into()), parent: None }).unwrap().created[0];
    h.state_mut()
        .doc
        .execute(Command::SetMaterial { id: a, material: Material { base_color: [0.9, 0.1, 0.1, 1.0], roughness: 0.2, ..Material::default() } })
        .unwrap();
    palette(&mut h, "Material Nodes");
    h.get_by_role_and_label(Role::Button, "Material of Cube");
    h.get_by_role_and_label(Role::Button, "Material of Ball");
    // With the sphere selected first and the cube last, the cube's material goes onto the sphere.
    h.state_mut().selection = vec![b, a];
    h.run();
    palette(&mut h, "Copy Material to Selection");
    let m = h.state().doc.scene.get(b).unwrap().material.clone();
    assert_eq!((m.base_color[0], m.roughness), (0.9, 0.2));
    // Nothing to copy onto says so.
    h.state_mut().selection = vec![a];
    palette(&mut h, "Copy Material to Selection");
    assert!(h.state().status.contains("Select the objects to copy onto"), "{}", h.state().status);
    let disabled = h.get_by_role_and_label(Role::Button, "Copy to Selection").accesskit_node().is_disabled();
    assert!(!disabled);
}
