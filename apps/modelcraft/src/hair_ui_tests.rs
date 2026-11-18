//! Keyboard, pointer and accessibility-tree tests of hair and grooming (egui_kittest).

use egui::accesskit::Role;
use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use modelcraft_engine::scene::ObjectKind;
use modelcraft_hair::{BrushKind, Guides};

use crate::app::ModelApp;
use crate::ui_tests::{harness, palette};

fn guides(h: &Harness<'static, ModelApp>) -> Guides {
    let id = crate::hair_ui::target_hair(h.state()).expect("hair is selected");
    match &h.state().doc.scene.get(id).unwrap().kind {
        ObjectKind::Hair { hair } => hair.guides.clone(),
        _ => panic!("not hair"),
    }
}

/// A sphere with hair, the hair selected.
fn with_hair() -> Harness<'static, ModelApp> {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Quad sphere");
    palette(&mut h, "Add Hair");
    h.run();
    h
}

#[test]
fn hair_is_added_through_the_palette_and_listed() {
    let h = with_hair();
    assert_eq!(h.state().doc.scene.len(), 2);
    let hair = crate::hair_ui::target_hair(h.state()).unwrap();
    assert_eq!(h.state().selection, vec![hair], "the new hair is selected");
    h.get_by_role_and_label(Role::TreeItem, "Hair");
    assert_eq!(guides(&h).len(), 1000);
    assert!(h.state().status.starts_with("Hair added"), "{}", h.state().status);
    // The viewport draws the guides: one polyline of 7 segments each.
    assert_eq!(crate::hair_ui::guide_lines(h.state()).len(), 1000 * 7);
    // Without a mesh the command explains instead of failing silently.
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Hair");
    assert!(h.state().status.contains("Select the mesh"), "{}", h.state().status);
}

#[test]
fn the_properties_section_names_every_brush_and_number() {
    let h = with_hair();
    for kind in BrushKind::ALL {
        let name = crate::hair_ui::HairAction::Brush(kind).label();
        h.get_by_role_and_label(Role::Button, &name);
    }
    for name in ["Hair brush radius", "Hair brush strength", "Hair brush falloff", "Guide count", "Hair count", "Clump", "Frizz", "Root width", "Tip width"] {
        h.get_by_role_and_label(Role::SpinButton, name);
    }
    for name in ["Hair Symmetry X", "Hair Symmetry Y", "Hair Symmetry Z", "Invert Hair Brush", "Groom with the Pointer"] {
        h.get_by_role_and_label(Role::CheckBox, name);
    }
    for name in ["Apply Hair Brush to All Guides", "Hair Mask All", "Hair Mask Invert", "Hair Mask Clear", "Regrow Guides", "Remove Hair"] {
        h.get_by_role_and_label(Role::Button, name);
    }
}

#[test]
fn cut_by_typing_a_length_and_undo_it() {
    let mut h = with_hair();
    let before = guides(&h);
    h.get_by_role_and_label(Role::Button, "Cut Hair Brush").click();
    h.run();
    assert_eq!(h.state().groom.brush.kind, BrushKind::Cut);
    h.get_by_role_and_label(Role::SpinButton, "Hair brush strength").set_value_accesskit(1.0);
    h.run();
    h.get_by_role_and_label(Role::SpinButton, "Cut length").set_value_accesskit(0.05);
    h.run();
    let undo = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::Button, "Apply Hair Brush to All Guides").click();
    h.run();
    assert_eq!(h.state().doc.undo.len(), undo + 1);
    let after = guides(&h);
    assert!((0..after.len()).all(|i| after.strand_length(i) <= 0.05 + 1e-3));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(guides(&h), before, "undo gives the guides back");
}

