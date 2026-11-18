use std::sync::Arc;

use glam::{Quat, Vec3};
use modelcraft_anim::Curve;
use proptest::prelude::*;

use super::*;
use crate::rig::JOINT_RADIUS;
use crate::{ObjectKind, Scene};

fn group(s: &mut Scene, name: &str, at: Vec3, parent: Option<ObjectId>) -> ObjectId {
    let id = s.add(name, ObjectKind::Group, parent).unwrap();
    s.edit(id).unwrap().transform.translation = at;
    id
}

fn joint(s: &mut Scene, name: &str, at: Vec3, parent: Option<ObjectId>) -> ObjectId {
    let id = s.add(name, ObjectKind::Joint { radius: JOINT_RADIUS }, parent).unwrap();
    s.edit(id).unwrap().transform.translation = at;
    id
}

fn pos(s: &Scene, id: ObjectId) -> Vec3 {
    s.world_matrix(id).transform_point3(Vec3::ZERO)
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-3
}

fn sources(ids: &[ObjectId]) -> Vec<Source> {
    ids.iter().map(|i| Source::new(*i)).collect()
}

/// A chain of `n` joints one unit apart up the Y axis; the first is the root.
fn chain(s: &mut Scene, n: usize) -> Vec<ObjectId> {
    let mut ids: Vec<ObjectId> = Vec::new();
    for i in 0..n {
        let parent = ids.last().copied();
        ids.push(joint(s, &format!("J{i}"), if i == 0 { Vec3::ZERO } else { Vec3::Y }, parent));
    }
    ids
}

fn ik(goal: ObjectId, pole: Option<ObjectId>, bones: u32, solver: IkSolver) -> ConstraintKind {
    ConstraintKind::Ik { goal, pole, bones, solver, iterations: 60, tolerance: 1e-5 }
}

