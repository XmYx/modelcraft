//! kittest tests of the clone modifiers, driven by the keyboard and the accessibility tree.

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use modelcraft_engine::Command;
use modelcraft_engine::clone::Mode;
use modelcraft_engine::scene::{ObjectId, ObjectKind};

use crate::app::ModelApp;
use crate::args::Cli;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
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

fn generator(h: &Harness<'static, ModelApp>) -> ObjectId {
    h.state().doc.scene.objects().iter().find(|o| matches!(o.kind, ObjectKind::Cloner { .. })).map(|o| o.id).expect("a generator")
}

/// The y coordinate of every clone, as the evaluator gives it.
fn heights(h: &mut Harness<'static, ModelApp>) -> Vec<f32> {
    let id = generator(h);
    let s = h.state_mut();
    s.doc.eval.evaluate(&s.doc.scene, id).instances.map_or(Vec::new(), |i| i.clones.instances.iter().map(|c| c.matrix.w_axis.y).collect())
}

fn with_modifier(kind: &str) -> Harness<'static, ModelApp> {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Linear Clones");
    palette(&mut h, &format!("Add {kind} Clone Modifier"));
    h.run();
    h
}

#[test]
fn the_palette_adds_a_modifier_inside_the_generator_and_the_clones_follow_it() {
    let mut h = with_modifier("Plain");
    let gen_id = generator(&h);
    let m = h.state().selection[0];
    assert!(matches!(h.state().doc.scene.get(m).unwrap().kind, ObjectKind::CloneModifier { .. }));
    assert_eq!(h.state().doc.scene.get(m).unwrap().parent, Some(gen_id));
    assert_eq!(heights(&mut h), vec![0.5; 5], "a plain modifier moves every clone the same");
    // The tree and the Properties panel name it, and the generator still has its source.
    h.get_by_role_and_label(Role::TreeItem, "Plain modifier");
    h.get_by_role_and_label(Role::TreeItem, "Clone source");
    h.get_by_role_and_label(Role::SpinButton, "Strength");
    h.get_by_label("0 falloff fields limit where it acts");
    // Undo takes the modifier away again.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(heights(&mut h), vec![0.0; 5]);
}

#[test]
fn adding_one_with_no_generator_selected_says_so() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Cube");
    palette(&mut h, "Add Random Clone Modifier");
    assert_eq!(h.state().status, "Select a clone generator for the modifier");
    assert_eq!(h.state().doc.scene.len(), 1);
}

#[test]
fn the_channels_change_from_named_fields_with_one_undo_step() {
    let mut h = with_modifier("Plain");
    let before = h.state().doc.undo.len();
    h.get_by_label("Move Y").set_value_accesskit(2.0);
    h.run();
    h.run();
    assert_eq!(heights(&mut h), vec![2.0; 5]);
    assert_eq!(h.state().doc.undo.len(), before + 1);
    h.get_by_role_and_label(Role::SpinButton, "Strength").set_value_accesskit(0.5);
    h.run();
    h.run();
    assert_eq!(heights(&mut h), vec![1.0; 5]);
    h.get_by_label("Grow X");
    h.get_by_label("Turn Y");
    h.get_by_role_and_label(Role::SpinButton, "Tint amount");
}

fn open_combo(h: &mut Harness<'static, ModelApp>, name: &str) {
    h.get_by_role_and_label(Role::ComboBox, name).focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.run();
}

