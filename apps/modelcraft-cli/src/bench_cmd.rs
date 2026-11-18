//! `modelcraft-cli render-bench`: error against time on the built-in benchmark scenes.
//!
//! Protocol (docs/render-research.md §8): each scene is measured against its own converged
//! reference, which is two independent half-renders averaged; their difference says how far the
//! reference itself is from converged. Every budget is rendered with several seeds; the report
//! gives the mean relative MSE with its spread, the time, and the efficiency 1 / (relMSE × time).
//! CPU and GPU are separate runs and are never mixed in one table.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use modelcraft_render::pathtrace::{DenoiseMode, Film, PixelOrder, RenderSettings, Renderer, RestirSettings, Scene, relative_mse, scenes};
use modelcraft_render::pathtrace_gpu::{GpuContext, GpuRenderer};

pub const USAGE: &str = "  render-bench [options]            error against time on the benchmark scenes (JSON lines, then a table)
      --scenes a,b,…                default: every benchmark scene
      --size N                      square image side (default 256)
      --spp 1,4,16,64               sample budgets (default 1,4,16,64,256)
      --seeds N                     independent runs per budget (default 5)
      --reference N                 samples of the reference (default 4096)
      --device cpu|gpu              default cpu
      --order z|independent         pixel order of the sample sequence (default z)
      --guiding                     learn where light comes from and sample that way (CPU)
      --splitting                   end or multiply paths by their expected contribution (CPU)
      --restir unbiased|viewport    resampled direct light at the first surface (CPU and GPU)
      --restir-candidates N --restir-neighbors N
      --denoise [svgf|nfor]         also filter each image and report its error and the filter's time
                                    (the reference is always rendered without either)";

pub struct BenchArgs {
    pub scenes: Vec<String>,
    pub size: u32,
    pub budgets: Vec<u32>,
    pub seeds: u32,
    pub reference: u32,
    pub gpu: bool,
    pub order: PixelOrder,
    pub guiding: bool,
    pub splitting: bool,
    pub restir: RestirSettings,
    pub denoise: DenoiseMode,
}

fn value<'a>(a: &'a [String], name: &str) -> Option<&'a str> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).map(String::as_str)
}

pub fn parse(a: &[String]) -> Result<BenchArgs, String> {
    let number = |name: &str, default: u32| -> Result<u32, String> {
        value(a, name).map_or(Ok(default), |v| v.parse().map_err(|_| format!("{name}: {v:?} is not a number")))
    };
    let budgets = match value(a, "--spp") {
        None => vec![1, 4, 16, 64, 256],
        Some(v) => v.split(',').map(|p| p.trim().parse::<u32>().map_err(|_| format!("--spp: {p:?} is not a number"))).collect::<Result<Vec<_>, _>>()?,
    };
    let scenes = value(a, "--scenes")
        .map_or_else(|| scenes::BENCHMARKS.iter().map(|s| s.to_string()).collect(), |v| v.split(',').map(|s| s.trim().to_string()).collect());
    Ok(BenchArgs {
        scenes,
        size: number("--size", 256)?.clamp(16, 2048),
        budgets: budgets.into_iter().filter(|b| *b > 0).collect(),
        seeds: number("--seeds", 5)?.clamp(1, 64),
        reference: number("--reference", 4096)?.clamp(2, 1 << 20),
        gpu: match value(a, "--device") {
            None | Some("cpu") => false,
            Some("gpu") => true,
            Some(other) => return Err(format!("--device: expected cpu or gpu, found {other:?}")),
        },
        order: match value(a, "--order") {
            None | Some("z") => PixelOrder::ZCurve,
            Some("independent") => PixelOrder::Independent,
            Some(other) => return Err(format!("--order: expected z or independent, found {other:?}")),
        },
        guiding: a.iter().any(|x| x == "--guiding"),
        splitting: a.iter().any(|x| x == "--splitting"),
        restir: crate::render_cmd::parse_restir(a)?,
        denoise: crate::render_cmd::parse_denoise(a)?,
    })
}

/// Render `spp` samples and return the film with the seconds spent rendering.
fn run(scene: &Arc<Scene>, gpu: Option<&GpuContext>, settings: RenderSettings, spp: u32) -> Result<(Film, f64), String> {
    match gpu {
        Some(ctx) => {
            let mut r = GpuRenderer::new(ctx, scene, settings).map_err(|e| e.to_string())?;
            let t = Instant::now();
            r.render(ctx, spp).map_err(|e| e.to_string())?;
            let seconds = t.elapsed().as_secs_f64();
            Ok((r.film(ctx).map_err(|e| e.to_string())?, seconds))
        }
        None => {
            let mut r = Renderer::new(scene.clone(), settings).map_err(|e| e.to_string())?;
            let t = Instant::now();
            r.render(spp, &AtomicBool::new(false));
            Ok((r.film().clone(), t.elapsed().as_secs_f64()))
        }
    }
}

