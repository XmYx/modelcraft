//! The compressed 8-wide hierarchy on the GPU and the CPU against the binary one.

use std::sync::Arc;

use craft_render_core::{RenderSettings, Scene, relative_mse, render_image, scenes};

use crate::GpuRenderer;
use crate::tests::gpu;

fn render_gpu(ctx: &crate::GpuContext, scene: &Scene, settings: RenderSettings, spp: u32) -> Vec<[f32; 3]> {
    let mut r = GpuRenderer::new(ctx, scene, settings).unwrap();
    r.render(ctx, spp).unwrap();
    r.film(ctx).unwrap().mean()
}

#[test]
fn wide_trees_give_the_same_pictures_on_both_devices() {
    for name in ["cornell", "showcase", "night-street"] {
        let binary = scenes::by_name(name).unwrap().unwrap();
        let mut wide = binary.clone();
        assert!(wide.enable_wide_bvh(), "{name} collapses");
        assert!(wide.accel.wide.is_some());
        let settings = RenderSettings { width: 40, height: 40, seed: 4, ..RenderSettings::default() };
        // CPU: the same hits, so the same image (up to nothing at all).
        let (cb, cw) = (render_image(Arc::new(binary.clone()), settings, 8).unwrap().mean(), render_image(Arc::new(wide.clone()), settings, 8).unwrap().mean());
        let e = relative_mse(&cw, &cb);
        assert!(e < 1e-9, "{name}: the CPU's wide and binary images differ by {e}");
        let Some(ctx) = gpu() else { continue };
        let (gb, gw) = (render_gpu(&ctx, &binary, settings, 8), render_gpu(&ctx, &wide, settings, 8));
        let (e_gpu, e_cpu) = (relative_mse(&gw, &gb), relative_mse(&gw, &cb));
        eprintln!("{name}: GPU wide against GPU binary {e_gpu:.7}, against CPU binary {e_cpu:.7}");
        assert!(e_gpu < 1e-4 && e_cpu < 1e-4, "{name}: {e_gpu} {e_cpu}");
    }
}

/// A 2-million-triangle height field lit by a sun and a sky (the benchmark scene).
pub(crate) fn height_field_scene() -> Scene {
    use craft_render_core::shapes::MeshData;
    use craft_render_core::{Camera, Light, Material, SceneBuilder};
    use glam::{Mat4, Vec3};
    let n = 1000usize;
    let height = |x: usize, y: usize| ((x as f32 * 0.05).sin() + (y as f32 * 0.07).cos()) * 3.0;
    let mut mesh = MeshData::default();
    for y in 0..=n {
        for x in 0..=n {
            mesh.positions.push(Vec3::new(x as f32 * 0.1, height(x, y), y as f32 * 0.1));
        }
    }
    let at = |x: usize, y: usize| (y * (n + 1) + x) as u32;
    for y in 0..n {
        for x in 0..n {
            mesh.indices.push([at(x, y), at(x + 1, y), at(x, y + 1)]);
            mesh.indices.push([at(x + 1, y), at(x + 1, y + 1), at(x, y + 1)]);
        }
    }
    let mut b = SceneBuilder::new();
    let grey = b.add_material(Material::diffuse(Vec3::splat(0.6))).unwrap();
    b.add_mesh(&mesh, Mat4::IDENTITY, grey).unwrap();
    b.add_light(Light::Sun { direction: Vec3::new(0.4, 0.8, 0.3), irradiance: Vec3::splat(3.0), angular_radius: 0.02 }).unwrap();
    b.set_environment(Vec3::new(0.3, 0.4, 0.6));
    b.set_camera(Camera { eye: Vec3::new(50.0, 12.0, -20.0), target: Vec3::new(50.0, 0.0, 50.0), ..Camera::default() });
    b.build()
}

/// A timing, not a check: `cargo test --release -p craft-render-gpu -- --ignored --nocapture bench_wide`
/// (the height field at 960×540, 16 samples).
#[test]
#[ignore = "benchmark"]
fn bench_wide_traversal_on_the_gpu() {
    let Some(ctx) = gpu() else { return };
    let binary = height_field_scene();
    let mut wide = binary.clone();
    let t = std::time::Instant::now();
    assert!(wide.enable_wide_bvh());
    eprintln!(
        "{} triangles; collapse to 8-wide {:.0} ms, {} nodes against {} binary nodes",
        binary.triangle_count(),
        t.elapsed().as_secs_f64() * 1e3,
        wide.accel.wide.as_ref().map_or(0, |w| w.nodes.len()),
        binary.accel.bvh.nodes.len()
    );
    let settings = RenderSettings { width: 960, height: 540, ..RenderSettings::default() };
    for (label, scene) in [("binary", &binary), ("8-wide", &wide)] {
        let mut r = GpuRenderer::new(&ctx, scene, settings).unwrap();
        r.render(&ctx, 2).unwrap(); // warm up
        let t = std::time::Instant::now();
        r.render(&ctx, 16).unwrap();
        let s = t.elapsed().as_secs_f64();
        eprintln!("{label}: 16 spp at 960x540 in {:.0} ms = {:.0} M paths/s", s * 1e3, 960.0 * 540.0 * 16.0 / s / 1e6);
    }
}