#[test]
fn comb_by_typing_a_direction_and_the_apply_shortcut() {
    let mut h = with_hair();
    let before = guides(&h);
    // No direction yet: the shortcut explains.
    h.key_press_modifiers(Modifiers::ALT, Key::Enter);
    h.run();
    assert!(h.state().status.contains("comb direction"), "{}", h.state().status);
    assert_eq!(guides(&h), before);
    h.get_by_role_and_label(Role::SpinButton, "Hair brush strength").set_value_accesskit(1.0);
    h.get_by_role_and_label(Role::SpinButton, "Comb direction X").set_value_accesskit(0.2);
    h.run();
    h.key_press_modifiers(Modifiers::ALT, Key::Enter);
    h.run();
    assert_ne!(guides(&h), before);
    assert_eq!(h.state().status, "comb all");
}

#[test]
fn symmetry_and_the_mask_are_checkboxes_and_buttons_with_state() {
    let mut h = with_hair();
    let off = h.get_by_role_and_label(Role::CheckBox, "Hair Symmetry X").accesskit_node().toggled();
    h.get_by_role_and_label(Role::CheckBox, "Hair Symmetry X").click();
    h.run();
    assert!(h.state().groom.symmetry.x);
    assert_ne!(h.get_by_role_and_label(Role::CheckBox, "Hair Symmetry X").accesskit_node().toggled(), off);
    palette(&mut h, "Hair Symmetry X");
    assert!(!h.state().groom.symmetry.x, "the palette toggles it back");
    let hair = crate::hair_ui::target_hair(h.state()).unwrap();
    let mask = |h: &Harness<'static, ModelApp>| match &h.state().doc.scene.get(hair).unwrap().kind {
        ObjectKind::Hair { hair } => hair.mask.clone(),
        _ => None,
    };
    h.get_by_role_and_label(Role::Button, "Hair Mask Invert").click();
    h.run();
    assert!(mask(&h).is_some_and(|m| m.iter().all(|v| *v == 0.0)), "inverting no mask protects everything");
    h.get_by_role_and_label(Role::Button, "Hair Mask Clear").click();
    h.run();
    assert!(mask(&h).is_none());
}

/// Drag the primary button across the viewport, from `from` to `to` in screen points.
fn drag(h: &mut Harness<'static, ModelApp>, from: egui::Pos2, to: egui::Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    h.event(egui::Event::PointerMoved(from));
    h.step();
    h.event(button(from, true));
    h.step();
    for k in 1..=12 {
        h.event(egui::Event::PointerMoved(from.lerp(to, k as f32 / 12.0)));
        h.step();
    }
    h.event(button(to, false));
    h.step();
    h.step();
}

#[test]
fn a_pointer_stroke_grooms_in_one_undo_step() {
    for kind in [BrushKind::Length, BrushKind::Comb, BrushKind::Frizz] {
        let mut h = with_hair();
        // Brush and groom mode from the keyboard path (palette) so the test needs no panel clicks.
        palette(&mut h, "Groom with the Pointer");
        assert!(h.state().groom.active);
        h.state_mut().groom.brush.kind = kind;
        h.state_mut().groom.brush.strength = 1.0;
        h.state_mut().groom.brush.radius = 0.6;
        h.run();
        let before = guides(&h);
        let undo = h.state().doc.undo.len();
        let rect = h.get_by_label_contains("Viewport, Perspective").rect();
        drag(&mut h, rect.center() - egui::vec2(40.0, 0.0), rect.center() + egui::vec2(40.0, 5.0));
        assert!(h.state().groom.stroke.is_none() && h.state().groom.preview.is_none(), "the stroke ended with the button");
        assert_eq!(h.state().doc.undo.len(), undo + 1, "{kind:?}: one stroke, one undo step");
        let after = guides(&h);
        assert_ne!(after, before, "{kind:?} changed the guides");
        assert!(after.all_points().iter().all(|p| p.is_finite()));
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run();
        assert_eq!(guides(&h), before, "{kind:?}: undo");
    }
}

