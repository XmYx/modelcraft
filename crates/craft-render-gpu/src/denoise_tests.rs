//! The GPU film carries the second half rendering the regression filter needs.

use std::sync::Arc;

use craft_render_core::{RenderSettings, relative_mse, render_image, scenes};

use crate::GpuRenderer;
use crate::tests::gpu;

#[test]
fn the_gpu_film_has_the_same_halves_as_the_cpu_film() {
    let Some(ctx) = gpu() else { return };
    let scene = scenes::cornell_box().unwrap();
    let settings = RenderSettings { width: 24, height: 24, seed: 3, ..RenderSettings::default() };
    let mut r = GpuRenderer::new(&ctx, &scene, settings).unwrap();
    // Two submissions: the parity of a sample is its index in the whole render, not in a pass.
    r.render(&ctx, 5).unwrap();
    r.render(&ctx, 6).unwrap();
    let g = r.film(&ctx).unwrap();
    let c = render_image(Arc::new(scene), settings, 11).unwrap();
    let ((ga, gb), (ca, cb)) = (g.halves().unwrap(), c.halves().unwrap());
    let (ea, eb) = (relative_mse(&ga, &ca), relative_mse(&gb, &cb));
    assert!(ea < 1e-4 && eb < 1e-4, "{ea} {eb}");
    assert!(relative_mse(&ga, &gb) > 100.0 * ea, "the halves are different renderings");
}
