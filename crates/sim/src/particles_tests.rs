//! Particles: emission, lifetime, capacity, forces, emitter shapes, collisions, determinism and
//! hostile input.

use std::sync::Arc;

use glam::{Mat4, Vec3};

use crate::cloth::{ColliderMesh, ColliderProvider};
use crate::particles::*;
use crate::{BakeConfig, DEAD, FrameCache, Silent, SimError, SimTiming, bake, is_live};

const FPS: f64 = 24.0;

fn quiet() -> ParticleSettings {
    ParticleSettings {
        rate: 0.0,
        burst: 0,
        gravity: [0.0; 3],
        drag: 0.0,
        speed: 0.0,
        spread: 0.0,
        collide: false,
        lifetime: 100.0,
        lifetime_variation: 0.0,
        speed_variation: 0.0,
        ..Default::default()
    }
}

fn point_at(origin: Vec3) -> EmitterGeometry {
    EmitterGeometry { origin, positions: vec![], triangles: vec![], motion: Vec::new() }
}

fn desc(settings: ParticleSettings, emitter: EmitterGeometry) -> ParticleDesc {
    ParticleDesc {
        settings: ParticleSettings { emitter: if emitter.triangles.is_empty() { EmitterKind::Point } else { settings.emitter }, ..settings },
        emitter,
        start_frame: 1.0,
        fps: FPS,
    }
}

fn cfg(frames: u32, substeps: u32) -> BakeConfig {
    BakeConfig { timing: SimTiming { start_frame: 1.0, end_frame: f64::from(frames), fps: FPS, substeps }, seed: 3, ..BakeConfig::default() }
}

fn run(s: &mut ParticleSolver, frames: u32, substeps: u32) -> FrameCache {
    bake(s, &cfg(frames, substeps), &mut Silent).unwrap()
}

fn alive(c: &FrameCache, f: usize) -> usize {
    c.frame(f).unwrap().iter().filter(|p| is_live(**p)).count()
}

fn floor(y: f32) -> ColliderMesh {
    let pos = vec![Vec3::new(-50.0, y, -50.0), Vec3::new(50.0, y, -50.0), Vec3::new(50.0, y, 50.0), Vec3::new(-50.0, y, 50.0)];
    ColliderMesh { positions: Arc::new(pos), triangles: Arc::new(vec![[0, 2, 1], [0, 3, 2]]) }
}

fn fixed(c: Vec<ColliderMesh>) -> Option<ColliderProvider> {
    Some(Box::new(move |_| c.clone()))
}

/// A unit cube centred at the origin, two metres across, as an emitter mesh.
fn cube_mesh() -> EmitterGeometry {
    let mut positions = Vec::new();
    for z in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for x in [-1.0, 1.0] {
                positions.push(Vec3::new(x, y, z));
            }
        }
    }
    let quads = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
    let triangles = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
    EmitterGeometry { origin: Vec3::ZERO, positions, triangles, motion: Vec::new() }
}

#[test]
fn the_rate_and_the_burst_decide_how_many_are_born() {
    let mut s = ParticleSolver::new(desc(ParticleSettings { rate: 100.0, burst: 50, ..quiet() }, point_at(Vec3::ZERO)), None).unwrap();
    let c = run(&mut s, 49, 4);
    assert_eq!(alive(&c, 0), 50, "the burst is born at the start frame");
    // Two seconds later 48 frames of 100 per second have been added.
    assert!((alive(&c, 48) as i32 - 250).abs() <= 2, "{}", alive(&c, 48));
    assert_eq!(s.counts(), (s.counts().0, 0));
    assert_eq!(alive(&c, 48), s.alive());
}

#[test]
fn particles_die_after_their_lifetime_and_the_counts_balance() {
    let mut s = ParticleSolver::new(desc(ParticleSettings { rate: 100.0, lifetime: 0.5, ..quiet() }, point_at(Vec3::ZERO)), None).unwrap();
    let c = run(&mut s, 96, 4);
    // Steady state: rate × lifetime alive.
    let steady = alive(&c, 90);
    assert!((steady as i32 - 50).abs() <= 3, "{steady}");
    let (born, died) = s.counts();
    assert_eq!(born - died, s.alive() as u64);
    assert!(died > 100);
}

