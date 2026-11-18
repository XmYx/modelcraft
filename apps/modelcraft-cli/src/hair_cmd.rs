//! `modelcraft-cli hair-bench`: grow guides on a sphere, interpolate render hairs, report time and
//! memory. Reproduces the numbers in `docs/performance.md` ("Hair").

use std::time::Instant;

use modelcraft_hair::{Amount, Blend, Clump, Frizz, GrowSpec, InterpSpec, Limits, Surface, SurfaceMaps, estimate_bytes, grow_guides, interpolate};
use modelcraft_mesh::PolyMesh;

pub const USAGE: &str = "  hair-bench [--strands N] [--guides N] [--points N] [--segments N] [--blend bary|knn] [--clump A] [--frizz A] [--gpu]
                                    grow guides on a sphere and interpolate render hairs: time,
                                    strands per second and memory (defaults: 1000000 strands,
                                    5000 guides, 8 points, 128 segments, knn, no clump or frizz);
                                    --gpu then draws the hairs at 1920x1080 at several display
                                    percentages and reports frame times (needs a GPU adapter)";

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
    let strands: usize = number(a, "--strands", 1_000_000)?;
    let guides: usize = number(a, "--guides", 5000)?;
    let points: usize = number(a, "--points", 8)?;
    let segments: u32 = number::<u32>(a, "--segments", 128)?.clamp(4, 512);
    let clump: f32 = number(a, "--clump", 0.0)?;
    let frizz: f32 = number(a, "--frizz", 0.0)?;
    let blend = match value(a, "--blend") {
        None | Some("knn") => Blend::Nearest { k: 4 },
        Some("bary") => Blend::Barycentric,
        Some(o) => return Err(format!("--blend: expected bary or knn, found {o:?}")),
    };
    let limits = Limits::default();
    let t = Instant::now();
    let mesh: PolyMesh =
        modelcraft_ops::primitives::Primitive::Sphere { radius: 1.0, segments, rings: (segments / 2).max(2) }.build().map_err(|e| e.to_string())?;
    let surface = Surface::from_mesh(&mesh).map_err(|e| e.to_string())?;
    println!("surface: {} triangles, area {:.2}, built in {:.0} ms", surface.triangle_count(), surface.area(), ms(t));

    let t = Instant::now();
    let spec = GrowSpec { amount: Amount::Count(guides), length: 0.25, length_jitter: 0.2, points_per_strand: points, ..GrowSpec::default() };
    let g = grow_guides(&surface, &spec, &SurfaceMaps::default(), &limits).map_err(|e| e.to_string())?;
    println!("guides: {} x {} points in {:.1} ms", g.len(), points, ms(t));

    let spec = InterpSpec {
        amount: Amount::Count(strands),
        blend,
        clump: Clump { amount: clump, tip_bias: 1.0, jitter: 0.2 },
        frizz: Frizz { amount: frizz, frequency: 4.0 },
        ..InterpSpec::default()
    };
    let t = Instant::now();
    let h = interpolate(&surface, &g, &spec, &SurfaceMaps::default(), &limits).map_err(|e| e.to_string())?;
    let took = ms(t);
    println!(
        "interpolate: {} strands x {} points in {:.0} ms ({:.2} M strands/s, {} threads), {:.0} MB held (estimate {:.0} MB)",
        h.strand_count(),
        h.points_per_strand(),
        took,
        h.strand_count() as f64 / took / 1000.0,
        std::thread::available_parallelism().map_or(1, |n| n.get()),
        h.bytes() as f64 / 1e6,
        estimate_bytes(strands, points) as f64 / 1e6
    );
    if a.iter().any(|x| x == "--gpu") {
        gpu_bench(&mesh, &h)?;
    }
    Ok(())
}

/// Draw the groom at 1920x1080 with the sphere under it, orbiting the camera, at several display
/// percentages: mean and 95th percentile frame time (render, submit and wait for the GPU).
fn gpu_bench(mesh: &PolyMesh, hair: &modelcraft_hair::HairCurves) -> Result<(), String> {
    use modelcraft_render::Camera;
    use modelcraft_render::gpu::{DrawItem, Frame, GpuMesh, Renderer, Target};
    use modelcraft_render::hair_gpu::{GpuHair, HairDraw};
    let Some((device, queue, name)) = modelcraft_render::gpu::headless() else {
        println!("gpu: no adapter, skipped");
        return Ok(());
    };
    println!("gpu: {name}");
    let t = Instant::now();
    let gpu_hair = GpuHair::new(&device, hair.all_points(), hair.points_per_strand()).ok_or("nothing to draw")?;
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    println!("upload: {} strands, {:.0} MB of GPU buffers in {:.0} ms", gpu_hair.strands, gpu_hair.bytes() as f64 / 1e6, ms(t));
    let sphere = GpuMesh::new(&device, &modelcraft_render::buffers::mesh_data(mesh));
    let target = Target::new(&device, [1920, 1080]);
    let mut renderer = Renderer::new(&device);
    for percent in [100.0f32, 50.0, 10.0, 1.0] {
        let mut times = Vec::new();
        for i in 0..70 {
            let cam = Camera { distance: 3.2, yaw: i as f32 * 0.05, ..Default::default() };
            let frame = Frame {
                view_proj: cam.view_proj(1920.0 / 1080.0),
                eye: cam.eye(),
                clear: [0.1, 0.1, 0.1, 1.0],
                draws: vec![DrawItem {
                    mesh: &sphere,
                    model: glam::Mat4::IDENTITY,
                    color: [0.5; 4],
                    selected: false,
                    show_mask: false,
                    patch_tint: false,
                    uv_checker: false,
                    roughness: 0.6,
                }],
                lines: &[],
                overlay: &[],
                hair: vec![HairDraw { hair: &gpu_hair, model: glam::Mat4::IDENTITY, color: [0.55, 0.38, 0.22, 0.9], fraction: percent / 100.0 }],
            };
            let t = Instant::now();
            renderer.render(&device, &queue, &target, &frame);
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
            if i >= 10 {
                times.push(ms(t));
            }
        }
        times.sort_by(f64::total_cmp);
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        let p95 = times.get(((times.len() as f64) * 0.95) as usize).copied().unwrap_or(mean);
        println!(
            "draw {percent:>5.1} %: {} strands, mean {mean:.2} ms ({:.0} fps), 95th percentile {p95:.2} ms",
            gpu_hair.strands_for(percent / 100.0),
            1000.0 / mean
        );
    }
    Ok(())
}