#[test]
fn point_orient_scale_parent_and_aim_follow_their_sources() {
    let mut s = Scene::new();
    let a = group(&mut s, "A", Vec3::new(2.0, 0.0, 0.0), None);
    let b = group(&mut s, "B", Vec3::new(0.0, 4.0, 0.0), None);
    let holder = group(&mut s, "Holder", Vec3::new(0.0, 0.0, 10.0), None);
    let t = group(&mut s, "T", Vec3::new(1.0, 1.0, 1.0), Some(holder));
    // Point: the weighted mean, in world space although the target has a moved parent.
    let mut k = ConstraintKind::Point { sources: sources(&[a, b]) };
    k.sources_mut().unwrap()[1].weight = 3.0;
    let point = s.constraints.add("p", t, k).unwrap();
    assert_eq!(s.solve_constraints(), 1);
    assert!(near(pos(&s, t), Vec3::new(0.5, 3.0, 0.0)), "{:?}", pos(&s, t));
    assert_eq!(s.constraints.authored(t).unwrap().translation, Vec3::ONE, "what the object had is kept");
    let rev = s.get(t).unwrap().rev;
    assert_eq!(s.solve_constraints(), 0, "solving again changes nothing");
    assert_eq!(s.get(t).unwrap().rev, rev);
    // Half weight: halfway from the authored position, however often it is solved.
    s.constraints.get_mut(point).unwrap().weight = 0.5;
    s.solve_constraints();
    s.solve_constraints();
    assert!(near(pos(&s, t), (Vec3::new(1.0, 1.0, 11.0) + Vec3::new(0.5, 3.0, 0.0)) * 0.5), "{:?}", pos(&s, t));
    // Moving a source moves the target; switching the constraint off puts the target back.
    s.edit(a).unwrap().transform.translation.x = 6.0;
    s.constraints.get_mut(point).unwrap().weight = 1.0;
    s.solve_constraints();
    assert!(near(pos(&s, t), Vec3::new(1.5, 3.0, 0.0)));
    s.constraints.get_mut(point).unwrap().enabled = false;
    s.solve_constraints();
    assert_eq!(s.get(t).unwrap().transform.translation, Vec3::ONE);
    assert!(s.constraints.authored(t).is_none());
    s.constraints.remove(point);

    // Orient and scale.
    s.edit(a).unwrap().transform.rotation = Quat::from_rotation_z(1.0);
    s.edit(a).unwrap().transform.scale = Vec3::splat(3.0);
    s.constraints.add("o", t, ConstraintKind::Orient { sources: sources(&[a]) }).unwrap();
    s.constraints.add("s", t, ConstraintKind::Scale { sources: sources(&[a]) }).unwrap();
    s.solve_constraints();
    let tt = s.get(t).unwrap().transform;
    assert!(tt.rotation.angle_between(Quat::from_rotation_z(1.0)) < 1e-4 && near(tt.scale, Vec3::splat(3.0)));
    assert_eq!(tt.translation, Vec3::ONE, "other channels are left alone");
    let ids: Vec<u32> = s.constraints.list().iter().map(|c| c.id).collect();
    for id in ids {
        s.constraints.remove(id);
    }
    s.solve_constraints();
    assert_eq!(s.get(t).unwrap().transform, Transform { translation: Vec3::ONE, ..Transform::default() });

    // Parent with a kept offset: nothing moves until the source does, then the target swings.
    s.edit(a).unwrap().transform = Transform { translation: Vec3::new(2.0, 0.0, 0.0), ..Transform::default() };
    let before = pos(&s, t);
    let mut k = ConstraintKind::Parent { sources: sources(&[a]) };
    s.keep_offsets(t, &mut k);
    s.constraints.add("parent", t, k).unwrap();
    s.solve_constraints();
    assert!(near(pos(&s, t), before));
    s.edit(a).unwrap().transform.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    s.solve_constraints();
    // T was at (−1, 1, 11) from A; a quarter turn about Y takes (x, z) to (z, −x).
    assert!(near(pos(&s, t), Vec3::new(2.0 + 11.0, 1.0, 1.0)), "{:?}", pos(&s, t));
    let id = s.constraints.list()[0].id;
    s.constraints.remove(id);
    s.solve_constraints();

    // Aim: the Y axis looks at the source, Z stays as near to world up as it can.
    s.edit(holder).unwrap().transform.translation = Vec3::ZERO;
    s.edit(t).unwrap().transform = Transform::default();
    s.edit(a).unwrap().transform = Transform { translation: Vec3::new(5.0, 0.0, 0.0), ..Transform::default() };
    let aim = ConstraintKind::Aim { sources: sources(&[a]), aim: 1, up: 2, up_object: None, up_vector: Vec3::Y, offset: Quat::IDENTITY };
    s.constraints.add("aim", t, aim).unwrap();
    s.solve_constraints();
    let r = s.get(t).unwrap().transform.rotation;
    assert!(near(r * Vec3::Y, Vec3::X) && near(r * Vec3::Z, Vec3::Y), "{:?} {:?}", r * Vec3::Y, r * Vec3::Z);
    assert!(s.constraints.problems().is_empty());
    // On top of its target there is no direction: reported, not NaN.
    s.edit(a).unwrap().transform.translation = Vec3::ZERO;
    s.solve_constraints();
    assert_eq!(s.constraints.problems().len(), 1);
    assert!(s.get(t).unwrap().transform.rotation.is_finite());
}

