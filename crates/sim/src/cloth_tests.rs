//! Cloth: hanging, draping over and resting on colliders, moving colliders, pins, wind, self
//! collision, determinism and hostile input.

use std::sync::Arc;

use glam::{Mat4, Vec3};

use crate::cloth::*;
use crate::field::Wind;
use crate::{BakeConfig, FrameCache, Silent, SimError, SimTiming, bake};

const FPS: f64 = 24.0;

/// A grid of `nx × nz` quads, `size` metres on a side, in the XZ plane at height `y`.
fn flat(nx: u32, nz: u32, size: f32, y: f32, origin: Vec3) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    let mut pts = Vec::new();
    for j in 0..=nz {
        for i in 0..=nx {
            pts.push(origin + Vec3::new(i as f32 / nx as f32 * size, y, j as f32 / nz as f32 * size));
        }
    }
    let at = |i: u32, j: u32| j * (nx + 1) + i;
    let mut quads = Vec::new();
    for j in 0..nz {
        for i in 0..nx {
            quads.push(vec![at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    (pts, quads)
}

/// The same grid standing up in the XY plane, top edge at `y`.
fn hanging(nx: u32, ny: u32, size: f32, y: f32) -> (Vec<Vec3>, Vec<Vec<u32>>) {
    let (flat_pts, quads) = flat(nx, ny, size, 0.0, Vec3::ZERO);
    (flat_pts.iter().map(|p| Vec3::new(p.x, y - p.z, 0.0)).collect(), quads)
}

fn top_row(nx: u32) -> Vec<u32> {
    (0..=nx).collect()
}

fn desc(points: Vec<Vec3>, polygons: Vec<Vec<u32>>, settings: ClothSettings) -> ClothDesc {
    ClothDesc { points, polygons, settings, pin_motion: Vec::new(), start_frame: 1.0 }
}

fn cfg(frames: u32, substeps: u32) -> BakeConfig {
    BakeConfig { timing: SimTiming { start_frame: 1.0, end_frame: f64::from(frames), fps: FPS, substeps }, seed: 1, ..BakeConfig::default() }
}

fn run(s: &mut ClothSolver, frames: u32, substeps: u32) -> FrameCache {
    bake(s, &cfg(frames, substeps), &mut Silent).unwrap()
}

/// A UV sphere as a collider.
fn sphere(centre: Vec3, r: f32) -> ColliderMesh {
    let (seg, ring) = (32u32, 16u32);
    let mut pos = Vec::new();
    for j in 0..=ring {
        let phi = std::f32::consts::PI * j as f32 / ring as f32;
        for i in 0..seg {
            let th = std::f32::consts::TAU * i as f32 / seg as f32;
            pos.push(centre + r * Vec3::new(phi.sin() * th.cos(), phi.cos(), phi.sin() * th.sin()));
        }
    }
    let mut tris = Vec::new();
    for j in 0..ring {
        for i in 0..seg {
            let (a, b) = (j * seg + i, j * seg + (i + 1) % seg);
            let (c, d) = (a + seg, b + seg);
            tris.push([a, c, b]);
            tris.push([b, c, d]);
        }
    }
    ColliderMesh { positions: Arc::new(pos), triangles: Arc::new(tris) }
}

/// A big square floor at height `y` (two triangles, normal up).
fn floor(y: f32) -> ColliderMesh {
    let pos = vec![Vec3::new(-50.0, y, -50.0), Vec3::new(50.0, y, -50.0), Vec3::new(50.0, y, 50.0), Vec3::new(-50.0, y, 50.0)];
    ColliderMesh { positions: Arc::new(pos), triangles: Arc::new(vec![[0, 2, 1], [0, 3, 2]]) }
}

fn fixed_colliders(c: Vec<ColliderMesh>) -> ColliderProvider {
    Box::new(move |_| c.clone())
}

#[test]
fn constraints_follow_the_mesh_topology() {
    let (p, q) = flat(2, 2, 1.0, 0.0, Vec3::ZERO);
    let s = ClothSolver::new(desc(p, q, ClothSettings::default()), None).unwrap();
    assert_eq!(s.constraint_counts(), (12, 8, 4), "edges, quad diagonals, interior edges");
    assert_eq!(s.triangles().len(), 8);
    // A triangle mesh has no shear constraints.
    let tri = ClothSolver::new(
        desc(vec![Vec3::ZERO, Vec3::X, Vec3::Z, Vec3::new(1.0, 0.0, 1.0)], vec![vec![0, 1, 2], vec![1, 3, 2]], ClothSettings::default()),
        None,
    )
    .unwrap();
    assert_eq!(tri.constraint_counts(), (5, 0, 1));
}

#[test]
fn a_cloth_pinned_along_its_top_hangs_and_the_pins_do_not_move() {
    let (p, q) = hanging(16, 16, 1.0, 2.0);
    let pins = top_row(16);
    let mut s = ClothSolver::new(desc(p.clone(), q, ClothSettings { pinned: pins.clone(), ..Default::default() }), None).unwrap();
    let cache = run(&mut s, 96, 8);
    for f in 0..cache.frames() {
        for i in &pins {
            assert_eq!(cache.frame(f).unwrap()[*i as usize], p[*i as usize], "pin {i} at frame {f}");
        }
    }
    assert!(s.max_stretch() < 0.06, "stiff cloth hardly stretches: {}", s.max_stretch());
    let low = cache.frame(95).unwrap().iter().map(|v| v.y).fold(f32::MAX, f32::min);
    assert!((0.93..1.02).contains(&low), "it hangs about a metre down from y = 2: {low}");
    // It settled: the last two frames are nearly the same.
    let drift = cache.frame(95).unwrap().iter().zip(cache.frame(94).unwrap()).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
    assert!(drift < 0.02, "at rest: {drift}");
}

#[test]
fn softer_cloth_stretches_more() {
    let sag = |stretch: f32| {
        let (p, q) = hanging(12, 12, 1.0, 2.0);
        let mut s = ClothSolver::new(desc(p, q, ClothSettings { pinned: top_row(12), stretch, iterations: 1, ..Default::default() }), None).unwrap();
        run(&mut s, 96, 8);
        s.max_stretch()
    };
    let (stiff, soft) = (sag(1.0), sag(0.25));
    assert!(stiff < 0.05, "inextensible: {stiff}");
    assert!(soft > stiff + 0.03, "a soft cloth gives: {soft} vs {stiff}");
}

#[test]
fn bending_stiffness_keeps_a_cantilever_flatter() {
    let droop = |bend: f32| {
        // A flat strip clamped along one edge and sticking out sideways.
        let (p, q) = flat(6, 4, 0.5, 1.0, Vec3::ZERO);
        // Clamped: the first two columns are held, so the sheet cannot swing about its pinned edge.
        let pins: Vec<u32> = (0..=4).flat_map(|j| [j * 7, j * 7 + 1]).collect();
        let mut s =
            ClothSolver::new(desc(p, q, ClothSettings { pinned: pins, bend, shear: 1.0, stretch: 1.0, iterations: 4, ..Default::default() }), None).unwrap();
        let c = run(&mut s, 96, 8);
        c.frame(95).unwrap().iter().map(|v| v.y).fold(f32::MAX, f32::min)
    };
    let (stiff, limp) = (droop(1.0), droop(0.0));
    assert!(stiff > limp + 0.05, "a stiff sheet droops less: {stiff} vs {limp}");
}

#[test]
fn cloth_drapes_over_a_sphere_without_going_into_it() {
    let (p, q) = flat(24, 24, 1.6, 1.0, Vec3::new(-0.8, 0.0, -0.8));
    let colliders = vec![sphere(Vec3::ZERO, 0.5)];
    let mut s = ClothSolver::new(desc(p, q, ClothSettings { thickness: 0.02, ..Default::default() }), Some(fixed_colliders(colliders))).unwrap();
    let cache = run(&mut s, 120, 8);
    let nearest = (0..cache.frames()).flat_map(|f| cache.frame(f).unwrap().to_vec()).map(|v| v.length()).fold(f32::MAX, f32::min);
    assert!(nearest > 0.5 + 0.01, "no vertex got inside the sphere: {nearest}");
    let last = cache.frame(119).unwrap();
    let top = last.iter().map(|v| v.y).fold(f32::MIN, f32::max);
    let centre = last[12 * 25 + 12];
    assert!(centre.y > 0.5 && centre.y < 0.6, "the middle of the cloth rests on top of the sphere: {centre:?}");
    assert!(top < 1.0, "it came down");
    let edge = last[0];
    assert!(edge.y < 0.5, "its corners hang down the sides: {edge:?}");
}

#[test]
fn a_cloth_rests_on_a_floor_and_a_rising_floor_lifts_it() {
    let (p, q) = flat(10, 10, 1.0, 0.3, Vec3::new(-0.5, 0.0, -0.5));
    let mut s = ClothSolver::new(desc(p.clone(), q.clone(), ClothSettings::default()), Some(fixed_colliders(vec![floor(0.0)]))).unwrap();
    let c = run(&mut s, 72, 8);
    let low = c.positions().iter().map(|v| v.y).fold(f32::MAX, f32::min);
    assert!(low > 0.02 - 0.01, "never through the floor: {low}");
    assert!(c.frame(71).unwrap().iter().all(|v| v.y < 0.06), "it lies on the floor");
    // The floor climbs one metre over 24 frames.
    let rising: ColliderProvider = Box::new(|frame| {
        let y = ((frame - 1.0) / 24.0).clamp(0.0, 1.0) as f32;
        vec![floor(y)]
    });
    let mut s = ClothSolver::new(desc(p, q, ClothSettings::default()), Some(rising)).unwrap();
    let c = run(&mut s, 72, 8);
    let end = c.frame(71).unwrap();
    assert!(end.iter().all(|v| v.y > 0.98), "the cloth went up with the floor: {}", end.iter().map(|v| v.y).fold(f32::MAX, f32::min));
    assert!(end.iter().all(|v| v.y < 1.1), "and rests on it: {}", end.iter().map(|v| v.y).fold(f32::MIN, f32::max));
}

#[test]
fn friction_lets_a_moving_collider_carry_the_cloth_sideways() {
    let carried = |friction: f32| {
        let (p, q) = flat(8, 8, 0.6, 0.05, Vec3::new(-0.3, 0.0, -0.3));
        let sliding: ColliderProvider = Box::new(|frame| {
            let x = ((frame - 1.0) / 24.0).clamp(0.0, 1.0) as f32 * 2.0;
            let mut f = floor(0.0);
            f.positions = Arc::new(f.positions.iter().map(|p| *p + Vec3::X * x).collect());
            vec![f]
        });
        let mut s = ClothSolver::new(desc(p, q, ClothSettings { friction, ..Default::default() }), Some(sliding)).unwrap();
        let c = run(&mut s, 48, 8);
        c.frame(47).unwrap().iter().map(|v| v.x).sum::<f32>() / 81.0
    };
    let (grip, ice) = (carried(1.5), carried(0.0));
    assert!(grip > ice + 0.5, "friction drags the cloth along: {grip} vs {ice}");
}

#[test]
fn pins_can_follow_a_moving_object() {
    let (p, q) = hanging(8, 8, 1.0, 2.0);
    let mut d = desc(p.clone(), q, ClothSettings { pinned: top_row(8), ..Default::default() });
    d.pin_motion = (0..48).map(|f| Mat4::from_translation(Vec3::new(f as f32 * 0.1, 0.0, 0.0))).collect();
    let mut s = ClothSolver::new(d, None).unwrap();
    let c = run(&mut s, 48, 4);
    for i in 0..=8 {
        let want = p[i] + Vec3::new(4.7, 0.0, 0.0);
        assert!(c.frame(47).unwrap()[i].distance(want) < 1e-4, "pin {i}: {:?} vs {want:?}", c.frame(47).unwrap()[i]);
    }
    let body = c.frame(47).unwrap()[8 * 9 + 4];
    assert!(body.x > 3.0, "the rest of the cloth is dragged after the pins: {body:?}");
}

#[test]
fn wind_blows_a_hanging_cloth_downwind_and_turbulence_is_seeded() {
    let sway = |wind: Wind, seed: u64| {
        let (p, q) = hanging(12, 12, 1.0, 2.0);
        let mut s = ClothSolver::new(desc(p, q, ClothSettings { pinned: top_row(12), wind, ..Default::default() }), None).unwrap();
        let c = bake(&mut s, &BakeConfig { seed, ..cfg(96, 8) }, &mut Silent).unwrap();
        let mean_z = c.frame(95).unwrap().iter().map(|v| v.z).sum::<f32>() / 169.0;
        (mean_z, c.checksum())
    };
    let blow = Wind { direction: [0.0, 0.0, 1.0], strength: 6.0, ..Wind::default() };
    let (calm, _) = sway(Wind::default(), 1);
    let (windy, _) = sway(blow, 1);
    assert!(calm.abs() < 0.01, "no wind, no sway: {calm}");
    assert!(windy > 0.15, "the cloth is blown downwind: {windy}");
    let gusty = Wind { turbulence: 0.7, ..blow };
    let (a, ca) = sway(gusty, 1);
    let (b, cb) = sway(gusty, 1);
    let (_, cc) = sway(gusty, 2);
    assert_eq!((a, ca), (b, cb), "the same seed gives the same gusts");
    assert_ne!(ca, cc, "another seed gives other gusts");
    assert!(a > 0.1);
}

#[test]
fn self_collision_keeps_a_falling_layer_on_top_of_another() {
    let build = |self_collision: bool| {
        // Two sheets in one mesh: a big lower one, pinned, and a small upper one dropped on it, shifted
        // by half a cell so its vertices fall in the gaps of the lower one.
        let (mut p, mut q) = flat(20, 20, 1.0, 0.0, Vec3::ZERO);
        let (p2, q2) = flat(10, 10, 0.5, 0.2, Vec3::new(0.275, 0.0, 0.275));
        let base = p.len() as u32;
        p.extend(p2);
        q.extend(q2.into_iter().map(|f| f.into_iter().map(|i| i + base).collect()));
        let pins: Vec<u32> = (0..base).collect();
        let mut s = ClothSolver::new(desc(p, q, ClothSettings { pinned: pins, self_collision, thickness: 0.03, ..Default::default() }), None).unwrap();
        let c = run(&mut s, 60, 8);
        c.frame(59).unwrap()[441..].iter().map(|v| v.y).fold(f32::MAX, f32::min)
    };
    let (with, without) = (build(true), build(false));
    assert!(with > 0.02, "the upper sheet stays above the lower one: {with}");
    assert!(without < -0.3, "without self collision it falls straight through: {without}");
}

#[test]
fn the_same_cloth_bakes_to_the_same_bits_and_a_solver_can_be_rerun() {
    let make = || {
        let (p, q) = hanging(10, 10, 1.0, 2.0);
        ClothSolver::new(
            desc(
                p,
                q,
                ClothSettings {
                    pinned: vec![0, 10],
                    wind: Wind { strength: 4.0, turbulence: 0.5, direction: [0.0, 0.0, 1.0], ..Wind::default() },
                    self_collision: true,
                    ..Default::default()
                },
            ),
            Some(fixed_colliders(vec![sphere(Vec3::new(0.5, 0.6, 0.6), 0.3)])),
        )
        .unwrap()
    };
    let (a, b) = (run(&mut make(), 60, 6), run(&mut make(), 60, 6));
    assert_eq!(a.checksum(), b.checksum());
    let mut s = make();
    let first = run(&mut s, 60, 6);
    assert_eq!(run(&mut s, 60, 6).checksum(), first.checksum());
}

#[test]
fn hostile_input_is_refused_or_repaired() {
    let (p, _) = flat(2, 2, 1.0, 0.0, Vec3::ZERO);
    let new = |p: Vec<Vec3>, q: Vec<Vec<u32>>| ClothSolver::new(desc(p, q, ClothSettings::default()), None);
    assert!(matches!(new(vec![], vec![]), Err(SimError::Invalid(_))));
    assert!(new(vec![Vec3::NAN; 3], vec![vec![0, 1, 2]]).is_err());
    assert!(new(p.clone(), vec![]).is_err(), "no faces");
    assert!(new(p.clone(), vec![vec![0, 1, 99], vec![0, 1]]).is_err(), "every face is unusable");
    assert!(new(p.clone(), vec![vec![0, 1, 99], vec![0, 1, 4]]).is_ok(), "bad faces are skipped, good ones kept");
    // Two vertices on top of each other: no zero-length constraint, no division by zero.
    let mut s = new(vec![Vec3::ZERO, Vec3::ZERO, Vec3::X, Vec3::Z], vec![vec![0, 1, 2], vec![0, 2, 3]]).unwrap();
    let c = run(&mut s, 24, 4);
    assert!(c.positions().iter().all(|v| v.is_finite()));
    let mut st = ClothSettings {
        mass: f32::NAN,
        stretch: 7.0,
        shear: -1.0,
        bend: f32::INFINITY,
        damping: -4.0,
        thickness: 0.0,
        friction: 99.0,
        iterations: 0,
        gravity: [f32::NAN, 1e9, 0.0],
        pinned: vec![5, 5, 1],
        collide_with: vec![3, 3],
        ..Default::default()
    };
    st.sanitize();
    assert_eq!((st.mass, st.stretch, st.shear, st.bend), (1.0, 1.0, 0.0, 0.3));
    assert_eq!((st.iterations, st.thickness, st.friction), (1, 1e-4, 2.0));
    assert_eq!((st.pinned.clone(), st.collide_with.clone()), (vec![1, 5], vec![3]));
    assert_eq!(st.gravity[0], 0.0);
    let json = serde_json::to_string(&st).unwrap();
    assert_eq!(serde_json::from_str::<ClothSettings>(&json).unwrap(), st);
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(16))]
    #[test]
    fn random_settings_never_blow_up(
        stretch in 0.0f32..1.0, shear in 0.0f32..1.0, bend in 0.0f32..1.0, iters in 1u32..4, mass in 0.01f32..50.0,
        wind in 0.0f32..30.0, turb in 0.0f32..1.0, damping in 0.0f32..5.0, substeps in 1u32..8, selfc in proptest::bool::ANY,
    ) {
        let (p, q) = hanging(8, 8, 1.0, 2.0);
        let settings = ClothSettings {
            stretch, shear, bend, iterations: iters, mass, damping, pinned: vec![0, 8], self_collision: selfc,
            wind: Wind { strength: wind, turbulence: turb, direction: [0.3, 0.0, 1.0], ..Wind::default() },
            ..Default::default()
        };
        let mut s = ClothSolver::new(desc(p, q, settings), Some(fixed_colliders(vec![floor(0.0)]))).unwrap();
        let c = bake(&mut s, &cfg(36, substeps), &mut Silent).unwrap();
        proptest::prop_assert!(c.positions().iter().all(|v| v.is_finite() && v.abs().max_element() < 200.0));
    }
}

/// A box as a collider (outward-facing or not: the solver must not care).
fn boxy(centre: Vec3, half: f32) -> ColliderMesh {
    let mut pos = Vec::new();
    for z in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for x in [-1.0, 1.0] {
                pos.push(centre + Vec3::new(x, y, z) * half);
            }
        }
    }
    let quads = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
    let tris = quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect();
    ColliderMesh { positions: Arc::new(pos), triangles: Arc::new(tris) }
}

#[test]
fn a_sheet_dropped_on_a_box_never_goes_into_it() {
    let (p, q) = flat(16, 16, 2.0, 1.5, Vec3::new(-1.0, 0.0, -1.0));
    let mut s = ClothSolver::new(desc(p, q, ClothSettings::default()), Some(fixed_colliders(vec![boxy(Vec3::ZERO, 0.5)]))).unwrap();
    let cache = run(&mut s, 72, 8);
    for f in 0..cache.frames() {
        let inside = cache.frame(f).unwrap().iter().filter(|v| v.x.abs() < 0.49 && v.z.abs() < 0.49 && v.y.abs() < 0.49).count();
        assert_eq!(inside, 0, "vertices inside the box at frame {f}");
    }
    let centre = cache.frame(71).unwrap()[8 * 17 + 8];
    assert!((0.5..0.58).contains(&centre.y), "{centre:?}");
}
