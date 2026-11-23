//! kittest tests of the pen tool and point editing: the pointer paths (with real pointer events and
//! through the functions they call), the keyboard paths and the numeric Properties list.

use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use glam::Vec3;
use modelcraft_engine::curves::{CurveKind, HandleMode, KnotRef, Spline};
use modelcraft_engine::scene::splines::SplineSource;
use modelcraft_engine::scene::{ObjectId, ObjectKind};

use crate::app::ModelApp;
use crate::args::Cli;
use crate::spline_pen;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).with_step_dt(0.02).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn palette(h: &mut Harness<'static, ModelApp>, text: &str) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.event(Event::Text(text.to_string()));
    h.run();
    h.key_press(Key::Enter);
    h.run();
}

fn focused_label(h: &Harness<'static, ModelApp>) -> Option<String> {
    h.root().query_all(egui_kittest::kittest::by()).find(|n| n.is_focused()).and_then(|n| n.accesskit_node().label())
}

fn spline(h: &mut Harness<'static, ModelApp>, id: ObjectId) -> Spline {
    let s = h.state_mut();
    (*s.doc.eval.spline(&s.doc.scene, id).unwrap().spline).clone()
}

/// The viewport's canvas rectangle on screen.
fn viewport_rect(h: &Harness<'static, ModelApp>) -> Rect {
    let canvas = h.root().query_all(egui_kittest::kittest::by().role(Role::Canvas)).next().expect("the viewport");
    canvas.rect()
}

fn screen_of(h: &Harness<'static, ModelApp>, world: Vec3) -> Pos2 {
    let rect = viewport_rect(h);
    crate::viewport::to_screen(&h.state().views[0].camera, rect, world).expect("on screen")
}

fn pointer(h: &mut Harness<'static, ModelApp>, kind: &str, at: Pos2) {
    match kind {
        "move" => h.event(Event::PointerMoved(at)),
        "down" => h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }),
        _ => h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }),
    }
    h.run();
}

fn click(h: &mut Harness<'static, ModelApp>, at: Pos2) {
    pointer(h, "move", at);
    pointer(h, "down", at);
    pointer(h, "up", at);
}

#[test]
fn the_pen_draws_a_curve_from_clicks_through_real_pointer_events() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Pen Tool");
    assert!(h.state().spline.pen_on());
    let pts = [Vec3::new(-2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0), Vec3::new(2.0, 0.0, -1.0)];
    for p in pts {
        let at = screen_of(&h, p);
        click(&mut h, at);
    }
    h.run();
    let id = h.state().selection[0];
    let s = spline(&mut h, id);
    assert_eq!((s.segments.len(), s.segments[0].knots.len()), (1, 3));
    for (k, want) in s.segments[0].knots.iter().zip(pts) {
        assert!(k.pos.distance(want) < 0.05, "{:?} vs {want:?}", k.pos);
    }
    assert!(!s.segments[0].closed);
    // One undo step for each point (the first also made the spline).
    let n = h.state().doc.undo.len();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(h.state().doc.undo.len(), n - 1);
    assert_eq!(spline(&mut h, id).segments[0].knots.len(), 2);
}

#[test]
fn dragging_a_pen_click_pulls_out_a_smooth_handle_and_clicking_the_first_point_closes() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Pen Tool");
    let a = screen_of(&h, Vec3::new(-2.0, 0.0, 0.0));
    click(&mut h, a);
    let b = screen_of(&h, Vec3::new(2.0, 0.0, 0.0));
    pointer(&mut h, "move", b);
    pointer(&mut h, "down", b);
    pointer(&mut h, "move", b + egui::vec2(40.0, 0.0));
    pointer(&mut h, "up", b + egui::vec2(40.0, 0.0));
    let c = screen_of(&h, Vec3::new(0.0, 0.0, 3.0));
    click(&mut h, c);
    let id = h.state().selection[0];
    let s = spline(&mut h, id);
    let mid = s.segments[0].knots[1];
    assert_eq!(mid.mode, HandleMode::Mirrored);
    assert!(mid.tan_out.length() > 0.1 && (mid.tan_in + mid.tan_out).length() < 1e-4, "{mid:?}");
    assert!(mid.tan_out.x > 0.0, "the handle points the way it was pulled");
    assert_eq!(s.segments[0].knots[0].tan_out, Vec3::ZERO, "a plain click is a corner");
    // The first point again: the segment closes and no point is added.
    click(&mut h, a);
    let s = spline(&mut h, id);
    assert!(s.segments[0].closed);
    assert_eq!(s.segments[0].knots.len(), 3);
}