#[test]
fn ik_reaches_blends_with_fk_and_follows_a_pole() {
    let mut s = Scene::new();
    let j = chain(&mut s, 3);
    let goal = group(&mut s, "Goal", Vec3::new(1.0, 1.0, 0.0), None);
    let pole = group(&mut s, "Pole", Vec3::new(0.0, 0.0, 5.0), None);
    let id = s.constraints.add("ik", j[2], ik(goal, Some(pole), 2, IkSolver::Auto)).unwrap();
    s.solve_constraints();
    assert!(near(pos(&s, j[2]), Vec3::new(1.0, 1.0, 0.0)), "{:?}", pos(&s, j[2]));
    assert!(pos(&s, j[1]).z > 0.5, "the middle joint bends towards the pole: {:?}", pos(&s, j[1]));
    assert!(((pos(&s, j[1]) - pos(&s, j[0])).length() - 1.0).abs() < 1e-4, "bones keep their length");
    assert_eq!(s.get(j[2]).unwrap().transform, Transform { translation: Vec3::Y, ..Transform::default() }, "the end joint itself is not turned");
    let solved = [s.get(j[0]).unwrap().transform.rotation, s.get(j[1]).unwrap().transform.rotation];
    assert_eq!(s.solve_constraints(), 0);
    // Blend 0 is the authored (FK) pose, in between is in between, and it does not creep.
    s.constraints.get_mut(id).unwrap().weight = 0.0;
    s.solve_constraints();
    assert!(near(pos(&s, j[2]), Vec3::Y * 2.0));
    s.constraints.get_mut(id).unwrap().weight = 0.5;
    for _ in 0..5 {
        s.solve_constraints();
    }
    let half = s.get(j[0]).unwrap().transform.rotation;
    assert!((half.angle_between(Quat::IDENTITY) - solved[0].angle_between(Quat::IDENTITY) * 0.5).abs() < 1e-3);
    // Posing the root by hand while blended becomes the new FK pose.
    s.constraints.get_mut(id).unwrap().weight = 0.0;
    s.solve_constraints();
    s.edit(j[0]).unwrap().transform.rotation = Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2);
    s.solve_constraints();
    assert!(near(pos(&s, j[2]), Vec3::X * 2.0), "{:?}", pos(&s, j[2]));
    // Moving the goal out of reach straightens the chain towards it.
    s.constraints.get_mut(id).unwrap().weight = 1.0;
    s.edit(goal).unwrap().transform.translation = Vec3::new(0.0, 0.0, -9.0);
    s.solve_constraints();
    assert!(near(pos(&s, j[2]), Vec3::new(0.0, 0.0, -2.0)), "{:?}", pos(&s, j[2]));

    // Longer chains with both iterative solvers, under a moved and turned parent.
    for solver in [IkSolver::Fabrik, IkSolver::Ccd, IkSolver::Auto] {
        let mut s = Scene::new();
        let holder = group(&mut s, "Holder", Vec3::new(3.0, 0.0, 0.0), None);
        s.edit(holder).unwrap().transform.rotation = Quat::from_rotation_x(0.7);
        let j = chain(&mut s, 6);
        s.reparent(j[0], Some(holder)).unwrap();
        let want = pos(&s, j[0]) + Vec3::new(1.5, 2.0, 1.0);
        let goal = group(&mut s, "Goal", want, None);
        s.constraints.add("ik", j[5], ik(goal, None, 5, solver)).unwrap();
        s.solve_constraints();
        assert!((pos(&s, j[5]) - want).length() < 2e-2, "{solver:?}: {:?} wanted {want:?}", pos(&s, j[5]));
        for w in j.windows(2) {
            assert!(((pos(&s, w[1]) - pos(&s, w[0])).length() - 1.0).abs() < 1e-3, "{solver:?}: bone length");
        }
        // Only the three joints the constraint names turn when the chain is shorter.
        let mut s2 = Scene::new();
        let j = chain(&mut s2, 6);
        let goal = group(&mut s2, "Goal", Vec3::new(1.0, 4.0, 0.0), None);
        s2.constraints.add("ik", j[5], ik(goal, None, 3, solver)).unwrap();
        s2.solve_constraints();
        assert_eq!(s2.get(j[1]).unwrap().transform.rotation, Quat::IDENTITY);
        assert!(near(pos(&s2, j[5]), Vec3::new(1.0, 4.0, 0.0)), "{solver:?}: {:?}", pos(&s2, j[5]));
    }
    // A chain that is not there is a problem, not a panic.
    let mut s = Scene::new();
    let lone = joint(&mut s, "Lone", Vec3::ZERO, None);
    let goal = group(&mut s, "Goal", Vec3::X, None);
    s.constraints.add("ik", lone, ik(goal, None, 2, IkSolver::Auto)).unwrap();
    s.solve_constraints();
    assert_eq!(s.constraints.problems().len(), 1);
}

