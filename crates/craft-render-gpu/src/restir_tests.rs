//! The GPU stages of resampled direct lighting against the CPU reference
//! (`craft_render_core::restir`).

use std::sync::Arc;

use craft_render_core::integrator::IntegratorSettings;
use craft_render_core::{RenderSettings, RestirMode, RestirSettings, Scene, mean_rgb, relative_mse, render_image, scenes};

use crate::GpuRenderer;
use crate::tests::gpu;

fn settings(size: u32, mode: RestirMode, depth: u32) -> RenderSettings {
    RenderSettings {
        width: size,
        height: size,
        seed: 7,
        integrator: IntegratorSettings { max_depth: depth, restir: RestirSettings { mode, ..RestirSettings::default() }, ..IntegratorSettings::default() },
        ..RenderSettings::default()
    }
}

/// Both devices on the same samples; `None` without an adapter (or one with too few buffers).
fn both(scene: Scene, size: u32, spp: u32, mode: RestirMode, depth: u32) -> Option<(Vec<[f32; 3]>, Vec<[f32; 3]>)> {
    let ctx = gpu()?;
    if ctx.caps.max_storage_buffers < 10 {
        eprintln!("skipped: this device binds only {} storage buffers per stage", ctx.caps.max_storage_buffers);
        return None;
    }
    let s = settings(size, mode, depth);
    let mut r = GpuRenderer::new(&ctx, &scene, s).unwrap();
    r.render(&ctx, spp).unwrap();
    let g = r.film(&ctx).unwrap();
    assert_eq!(g.samples, spp);
    let c = render_image(Arc::new(scene), s, spp).unwrap();
    Some((g.mean(), c.mean()))
}

#[test]
fn gpu_resampling_matches_the_cpu_reference() {
    for (name, mode) in
        [("night-street", RestirMode::Unbiased), ("night-street", RestirMode::Viewport), ("studio", RestirMode::Unbiased), ("interior", RestirMode::Unbiased)]
    {
        let scene = scenes::by_name(name).unwrap().unwrap();
        let Some((g, c)) = both(scene, 48, 32, mode, 3) else { return };
        // The same numbers drive both devices; only floating-point rounding can send a
        // reservoir's choice elsewhere.
        let err = relative_mse(&g, &c);
        let (gm, cm) = (mean_rgb(&g), mean_rgb(&c));
        eprintln!("{name} {mode:?}: relative MSE between GPU and CPU {err:.6}, means {gm:?} {cm:?}");
        assert!(err < 1e-4, "{name} {mode:?}: relative MSE between GPU and CPU {err}");
        for k in 0..3 {
            assert!((gm[k] - cm[k]).abs() < 0.03 * cm[k].max(1e-3), "{name} {mode:?}: {gm:?} against {cm:?}");
        }
    }
}

#[test]
fn gpu_resampling_is_progressive_and_declines_what_it_cannot_do() {
    let Some(ctx) = gpu() else { return };
    if ctx.caps.max_storage_buffers < 10 {
        return;
    }
    let scene = scenes::cornell_box().unwrap();
    let s = settings(16, RestirMode::Unbiased, 3);
    let mut r = GpuRenderer::new(&ctx, &scene, s).unwrap();
    r.render(&ctx, 3).unwrap();
    r.render(&ctx, 5).unwrap();
    assert_eq!(r.film(&ctx).unwrap().samples, 8);
    // A device with fewer storage buffers than the stages bind says so instead of failing.
    let few = crate::GpuContext { device: ctx.device.clone(), queue: ctx.queue.clone(), caps: crate::Caps { max_storage_buffers: 8, ..ctx.caps.clone() } };
    assert!(matches!(GpuRenderer::new(&few, &scene, s), Err(crate::GpuError::Unsupported(_))));
    // Without lights there is nothing to resample, and the plain kernel runs.
    let dark = craft_render_core::SceneBuilder::new().build();
    let mut r = GpuRenderer::new(&ctx, &dark, s).unwrap();
    r.render(&ctx, 2).unwrap();
    assert_eq!(r.film(&ctx).unwrap().samples, 2);
}
