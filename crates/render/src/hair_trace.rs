//! Hair for the path tracer: the strands of every visible hair object become one curve set each.
//!
//! Strands are made from the guides for the emitter's evaluated mesh, placed by the emitter's world
//! matrix, with a radius at every point from the strand's width and taper. The fibre colour is the
//! hair object's base colour and its roughness drives both the longitudinal and the azimuthal
//! roughness of the fibre model (`craft_render_core::hair`). Rendering is capped at
//! [`MAX_RENDER_STRANDS`] strands per hair object (a prefix of the groom, so a thinner but
//! unbiased subset); the GPU path tracer does not render hair yet and the CPU one is used.

use craft_render_core::hair::HairParams;
use craft_render_core::{SceneBuilder, SceneError};
use glam::{Vec3, Vec3A};
use modelcraft_scene::{Evaluator, ObjectKind, Scene};

/// Most strands one hair object contributes to a render.
pub const MAX_RENDER_STRANDS: usize = 1_000_000;

/// Add every visible hair object of `scene` to `builder`.
pub fn add_hair(builder: &mut SceneBuilder, scene: &Scene, eval: &mut Evaluator) -> Result<(), SceneError> {
    for o in scene.objects().iter().filter(|o| scene.visible(o.id)) {
        let (ObjectKind::Hair { hair }, Some(emitter)) = (&o.kind, o.parent) else { continue };
        let mesh = eval.evaluate(scene, emitter).mesh;
        let wanted = hair.strand_count(&mesh).min(MAX_RENDER_STRANDS);
        if wanted == 0 {
            continue;
        }
        let curves = match hair.strands_with_amount(&mesh, modelcraft_hair::Amount::Count(wanted)) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("hair {:?}: {e}; it is left out of the render", o.name);
                continue;
            }
        };
        if hair.strand_count(&mesh) > wanted {
            log::warn!("hair {:?}: rendering {wanted} of {} strands", o.name, hair.strand_count(&mesh));
        }
        let world = scene.world_matrix(emitter);
        let scale = world.to_scale_rotation_translation().0.abs().max_element().max(1e-6);
        let pps = curves.points_per_strand();
        let points: Vec<Vec3> = curves.all_points().iter().map(|p| world.transform_point3(*p)).collect();
        let mut radii = Vec::with_capacity(points.len());
        for i in 0..curves.strand_count() {
            for k in 0..pps {
                radii.push(0.5 * scale * curves.width_at(i, k as f32 / (pps - 1) as f32));
            }
        }
        let c = o.material.base_color;
        let mut params = HairParams::from_color(Vec3A::new(c[0], c[1], c[2]), o.material.roughness.clamp(0.05, 1.0));
        params.beta_m = o.material.roughness.clamp(0.05, 1.0);
        builder.add_curves(&points, &radii, pps, params)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use craft_render_core::{RenderSettings, mean_rgb, render_image};
    use modelcraft_hair::{Amount, GrowSpec, InterpSpec};
    use modelcraft_scene::hair::HairData;
    use modelcraft_scene::{Material, ObjectKind, Primitive, Scene, Source};

    use crate::camera::Camera;
    use crate::trace::{ExportOptions, Lighting, export_scene};

    fn head_with_hair(strands: usize) -> Scene {
        let mut s = Scene::new();
        let head = s
            .add(
                "Head",
                ObjectKind::Mesh { source: Source::Primitive { primitive: Primitive::Sphere { radius: 1.0, segments: 24, rings: 12 } }, steps: vec![] },
                None,
            )
            .unwrap();
        let mesh = modelcraft_ops::primitives::Primitive::Sphere { radius: 1.0, segments: 24, rings: 12 }.build().unwrap();
        let grow = GrowSpec { amount: Amount::Count(200), length: 0.3, points_per_strand: 6, ..GrowSpec::default() };
        let mut interp = InterpSpec { amount: Amount::Count(strands), ..InterpSpec::default() };
        interp.width.root = 0.03;
        interp.width.tip = 0.01;
        let data = HairData::grow(&mesh, grow, interp).unwrap();
        let hair = s.add("Hair", ObjectKind::Hair { hair: Arc::new(data) }, Some(head)).unwrap();
        s.edit(hair).unwrap().material = Material { base_color: [0.5, 0.3, 0.15, 1.0], roughness: 0.4, ..Material::default() };
        s
    }

    fn options() -> ExportOptions {
        ExportOptions { lighting: Lighting::Sky(glam::Vec3::ONE), ground: false, ..ExportOptions::default() }
    }

    #[test]
    fn hair_objects_become_curve_sets_and_show_in_the_picture() {
        let scene = head_with_hair(3000);
        let cam = Camera { distance: 5.0, ..Camera::default() };
        let exported = export_scene(&scene, &mut modelcraft_scene::Evaluator::default(), &cam, &options()).unwrap();
        assert_eq!(exported.curves.len(), 1);
        assert_eq!(exported.curve_segment_count(), 3000 * 5, "five segments per six-point strand");
        // Hidden hair is left out.
        let mut hidden = head_with_hair(100);
        let id = hidden.objects().iter().find(|o| matches!(o.kind, ObjectKind::Hair { .. })).unwrap().id;
        hidden.edit(id).unwrap().visible = false;
        let none = export_scene(&hidden, &mut modelcraft_scene::Evaluator::default(), &cam, &options()).unwrap();
        assert!(none.curves.is_empty());
        // The picture differs and is finite: the hair shades the head.
        let render = |s: craft_render_core::Scene| {
            render_image(Arc::new(s), RenderSettings { width: 32, height: 32, seed: 3, ..RenderSettings::default() }, 16).unwrap()
        };
        let (with, without) = (render(exported), render(none));
        assert!(with.sum.iter().all(|p| p.iter().all(|v| v.is_finite())));
        let (a, b) = (mean_rgb(&with.mean()), mean_rgb(&without.mean()));
        assert!((a[0] - b[0]).abs() > 1e-3, "hair changes the image: {a:?} vs {b:?}");
    }

    #[test]
    fn the_gpu_path_tracer_declines_hair_so_the_cpu_is_used() {
        let scene = head_with_hair(50);
        let exported = export_scene(&scene, &mut modelcraft_scene::Evaluator::default(), &Camera::default(), &options()).unwrap();
        let Some(ctx) = craft_render_gpu::GpuContext::new() else { return };
        let r = craft_render_gpu::GpuRenderer::new(&ctx, &exported, RenderSettings::default());
        assert!(matches!(r, Err(craft_render_gpu::GpuError::Unsupported(_))));
    }
}
