//! `modelcraft-cli sim-bench`: bake a simulation of a given size and report the solve time.
//! Reproduces the numbers in `docs/performance.md` ("Simulation").

use std::time::Instant;

use glam::Vec3;
use modelcraft_engine::simulation::rigid::{BodyDesc, RigidSettings, RigidWorld, WorldDesc};
use modelcraft_engine::simulation::{BakeConfig, Silent, SimTiming, Solver, bake};

pub const USAGE: &str = "  sim-bench rigid [--bodies N] [--frames N] [--substeps N]
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
        Some(other) => Err(format!("sim-bench: unknown simulation {other:?} (rigid)")),
        None => Err("sim-bench: which simulation? (rigid)".into()),
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
