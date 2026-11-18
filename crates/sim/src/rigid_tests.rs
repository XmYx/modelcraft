//! Rigid bodies: ground contact, energy and penetration bounds, bounce, friction, sleeping,
//! joints, animated passive bodies, mesh colliders and hostile input.

use glam::{Mat4, Vec3};

use crate::rigid::*;
use crate::{BakeConfig, FrameCache, Silent, SimError, SimTiming, Solver, StepContext, bake};

const FPS: f64 = 24.0;

/// The eight corners and twelve triangles of an axis-aligned box.
fn cuboid(centre: Vec3, half: Vec3) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let mut pts = Vec::new();
    for z in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for x in [-1.0, 1.0] {
                pts.push(centre + half * Vec3::new(x, y, z));
            }
        }
    }
    let quads = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
    let tris = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
    (pts, tris)
}

/// A rough sphere: the corners of a cube pushed onto a sphere plus the face centres.
fn ball(centre: Vec3, r: f32) -> Vec<Vec3> {
    let mut pts: Vec<Vec3> = Vec::new();
    for x in [-1.0f32, 0.0, 1.0] {
        for y in [-1.0f32, 0.0, 1.0] {
            for z in [-1.0f32, 0.0, 1.0] {
                let v = Vec3::new(x, y, z);
                if v != Vec3::ZERO {
                    pts.push(centre + v.normalize() * r);
                }
            }
        }
    }
    pts
}

fn body(id: u64, pts: Vec<Vec3>, tris: Vec<[u32; 3]>, settings: RigidSettings) -> BodyDesc {
    BodyDesc { id, points: pts, triangles: tris, settings, motion: Vec::new() }
}

fn active(id: u64, centre: Vec3, half: Vec3, s: RigidSettings) -> BodyDesc {
    let (p, t) = cuboid(centre, half);
    body(id, p, t, s)
}

fn world(bodies: Vec<BodyDesc>, ground: Option<f32>) -> RigidWorld {
    RigidWorld::new(WorldDesc { gravity: Vec3::new(0.0, -9.81, 0.0), ground, bodies }).unwrap()
}

fn timing(frames: u32, substeps: u32) -> SimTiming {
    SimTiming { start_frame: 1.0, end_frame: f64::from(frames), fps: FPS, substeps }
}

fn run(w: &mut RigidWorld, frames: u32, substeps: u32) -> FrameCache {
    bake(w, &BakeConfig { timing: timing(frames, substeps), seed: 1, ..BakeConfig::default() }, &mut Silent).unwrap()
}

/// The lowest, highest and mean height of a body's points at frame `f`.
fn extent(c: &FrameCache, f: usize, range: std::ops::Range<usize>) -> (f32, f32, Vec3) {
    let pts = &c.frame(f).unwrap()[range];
    let lo = pts.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let hi = pts.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    (lo, hi, pts.iter().copied().sum::<Vec3>() / pts.len() as f32)
}

/// Step by hand, recording the world's energy after every frame.
fn energies(w: &mut RigidWorld, frames: u32, substeps: u32) -> Vec<f32> {
    w.reset(1);
    let dt = timing(frames, substeps).dt();
    let mut out = vec![w.mechanical_energy()];
    for f in 1..frames {
        w.begin_frame(f64::from(f) + 1.0);
        for s in 0..substeps {
            w.step(&StepContext { dt, substep: s, substeps, frame: f64::from(f) + 1.0 });
        }
        out.push(w.mechanical_energy());
    }
    out
}

