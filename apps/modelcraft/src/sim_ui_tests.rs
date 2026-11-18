//! Keyboard and accessibility-tree tests of the Simulation panel and actions (egui_kittest).

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::scene::{ObjectId, ObjectKind};
use modelcraft_engine::simulation::SimKind;

use crate::app::{ModelApp, SelectMode};
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(egui::Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
}

/// Let the bake thread finish: draw frames until the worker is collected.
fn wait_for_bake(h: &mut Harness<'static, ModelApp>) {
    for _ in 0..600 {
        h.run();
        if !h.state().sim.busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the bake did not finish");
}

fn sim_of(h: &Harness<'static, ModelApp>, mesh: ObjectId) -> Option<ObjectId> {
    h.state().doc.scene.sim_children(mesh).first().map(|(s, _)| *s)
}

fn rigid_mass(h: &Harness<'static, ModelApp>, sim: ObjectId) -> f32 {
    match &h.state().doc.scene.get(sim).unwrap().kind {
        ObjectKind::Sim { sim } => match &sim.kind {
            SimKind::Rigid(r) => r.mass,
            _ => panic!("not rigid"),
        },
        _ => panic!("not a simulation"),
    }
}

/// A cube dropped from a height, with a rigid body, a short range and the panel open.
fn dropped_cube(h: &mut Harness<'static, ModelApp>) -> (ObjectId, ObjectId) {
    h.run();
    palette(h, "Add Cube");
    let cube = h.state().selection[0];
    h.state_mut().run(modelcraft_engine::Command::SetTransform {
        id: cube,
        transform: modelcraft_engine::scene::Transform { translation: glam::Vec3::new(0.0, 3.0, 0.0), scale: glam::Vec3::splat(0.5), ..Default::default() },
    });
    palette(h, "Add Rigid Body");
    let sim = sim_of(h, cube).expect("the rigid body is a child of the cube");
    assert_eq!(h.state().selection, vec![sim], "the new simulation is selected");
    palette(h, "Simulation Panel");
    (cube, sim)
}

#[test]
fn add_edit_bake_and_play_a_rigid_body_by_keyboard() {
    let mut h = harness();
    let (cube, sim) = dropped_cube(&mut h);
    // The panel lists what it holds, by name.
    h.get_by_role_and_label(Role::Button, "Bake Simulation");
    h.get_by_role_and_label(Role::Button, "Cancel");
    h.get_by_role_and_label(Role::SpinButton, "Mass (kg)");
    h.get_by_role_and_label(Role::SpinButton, "Start frame");
    h.get_by_role_and_label(Role::SpinButton, "Substeps per frame");
    h.get_by_label("No cache: bake the simulation to see it");
    // A number typed into the panel is a command (and one undo step).
    h.get_by_role_and_label(Role::SpinButton, "Mass (kg)").set_value_accesskit(5.0);
    h.run();
    assert_eq!(rigid_mass(&h, sim), 5.0);
    h.state_mut().run(modelcraft_engine::Command::Sim(modelcraft_engine::SimCommand::Set {
        id: sim,
        start_frame: Some(1.0),
        end_frame: Some(48.0),
        fps: None,
        substeps: None,
        seed: None,
        max_cache_mib: None,
    }));
    // A ground at height zero so the cube lands; set through the engine (the panel has the checkbox).
    h.get_by_role_and_label(Role::CheckBox, "Ground plane").click();
    h.run();
    // Bake from the palette: a worker thread runs, then the cache is stored.
    palette(&mut h, "Bake Simulation");
    wait_for_bake(&mut h);
    assert!(h.state().status.starts_with("Baked"), "{}", h.state().status);
    let cache = match &h.state().doc.scene.get(sim).unwrap().kind {
        ObjectKind::Sim { sim } => sim.cache.clone().expect("a cache"),
        _ => panic!(),
    };
    assert_eq!((cache.frames(), cache.points()), (48, 8));
    h.run();
    h.get_by_label_contains("Cache: 48 frames of 8 points");
    // The mesh follows the cache on the timeline: at the last frame it rests on the ground.
    h.state_mut().doc.set_frame(48.0);
    let scene = h.state().doc.scene.clone();
    let world = scene.world_matrix(cube);
    let mesh = h.state_mut().doc.eval.evaluate(&scene, cube).mesh;
    let lowest = mesh.positions().iter().map(|p| world.transform_point3(*p).y).fold(f32::MAX, f32::min);
    assert!(lowest.abs() < 0.05, "resting on the ground: {lowest}");
    // Play Cache goes to the cache's first frame and starts the timeline.
    // (Through the action directly: playing keeps the window repainting, which the harness's run refuses.)
    crate::sim_ui::act(h.state_mut(), crate::sim_ui::SimAction::PlayCache);
    assert_eq!(h.state().doc.scene.anim.frame, 1.0);
    assert!(h.state().anim.playback.playing);
    h.state_mut().anim.playback.playing = false;
    h.run();
    // Clear it from the keyboard; undo brings it back.
    palette(&mut h, "Clear Simulation Cache");
    assert!(matches!(&h.state().doc.scene.get(sim).unwrap().kind, ObjectKind::Sim { sim } if sim.cache.is_none()));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(matches!(&h.state().doc.scene.get(sim).unwrap().kind, ObjectKind::Sim { sim } if sim.cache.is_some()));
}

#[test]
fn a_long_bake_shows_progress_and_escape_cancels_it() {
    let mut h = harness();
    let (_cube, sim) = dropped_cube(&mut h);
    // Enough frames that the worker is still running when we look.
    h.state_mut().run(modelcraft_engine::Command::Sim(modelcraft_engine::SimCommand::Set {
        id: sim,
        start_frame: Some(1.0),
        end_frame: Some(31_000.0),
        fps: None,
        substeps: Some(64),
        seed: None,
        max_cache_mib: Some(64),
    }));
    palette(&mut h, "Bake Simulation");
    assert!(h.state().sim.busy());
    h.run();
    // The progress is a named, valued node, and the busy state disables Bake.
    let bar = h.get_by_role(Role::ProgressIndicator);
    assert!(bar.accesskit_node().label().is_some_and(|l| l.starts_with("Baking: frame")), "{:?}", bar.accesskit_node().label());
    assert!(h.get_by_role_and_label(Role::Button, "Bake Simulation").accesskit_node().is_disabled());
    assert!(!h.get_by_role_and_label(Role::Button, "Cancel").accesskit_node().is_disabled());
    // Escape stops it after the current frame; nothing is stored.
    h.key_press(Key::Escape);
    wait_for_bake(&mut h);
    assert!(h.state().status.starts_with("Simulation cancelled"), "{}", h.state().status);
    assert!(matches!(&h.state().doc.scene.get(sim).unwrap().kind, ObjectKind::Sim { sim } if sim.cache.is_none()));
    // The Cancel button does the same.
    palette(&mut h, "Bake Simulation");
    h.run();
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    wait_for_bake(&mut h);
    assert!(h.state().status.starts_with("Simulation cancelled"), "{}", h.state().status);
    // With nothing baking the action says so.
    palette(&mut h, "Cancel Simulation");
    assert_eq!(h.state().status, "No simulation is baking");
}

#[test]
fn a_mesh_without_a_simulation_offers_to_add_one_and_nothing_selected_says_so() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Simulation Panel");
    h.get_by_label_contains("No simulation on the selected object");
    h.get_by_label_contains("Select a mesh first");
    palette(&mut h, "Add Cloth");
    assert_eq!(h.state().status, "Select the object that gets the simulation first");
    palette(&mut h, "Bake Simulation");
    assert_eq!(h.state().status, "Select an object with a simulation first");
    // With a mesh selected the add buttons work.
    palette(&mut h, "Add Plane");
    let plane = h.state().selection[0];
    h.run();
    h.get_by_role_and_label(Role::Button, "Add Cloth").click();
    h.run();
    assert!(sim_of(&h, plane).is_some());
    // The panel now shows the cloth's settings by name.
    h.get_by_role_and_label(Role::SpinButton, "Stretch stiffness");
    h.get_by_role_and_label(Role::CheckBox, "Self collision");
    h.get_by_role_and_label(Role::CheckBox, "Pin vertices with a sculpt mask");
    // Pinning needs selected vertices in vertex mode.
    palette(&mut h, "Pin Selected Cloth Vertices");
    assert_eq!(h.state().status, "Select cloth vertices in vertex mode first");
    h.state_mut().select_only(plane);
    h.state_mut().mode = SelectMode::Vertex;
    h.state_mut().components = [0u32, 1, 2].into_iter().collect();
    palette(&mut h, "Pin Selected Cloth Vertices");
    let sim = sim_of(&h, plane).unwrap();
    let pinned = match &h.state().doc.scene.get(sim).unwrap().kind {
        ObjectKind::Sim { sim } => match &sim.kind {
            SimKind::Cloth(c) => c.pinned.clone(),
            _ => panic!(),
        },
        _ => panic!(),
    };
    assert_eq!(pinned, vec![0, 1, 2]);
}

#[test]
fn particles_come_from_a_group_and_the_panel_has_their_settings() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Group");
    palette(&mut h, "Simulation Panel");
    palette(&mut h, "Add Particles");
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Rate per second");
    h.get_by_role_and_label(Role::SpinButton, "Pool size");
    h.get_by_role_and_label(Role::CheckBox, "Collide with other objects");
    palette(&mut h, "Bake Simulation");
    wait_for_bake(&mut h);
    assert!(h.state().status.starts_with("Baked"), "{}", h.state().status);
}