#[test]
fn constraints_run_in_dependency_order_whatever_their_list_order() {
    let mut s = Scene::new();
    let c = group(&mut s, "C", Vec3::new(0.0, 5.0, 0.0), None);
    let b = group(&mut s, "B", Vec3::ZERO, None);
    let a = group(&mut s, "A", Vec3::ZERO, None);
    let j = chain(&mut s, 3);
    // Listed last-first: the IK reaches for A, A follows B, B follows C.
    s.constraints.add("ik", j[2], ik(a, None, 2, IkSolver::Auto)).unwrap();
    s.constraints.add("a to b", a, ConstraintKind::Point { sources: sources(&[b]) }).unwrap();
    s.constraints.add("b to c", b, ConstraintKind::Point { sources: sources(&[c]) }).unwrap();
    let (order, cyclic) = s.constraint_order();
    assert_eq!((order, cyclic), (vec![2, 1, 0], vec![]));
    s.edit(c).unwrap().transform.translation = Vec3::new(1.0, 1.0, 0.0);
    s.solve_constraints();
    assert!(near(pos(&s, a), Vec3::new(1.0, 1.0, 0.0)));
    assert!(near(pos(&s, j[2]), Vec3::new(1.0, 1.0, 0.0)), "one solve carries the move through all three: {:?}", pos(&s, j[2]));
    // A source under a constrained parent waits for the parent's constraint too.
    let child = group(&mut s, "Child", Vec3::X, Some(b));
    let follower = group(&mut s, "Follower", Vec3::ZERO, None);
    let f = s.constraints.add("follow child", follower, ConstraintKind::Point { sources: sources(&[child]) }).unwrap();
    // Put it first in the list.
    let moved = s.constraints.remove(f).unwrap();
    s.constraints.list.insert(0, moved);
    s.edit(c).unwrap().transform.translation = Vec3::new(0.0, 7.0, 0.0);
    s.solve_constraints();
    assert!(near(pos(&s, follower), Vec3::new(1.0, 7.0, 0.0)), "{:?}", pos(&s, follower));
}