#[test]
fn grooming_off_leaves_the_pointer_to_selection() {
    let mut h = with_hair();
    assert!(!h.state().groom.active);
    let before = guides(&h);
    let undo = h.state().doc.undo.len();
    let rect = h.get_by_label_contains("Viewport, Perspective").rect();
    drag(&mut h, rect.center() - egui::vec2(40.0, 0.0), rect.center() + egui::vec2(40.0, 5.0));
    assert_eq!(guides(&h), before);
    assert_eq!(h.state().doc.undo.len(), undo, "a drag with grooming off is not a stroke");
    // The toggle shortcut works from the keyboard and says so.
    h.key_press_modifiers(Modifiers::ALT, Key::H);
    h.run();
    assert!(h.state().groom.active);
    assert!(h.state().status.starts_with("Grooming"), "{}", h.state().status);
}

#[test]
fn regrow_and_remove() {
    let mut h = with_hair();
    let before = guides(&h);
    h.get_by_role_and_label(Role::SpinButton, "Guide count").set_value_accesskit(200.0);
    h.run();
    // The button sits below the fold of the panel in a window this size; the palette runs the same action.
    palette(&mut h, "Regrow Guides");
    assert_eq!(guides(&h).len(), 200);
    assert_ne!(guides(&h), before);
    h.get_by_role_and_label(Role::Button, "Remove Hair").click();
    h.run();
    assert!(h.state().doc.scene.objects().iter().all(|o| !matches!(o.kind, ObjectKind::Hair { .. })), "{}", h.state().status);
}

#[test]
fn the_display_percentage_and_limit_decide_how_many_strands_are_drawn() {
    use crate::hair_ui::{DisplaySettings, EDIT_STRAND_LIMIT, strands_to_draw};
    let d = DisplaySettings::default();
    assert_eq!(strands_to_draw(0, &d), 0);
    assert_eq!(strands_to_draw(20_000, &d), 20_000);
    assert_eq!(strands_to_draw(1_000_000, &d), EDIT_STRAND_LIMIT, "the limit caps a large groom while editing");
    assert_eq!(strands_to_draw(1_000_000, &DisplaySettings { limit: false, ..d }), 1_000_000);
    assert_eq!(strands_to_draw(1_000_000, &DisplaySettings { limit: false, percent: 10.0, ..d }), 100_000);
    assert_eq!(strands_to_draw(1_000, &DisplaySettings { percent: 0.1, ..d }), 1);
    assert_eq!(strands_to_draw(50, &DisplaySettings { percent: 0.0, ..d }), 1, "a groom never disappears into a rounding error");
    assert_eq!(strands_to_draw(50, &DisplaySettings { percent: f32::NAN, ..d }), 50);
    assert_eq!(strands_to_draw(50, &DisplaySettings { strands: false, ..d }), 0);
}

#[test]
fn display_options_are_named_controls_with_a_palette_path() {
    let mut h = with_hair();
    for name in ["Show Hair Strands", "Show Hair Guides", "Limit strands while editing"] {
        h.get_by_role_and_label(Role::CheckBox, name);
    }
    h.get_by_role_and_label(Role::SpinButton, "Hair display percent").set_value_accesskit(25.0);
    h.run();
    assert_eq!(h.state().groom.display.percent, 25.0);
    assert!(h.state().status.is_empty() || !h.state().status.contains("failed"));
    h.get_by_label_contains("Showing 5000 of 20000 strands");
    // Guides are drawn for selected hair; with them off nothing is, unless strands are off too.
    assert_eq!(crate::hair_ui::guide_lines(h.state()).len(), 1000 * 7);
    palette(&mut h, "Show Hair Guides");
    assert!(!h.state().groom.display.guides);
    assert!(crate::hair_ui::guide_lines(h.state()).is_empty());
    palette(&mut h, "Show Hair Strands");
    assert!(!h.state().groom.display.strands);
    assert_eq!(h.state().status, "Hair strands hidden");
    assert!(crate::hair_ui::guide_lines(h.state()).is_empty(), "guides off stays off");
    palette(&mut h, "Show Hair Guides");
    assert_eq!(crate::hair_ui::guide_lines(h.state()).len(), 1000 * 7, "with strands hidden the guides are what is shown");
}

