//! `modelcraft-cli sim-bench`: bake a simulation of a given size and report the solve time.
//! Reproduces the numbers in `docs/performance.md` ("Simulation").

use std::time::Instant;

use glam::Vec3;
use modelcraft_engine::simulation::cloth::{ClothDesc, ClothSettings, ClothSolver, ColliderMesh, ColliderProvider};
use modelcraft_engine::simulation::rigid::{BodyDesc, RigidSettings, RigidWorld, WorldDesc};
use modelcraft_engine::simulation::{BakeConfig, Silent, SimTiming, Solver, bake};

pub const USAGE: &str = "  sim-bench cloth [--vertices N] [--frames N] [--substeps N] [--iterations N] [--self] [--sphere]
                                    drop a square cloth (about N vertices, pinned at two corners)
                                    over a sphere: solve time per frame and per substep; --self adds
                                    self collision, --sphere the collider (defaults: 10000 vertices,
                                    120 frames at 24 fps, 8 substeps, 2 iterations)
  sim-bench rigid [--bodies N] [--frames N] [--substeps N]
                                    bake N boxes falling into a pile on a ground: solve time per
                                    frame, bodies asleep at the end and cache size (defaults:
                                    1000 bodies, 120 frames at 24 fps, 4 substeps)";

fn value<'a>(a: &'a [String], name: &str) -> Option<&'a str> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).map(String::as_str)
}

fn number<T: std::str::FromStr>(a: &[String], name: &str, default: T) -> Result<T, String> {
    value(a, name).map_or(Ok(default), |v| v.parse().map_err(|_| format!("{name}: {v:?} is not a number")))
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

pub fn bench(a: &[String]) -> Result<(), String> {
    match a.first().map(String::as_str) {
        Some("rigid") => rigid(a),
        Some("cloth") => cloth(a),
        Some(other) => Err(format!("sim-bench: unknown simulation {other:?} (rigid, cloth)")),
        None => Err("sim-bench: which simulation? (rigid, cloth)".into()),
    }
}

/// A unit box's eight corners and twelve triangles at `c`.
fn unit_box(c: Vec3) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let mut pts = Vec::with_capacity(8);
    for z in [-0.5, 0.5] {
        for y in [-0.5, 0.5] {
            for x in [-0.5, 0.5] {
                pts.push(c + Vec3::new(x, y, z));
            }
        }
    }
    let quads = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
    (pts, quads.iter().flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]]).collect())
}

fn rigid(a: &[String]) -> Result<(), String> {
    let n: usize = number::<usize>(a, "--bodies", 1000)?.clamp(1, 4096);
    let frames: u32 = number::<u32>(a, "--frames", 120)?.clamp(2, 10_000);
    let substeps: u32 = number::<u32>(a, "--substeps", 4)?.clamp(1, 64);
    // A column grid of boxes, each a little off its neighbour so they topple into a pile.
    let side = (n as f32).cbrt().ceil() as usize;
    let mut bodies = Vec::with_capacity(n);
    for i in 0..n {
        let (x, y, z) = (i % side, (i / side) % side, i / (side * side));
        let jitter = ((i * 7919) % 100) as f32 / 400.0;
        let c = Vec3::new(x as f32 * 1.3 + jitter, 0.6 + z as f32 * 1.4, y as f32 * 1.3 - jitter);
        let (points, triangles) = unit_box(c);
        bodies.push(BodyDesc { id: i as u64, points, triangles, settings: RigidSettings::default(), motion: Vec::new() });
    }
    let t = Instant::now();
    let mut world = RigidWorld::new(WorldDesc { gravity: Vec3::new(0.0, -9.81, 0.0), ground: Some(0.0), bodies }).map_err(|e| e.to_string())?;
    let setup = ms(t);
    let cfg = BakeConfig { timing: SimTiming { start_frame: 1.0, end_frame: f64::from(frames), fps: 24.0, substeps }, seed: 1, ..BakeConfig::default() };
    let t = Instant::now();
    let cache = bake(&mut world, &cfg, &mut Silent).map_err(|e| e.to_string())?;
    let solve = ms(t);
    println!(
        "{{\"sim\":\"rigid\",\"bodies\":{n},\"points\":{},\"frames\":{},\"substeps\":{substeps},\"setup_ms\":{setup:.1},\"solve_ms\":{solve:.1},\"ms_per_frame\":{:.2},\"ms_per_step\":{:.3},\"asleep_at_end\":{},\"cache_mib\":{:.1}}}",
        world.point_count(),
        cache.frames(),
        solve / f64::from(frames),
        solve / (f64::from(frames) * f64::from(substeps)),
        world.sleeping(),
        cache.bytes() as f64 / (1 << 20) as f64
    );
    Ok(())
}