#[test]
fn the_mode_combo_switches_modes_by_keyboard_and_shows_that_modes_fields() {
    let mut h = with_modifier("Plain");
    assert!(h.query_by_role_and_label(Role::SpinButton, "Duration (seconds)").is_none());
    open_combo(&mut h, "Mode");
    h.key_press(Key::ArrowDown);
    h.run();
    // Plain, Random, Noise, Step, Delay: four Tab presses from Plain reach Delay.
    for _ in 0..4 {
        h.key_press(Key::Tab);
        h.run();
    }
    h.key_press(Key::Enter);
    h.run();
    h.run();
    let m = h.state().selection[0];
    assert!(matches!(h.state().doc.scene.get(m).unwrap().kind, ObjectKind::CloneModifier { modifier, .. } if matches!(modifier.mode, Mode::Delay { .. })));
    h.get_by_role_and_label(Role::SpinButton, "Duration (seconds)");
    h.get_by_role_and_label(Role::SpinButton, "Delay per clone (seconds)");
    assert!(h.get_by_role_and_label(Role::ComboBox, "Mode").is_focused(), "focus returns to the combo");
    assert_eq!(h.state().status, "Clone modifier settings");
    h.get_by_label_contains("Follows the timeline");
}

#[test]
fn a_delay_modifier_follows_the_timeline() {
    let mut h = with_modifier("Delay");
    let m = h.state().selection[0];
    let delay = modelcraft_engine::clone::Modifier {
        mode: Mode::Delay { start: 0.0, stagger: 0.5, duration: 1.0, ease: modelcraft_engine::clone::Ease::Linear, reverse: false },
        ..modelcraft_engine::clone::Modifier::default_of("delay").unwrap()
    };
    h.state_mut().run(Command::SetCloneModifier { id: m, modifier: delay, target: None });
    h.state_mut().doc.set_frame(0.0);
    h.run();
    assert_eq!(heights(&mut h), vec![0.0; 5], "nothing has moved at the start");
    let fps = h.state().doc.scene.anim.time.fps;
    h.state_mut().doc.set_frame(fps * 0.5);
    let mid = heights(&mut h);
    assert!((mid[0] - 0.25).abs() < 1e-4 && mid[1].abs() < 1e-4, "{mid:?}");
    h.state_mut().doc.set_frame(fps * 10.0);
    assert_eq!(heights(&mut h), vec![0.5; 5], "everything has arrived");
}

#[test]
fn a_falloff_field_inside_the_modifier_limits_where_it_acts() {
    let mut h = with_modifier("Plain");
    palette(&mut h, "Add Sphere Falloff Field");
    let field = h.state().selection[0];
    let m = h.state().doc.scene.get(field).unwrap().parent.expect("inside the modifier");
    assert!(matches!(h.state().doc.scene.get(m).unwrap().kind, ObjectKind::CloneModifier { .. }));
    // A small sphere at the first clone: only that clone moves.
    let sphere = modelcraft_engine::scene::Field {
        shape: modelcraft_engine::field::Shape::Sphere { radius: 0.4, inner: 0.0 },
        ..modelcraft_engine::scene::Field::default_of("sphere").unwrap()
    };
    h.state_mut().run(Command::SetField { id: field, field: sphere });
    let ys = heights(&mut h);
    assert!(ys[0] > 0.4 && ys[1..].iter().all(|y| y.abs() < 1e-4), "{ys:?}");
    h.state_mut().select_only(m);
    h.run();
    h.get_by_label("1 falloff fields limit where it acts");
}