#[test]
fn a_full_pool_stops_emitting_and_a_freed_slot_is_reused() {
    let mut s = ParticleSolver::new(desc(ParticleSettings { rate: 1000.0, capacity: 100, lifetime: 1.0, ..quiet() }, point_at(Vec3::ZERO)), None).unwrap();
    let c = run(&mut s, 72, 4);
    assert!((0..c.frames()).all(|f| alive(&c, f) <= 100));
    assert_eq!(alive(&c, 10), 100, "the pool filled up");
    assert!(s.counts().0 > 150, "slots were reused after their particles died: {}", s.counts().0);
}

#[test]
fn gravity_drag_and_wind_move_a_particle_as_physics_says() {
    // Free fall from rest: y = y0 − ½·g·t².
    let mut s =
        ParticleSolver::new(desc(ParticleSettings { burst: 1, gravity: [0.0, -9.81, 0.0], ..quiet() }, point_at(Vec3::new(0.0, 10.0, 0.0))), None).unwrap();
    let c = run(&mut s, 25, 8);
    let y = c.frame(24).unwrap()[0].y;
    assert!((y - (10.0 - 0.5 * 9.81)).abs() < 0.1, "after a second: {y}");
    // Drag gives a terminal speed g / drag.
    let mut s =
        ParticleSolver::new(desc(ParticleSettings { burst: 1, drag: 2.0, gravity: [0.0, -9.81, 0.0], ..quiet() }, point_at(Vec3::new(0.0, 100.0, 0.0))), None)
            .unwrap();
    let c = run(&mut s, 121, 4);
    let (a, b) = (c.frame(119).unwrap()[0].y, c.frame(120).unwrap()[0].y);
    let speed = (a - b) * FPS as f32;
    assert!((speed - 9.81 / 2.0).abs() < 0.2, "terminal speed {speed}");
    // Drag against a wind drags the particle along it.
    let wind = crate::field::Wind { direction: [1.0, 0.0, 0.0], strength: 5.0, ..Default::default() };
    let mut s = ParticleSolver::new(desc(ParticleSettings { burst: 1, drag: 3.0, gravity: [0.0; 3], wind, ..quiet() }, point_at(Vec3::ZERO)), None).unwrap();
    let c = run(&mut s, 49, 4);
    let (a, b) = (c.frame(46).unwrap()[0].x, c.frame(47).unwrap()[0].x);
    assert!(((b - a) * FPS as f32 - 5.0).abs() < 0.3, "carried at the wind's speed: {}", (b - a) * FPS as f32);
}

#[test]
fn attractors_pull_and_vortices_swirl() {
    let pull = Attractor { strength: 10.0, ..Default::default() };
    let mut s = ParticleSolver::new(desc(ParticleSettings { burst: 1, attractors: vec![pull], ..quiet() }, point_at(Vec3::new(3.0, 0.0, 0.0))), None).unwrap();
    let c = run(&mut s, 24, 4);
    assert!(c.frame(23).unwrap()[0].x < 3.0 - 1.0, "pulled towards the origin: {:?}", c.frame(23).unwrap()[0]);
    // Out of its radius an attractor does nothing.
    let near = Attractor { strength: 10.0, radius: 1.0, ..Default::default() };
    let mut s = ParticleSolver::new(desc(ParticleSettings { burst: 1, attractors: vec![near], ..quiet() }, point_at(Vec3::new(3.0, 0.0, 0.0))), None).unwrap();
    let c = run(&mut s, 24, 4);
    assert_eq!(c.frame(23).unwrap()[0], Vec3::new(3.0, 0.0, 0.0));
    // A vortex about Y turns the particle around Y: it gains angular momentum, not height.
    let swirl = Attractor { kind: AttractorKind::Vortex, strength: 5.0, ..Default::default() };
    let mut s = ParticleSolver::new(desc(ParticleSettings { burst: 1, attractors: vec![swirl], ..quiet() }, point_at(Vec3::new(2.0, 0.0, 0.0))), None).unwrap();
    let c = run(&mut s, 24, 4);
    let p = c.frame(23).unwrap()[0];
    assert!(p.z.abs() > 0.5 && p.y.abs() < 1e-4, "swung sideways around the axis: {p:?}");
}