#[test]
fn a_dropped_box_lands_on_the_ground_stops_there_and_goes_to_sleep() {
    let mut w = world(vec![active(7, Vec3::new(0.0, 3.0, 0.0), Vec3::splat(0.5), RigidSettings { bounciness: 0.0, ..Default::default() })], Some(0.0));
    let outs = w.outputs();
    assert_eq!(outs, vec![(7, 0..8)]);
    let cache = run(&mut w, 150, 4);
    let (lo0, _, _) = extent(&cache, 0, 0..8);
    assert!((lo0 - 2.5).abs() < 1e-5, "frame 1 is the rest state");
    let (lo, hi, mean) = extent(&cache, 149, 0..8);
    assert!(lo > -0.03 && lo < 0.03, "resting on the ground, not in it or above it: {lo}");
    assert!((hi - lo - 1.0).abs() < 0.02, "the box kept its size: {}", hi - lo);
    assert!(mean.x.abs() < 0.05 && mean.z.abs() < 0.05, "it fell straight: {mean:?}");
    // Penetration never exceeded a few centimetres at any frame.
    let deepest = (0..cache.frames()).map(|f| extent(&cache, f, 0..8).0).fold(f32::MAX, f32::min);
    assert!(deepest > -0.05, "deepest penetration {deepest}");
    assert_eq!(w.sleeping(), 1, "settled bodies sleep");
    // A body that is not allowed to sleep stays awake.
    let mut awake = world(vec![active(7, Vec3::new(0.0, 1.0, 0.0), Vec3::splat(0.5), RigidSettings { can_sleep: false, ..Default::default() })], Some(0.0));
    run(&mut awake, 150, 4);
    assert_eq!(awake.sleeping(), 0);
}

#[test]
fn energy_is_never_gained_and_a_bounce_keeps_only_its_share() {
    let ball_body = |e: f32| {
        body(
            1,
            ball(Vec3::new(0.0, 4.0, 0.0), 0.3),
            vec![],
            RigidSettings { shape: ShapeKind::Sphere, bounciness: e, linear_damping: 0.0, angular_damping: 0.0, ..Default::default() },
        )
    };
    for e in [0.0, 0.5, 0.9] {
        let mut w = world(vec![ball_body(e)], Some(0.0));
        let en = energies(&mut w, 120, 8);
        let e0 = en[0];
        let worst = en.iter().map(|x| x - e0).fold(f32::MIN, f32::max);
        // Potential energy is measured from y = 0, so the scale is m·g·h ≈ 9.81·3.7.
        assert!(worst < 0.05 * e0.abs(), "restitution {e}: energy grew by {worst} from {e0}");
    }
    // Apex heights: the more bounce, the higher the ball goes after the first hit.
    let apex_after_first_bounce = |e: f32| {
        let mut w = world(vec![ball_body(e)], Some(0.0));
        let c = run(&mut w, 120, 8);
        let ys: Vec<f32> = (0..c.frames()).map(|f| extent(&c, f, 0..26).2.y).collect();
        let hit = ys.iter().position(|y| *y < 0.4).unwrap_or(0);
        ys[hit..].iter().copied().fold(0.0, f32::max)
    };
    let (dead, lively) = (apex_after_first_bounce(0.0), apex_after_first_bounce(0.9));
    assert!(lively > dead + 0.5, "a bouncy ball comes back up: {dead} vs {lively}");
    assert!(lively < 4.0, "and never higher than it was dropped from: {lively}");
}

#[test]
fn a_stack_of_boxes_does_not_sink_into_itself() {
    let boxes: Vec<BodyDesc> = (0..4)
        .map(|i| active(i, Vec3::new(0.0, 0.5 + 1.05 * i as f32, 0.0), Vec3::splat(0.5), RigidSettings { bounciness: 0.0, ..Default::default() }))
        .collect();
    let mut w = world(boxes, Some(0.0));
    let outs = w.outputs();
    let cache = run(&mut w, 96, 4);
    let mut worst = 0.0f32;
    for f in 0..cache.frames() {
        let ext: Vec<(f32, f32, Vec3)> = outs.iter().map(|(_, r)| extent(&cache, f, r.clone())).collect();
        worst = worst.max(-ext[0].0);
        for pair in ext.windows(2) {
            // How far the upper box's bottom is below the lower box's top.
            worst = worst.max(pair[0].1 - pair[1].0);
        }
    }
    assert!(worst < 0.05, "deepest overlap in the stack {worst}");
    let top = extent(&cache, 95, outs[3].1.clone());
    assert!((top.1 - 4.0).abs() < 0.15, "the stack still stands four boxes high: {}", top.1);
}