#[test]
fn the_target_mode_asks_for_a_target_and_the_combo_works_by_keyboard() {
    let mut h = with_modifier("Target");
    assert!(h.state().status.contains("target"), "announced through the status line: {}", h.state().status);
    h.get_by_role_and_label(Role::ComboBox, "Target object");
    h.get_by_role_and_label(Role::SpinButton, "Pull toward target");
    let gen_id = generator(&h);
    let m = h.state().selection[0];
    palette(&mut h, "Add Sphere");
    let sphere = h.state().selection[0];
    h.state_mut().run(Command::SetTransform {
        id: sphere,
        transform: modelcraft_engine::scene::Transform { translation: glam::Vec3::new(0.0, 5.0, 0.0), ..Default::default() },
    });
    h.state_mut().select_only(m);
    h.run();
    h.run();
    open_combo(&mut h, "Target object");
    h.key_press(Key::ArrowDown);
    h.run();
    h.key_press(Key::Tab);
    h.run();
    // Nothing from the generator's own subtree is offered.
    let offered: Vec<String> = h.get_all_by_role(Role::Button).filter_map(|n| egui_kittest::kittest::NodeT::accesskit_node(&n).label()).collect();
    for own in ["Clones", "Clone source"] {
        assert!(!offered.iter().any(|l| l == own), "{own} must not be offered: {offered:?}");
    }
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert!(matches!(h.state().doc.scene.get(m).unwrap().kind, ObjectKind::CloneModifier { target: Some(t), .. } if t == sphere));
    // The default pull of one half moves every clone halfway up to the sphere.
    let ys = heights(&mut h);
    assert!(ys.iter().all(|y| (*y - 2.5).abs() < 0.3), "{ys:?}");
    // The command refuses the generator itself and a stale object.
    let modifier = match h.state().doc.scene.get(m).unwrap().kind {
        ObjectKind::CloneModifier { modifier, .. } => modifier,
        _ => unreachable!(),
    };
    assert!(h.state_mut().doc.execute(Command::SetCloneModifier { id: m, modifier, target: Some(gen_id) }).is_err());
    assert!(h.state_mut().doc.execute(Command::SetCloneModifier { id: m, modifier, target: Some(ObjectId(9999)) }).is_err());
}

#[test]
fn modifiers_apply_in_order_and_survive_a_save_and_load() {
    let mut h = with_modifier("Plain");
    palette(&mut h, "Add Step Clone Modifier");
    let ys = heights(&mut h);
    assert!((ys[0] - 0.5).abs() < 1e-5 && (ys[4] - 1.0).abs() < 1e-5, "{ys:?}");
    let json = serde_json::to_string(&h.state().doc.scene).unwrap();
    let back: modelcraft_engine::scene::Scene = serde_json::from_str(&json).unwrap();
    let back = back.repaired().unwrap();
    let mut ev = modelcraft_engine::scene::Evaluator::new();
    let id = back.objects().iter().find(|o| matches!(o.kind, ObjectKind::Cloner { .. })).unwrap().id;
    let again: Vec<f32> = ev.evaluate(&back, id).instances.unwrap().clones.instances.iter().map(|c| c.matrix.w_axis.y).collect();
    assert_eq!(again, ys);
    // A hidden modifier is off.
    let first = back.clone_modifiers(id)[0];
    let mut hidden = back.clone();
    hidden.edit(first).unwrap().visible = false;
    let ys2: Vec<f32> = ev.evaluate(&hidden, id).instances.unwrap().clones.instances.iter().map(|c| c.matrix.w_axis.y).collect();
    assert!((ys2[0]).abs() < 1e-5 && (ys2[4] - 0.5).abs() < 1e-5);
}

#[test]
fn the_commands_validate_their_input() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Linear Clones");
    let gen_id = generator(&h);
    let n = h.state().doc.undo.len();
    let cube = h.state().doc.scene.objects().iter().find(|o| o.name == "Clone source").map(|o| o.id).unwrap();
    let doc = &mut h.state_mut().doc;
    assert!(doc.execute(Command::AddCloneModifier { kind: "shader".into(), cloner: gen_id }).is_err());
    assert!(doc.execute(Command::AddCloneModifier { kind: "plain".into(), cloner: cube }).is_err());
    assert!(doc.execute(Command::AddCloneModifier { kind: "plain".into(), cloner: ObjectId(777) }).is_err());
    assert!(doc.execute(Command::SetCloneModifier { id: cube, modifier: modelcraft_engine::clone::Modifier::default_of("plain").unwrap(), target: None }).is_err());
    assert_eq!(doc.undo.len(), n, "failed commands leave no undo step");
    // The command names are in the registry for the CLI and scripts.
    let names: Vec<&str> = modelcraft_engine::COMMANDS.iter().map(|c| c.0).collect();
    assert!(names.contains(&"add_clone_modifier") && names.contains(&"set_clone_modifier"));
}