#[test]
fn enter_ends_a_curve_backspace_takes_a_point_back_and_escape_stops_the_pen() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Pen Tool");
    let rect = viewport_rect(&h);
    let place = |h: &mut Harness<'static, ModelApp>, p: Vec3| {
        let at = crate::viewport::to_screen(&h.state().views[0].camera, rect, p).unwrap();
        assert!(spline_pen::pen_place(h.state_mut(), 0, rect, at, at));
    };
    place(&mut h, Vec3::new(-1.0, 0.0, 0.0));
    place(&mut h, Vec3::new(1.0, 0.0, 0.0));
    let id = h.state().selection[0];
    assert_eq!(spline(&mut h, id).segments[0].knots.len(), 2);
    spline_pen::pen_undo_point(h.state_mut());
    assert_eq!(spline(&mut h, id).segments[0].knots.len(), 1);
    place(&mut h, Vec3::new(1.0, 0.0, 1.0));
    // Enter, with the viewport focused: the next point starts a second curve in the same spline.
    h.get_by_role(Role::Canvas).focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    place(&mut h, Vec3::new(0.0, 0.0, 2.0));
    let s = spline(&mut h, id);
    assert_eq!(s.segments.len(), 2);
    assert_eq!((s.segments[0].knots.len(), s.segments[1].knots.len()), (2, 1));
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.state().spline.pen_on());
}

#[test]
fn the_pen_continues_an_editable_spline_and_leaves_a_primitive_alone() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Circle Spline");
    let circle = h.state().selection[0];
    palette(&mut h, "Pen Tool");
    let rect = viewport_rect(&h);
    let at = crate::viewport::to_screen(&h.state().views[0].camera, rect, Vec3::new(0.0, 0.0, 3.0)).unwrap();
    assert!(spline_pen::pen_place(h.state_mut(), 0, rect, at, at));
    let drawn = h.state().selection[0];
    assert_ne!(drawn, circle, "a primitive is not drawn into: a new spline starts");
    assert_eq!(spline(&mut h, circle).knot_count(), 4);
    // Switch the pen off and on over the new, editable spline: it continues it.
    palette(&mut h, "Pen Tool");
    assert!(!h.state().spline.pen_on());
    palette(&mut h, "Pen Tool");
    let at2 = crate::viewport::to_screen(&h.state().views[0].camera, rect, Vec3::new(1.0, 0.0, 3.0)).unwrap();
    assert!(spline_pen::pen_place(h.state_mut(), 0, rect, at2, at2));
    assert_eq!(spline(&mut h, drawn).segments[0].knots.len(), 2);
    assert_eq!(h.state().doc.scene.objects().iter().filter(|o| matches!(o.kind, ObjectKind::Spline { .. })).count(), 2);
}