#[test]
fn a_fast_ball_does_not_tunnel_through_a_thin_wall() {
    let (wp, wt) = cuboid(Vec3::new(5.0, 1.0, 0.0), Vec3::new(0.05, 2.0, 3.0));
    let wall = body(2, wp, wt, RigidSettings::passive());
    let shot = body(
        1,
        ball(Vec3::new(0.0, 1.0, 0.0), 0.2),
        vec![],
        RigidSettings { shape: ShapeKind::Sphere, velocity: [60.0, 0.0, 0.0], gravity: [0.0; 3], ..Default::default() },
    );
    let mut w = world(vec![shot, wall], None);
    let cache = run(&mut w, 48, 2);
    let max_x = (0..cache.frames()).map(|f| extent_x(&cache, f)).fold(f32::MIN, f32::max);
    assert!(max_x < 5.0, "the ball stayed on the near side: {max_x}");
}

fn extent_x(c: &FrameCache, f: usize) -> f32 {
    c.frame(f).unwrap().iter().map(|p| p.x).fold(f32::MIN, f32::max)
}

#[test]
fn friction_slows_a_sliding_box_and_more_friction_stops_it_sooner() {
    let slide = |friction: f32| {
        let s = RigidSettings { friction, bounciness: 0.0, velocity: [6.0, 0.0, 0.0], ..Default::default() };
        let mut w = world(vec![active(1, Vec3::new(0.0, 0.5, 0.0), Vec3::splat(0.5), s)], Some(0.0));
        let c = run(&mut w, 120, 4);
        extent(&c, 119, 0..8).2.x
    };
    let (slippery, grippy) = (slide(0.05), slide(1.0));
    assert!(grippy < slippery - 0.5, "stops sooner with friction: {grippy} vs {slippery}");
    assert!(grippy > 0.1, "but it did slide: {grippy}");
}

#[test]
fn a_hinge_to_the_world_swings_on_a_circle_and_gains_no_energy() {
    // A box hung two metres to the side of a pin at the origin, axis Z.
    let mut b = active(
        1,
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::splat(0.2),
        RigidSettings { linear_damping: 0.0, angular_damping: 0.0, can_sleep: false, ..Default::default() },
    );
    b.settings.joints.push(JointSpec { axis: [0.0, 0.0, 1.0], ..JointSpec::new(JointKind::Hinge, None, [0.0; 3]) });
    let mut w = world(vec![b], None);
    let en = energies(&mut w, 96, 8);
    assert!(en.iter().all(|e| *e < en[0] + 0.05 * en[0].abs().max(5.0)), "no energy gained: {:?}", en.iter().cloned().fold(f32::MIN, f32::max));
    let cache = run(&mut w, 96, 8);
    for f in 0..cache.frames() {
        let c = extent(&cache, f, 0..8).2;
        assert!((c.length() - 2.0).abs() < 0.08, "frame {f}: the centre left its circle, radius {}", c.length());
        assert!(c.z.abs() < 0.05, "it stays in the plane");
    }
    let lowest = (0..cache.frames()).map(|f| extent(&cache, f, 0..8).2.y).fold(f32::MAX, f32::min);
    assert!(lowest < -1.5, "it swung down: {lowest}");
}

#[test]
fn a_point_joint_lets_a_body_turn_about_its_anchor_and_a_fixed_joint_holds_it_still() {
    let hung = |kind: JointKind| {
        let mut b = active(1, Vec3::new(1.0, 0.0, 0.0), Vec3::splat(0.2), RigidSettings { can_sleep: false, ..Default::default() });
        b.settings.joints.push(JointSpec::new(kind, None, [0.0; 3]));
        let mut w = world(vec![b], None);
        let c = run(&mut w, 72, 8);
        (0..c.frames()).map(|f| extent(&c, f, 0..8).2).collect::<Vec<_>>()
    };
    let ball_joint = hung(JointKind::Point);
    assert!(ball_joint.iter().all(|c| (c.length() - 1.0).abs() < 0.08), "the distance to the anchor holds");
    assert!(ball_joint.last().is_some_and(|c| c.y < -0.5), "and it swings down");
    let fixed = hung(JointKind::Fixed);
    assert!(fixed.iter().all(|c| (*c - Vec3::X).length() < 0.05), "a fixed joint to the world does not move");
}

