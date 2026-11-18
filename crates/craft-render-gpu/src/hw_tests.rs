//! The hardware ray-query backend against the software traversal, and the fallback to software.

use craft_render_core::integrator::IntegratorSettings;
use craft_render_core::{RenderSettings, relative_mse, scenes};

use crate::tests::gpu;
use crate::{GpuOptions, GpuRenderer};

#[test]
fn asking_for_hardware_rays_falls_back_to_software_when_they_are_not_available() {
    let Some(ctx) = gpu() else { return };
    // This context was not opened with ray queries, so whatever is asked, the software path runs.
    assert!(!ctx.caps.hardware_rays);
    let settings = RenderSettings { width: 16, height: 16, ..RenderSettings::default() };
    let scene = scenes::cornell_box().unwrap();
    let mut r = GpuRenderer::with_options(&ctx, &scene, settings, GpuOptions { hardware_rays: true }).unwrap();
    assert!(!r.uses_hardware());
    r.render(&ctx, 2).unwrap();
    assert_eq!(r.film(&ctx).unwrap().samples, 2);
    // The default is software.
    assert!(!GpuOptions::default().hardware_rays);
}

#[cfg(feature = "hw-ray-query")]
mod hardware {
    use super::*;
    use crate::GpuContext;

    /// Closest-hit results of fixed rays (the first surface of every pixel's camera ray: its
    /// depth, albedo and normal) and a full lit image, on one device, in software and in hardware.
    #[test]
    fn hardware_and_software_ray_queries_agree_on_fixed_rays() {
        let Some(ctx) = GpuContext::new_hardware() else {
            eprintln!("skipped: no adapter with ray query support on this machine; the software path covers this");
            return;
        };
        let _turn = crate::tests::gpu_turn();
        eprintln!("hardware rays on {} ({})", ctx.caps.adapter, ctx.caps.backend);
        for name in ["cornell", "materials", "night-street"] {
            let scene = scenes::by_name(name).unwrap().unwrap();
            if scene.has_cutouts {
                continue;
            }
            let settings = RenderSettings {
                width: 64,
                height: 48,
                seed: 3,
                integrator: IntegratorSettings { max_depth: 4, ..IntegratorSettings::default() },
                ..RenderSettings::default()
            };
            let run = |hardware: bool| {
                let mut r = GpuRenderer::with_options(&ctx, &scene, settings, GpuOptions { hardware_rays: hardware }).unwrap();
                assert_eq!(r.uses_hardware(), hardware, "{name}");
                r.render(&ctx, 8).unwrap();
                r.film(&ctx).unwrap()
            };
            let (soft, hard) = (run(false), run(true));
            // Fixed rays: the first-hit depth of each pixel agrees (rays that graze an edge may
            // fall either side of it, so a tiny share of pixels may differ).
            let close = |a: f32, b: f32| (a - b).abs() <= 2e-3 * a.abs().max(b.abs()).max(1e-3);
            let bad = soft.depth.iter().zip(&hard.depth).filter(|(a, b)| !close(**a, **b)).count();
            assert!(bad * 200 <= soft.depth.len(), "{name}: {bad} of {} first-hit depths differ", soft.depth.len());
            let err = relative_mse(&soft.mean(), &hard.mean());
            eprintln!("{name}: {bad} depths differ, image relative MSE {err:.7}");
            assert!(err < 1e-3, "{name}: images differ by {err}");
        }
    }

    /// A timing, not a check: `cargo test --release -p craft-render-gpu --features hw-ray-query -- --ignored --nocapture bench_hardware`.
    #[test]
    #[ignore = "benchmark"]
    fn bench_hardware_rays() {
        let Some(ctx) = GpuContext::new_hardware() else { return };
        let _turn = crate::tests::gpu_turn();
        let scene = crate::wide_tests::height_field_scene();
        let settings = RenderSettings { width: 960, height: 540, ..RenderSettings::default() };
        for hardware in [false, true] {
            let t = std::time::Instant::now();
            let mut r = GpuRenderer::with_options(&ctx, &scene, settings, GpuOptions { hardware_rays: hardware }).unwrap();
            let setup = t.elapsed().as_secs_f64();
            r.render(&ctx, 2).unwrap();
            let t = std::time::Instant::now();
            r.render(&ctx, 16).unwrap();
            let s = t.elapsed().as_secs_f64();
            eprintln!(
                "{}: setup {:.0} ms, 16 spp at 960x540 in {:.0} ms = {:.0} M paths/s",
                if hardware { "hardware" } else { "software" },
                setup * 1e3,
                s * 1e3,
                960.0 * 540.0 * 16.0 / s / 1e6
            );
        }
    }
}