#[test]
fn cycles_are_reported_and_skipped_and_the_rest_still_runs() {
    let mut s = Scene::new();
    let a = group(&mut s, "A", Vec3::X, None);
    let b = group(&mut s, "B", Vec3::Y, None);
    let c = group(&mut s, "C", Vec3::Z, None);
    let free = group(&mut s, "Free", Vec3::ZERO, None);
    let x = s.constraints.add("a to b", a, ConstraintKind::Point { sources: sources(&[b]) }).unwrap();
    let y = s.constraints.add("b to a", b, ConstraintKind::Point { sources: sources(&[a]) }).unwrap();
    // Downstream of the cycle: skipped as well. Independent of it: runs.
    let z = s.constraints.add("c to a", c, ConstraintKind::Point { sources: sources(&[a]) }).unwrap();
    let d = group(&mut s, "D", Vec3::ZERO, None);
    s.constraints.add("free to d", free, ConstraintKind::Orient { sources: sources(&[d]) }).unwrap();
    s.edit(d).unwrap().transform.rotation = Quat::from_rotation_x(1.0);
    s.solve_constraints();
    let flagged: Vec<u32> = s.constraints.problems().iter().map(|p| p.constraint).collect();
    assert_eq!(flagged, vec![x, y, z]);
    assert!(s.constraints.problems()[0].message.contains("cycle"));
    assert_eq!((pos(&s, a), pos(&s, b), pos(&s, c)), (Vec3::X, Vec3::Y, Vec3::Z), "nothing in the cycle moved");
    assert!(s.get(free).unwrap().transform.rotation.angle_between(Quat::from_rotation_x(1.0)) < 1e-4);
    // Breaking the cycle heals it on the next solve.
    s.constraints.get_mut(y).unwrap().enabled = false;
    s.solve_constraints();
    assert!(s.constraints.problems().is_empty());
    assert_eq!((pos(&s, a), pos(&s, c)), (Vec3::Y, Vec3::Y));

    // A cycle through a parent: constraining a parent to its own child.
    let mut s = Scene::new();
    let p = group(&mut s, "P", Vec3::ZERO, None);
    let k = group(&mut s, "K", Vec3::X, Some(p));
    s.constraints.add("p to k", p, ConstraintKind::Point { sources: sources(&[k]) }).unwrap();
    s.solve_constraints();
    assert_eq!(s.constraints.problems().len(), 1);
    assert_eq!(pos(&s, p), Vec3::ZERO);
    // An expression that feeds itself, a driven key that reads what it writes, an IK whose goal
    // hangs on its own chain.
    let mut s = Scene::new();
    let o = group(&mut s, "O", Vec3::ZERO, None);
    s.constraints.add("grow", o, ConstraintKind::Expression { path: "translate.x".into(), expr: "self.translate.x + 1".into() }).unwrap();
    let curve = Arc::new(Curve::from_points(&[(0.0, 0.0), (1.0, 1.0)]));
    s.constraints.add("loop", o, ConstraintKind::Driven { path: "rotate.z".into(), driver: o, driver_path: "rotate.x".into(), curve }).unwrap();
    let j = chain(&mut s, 3);
    let goal = group(&mut s, "Goal", Vec3::X, Some(j[1]));
    s.constraints.add("ik", j[2], ik(goal, None, 2, IkSolver::Auto)).unwrap();
    for _ in 0..3 {
        s.solve_constraints();
    }
    assert_eq!(s.constraints.problems().len(), 3);
    assert_eq!(s.get(o).unwrap().transform, Transform::default());
    // Reading one channel and writing another of the same object is fine.
    let free = group(&mut s, "Free", Vec3::X, None);
    let ok = s.constraints.add("fine", free, ConstraintKind::Expression { path: "scale.y".into(), expr: "self.translate.x + 2".into() }).unwrap();
    s.solve_constraints();
    assert!(!s.constraints.problems().iter().any(|p| p.constraint == ok));
    assert_eq!(s.get(free).unwrap().transform.scale.y, 3.0);
}

#[test]
fn driven_keys_and_expressions_drive_attributes() {
    let mut s = Scene::new();
    let driver = group(&mut s, "Driver", Vec3::ZERO, None);
    let door = group(&mut s, "Door", Vec3::ZERO, None);
    let curve = Arc::new(Curve::from_points(&[(0.0, 0.0), (2.0, 90.0)]));
    s.constraints.add("open", door, ConstraintKind::Driven { path: "rotate.y".into(), driver, driver_path: "translate.x".into(), curve }).unwrap();
    s.constraints.add("bob", door, ConstraintKind::Expression { path: "translate.y".into(), expr: "sin(time * pi) * 2 + @1.translate.x".into() }).unwrap();
    // Chained: an expression reading what the driven key wrote, listed before... after it runs.
    let follower = group(&mut s, "Follower", Vec3::ZERO, None);
    let e = s.constraints.add("half", follower, ConstraintKind::Expression { path: "translate.z".into(), expr: "@2.rotate.y / 2".into() }).unwrap();
    let last = s.constraints.remove(e).unwrap();
    s.constraints.list.insert(0, last);
    assert_eq!(driver, ObjectId(1));
    s.edit(driver).unwrap().transform.translation.x = 1.0;
    s.anim.time.fps = 24.0;
    s.set_frame(12.0);
    let t = s.get(door).unwrap().transform;
    assert!((t.euler_degrees().y - 45.0).abs() < 1e-3, "{:?}", t.euler_degrees());
    assert!((t.translation.y - 3.0).abs() < 1e-5, "sin(0.5 s · π) · 2 + 1 = 3, not {}", t.translation.y);
    assert!((s.get(follower).unwrap().transform.translation.z - 22.5).abs() < 1e-3);
    assert!(s.constraints.problems().is_empty(), "{:?}", s.constraints.problems());
    s.set_frame(24.0);
    assert!((s.get(door).unwrap().transform.translation.y - 1.0).abs() < 1e-4);
    // Bad formulas and missing attributes are problems; the rest keeps running.
    s.constraints.add("bad", door, ConstraintKind::Expression { path: "translate.z".into(), expr: "1 +".into() }).unwrap();
    s.constraints.add("gone", door, ConstraintKind::Expression { path: "scale.x".into(), expr: "@99.translate.x".into() }).unwrap();
    s.constraints.add("no attr", door, ConstraintKind::Expression { path: "nothing.here".into(), expr: "1".into() }).unwrap();
    s.solve_constraints();
    assert_eq!(s.constraints.problems().len(), 3);
    assert!((s.get(door).unwrap().transform.euler_degrees().y - 45.0).abs() < 1e-3);
}