#[test]
fn a_fixed_joint_between_two_bodies_keeps_their_offset() {
    let mut anchor = active(1, Vec3::ZERO, Vec3::splat(0.25), RigidSettings { can_sleep: false, ..Default::default() });
    anchor.settings.joints.push(JointSpec::new(JointKind::Fixed, None, [0.0; 3]));
    let mut hanger = active(2, Vec3::new(0.0, -1.0, 0.0), Vec3::splat(0.25), RigidSettings { can_sleep: false, ..Default::default() });
    hanger.settings.joints.push(JointSpec::new(JointKind::Fixed, Some(1), [0.0, -0.5, 0.0]));
    let mut w = world(vec![anchor, hanger], None);
    let cache = run(&mut w, 72, 8);
    for f in 0..cache.frames() {
        let (a, b) = (extent(&cache, f, 0..8).2, extent(&cache, f, 8..16).2);
        assert!((b - a - Vec3::new(0.0, -1.0, 0.0)).length() < 0.05, "frame {f}: offset {:?}", b - a);
    }
    // A joint that names a body that is not in the world is dropped, not an error.
    let mut lonely = active(3, Vec3::new(0.0, 5.0, 0.0), Vec3::splat(0.25), RigidSettings::default());
    lonely.settings.joints.push(JointSpec::new(JointKind::Hinge, Some(99), [0.0, 5.0, 0.0]));
    let mut w = world(vec![lonely], Some(0.0));
    let c = run(&mut w, 48, 4);
    assert!(extent(&c, 47, 0..8).0 < 3.0, "it fell, nothing held it");
}

#[test]
fn a_moving_platform_carries_the_body_resting_on_it() {
    let (pp, pt) = cuboid(Vec3::new(0.0, -0.25, 0.0), Vec3::new(2.0, 0.25, 2.0));
    let mut platform = body(2, pp, pt, RigidSettings::passive());
    // Up one metre over the first 24 frames, then still.
    platform.motion = (0..60).map(|f| Mat4::from_translation(Vec3::Y * (f as f32 / 24.0).min(1.0))).collect();
    let crate_ = active(1, Vec3::new(0.0, 0.5, 0.0), Vec3::splat(0.5), RigidSettings { bounciness: 0.0, ..Default::default() });
    let mut w = world(vec![crate_, platform], None);
    let cache = run(&mut w, 60, 4);
    let (lo0, _, _) = extent(&cache, 0, 0..8);
    let (lo, _, mean) = extent(&cache, 59, 0..8);
    assert!((lo - lo0 - 1.0).abs() < 0.1, "the crate rose with the platform: {}", lo - lo0);
    assert!(mean.x.abs() < 0.2, "and stayed on it: {mean:?}");
}

#[test]
fn a_triangle_mesh_collider_holds_up_a_ball() {
    // A two-triangle floor at y = 0.
    let floor = body(
        2,
        vec![Vec3::new(-10.0, 0.0, -10.0), Vec3::new(10.0, 0.0, -10.0), Vec3::new(10.0, 0.0, 10.0), Vec3::new(-10.0, 0.0, 10.0)],
        vec![[0, 2, 1], [0, 3, 2]],
        RigidSettings { shape: ShapeKind::Mesh, ..RigidSettings::passive() },
    );
    let ball_body = body(1, ball(Vec3::new(1.0, 2.0, 1.0), 0.25), vec![], RigidSettings { shape: ShapeKind::Sphere, ..Default::default() });
    let mut w = world(vec![ball_body, floor], None);
    let cache = run(&mut w, 96, 4);
    let lowest = (0..cache.frames()).map(|f| extent(&cache, f, 0..26).0).fold(f32::MAX, f32::min);
    assert!(lowest > -0.06, "the ball never went through the floor: {lowest}");
    assert!(extent(&cache, 95, 0..26).0 < 0.1, "and it is resting on it");
}