#[test]
fn emitter_shapes_birth_where_they_should_and_leave_the_right_way() {
    let cube = cube_mesh();
    let at_birth = |kind: EmitterKind, dir: Direction| {
        let st = ParticleSettings { emitter: kind, direction: dir, burst: 400, speed: 1.0, spread: 0.0, ..quiet() };
        let mut s = ParticleSolver::new(ParticleDesc { settings: st, emitter: cube.clone(), start_frame: 1.0, fps: FPS }, None).unwrap();
        let c = bake(&mut s, &cfg(2, 1), &mut Silent).unwrap();
        c.frame(0).unwrap().to_vec()
    };
    let surface = at_birth(EmitterKind::Surface, Direction::Normal);
    assert_eq!(surface.len(), 5000);
    let born: Vec<Vec3> = surface.iter().copied().filter(|p| is_live(*p)).collect();
    assert_eq!(born.len(), 400);
    assert!(born.iter().all(|p| (p.abs().max_element() - 1.0).abs() < 1e-4), "every particle starts on a face of the cube");
    let faces = born.iter().filter(|p| p.x > 0.999).count();
    assert!(faces > 30 && faces < 110, "an even spread over the six faces: {faces} on +X");
    let volume = at_birth(EmitterKind::Volume, Direction::Random);
    let inside: Vec<Vec3> = volume.iter().copied().filter(|p| is_live(*p)).collect();
    assert_eq!(inside.len(), 400);
    assert!(inside.iter().all(|p| p.abs().max_element() < 1.0), "inside the cube");
    assert!(inside.iter().filter(|p| p.abs().max_element() < 0.5).count() > 20, "and through the middle, not just near the skin");
    // A surface emitter sends particles out along the face normal (zero spread): after a frame
    // each is one step further from the cube along the axis it started on.
    let mut s = ParticleSolver::new(
        ParticleDesc {
            settings: ParticleSettings { burst: 200, speed: 1.0, ..ParticleSettings { emitter: EmitterKind::Surface, ..quiet() } },
            emitter: cube.clone(),
            start_frame: 1.0,
            fps: FPS,
        },
        None,
    )
    .unwrap();
    let c = bake(&mut s, &cfg(25, 4), &mut Silent).unwrap();
    for (a, b) in c.frame(0).unwrap().iter().zip(c.frame(24).unwrap()).filter(|(a, _)| is_live(**a)) {
        let axis = if a.x.abs() > 0.999 {
            0
        } else if a.y.abs() > 0.999 {
            1
        } else {
            2
        };
        let moved = *b - *a;
        assert!((moved[axis].abs() - 1.0).abs() < 0.02 && moved.length() < 1.02, "left along its face normal: {moved:?}");
    }
}