#[test]
fn curves_set_what_constraints_start_from_and_weights_can_be_keyed() {
    let mut s = Scene::new();
    let j = chain(&mut s, 3);
    let goal = group(&mut s, "Goal", Vec3::new(1.0, 1.0, 0.0), None);
    let id = s.constraints.add("ik", j[2], ik(goal, None, 2, IkSolver::Auto)).unwrap();
    s.solve_constraints();
    // The FK pose is animated: the root swings from 0° to −90° about Z over ten frames.
    s.constraints.get_mut(id).unwrap().weight = 0.0;
    s.solve_constraints();
    s.set_key(j[0], "rotate.z", 0.0, Some(0.0)).unwrap();
    s.set_key(j[0], "rotate.z", 10.0, Some(-90.0)).unwrap();
    // The blend is keyed on the end joint: FK at frame 0, IK from frame 10.
    let path = weight_path(id);
    assert_eq!(s.attr(j[2], &path), Some(0.0));
    assert_eq!(s.attr(j[0], &path), None, "the weight belongs to the constraint's target");
    s.set_key(j[2], &path, 0.0, Some(0.0)).unwrap();
    s.set_key(j[2], &path, 10.0, Some(1.0)).unwrap();
    s.set_frame(0.0);
    assert!(near(pos(&s, j[2]), Vec3::Y * 2.0), "{:?}", pos(&s, j[2]));
    s.set_frame(10.0);
    assert!(near(pos(&s, j[2]), Vec3::new(1.0, 1.0, 0.0)), "{:?}", pos(&s, j[2]));
    // Scrubbing back and forth never drifts: the same frame is the same pose.
    s.set_frame(5.0);
    let mid = pos(&s, j[2]);
    for f in [0.0, 10.0, 3.0, 5.0] {
        s.set_frame(f);
    }
    assert!((pos(&s, j[2]) - mid).length() < 1e-5);
    // With the IK off at frame 10 the joint shows its curve again.
    s.constraints.get_mut(id).unwrap().enabled = false;
    s.set_frame(10.0);
    assert!(near(pos(&s, j[2]), Vec3::X * 2.0), "{:?}", pos(&s, j[2]));
}