#[test]
fn shapes_box_sphere_and_hull_all_come_to_rest_on_the_ground() {
    for shape in [ShapeKind::Auto, ShapeKind::Box, ShapeKind::Sphere, ShapeKind::Hull] {
        let s = RigidSettings { shape, bounciness: 0.0, ..Default::default() };
        let mut w = world(vec![active(1, Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.4, 0.3, 0.5), s)], Some(0.0));
        let c = run(&mut w, 120, 4);
        let (lo, _, _) = extent(&c, 119, 0..8);
        // A sphere collider of the mean half-extent can leave a corner above or below the ground.
        assert!((-0.2..0.2).contains(&lo), "{shape:?}: {lo}");
    }
}

#[test]
fn the_same_world_bakes_to_the_same_bits() {
    let make = || {
        let mut b = active(1, Vec3::new(0.3, 3.0, 0.0), Vec3::splat(0.4), RigidSettings { spin: [1.0, 2.0, 3.0], ..Default::default() });
        b.settings.joints.push(JointSpec::new(JointKind::Point, None, [0.0, 4.0, 0.0]));
        let c = active(2, Vec3::new(-0.2, 1.0, 0.1), Vec3::splat(0.3), RigidSettings::default());
        world(vec![b, c], Some(0.0))
    };
    let (a, b) = (run(&mut make(), 80, 4), run(&mut make(), 80, 4));
    assert_eq!(a.checksum(), b.checksum());
    // Re-running one world object starts over.
    let mut w = make();
    let first = run(&mut w, 80, 4);
    assert_eq!(run(&mut w, 80, 4).checksum(), first.checksum());
}

#[test]
fn hostile_descriptions_are_refused_not_crashed_on() {
    let ok = || active(1, Vec3::ZERO, Vec3::ONE, RigidSettings::default());
    let new = |bodies| RigidWorld::new(WorldDesc { gravity: Vec3::new(0.0, -9.81, 0.0), ground: None, bodies });
    assert!(new(vec![ok()]).is_ok());
    assert!(matches!(new(vec![body(1, vec![], vec![], RigidSettings::default())]), Err(SimError::Invalid(_))), "no points");
    assert!(new(vec![body(1, vec![Vec3::NAN], vec![], RigidSettings::default())]).is_err());
    assert!(new(vec![body(1, vec![Vec3::splat(1e30)], vec![], RigidSettings::default())]).is_err());
    assert!(new(vec![body(1, vec![Vec3::ZERO; 3], vec![[0, 1, 9]], RigidSettings::default())]).is_err(), "a triangle past the points");
    assert!(new(vec![active(1, Vec3::ZERO, Vec3::ONE, RigidSettings { shape: ShapeKind::Mesh, ..Default::default() })]).is_err(), "a moving mesh shape");
    assert!(new((0..MAX_BODIES as u64 + 1).map(|i| active(i, Vec3::ZERO, Vec3::ONE, RigidSettings::default())).collect()).is_err());
    assert!(RigidWorld::new(WorldDesc { gravity: Vec3::NAN, ground: None, bodies: vec![ok()] }).is_err());
    // Degenerate bodies still simulate: all points equal, and a flat plane.
    let mut w = world(vec![body(1, vec![Vec3::new(0.0, 2.0, 0.0); 4], vec![], RigidSettings::default())], Some(0.0));
    run(&mut w, 24, 2);
    let (p, t) = (vec![Vec3::new(0.0, 2.0, 0.0), Vec3::new(1.0, 2.0, 0.0), Vec3::new(0.0, 2.0, 1.0), Vec3::new(1.0, 2.0, 1.0)], vec![[0, 1, 2], [1, 3, 2]]);
    let mut w = world(vec![body(1, p, t, RigidSettings::default())], Some(0.0));
    run(&mut w, 24, 2);
}