#[test]
fn dynamics_are_typed_baked_scrubbed_and_undone() {
    use modelcraft_engine::Command;
    let mut h = with_hair();
    for name in [
        "Gravity X",
        "Gravity Y",
        "Gravity Z",
        "Wind X",
        "Wind Y",
        "Wind Z",
        "Hair damping",
        "Hair stiffness",
        "Collision clearance",
        "Bake start frame",
        "Bake end frame",
    ] {
        h.get_by_role_and_label(Role::SpinButton, name);
    }
    h.get_by_role_and_label(Role::CheckBox, "Collide with the emitter");
    for name in ["Bake Hair Dynamics", "Clear Hair Bake", "Export Hair as glTF…", "Export Hair Point Cache…"] {
        h.get_by_role_and_label(Role::Button, name);
    }
    let rest = guides(&h);
    // No forces by typing: the bake leaves the hair where it is.
    h.get_by_role_and_label(Role::SpinButton, "Gravity Y").set_value_accesskit(0.0);
    h.get_by_role_and_label(Role::SpinButton, "Bake end frame").set_value_accesskit(12.0);
    h.run();
    palette(&mut h, "Bake Hair Dynamics");
    assert!(h.state().status.starts_with("Hair baked"), "{}", h.state().status);
    let hair = crate::hair_ui::target_hair(h.state()).unwrap();
    let data = |h: &Harness<'static, ModelApp>| match &h.state().doc.scene.get(hair).unwrap().kind {
        ObjectKind::Hair { hair } => hair.clone(),
        _ => panic!("not hair"),
    };
    let baked = data(&h);
    assert_eq!(baked.baked.as_ref().map(|c| c.frames()), Some(12));
    assert_eq!(baked.at_frame(8.0).guides.all_points(), rest.all_points());
    // With gravity back on, a bake sags: the drawn guides at the last frame differ from the first.
    h.get_by_role_and_label(Role::SpinButton, "Gravity Y").set_value_accesskit(-9.81);
    h.run();
    palette(&mut h, "Bake Hair Dynamics");
    let lines_at = |h: &mut Harness<'static, ModelApp>, frame: f64| {
        h.state_mut().doc.scene.anim.frame = frame;
        crate::hair_ui::guide_lines(h.state())
    };
    let (first, last) = (lines_at(&mut h, 1.0), lines_at(&mut h, 12.0));
    assert_eq!(first.len(), last.len());
    assert!(first.iter().zip(&last).any(|(a, b)| a.1 != b.1), "the guides on screen follow the cache by frame");
    // One undo takes the bake away; the hair is its groomed self again.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(data(&h).baked.is_none());
    h.state_mut().run(Command::Hair(modelcraft_engine::HairCommand::Bake { id: hair }));
    palette(&mut h, "Clear Hair Bake");
    assert!(data(&h).baked.is_none());
}

#[test]
fn hair_exports_from_the_app_with_a_status_line() {
    let mut h = with_hair();
    let id = crate::hair_ui::target_hair(h.state()).unwrap();
    let dir = std::env::temp_dir().join(format!("mc-hair-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (glb, cache) = (dir.join("hair.glb"), dir.join("hair.mcpc"));
    crate::hair_ui::export_to(h.state_mut(), id, &glb, true);
    assert!(h.state().status.contains("strands of 8 points written"), "{}", h.state().status);
    crate::hair_ui::export_to(h.state_mut(), id, &cache, false);
    assert!(h.state().status.contains("1000 guides of 8 points written"), "{}", h.state().status);
    assert_eq!(&std::fs::read(&glb).unwrap()[0..4], b"glTF");
    assert_eq!(&std::fs::read(&cache).unwrap()[0..4], b"MCPC");
    crate::hair_ui::export_to(h.state_mut(), id, &dir.join("no/such/dir/x.glb"), true);
    assert!(h.state().status.contains("failed"), "{}", h.state().status);
    std::fs::remove_dir_all(&dir).ok();
}