#[test]
fn the_spread_is_a_cone_around_the_direction() {
    let angles = |spread: f32| {
        let st = ParticleSettings { burst: 600, speed: 1.0, spread, direction: Direction::Fixed, fixed_direction: [0.0, 1.0, 0.0], ..quiet() };
        let mut s = ParticleSolver::new(desc(st, point_at(Vec3::ZERO)), None).unwrap();
        let c = bake(&mut s, &cfg(2, 1), &mut Silent).unwrap();
        c.frame(1).unwrap().iter().filter(|p| is_live(**p)).map(|p| p.normalize().angle_between(Vec3::Y).to_degrees()).collect::<Vec<f32>>()
    };
    let narrow = angles(20.0);
    assert!(narrow.iter().all(|a| *a <= 20.5), "inside the cone: {}", narrow.iter().cloned().fold(0.0, f32::max));
    assert!(narrow.iter().cloned().fold(0.0, f32::max) > 18.0, "and filling it");
    let wide = angles(180.0);
    let mean_y = wide.iter().map(|a| a.to_radians().cos()).sum::<f32>() / wide.len() as f32;
    assert!(mean_y.abs() < 0.1, "all directions: {mean_y}");
    assert!(angles(0.0).iter().all(|a| *a < 0.01), "no spread: all the way up");
}

#[test]
fn particles_bounce_slide_stop_and_never_go_through_a_floor() {
    let drop = |e: f32, mu: f32, vx: f32| {
        let st = ParticleSettings {
            burst: 1,
            gravity: [0.0, -9.81, 0.0],
            collide: true,
            bounciness: e,
            friction: mu,
            speed: vx,
            direction: Direction::Fixed,
            fixed_direction: [1.0, 0.0, 0.0],
            ..quiet()
        };
        let mut s = ParticleSolver::new(desc(st, point_at(Vec3::new(0.0, 3.0, 0.0))), fixed(vec![floor(0.0)])).unwrap();
        run(&mut s, 120, 4)
    };
    for (e, mu, vx) in [(0.0, 0.0, 0.0), (0.8, 0.0, 0.0), (0.5, 0.5, 3.0), (1.0, 0.0, 0.0)] {
        let c = drop(e, mu, vx);
        let lowest = (0..c.frames()).map(|f| c.frame(f).unwrap()[0].y).fold(f32::MAX, f32::min);
        assert!(lowest > -1e-3, "e {e}: through the floor, lowest {lowest}");
    }
    let dead = drop(0.0, 0.0, 0.0);
    assert!(dead.frame(119).unwrap()[0].y < 0.01, "no bounce: it rests on the floor");
    let lively = drop(0.8, 0.0, 0.0);
    let ys: Vec<f32> = (0..lively.frames()).map(|f| lively.frame(f).unwrap()[0].y).collect();
    let hit = ys.iter().position(|y| *y < 0.2).unwrap_or(0);
    let apex = ys[hit..].iter().cloned().fold(0.0f32, f32::max);
    assert!(apex > 0.3 && apex < 3.0, "a bounce keeps part of the height: {apex}");
    // Friction slows the slide; a frictionless particle keeps its speed.
    let (slippery, grippy) = (drop(0.0, 0.0, 3.0), drop(0.0, 0.8, 3.0));
    assert!(slippery.frame(119).unwrap()[0].x > grippy.frame(119).unwrap()[0].x + 1.0, "friction slows the slide");
}

#[test]
fn a_fast_particle_does_not_tunnel_a_single_triangle() {
    let wall = ColliderMesh {
        positions: Arc::new(vec![Vec3::new(5.0, -10.0, -10.0), Vec3::new(5.0, 10.0, -10.0), Vec3::new(5.0, 0.0, 10.0)]),
        triangles: Arc::new(vec![[0, 1, 2]]),
    };
    let st =
        ParticleSettings { burst: 1, speed: 400.0, direction: Direction::Fixed, fixed_direction: [1.0, 0.0, 0.0], collide: true, bounciness: 0.5, ..quiet() };
    let mut s = ParticleSolver::new(desc(st, point_at(Vec3::ZERO)), fixed(vec![wall])).unwrap();
    let c = run(&mut s, 24, 1);
    let furthest = (0..c.frames()).filter_map(|f| Some(c.frame(f).unwrap()[0]).filter(|p| is_live(*p))).map(|p| p.x).fold(f32::MIN, f32::max);
    assert!(furthest < 5.001, "stopped by the wall at x = 5: {furthest}");
    assert!(c.frame(23).unwrap()[0].x < 5.0 - 10.0, "and thrown back: {:?}", c.frame(23).unwrap()[0]);
}