#[test]
fn settings_are_clamped_and_round_trip_through_json() {
    let mut s = RigidSettings {
        mass: -5.0,
        friction: f32::NAN,
        bounciness: 7.0,
        linear_damping: f32::INFINITY,
        velocity: [f32::NAN, 1e9, 0.0],
        gravity: [0.0, f32::NEG_INFINITY, 0.0],
        ground: Some(f32::NAN),
        joints: (0..40)
            .map(|_| JointSpec { axis: [0.0; 3], limit: Some([90.0, -90.0]), ..JointSpec::new(JointKind::Hinge, None, [f32::NAN, 0.0, 0.0]) })
            .collect(),
        ..Default::default()
    };
    s.sanitize();
    assert_eq!((s.mass, s.friction, s.bounciness), (1e-3, 0.5, 1.0));
    assert!(s.linear_damping.is_finite() && s.velocity.iter().all(|v| v.is_finite() && v.abs() <= 1000.0));
    assert_eq!(s.gravity, [0.0; 3]);
    assert_eq!(s.ground, Some(0.0));
    assert_eq!(s.joints.len(), MAX_JOINTS);
    assert_eq!(s.joints[0].axis, [0.0, 1.0, 0.0]);
    assert_eq!(s.joints[0].limit, Some([-90.0, 90.0]));
    assert_eq!(s.joints[0].anchor, [0.0; 3]);
    let json = serde_json::to_string(&s).unwrap();
    assert_eq!(serde_json::from_str::<RigidSettings>(&json).unwrap(), s);
}

/// A stack of spinning boxes dropped on the ground, as the property test below builds it; the
/// lowest point any box reaches.
fn spinning_stack_lowest(n: usize, size: f32, drop: f32, e: f32, mu: f32, spin: f32) -> f32 {
    let bodies: Vec<BodyDesc> = (0..n)
        .map(|i| {
            active(
                i as u64,
                Vec3::new(0.1 * i as f32, size * 0.5 + drop + 1.3 * size * i as f32, 0.0),
                Vec3::splat(size * 0.5),
                RigidSettings { bounciness: e, friction: mu, spin: [spin, 0.0, spin], ..Default::default() },
            )
        })
        .collect();
    let mut w = world(bodies, Some(0.0));
    let cache = bake(&mut w, &BakeConfig { timing: timing(48, 4), seed: 1, ..BakeConfig::default() }, &mut Silent).unwrap();
    assert!(cache.positions().iter().all(|p| p.is_finite()));
    cache.positions().iter().map(|p| p.y).fold(f32::MAX, f32::min)
}

#[test]
fn spinning_stacks_that_used_to_sink_and_fly_off_stay_on_the_ground() {
    // Cases the property test found against the old box-shaped ground: the bottom box sank about
    // 0.22 into the floor and the boxes above it were flung upwards at over 15 m/s.
    for (n, size, drop, e, mu, spin) in [
        (4, 0.261_325_12, 0.471_679_4, 0.035_465_34, 0.697_428_9, 2.370_561),
        (5, 0.91, 0.16, 0.77, 0.89, 3.76),
        (4, 0.26, 0.47, 0.77, 0.89, 3.76),
        (1, 0.527_940_9, 1.531_206, 0.606_503_3, 1.175_139_3, -3.919_208),
    ] {
        let lowest = spinning_stack_lowest(n, size, drop, e, mu, spin);
        assert!(lowest > -0.1 * size, "n={n} size={size}: lowest point {lowest}");
    }
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(24))]
    #[test]
    fn random_stacks_never_produce_non_finite_points_or_sink_through_the_ground(
        n in 1usize..6, size in 0.2f32..1.0, drop in 0.0f32..2.0, e in 0.0f32..1.0, mu in 0.0f32..2.0, spin in -5.0f32..5.0,
    ) {
        let bodies: Vec<BodyDesc> = (0..n)
            .map(|i| active(i as u64, Vec3::new(0.1 * i as f32, size * 0.5 + drop + 1.3 * size * i as f32, 0.0), Vec3::splat(size * 0.5),
                RigidSettings { bounciness: e, friction: mu, spin: [spin, 0.0, spin], ..Default::default() }))
            .collect();
        let mut w = world(bodies, Some(0.0));
        let cache = bake(&mut w, &BakeConfig { timing: timing(48, 4), seed: 1, ..BakeConfig::default() }, &mut Silent).unwrap();
        let lowest = cache.positions().iter().map(|p| p.y).fold(f32::MAX, f32::min);
        proptest::prop_assert!(cache.positions().iter().all(|p| p.is_finite()));
        proptest::prop_assert!(lowest > -0.3 * size.max(0.5), "lowest point {lowest}");
    }
}