/// The middle value (the mean of the middle two for an even count); rare bright samples move it
/// far less than they move the mean.
fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    match v.len() {
        0 => 0.0,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) * 0.5,
    }
}

/// One scene's measurements as JSON.
pub fn bench_scene(name: &str, args: &BenchArgs, gpu: Option<&GpuContext>) -> Result<serde_json::Value, String> {
    let t = Instant::now();
    let scene = Arc::new(scenes::by_name(name).ok_or_else(|| format!("unknown scene {name:?}"))?.map_err(|e| e.to_string())?);
    let build_seconds = t.elapsed().as_secs_f64();
    let settings = |seed: u32| RenderSettings { width: args.size, height: args.size, seed, pixel_order: args.order, ..RenderSettings::default() };
    // The technique under test; the reference is always the plain estimator, so a technique that
    // changed the expected image would show as an error that does not fall with samples.
    let tested = |seed: u32| {
        let mut s = settings(seed);
        s.integrator.guiding = args.guiding;
        s.integrator.adaptive_splitting = args.splitting;
        s.integrator.restir = args.restir;
        s
    };
    // The reference: two independent halves, compared and then averaged.
    let half = (args.reference / 2).max(1);
    let (a, _) = run(&scene, gpu, settings(0x5eed_0001), half)?;
    let (b, _) = run(&scene, gpu, settings(0x5eed_0002), half)?;
    let (a, b) = (a.mean(), b.mean());
    let reference: Vec<[f32; 3]> = a.iter().zip(&b).map(|(p, q)| [(p[0] + q[0]) * 0.5, (p[1] + q[1]) * 0.5, (p[2] + q[2]) * 0.5]).collect();
    // The average of two halves has a quarter of the squared difference between them as its own error.
    let reference_error = relative_mse(&a, &b) / 4.0;
    let mut rows = Vec::new();
    for spp in &args.budgets {
        let (mut errors, mut seconds) = (Vec::new(), Vec::new());
        let (mut filtered_errors, mut filter_seconds) = (Vec::new(), Vec::new());
        for seed in 0..args.seeds {
            let (film, s) = run(&scene, gpu, tested(seed + 1), *spp)?;
            errors.push(relative_mse(&film.mean(), &reference));
            seconds.push(s);
            if args.denoise != DenoiseMode::Off {
                let t = Instant::now();
                let image = film.denoise(args.denoise).map_err(|e| e.to_string())?;
                filter_seconds.push(t.elapsed().as_secs_f64());
                filtered_errors.push(relative_mse(&image, &reference));
            }
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let (e, s) = (mean(&errors), mean(&seconds));
        let denoised = (!filtered_errors.is_empty()).then(|| (mean(&filtered_errors), median(&filtered_errors), mean(&filter_seconds)));
        rows.push(serde_json::json!({
            "denoised_rel_mse": denoised.map(|d| d.0),
            "denoised_rel_mse_median": denoised.map(|d| d.1),
            "denoise_seconds": denoised.map(|d| d.2),
            "spp": spp,
            "seconds": s,
            "rel_mse": e,
            "rel_mse_median": median(&errors),
            "rel_mse_min": errors.iter().copied().fold(f64::INFINITY, f64::min),
            "rel_mse_max": errors.iter().copied().fold(0.0, f64::max),
            "efficiency": if e * s > 0.0 { 1.0 / (e * s) } else { 0.0 },
        }));
    }
    Ok(serde_json::json!({
        "scene": name,
        "device": gpu.map_or_else(|| format!("cpu: {} threads", std::thread::available_parallelism().map_or(1, usize::from)), |c| format!("gpu: {}", c.caps.adapter)),
        "size": args.size,
        "guiding": args.guiding,
        "splitting": args.splitting,
        "restir": format!("{:?}", args.restir.mode),
        "denoise": format!("{:?}", args.denoise),
        "triangles": scene.triangle_count(),
        "build_seconds": build_seconds,
        "reference_spp": half * 2,
        "reference_rel_mse": reference_error,
        "seeds": args.seeds,
        "budgets": rows,
    }))
}

/// Seconds to reach a relative MSE, interpolated on the log–log error-against-time line.
pub fn time_to(rows: &[serde_json::Value], target: f64) -> Option<f64> {
    let points: Vec<(f64, f64)> =
        rows.iter().filter_map(|r| Some((r["seconds"].as_f64()?, r["rel_mse"].as_f64()?))).filter(|(s, e)| *s > 0.0 && *e > 0.0).collect();
    for pair in points.windows(2) {
        let ((s0, e0), (s1, e1)) = (pair[0], pair[1]);
        if e0 >= target && e1 <= target && e0 > e1 {
            let k = (e0.ln() - target.ln()) / (e0.ln() - e1.ln());
            return Some((s0.ln() + k * (s1.ln() - s0.ln())).exp());
        }
    }
    None
}

pub fn run_all(args: &BenchArgs) -> Result<(), String> {
    let gpu = if args.gpu { Some(GpuContext::new().ok_or("no GPU adapter is available")?) } else { None };
    let mut table = String::from(
        "| Scene | Triangles | spp | Seconds | relMSE mean (min–max) | relMSE median | Efficiency | Seconds to relMSE 0.01 |\n|---|---|---|---|---|---|---|---|\n",
    );
    let mut filtered = String::from("| Scene | spp | relMSE before | relMSE after | Times lower | Filter ms per image |\n|---|---|---|---|---|---|\n");
    for name in &args.scenes {
        match bench_scene(name, args, gpu.as_ref()) {
            Ok(report) => {
                println!("{report}");
                for r in report["budgets"].as_array().into_iter().flatten() {
                    if let (Some(after), Some(secs)) = (r["denoised_rel_mse"].as_f64(), r["denoise_seconds"].as_f64()) {
                        let before = r["rel_mse"].as_f64().unwrap_or(0.0);
                        filtered.push_str(&format!(
                            "| {name} | {} | {before:.4} | {after:.4} | {:.1} | {:.1} |\n",
                            r["spp"],
                            before / after.max(1e-12),
                            secs * 1000.0
                        ));
                    }
                }
                let rows = report["budgets"].as_array().cloned().unwrap_or_default();
                let reach = time_to(&rows, 0.01).map_or_else(|| "not within these budgets".to_string(), |s| format!("{s:.2}"));
                for (i, r) in rows.iter().enumerate() {
                    let f = |k: &str| r[k].as_f64().unwrap_or(0.0);
                    table.push_str(&format!(
                        "| {} | {} | {} | {:.3} | {:.4} ({:.4}–{:.4}) | {:.4} | {:.1} | {} |\n",
                        if i == 0 { name.as_str() } else { "" },
                        if i == 0 { report["triangles"].to_string() } else { String::new() },
                        r["spp"],
                        f("seconds"),
                        f("rel_mse"),
                        f("rel_mse_min"),
                        f("rel_mse_max"),
                        f("rel_mse_median"),
                        f("efficiency"),
                        if i == 0 { reach.as_str() } else { "" },
                    ));
                }
                eprintln!("{name}: reference {} spp, its own relMSE about {:.5}", report["reference_spp"], report["reference_rel_mse"].as_f64().unwrap_or(0.0));
            }
            Err(e) => {
                println!("{}", serde_json::json!({ "scene": name, "declined": e }));
                table.push_str(&format!("| {name} | | | | not rendered: {e} | | |\n"));
            }
        }
    }
    println!("\n{table}");
    if args.denoise != DenoiseMode::Off {
        println!("{:?} filter:\n\n{filtered}", args.denoise);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmarks_one_scene_and_interpolates_time_to_error() {
        let a = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let args = parse(&a(&["--scenes", "cornell", "--size", "24", "--spp", "1,4,16", "--seeds", "2", "--reference", "128"])).unwrap();
        assert_eq!((args.scenes.len(), args.size, args.budgets.clone(), args.seeds, args.gpu), (1, 24, vec![1, 4, 16], 2, false));
        let report = bench_scene("cornell", &args, None).unwrap();
        let rows = report["budgets"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        // Error falls as samples rise, and each row's spread brackets its mean.
        let e = |i: usize| rows[i]["rel_mse"].as_f64().unwrap();
        assert!(e(0) > e(1) && e(1) > e(2), "{} {} {}", e(0), e(1), e(2));
        assert!(rows[0]["rel_mse_min"].as_f64().unwrap() <= e(0) && e(0) <= rows[0]["rel_mse_max"].as_f64().unwrap());
        assert!(report["reference_rel_mse"].as_f64().unwrap() < e(2));
        // A made-up curve: error 0.04 at 1 s and 0.0025 at 16 s crosses 0.01 at 4 s on a log–log line.
        let curve = [serde_json::json!({"seconds": 1.0, "rel_mse": 0.04}), serde_json::json!({"seconds": 16.0, "rel_mse": 0.0025})];
        assert!((time_to(&curve, 0.01).unwrap() - 4.0).abs() < 1e-6);
        assert!(time_to(&curve, 1e-6).is_none());
        assert!(bench_scene("teapot", &args, None).is_err());
        assert!(parse(&a(&["--device", "abacus"])).is_err() && parse(&a(&["--spp", "many"])).is_err());
    }
}