#[test]
fn a_moving_collider_pushes_a_particle_with_its_own_speed() {
    // A floor rising at two metres per second under a resting particle carries it up.
    let rising: ColliderProvider = Box::new(|frame| vec![floor(((frame - 1.0) / 24.0 * 2.0) as f32)]);
    let st = ParticleSettings { burst: 1, collide: true, bounciness: 0.0, gravity: [0.0, -9.81, 0.0], ..quiet() };
    let mut s = ParticleSolver::new(desc(st, point_at(Vec3::new(0.0, 0.5, 0.0))), Some(rising)).unwrap();
    let c = run(&mut s, 48, 8);
    let y = c.frame(47).unwrap()[0].y;
    assert!(y > 3.5, "pushed up with the floor to about 3.9: {y}");
}

#[test]
fn an_emitter_that_moves_and_hands_down_its_speed() {
    let motion: Vec<Mat4> = (0..49).map(|f| Mat4::from_translation(Vec3::new(f as f32 * 0.5, 0.0, 0.0))).collect();
    let make = |inherit: f32| {
        let st = ParticleSettings { rate: 24.0, inherit, ..quiet() };
        let g = EmitterGeometry { origin: Vec3::ZERO, positions: vec![], triangles: vec![], motion: motion.clone() };
        let mut s = ParticleSolver::new(desc(st, g), None).unwrap();
        run(&mut s, 49, 4)
    };
    let plain = make(0.0);
    // Born along the emitter's path: the late ones start far from the first.
    let live: Vec<Vec3> = plain.frame(48).unwrap().iter().copied().filter(|p| is_live(*p)).collect();
    assert!(live.iter().map(|p| p.x).fold(f32::MIN, f32::max) > 20.0, "particles are born where the emitter is");
    assert!(live.iter().map(|p| p.x).fold(f32::MAX, f32::min) < 2.0);
    let inheriting = make(1.0);
    let first_plain = plain.frame(48).unwrap()[0].x;
    let first_inherit = inheriting.frame(48).unwrap()[0].x;
    assert!(first_inherit > first_plain + 10.0, "inherited speed carries them on: {first_plain} vs {first_inherit}");
}

#[test]
fn the_same_seed_gives_the_same_particles_and_another_seed_other_ones() {
    let make = || {
        let st = ParticleSettings {
            emitter: EmitterKind::Surface,
            rate: 300.0,
            collide: true,
            noise_strength: 2.0,
            wind: crate::field::Wind { strength: 3.0, turbulence: 0.5, ..Default::default() },
            drag: 0.5,
            ..Default::default()
        };
        ParticleSolver::new(ParticleDesc { settings: st, emitter: cube_mesh(), start_frame: 1.0, fps: FPS }, fixed(vec![floor(-3.0)])).unwrap()
    };
    let (a, b) = (run(&mut make(), 48, 4), run(&mut make(), 48, 4));
    assert_eq!(a.checksum(), b.checksum());
    let other = bake(&mut make(), &BakeConfig { seed: 99, ..cfg(48, 4) }, &mut Silent).unwrap();
    assert_ne!(a.checksum(), other.checksum());
}

#[test]
fn the_size_of_a_slot_is_fixed_and_within_its_variation() {
    let st = ParticleSettings { size: 0.1, size_variation: 0.5, ..Default::default() };
    let sizes: Vec<f32> = (0..1000).map(|i| st.size_of(i)).collect();
    assert!(sizes.iter().all(|s| (0.05..=0.15).contains(s)));
    assert_eq!(sizes[17], st.size_of(17));
    assert!(sizes.iter().cloned().fold(0.0, f32::max) - sizes.iter().cloned().fold(1.0, f32::min) > 0.05, "the sizes vary");
    assert_eq!(ParticleSettings { size_variation: 0.0, ..st }.size_of(3), 0.1);
}