#[test]
fn constraints_are_saved_repaired_and_dropped_with_their_objects() {
    let mut s = Scene::new();
    let a = group(&mut s, "A", Vec3::X, None);
    let t = group(&mut s, "T", Vec3::ZERO, None);
    let j = chain(&mut s, 3);
    let mut k = ConstraintKind::Parent { sources: sources(&[a]) };
    s.keep_offsets(t, &mut k);
    s.constraints.add("parent", t, k).unwrap();
    let ik_id = s.constraints.add("ik", j[2], ik(a, Some(t), 2, IkSolver::Ccd)).unwrap();
    s.constraints.add("e", t, ConstraintKind::Expression { path: "scale.x".into(), expr: "frame + 1".into() }).unwrap();
    s.solve_constraints();
    let json = serde_json::to_string(&s).unwrap();
    let mut back = serde_json::from_str::<Scene>(&json).unwrap().repaired().unwrap();
    assert_eq!(back.constraints, s.constraints);
    // A scene from a file holds the solved pose; solving it again starts from the saved
    // authored transforms and changes nothing.
    assert_eq!(back.solve_constraints(), 0);
    for o in s.objects() {
        assert_eq!(back.get(o.id).unwrap().transform, o.transform);
    }
    // An older file without the block reads as no constraints.
    assert!(serde_json::from_str::<Scene>(r#"{"objects":[]}"#).unwrap().constraints.is_empty());
    assert!(!serde_json::to_string(&Scene::new()).unwrap().contains("constraints"));
    // Removing the pole keeps the IK; removing the goal removes it; removing a source removes
    // the constraint that has nothing left.
    s.remove(t);
    assert!(matches!(s.constraints.get(ik_id).unwrap().kind, ConstraintKind::Ik { pole: None, .. }));
    assert_eq!(s.constraints.list().len(), 1);
    s.remove(a);
    assert!(s.constraints.list().is_empty());
    assert_eq!(s.solve_constraints(), 2, "the joints go back to their own rotations");
    assert!(s.constraints.is_empty());

    // Hostile input.
    let json = r#"{"objects":[{"id":1,"name":"A","kind":{"type":"group"}},{"id":2,"name":"B","kind":{"type":"group"}}],
      "constraints":{"next_id":1,"authored":{"2":{"translation":[null,0,0],"rotation":[0,0,0,0],"scale":[0,0,0]},"77":{"translation":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]}},
      "list":[
        {"id":5,"name":"w","target":2,"weight":-3,"type":"aim","sources":[{"object":1,"weight":1e30},{"object":9}],"aim":7,"up":7,"up_vector":[0,0,0],"offset":[0,0,0,0]},
        {"id":5,"name":"dup","target":2,"type":"point","sources":[]},
        {"id":6,"name":"gone","target":42,"type":"point","sources":[{"object":1}]},
        {"id":7,"name":"ik","target":2,"type":"ik","goal":1,"bones":4000000000,"iterations":0,"tolerance":-1},
        {"id":8,"name":"self","target":2,"type":"point","sources":[{"object":2}]}
      ]}}"#;
    let parsed = serde_json::from_str::<Scene>(json);
    // A null number is refused by the reader; the rest must repair.
    assert!(parsed.is_err());
    let mut s = serde_json::from_str::<Scene>(&json.replace("null", "1e39")).unwrap().repaired().unwrap();
    let ids: Vec<u32> = s.constraints.list().iter().map(|c| c.id).collect();
    assert_eq!(ids, vec![5, 7, 8]);
    let c = s.constraints.get(5).unwrap();
    assert_eq!(c.weight, 0.0);
    assert!(matches!(&c.kind, ConstraintKind::Aim { aim: 1, up: 2, up_vector, offset, .. } if *up_vector == Vec3::Y && *offset == Quat::IDENTITY));
    assert!(matches!(s.constraints.get(7).unwrap().kind, ConstraintKind::Ik { bones, iterations: 1, .. } if (bones as usize) < MAX_DEPTH));
    assert!(s.constraints.authored(ObjectId(77)).is_none());
    s.solve_constraints();
    assert!(s.objects().iter().all(|o| o.transform.translation.is_finite() && o.transform.rotation.is_finite()));
    let next = s.constraints.add("new", ObjectId(1), ConstraintKind::Point { sources: sources(&[ObjectId(2)]) }).unwrap();
    assert_eq!(next, 9, "ids are never reused");
}