#[test]
fn dragging_a_point_moves_it_in_one_undo_step_and_a_handle_changes_the_curve() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Rectangle Spline");
    let id = h.state().selection[0];
    h.run();
    let before = h.state().doc.undo.len();
    // The corner at (1, 0.75, 0) is a point: grab it and pull it to the right.
    let corner = Vec3::new(1.0, 0.75, 0.0);
    let from = screen_of(&h, corner);
    pointer(&mut h, "move", from);
    pointer(&mut h, "down", from);
    for step in 1..=5 {
        pointer(&mut h, "move", from + egui::vec2(12.0 * step as f32, 0.0));
    }
    // The preview shows while the button is down, with no undo step yet.
    assert_eq!(h.state().doc.undo.len(), before);
    pointer(&mut h, "up", from + egui::vec2(60.0, 0.0));
    h.run();
    assert_eq!(h.state().doc.undo.len(), before + 1, "one step for the whole drag");
    let s = spline(&mut h, id);
    let moved = s.segments[0].knots.iter().find(|k| k.pos.x > 1.2).expect("a point moved right");
    // The drag is in the view's plane, so depth may drift a little; height stays.
    assert!((moved.pos.y - 0.75).abs() < 0.05, "{:?}", moved.pos);
    // The primitive became editable points.
    assert!(matches!(h.state().doc.scene.get(id).unwrap().kind, ObjectKind::Spline { source: SplineSource::Curve { .. }, .. }));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert!(spline(&mut h, id).segments[0].knots.iter().all(|k| k.pos.x <= 1.0 + 1e-4));
}

#[test]
fn a_bezier_handle_drags_and_the_double_click_inserts_a_point() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Circle Spline");
    let id = h.state().selection[0];
    h.run();
    // The circle's handle at the right-hand point leaves upwards, length 0.5523.
    let tip = Vec3::new(1.0, 0.552_284_8, 0.0);
    let from = screen_of(&h, tip);
    pointer(&mut h, "move", from);
    pointer(&mut h, "down", from);
    pointer(&mut h, "move", from + egui::vec2(0.0, -20.0));
    pointer(&mut h, "move", from + egui::vec2(0.0, -40.0));
    pointer(&mut h, "up", from + egui::vec2(0.0, -40.0));
    h.run();
    let s = spline(&mut h, id);
    let k0 = s.segments[0].knots[0];
    assert!(k0.tan_out.y > 0.7, "the handle grew: {:?}", k0.tan_out);
    assert_eq!(k0.mode, HandleMode::Mirrored);
    assert!((k0.tan_in + k0.tan_out).length() < 1e-4, "mirrored: the other side follows");
    // Double-click the curve between points: a point appears there.
    let curve_pt = {
        let s = spline(&mut h, id);
        let (p, _) = s.segments[0].sample(0.5);
        p
    };
    let at = screen_of(&h, curve_pt);
    click(&mut h, at);
    pointer(&mut h, "down", at);
    pointer(&mut h, "up", at);
    let s = spline(&mut h, id);
    assert_eq!(s.segments[0].knots.len(), 5, "a fifth point");
    assert!(s.segments[0].knots[1].pos.distance(curve_pt) < 0.06, "{:?} vs {curve_pt:?}", s.segments[0].knots[1].pos);
}

#[test]
fn clicking_a_point_selects_it_shift_adds_and_delete_removes_the_selected_points() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Rectangle Spline");
    let id = h.state().selection[0];
    h.run();
    let corners = [Vec3::new(1.0, -0.75, 0.0), Vec3::new(1.0, 0.75, 0.0), Vec3::new(-1.0, 0.75, 0.0)];
    let first = screen_of(&h, corners[0]);
    click(&mut h, first);
    assert_eq!(h.state().spline.selected.len(), 1);
    // Shift adds to the selection (the keyboard modifier is not part of the pointer events in tests).
    spline_pen::select_knot(h.state_mut(), KnotRef { segment: 0, knot: 2 }, true);
    assert_eq!(h.state().spline.selected.len(), 2, "after the call: {:?}", h.state().spline.selected);
    h.run();
    assert_eq!(h.state().spline.selected.len(), 2, "{:?}", h.state().spline.selected);
    // Delete with the viewport focused removes the points, not the object.
    h.get_by_role(Role::Canvas).focus();
    h.run();
    let before = h.state().doc.undo.len();
    h.key_press(Key::Delete);
    h.run();
    h.run();
    assert_eq!(h.state().doc.undo.len(), before + 1);
    assert_eq!(spline(&mut h, id).segments[0].knots.len(), 2);
    assert!(h.state().doc.scene.get(id).is_some());
    assert!(h.state().spline.selected.is_empty());
    // With no point selected, Delete deletes the object as before.
    h.key_press(Key::Delete);
    h.run();
    assert!(h.state().doc.scene.get(id).is_none());
}