/// A UV sphere of radius `r` at `c` as a collider.
fn sphere(c: Vec3, r: f32) -> ColliderMesh {
    let (seg, ring) = (48u32, 24u32);
    let mut pos = Vec::new();
    for j in 0..=ring {
        let phi = std::f32::consts::PI * j as f32 / ring as f32;
        for i in 0..seg {
            let th = std::f32::consts::TAU * i as f32 / seg as f32;
            pos.push(c + r * Vec3::new(phi.sin() * th.cos(), phi.cos(), phi.sin() * th.sin()));
        }
    }
    let mut tris = Vec::new();
    for j in 0..ring {
        for i in 0..seg {
            let (a, b) = (j * seg + i, j * seg + (i + 1) % seg);
            tris.push([a, a + seg, b]);
            tris.push([b, a + seg, b + seg]);
        }
    }
    ColliderMesh { positions: std::sync::Arc::new(pos), triangles: std::sync::Arc::new(tris) }
}

fn cloth(a: &[String]) -> Result<(), String> {
    let n: usize = number::<usize>(a, "--vertices", 10_000)?.clamp(16, 1_000_000);
    let frames: u32 = number::<u32>(a, "--frames", 120)?.clamp(2, 10_000);
    let substeps: u32 = number::<u32>(a, "--substeps", 8)?.clamp(1, 64);
    let iterations: u32 = number::<u32>(a, "--iterations", 2)?.clamp(1, 16);
    let (self_collision, with_sphere) = (a.iter().any(|x| x == "--self"), a.iter().any(|x| x == "--sphere"));
    // A square sheet two metres across, a metre above the sphere, a quad per cell.
    let side = (n as f32).sqrt().round().max(4.0) as u32;
    let mut points = Vec::with_capacity(((side + 1) * (side + 1)) as usize);
    for j in 0..=side {
        for i in 0..=side {
            points.push(Vec3::new(-1.0 + 2.0 * i as f32 / side as f32, 1.0, -1.0 + 2.0 * j as f32 / side as f32));
        }
    }
    let at = |i: u32, j: u32| j * (side + 1) + i;
    let polygons: Vec<Vec<u32>> =
        (0..side).flat_map(|j| (0..side).map(move |i| (i, j))).map(|(i, j)| vec![at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]).collect();
    let vertices = points.len();
    let settings =
        ClothSettings { iterations, self_collision, thickness: (2.0 / side as f32) * 0.5, pinned: vec![at(0, 0), at(side, 0)], ..ClothSettings::default() };
    let provider: Option<ColliderProvider> = with_sphere.then(|| {
        let c = vec![sphere(Vec3::new(0.0, 0.2, 0.0), 0.45)];
        Box::new(move |_: f64| c.clone()) as ColliderProvider
    });
    let t = Instant::now();
    let mut solver =
        ClothSolver::new(ClothDesc { points, polygons, settings, pin_motion: Vec::new(), start_frame: 1.0 }, provider).map_err(|e| e.to_string())?;
    let setup = ms(t);
    let (stretch, shear, bend) = solver.constraint_counts();
    let cfg = BakeConfig { timing: SimTiming { start_frame: 1.0, end_frame: f64::from(frames), fps: 24.0, substeps }, seed: 1, ..BakeConfig::default() };
    let t = Instant::now();
    let cache = bake(&mut solver, &cfg, &mut Silent).map_err(|e| e.to_string())?;
    let solve = ms(t);
    println!(
        "{{\"sim\":\"cloth\",\"vertices\":{vertices},\"constraints\":{},\"frames\":{},\"substeps\":{substeps},\"iterations\":{iterations},\"self_collision\":{self_collision},\"sphere\":{with_sphere},\"setup_ms\":{setup:.1},\"solve_ms\":{solve:.1},\"ms_per_frame\":{:.2},\"ms_per_substep\":{:.3},\"max_stretch\":{:.3},\"cache_mib\":{:.1}}}",
        stretch + shear + bend,
        cache.frames(),
        solve / f64::from(frames),
        solve / (f64::from(frames) * f64::from(substeps)),
        solver.max_stretch(),
        cache.bytes() as f64 / (1 << 20) as f64
    );
    Ok(())
}