fn kind_strategy(n: u64) -> impl Strategy<Value = (u64, ConstraintKind)> {
    let id = move || (1..=n).prop_map(ObjectId);
    let srcs = move || {
        proptest::collection::vec((id(), 0.0f32..2.0), 1..3)
            .prop_map(|v| v.into_iter().map(|(object, weight)| Source { weight, ..Source::new(object) }).collect::<Vec<_>>())
    };
    let kind = prop_oneof![
        srcs().prop_map(|sources| ConstraintKind::Point { sources }),
        srcs().prop_map(|sources| ConstraintKind::Orient { sources }),
        srcs().prop_map(|sources| ConstraintKind::Scale { sources }),
        srcs().prop_map(|sources| ConstraintKind::Parent { sources }),
        (srcs(), 0u8..3, 0u8..3).prop_map(|(sources, aim, up)| ConstraintKind::Aim {
            sources,
            aim,
            up,
            up_object: None,
            up_vector: Vec3::Y,
            offset: Quat::IDENTITY
        }),
        (id(), proptest::option::of(id()), 1u32..5, 0usize..4).prop_map(|(goal, pole, bones, s)| ConstraintKind::Ik {
            goal,
            pole,
            bones,
            solver: IkSolver::ALL[s],
            iterations: 8,
            tolerance: 1e-3
        }),
        (id(), 0usize..9, 0usize..9).prop_map(|(driver, a, b)| ConstraintKind::Driven {
            path: crate::anim::TRANSFORM_PATHS[a].into(),
            driver,
            driver_path: crate::anim::TRANSFORM_PATHS[b].into(),
            curve: Arc::new(Curve::from_points(&[(0.0, 0.5), (1.0, 2.0)])),
        }),
        (0usize..9, id(), 0usize..9).prop_map(|(a, o, b)| ConstraintKind::Expression {
            path: crate::anim::TRANSFORM_PATHS[a].into(),
            expr: format!("@{}.{} * 0.5 + frame", o.0, crate::anim::TRANSFORM_PATHS[b])
        }),
    ];
    (1..=n, kind)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Any tangle of constraints over a small tree solves to finite transforms, and solving
    /// again (or scrubbing away and back) gives the same pose: nothing accumulates.
    #[test]
    fn any_constraints_solve_finitely_and_repeatably(
        parents in proptest::collection::vec(proptest::option::of(0usize..8), 8),
        places in proptest::collection::vec((-3.0f32..3.0, -3.0f32..3.0, -3.0f32..3.0), 8),
        kinds in proptest::collection::vec(kind_strategy(8), 0..10),
        weights in proptest::collection::vec(0.0f32..1.0, 10),
    ) {
        let mut s = Scene::new();
        for (i, (p, at)) in parents.iter().zip(&places).enumerate() {
            // Parents come earlier in the list, so the tree has no cycles.
            let parent = p.filter(|p| *p < i).map(|p| ObjectId(p as u64 + 1));
            joint(&mut s, &format!("O{i}"), Vec3::new(at.0, at.1, at.2), parent);
        }
        for ((target, kind), w) in kinds.into_iter().zip(&weights) {
            let id = s.constraints.add("c", ObjectId(target), kind).unwrap();
            s.constraints.get_mut(id).unwrap().weight = *w;
        }
        s.solve_constraints();
        let first: Vec<Transform> = s.objects().iter().map(|o| o.transform).collect();
        for t in &first {
            prop_assert!(t.translation.is_finite() && t.rotation.is_finite() && t.scale.is_finite());
        }
        s.solve_constraints();
        s.set_frame(0.0);
        let again: Vec<Transform> = s.objects().iter().map(|o| o.transform).collect();
        for (a, b) in first.iter().zip(&again) {
            prop_assert!((a.translation - b.translation).length() < 1e-4 && a.rotation.dot(b.rotation).abs() > 1.0 - 1e-5 && (a.scale - b.scale).length() < 1e-4, "{a:?} became {b:?}");
        }
        // Every skipped constraint says why; the order holds each enabled constraint at most once.
        let (order, cyclic) = s.constraint_order();
        prop_assert_eq!(order.len() + cyclic.len(), s.constraints.list().len());
    }
}
