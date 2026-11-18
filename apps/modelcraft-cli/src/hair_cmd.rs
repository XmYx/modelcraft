//! `modelcraft-cli hair-bench`: grow guides on a sphere, interpolate render hairs, report time and
//! memory. Reproduces the numbers in `docs/performance.md` ("Hair").

use std::time::Instant;

use modelcraft_hair::{Amount, Blend, Clump, Frizz, GrowSpec, InterpSpec, Limits, Surface, SurfaceMaps, estimate_bytes, grow_guides, interpolate};
use modelcraft_mesh::PolyMesh;

pub const USAGE: &str = "  hair-bench [--strands N] [--guides N] [--points N] [--segments N] [--blend bary|knn] [--clump A] [--frizz A]
                                    grow guides on a sphere and interpolate render hairs: time,
                                    strands per second and memory (defaults: 1000000 strands,
                                    5000 guides, 8 points, 128 segments, knn, no clump or frizz)";

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
    Ok(())
}
