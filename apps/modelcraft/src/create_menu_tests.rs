//! kittest tests of the grouped Create menu, the icon palettes and the UV, Animate and Groom
//! workspaces.

use egui::accesskit::{Role, Toggled};
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

use crate::actions::Action;
use crate::app::{ModelApp, SelectMode, Tool, Workspace};
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    let mut h = Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app);
    h.run();
    h
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
}

#[test]
fn the_create_menu_has_a_submenu_per_group_and_every_command_stays_in_the_palette() {
    let mut h = harness();
    h.get_by_label("Create").click();
    h.run();
    h.run();
    for g in ["Primitives", "Generators", "Clones", "Deformers", "Fields", "Rig", "Simulation"] {
        h.get_all_by_label(g).next().expect(g);
    }
    h.get_by_label("Primitives").click();
    h.run();
    h.run();
    h.get_by_label_contains("Add Cube").click();
    h.run();
    h.run();
    assert_eq!(h.state().doc.scene.len(), 1, "a menu entry in a submenu runs its command");
    // Every grouped command is a registered command, so the palette lists it.
    let all = crate::actions::all();
    for g in crate::create_menu::groups() {
        assert!(!g.items.is_empty(), "{} is not empty", g.name);
        for a in g.items {
            assert!(all.contains(&a), "{a:?} is reachable from the palette");
        }
    }
}

#[test]
fn command_labels_in_the_groups_are_unique() {
    let mut seen = std::collections::BTreeSet::new();
    for g in crate::create_menu::groups() {
        for a in g.items {
            assert!(seen.insert(a.label()), "{} appears twice", a.label());
        }
    }
}

#[test]
fn the_icon_palettes_are_off_by_default_and_turn_on_from_the_palette() {
    let mut h = harness();
    assert!(!h.state().icon_palettes);
    assert!(h.query_all_by_role_and_label(Role::Button, "Add Cube").next().is_none());
    palette(&mut h, "Icon Palettes");
    assert!(h.state().icon_palettes);
    h.get_by_role_and_label(Role::Button, "Add Cube");
    h.get_by_role_and_label(Role::Button, &Action::ToolMove.label());
}

#[test]
fn icon_buttons_create_and_switch_tools_from_the_keyboard() {
    let mut h = harness();
    palette(&mut h, "Icon Palettes");
    h.get_by_role_and_label(Role::Button, "Add Cube").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(h.state().doc.scene.len(), 1);
    let (move_name, rotate_name) = (Action::ToolMove.label(), Action::ToolRotate.label());
    assert_eq!(h.get_by_role_and_label(Role::Button, &move_name).accesskit_node().toggled(), Some(Toggled::True), "the current tool is shown as on");
    h.get_by_role_and_label(Role::Button, &rotate_name).focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
    assert_eq!(h.state().tool, Tool::Rotate);
    assert_eq!(h.get_by_role_and_label(Role::Button, &rotate_name).accesskit_node().toggled(), Some(Toggled::True));
    assert_eq!(h.get_by_role_and_label(Role::Button, &move_name).accesskit_node().toggled(), Some(Toggled::False));
    let face = Action::ModeFace.label();
    h.get_by_role_and_label(Role::Button, &face).focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(h.state().mode, SelectMode::Face);
}

#[test]
fn the_new_workspaces_open_their_window_and_keep_the_modeling_tools() {
    let mut h = harness();
    palette(&mut h, "UV Workspace");
    assert_eq!(h.state().workspace, Workspace::Uv);
    assert!(h.state().uv.open, "the UV Editor comes with the workspace");
    h.get_by_role_and_label(Role::Button, "Mark Seam");
    // A shortcut switches too, from the keyboard.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num5);
    h.run();
    h.run();
    assert_eq!(h.state().workspace, Workspace::Animate);
    assert!(h.state().anim.open);
    palette(&mut h, "Groom Workspace");
    assert_eq!(h.state().workspace, Workspace::Groom);
    // They edit like Model: modes and tools are there.
    assert!(h.state().workspace.models());
    h.get_all_by_label("Vertex").next().expect("mode tabs");
    palette(&mut h, "Model Workspace");
    assert_eq!(h.state().workspace, Workspace::Model);
}

#[test]
fn every_workspace_has_a_valid_layout_and_older_saved_layouts_still_load() {
    let l = crate::dock::Layouts::default();
    for ws in crate::dock::WORKSPACES {
        assert!(l.get(ws).validate().is_ok());
        assert!(l.get(ws).contains(&crate::dock::Pane::Viewport));
    }
    let mut old = std::collections::BTreeMap::new();
    let model = l.to_saved()["model"].clone();
    old.insert("model".to_string(), model);
    old.insert("sculpt".to_string(), l.to_saved()["sculpt"].clone());
    old.insert("paint".to_string(), l.to_saved()["paint"].clone());
    let back = crate::dock::Layouts::from_saved(&old);
    assert_eq!(back.to_saved(), l.to_saved(), "a three-workspace file loads, the new workspaces take their defaults");
    assert_eq!(crate::dock::WORKSPACES.len(), 6);
}