#[test]
fn hostile_input_is_refused_or_repaired() {
    let new = |g: EmitterGeometry, st: ParticleSettings| ParticleSolver::new(ParticleDesc { settings: st, emitter: g, start_frame: 1.0, fps: FPS }, None);
    assert!(new(point_at(Vec3::NAN), ParticleSettings::default()).is_err());
    assert!(
        matches!(new(point_at(Vec3::ZERO), ParticleSettings { emitter: EmitterKind::Surface, ..Default::default() }), Err(SimError::Invalid(_))),
        "a surface emitter needs faces"
    );
    let mut g = cube_mesh();
    g.triangles.push([0, 1, 99]);
    assert!(new(g, ParticleSettings::default()).is_err());
    let mut st = ParticleSettings {
        rate: f32::NAN,
        lifetime: -3.0,
        speed: f32::INFINITY,
        spread: 1e9,
        capacity: 0,
        burst: u32::MAX,
        fixed_direction: [0.0; 3],
        attractors: vec![Attractor { position: [f32::NAN; 3], strength: f32::INFINITY, axis: [0.0; 3], ..Default::default() }; 200],
        collide_with: vec![4, 4, 2],
        ..Default::default()
    };
    st.sanitize();
    assert_eq!((st.rate, st.lifetime, st.spread, st.capacity), (0.0, 0.01, 180.0, 1));
    assert_eq!(st.burst, MAX_CAPACITY);
    assert_eq!(st.attractors.len(), MAX_ATTRACTORS);
    assert_eq!(st.attractors[0].position, [0.0; 3]);
    assert_eq!(st.attractors[0].axis, [0.0, 1.0, 0.0]);
    assert_eq!(st.fixed_direction, [0.0, 1.0, 0.0]);
    assert_eq!(st.collide_with, vec![2, 4]);
    let json = serde_json::to_string(&st).unwrap();
    assert_eq!(serde_json::from_str::<ParticleSettings>(&json).unwrap(), st);
    // A burst larger than the pool just fills it.
    let mut s = new(point_at(Vec3::ZERO), ParticleSettings { emitter: EmitterKind::Point, burst: 500, capacity: 10, ..quiet() }).unwrap();
    assert_eq!(alive(&run(&mut s, 3, 1), 0), 10);
    let _ = DEAD;
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(16))]
    #[test]
    fn random_systems_stay_finite_within_their_pool(
        rate in 0.0f32..2000.0, life in 0.05f32..3.0, speed in 0.0f32..50.0, spread in 0.0f32..180.0, drag in 0.0f32..20.0,
        e in 0.0f32..1.0, mu in 0.0f32..1.0, noise in 0.0f32..30.0, cap in 1u32..300, substeps in 1u32..6, kind in 0usize..3,
    ) {
        let st = ParticleSettings {
            emitter: [EmitterKind::Point, EmitterKind::Surface, EmitterKind::Volume][kind], rate, lifetime: life, speed, spread, drag,
            bounciness: e, friction: mu, noise_strength: noise, capacity: cap, burst: cap / 2,
            attractors: vec![Attractor { strength: 20.0, ..Default::default() }, Attractor { kind: AttractorKind::Vortex, strength: 8.0, radius: 3.0, ..Default::default() }],
            ..Default::default()
        };
        let mut s = ParticleSolver::new(ParticleDesc { settings: st, emitter: cube_mesh(), start_frame: 1.0, fps: FPS }, fixed(vec![floor(-2.0)])).unwrap();
        let c = bake(&mut s, &cfg(30, substeps), &mut Silent).unwrap();
        for f in 0..c.frames() {
            let live = c.frame(f).unwrap().iter().filter(|p| is_live(**p)).count();
            proptest::prop_assert!(live <= cap as usize);
        }
        proptest::prop_assert!(c.positions().iter().all(|p| !is_live(*p) || p.abs().max_element() < 1e6));
        let (born, died) = s.counts();
        proptest::prop_assert_eq!(born - died, s.alive() as u64);
    }
}