#[test]
fn the_points_list_edits_numerically_and_by_keyboard() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Rectangle Spline");
    let id = h.state().selection[0];
    palette(&mut h, "Make Spline Editable");
    h.run();
    h.get_by_role_and_label(Role::ComboBox, "Curve type");
    h.get_by_role_and_label(Role::CheckBox, "Closed");
    // Pick the second point from the list with the keyboard.
    h.get_by_label("Point 2").focus();
    h.run();
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().spline.selected, vec![KnotRef { segment: 0, knot: 1 }]);
    h.run();
    let before = h.state().doc.undo.len();
    h.get_by_role_and_label(Role::SpinButton, "Point position X").set_value_accesskit(3.0);
    h.run();
    h.run();
    assert_eq!(h.state().doc.undo.len(), before + 1);
    assert!((spline(&mut h, id).segments[0].knots[1].pos.x - 3.0).abs() < 1e-4);
    // The curve type combo is a keyboard-driven picker: Linear -> Bezier.
    h.get_by_role_and_label(Role::ComboBox, "Curve type").focus();
    h.run();
    h.key_press(Key::Space);
    h.run();
    h.key_press(Key::ArrowDown);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Polyline"));
    h.key_press(Key::Tab);
    h.run();
    assert_eq!(focused_label(&h).as_deref(), Some("Bezier"));
    h.key_press(Key::Enter);
    h.run();
    h.run();
    assert_eq!(spline(&mut h, id).segments[0].kind, CurveKind::Bezier);
    assert!(h.get_by_role_and_label(Role::ComboBox, "Curve type").is_focused());
    // Handles show for a Bezier point; a handle edits through its fields.
    h.get_by_role_and_label(Role::SpinButton, "Handle out Y").set_value_accesskit(1.5);
    h.run();
    h.run();
    assert!((spline(&mut h, id).segments[0].knots[1].tan_out.y - 1.5).abs() < 1e-4);
    // Open the curve, then add, insert and delete points with buttons.
    h.get_by_role_and_label(Role::CheckBox, "Closed").click();
    h.run();
    assert!(!spline(&mut h, id).segments[0].closed);
    h.get_by_role_and_label(Role::Button, "Add Point").click();
    h.run();
    assert_eq!(spline(&mut h, id).segments[0].knots.len(), 5);
    h.get_by_role_and_label(Role::Button, "Insert Point").click();
    h.run();
    assert_eq!(spline(&mut h, id).segments[0].knots.len(), 6);
    h.get_by_role_and_label(Role::Button, "Delete Points").click();
    h.run();
    assert_eq!(spline(&mut h, id).segments[0].knots.len(), 5);
    h.get_by_role_and_label(Role::CheckBox, "Closed").click();
    h.run();
    assert!(spline(&mut h, id).segments[0].closed);
}

#[test]
fn points_are_not_drawn_for_other_objects_and_a_deleted_point_drops_out_of_the_selection() {
    let mut h = harness();
    h.run();
    palette(&mut h, "Add Rectangle Spline");
    let id = h.state().selection[0];
    h.state_mut().spline.selected = vec![KnotRef { segment: 0, knot: 3 }];
    h.state_mut().run(modelcraft_engine::Command::Spline(modelcraft_engine::SplineCommand::Edit {
        id,
        edit: modelcraft_engine::curves::SplineEdit::Remove { knots: vec![KnotRef { segment: 0, knot: 3 }] },
    }));
    h.run();
    assert!(h.state().spline.selected.is_empty(), "the removed point is not selected any more");
    palette(&mut h, "Add Cube");
    h.run();
    assert!(h.state().spline.selected.is_empty());
}
