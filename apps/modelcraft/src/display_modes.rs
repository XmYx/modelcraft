//! Display modes: how the viewport draws surfaces. The view has one mode ([`ModelApp::display_mode`]);
//! an object's Display tag overrides it for that object. Smooth and flat are shaded; wireframe,
//! see-through and bounding box leave the solid out and add lines (and, for see-through, a faint
//! alpha-blended surface) to the line and overlay lists instead.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use modelcraft_engine::mesh::PolyMesh;
use modelcraft_engine::scene::{DisplayMode, ObjectId, Scene};
use modelcraft_render::buffers::LineVertex;

/// Most edges drawn for one object in the line modes.
const MAX_EDGES: usize = 400_000;
/// Most triangles drawn as faint surface for one object in see-through mode.
const MAX_FACES: usize = 200_000;

/// The mode an object is drawn in: its tag's, else the view's.
pub fn mode_of(scene: &Scene, id: ObjectId, view: DisplayMode) -> DisplayMode {
    match scene.display_mode_of(id) {
        DisplayMode::Auto => view,
        m => m,
    }
}

/// Whether the mode draws the surface itself (shaded), as against lines only.
pub fn is_solid(mode: DisplayMode) -> bool {
    matches!(mode, DisplayMode::Auto | DisplayMode::Smooth | DisplayMode::Flat)
}

/// Lines and faint surfaces for the objects not drawn solid, and which items are drawn solid.
#[derive(Default)]
pub struct Extras {
    pub lines: Vec<LineVertex>,
    pub overlay: Vec<LineVertex>,
    /// Per item: how its surface is drawn.
    pub modes: Vec<DisplayMode>,
}

fn segment(out: &mut Vec<LineVertex>, a: Vec3, b: Vec3, color: [f32; 4]) {
    out.push(LineVertex { pos: a.to_array(), color });
    out.push(LineVertex { pos: b.to_array(), color });
}

/// The 12 edges of a box in world space.
pub fn box_lines(lo: Vec3, hi: Vec3, world: Mat4, color: [f32; 4]) -> Vec<LineVertex> {
    let corner = |k: usize| {
        let p = Vec3::new(if k & 1 == 0 { lo.x } else { hi.x }, if k & 2 == 0 { lo.y } else { hi.y }, if k & 4 == 0 { lo.z } else { hi.z });
        world.transform_point3(p)
    };
    let mut out = Vec::with_capacity(24);
    for k in 0..8usize {
        for bit in [1usize, 2, 4] {
            if k & bit == 0 {
                segment(&mut out, corner(k), corner(k | bit), color);
            }
        }
    }
    out
}

/// Plan the drawing of `items` (object, mesh or `None` for the sculpt mesh, world matrix, colour).
pub fn plan(scene: &Scene, view: DisplayMode, selected: &dyn Fn(ObjectId) -> bool, items: &[(ObjectId, Option<Arc<PolyMesh>>, Mat4, [f32; 4])]) -> Extras {
    let mut extras = Extras::default();
    for (id, mesh, world, color) in items {
        let mode = if mesh.is_none() { DisplayMode::Smooth } else { mode_of(scene, *id, view) };
        extras.modes.push(mode);
        let Some(m) = mesh else { continue };
        let tint = if selected(*id) { [1.0, 0.62, 0.25, 0.95] } else { [0.72, 0.78, 0.86, 0.9] };
        let color = scene.get(*id).map_or(*color, |o| o.material.base_color);
        match mode {
            DisplayMode::Auto | DisplayMode::Smooth | DisplayMode::Flat => {}
            DisplayMode::Wireframe | DisplayMode::SeeThrough => {
                if m.edge_count() <= MAX_EDGES {
                    for e in 0..m.edge_count() as u32 {
                        let (a, b) = m.edge_vertices(e);
                        segment(&mut extras.lines, world.transform_point3(m.position(a)), world.transform_point3(m.position(b)), tint);
                    }
                }
                if mode == DisplayMode::SeeThrough && m.face_count() <= MAX_FACES {
                    let c = [color[0], color[1], color[2], 0.2];
                    for (a, b, d) in m.triangulate().0.iter().map(|t| (t[0], t[1], t[2])) {
                        for v in [a, b, d] {
                            extras.overlay.push(LineVertex { pos: world.transform_point3(m.position(v)).to_array(), color: c });
                        }
                    }
                }
            }
            DisplayMode::BoundingBox => {
                if m.vertex_count() > 0 {
                    let (lo, hi) = m.bounds();
                    extras.lines.extend(box_lines(lo, hi, *world, tint));
                }
            }
        }
    }
    extras
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_has_twelve_edges() {
        let l = box_lines(Vec3::splat(-1.0), Vec3::splat(1.0), Mat4::IDENTITY, [1.0; 4]);
        assert_eq!(l.len(), 24);
        // Every edge is axis-aligned and 2 long.
        for p in l.chunks(2) {
            let d = Vec3::from(p[1].pos) - Vec3::from(p[0].pos);
            assert!((d.length() - 2.0).abs() < 1e-6, "{d:?}");
        }
    }

    /// Render a unit cube (one quad per side) seen head on in `mode`, and read the picture back:
    /// 64 x 64 RGBA, or `None` without a GPU adapter.
    fn picture(mode: DisplayMode, mesh: PolyMesh) -> Option<Vec<u8>> {
        use modelcraft_render::buffers;
        use modelcraft_render::gpu::{DrawItem, Frame, GpuMesh, Renderer, Target};
        let Some((device, queue, _)) = modelcraft_render::gpu::headless() else {
            eprintln!("no GPU adapter: the display-mode picture test is skipped");
            return None;
        };
        let m = Arc::new(mesh);
        let mut scene = Scene::new();
        let id = scene.add("A", modelcraft_engine::scene::ObjectKind::Group, None).ok()?;
        let items = vec![(id, Some(m.clone()), Mat4::IDENTITY, [1.0; 4])];
        let extras = plan(&scene, mode, &|_| false, &items);
        let data = if extras.modes[0] == DisplayMode::Flat { buffers::flat_data(&m) } else { buffers::mesh_data(&m) };
        let gpu_mesh = GpuMesh::new(&device, &data);
        let target = Target::new(&device, [64, 64]);
        let mut r = Renderer::new(&device);
        let cam = modelcraft_render::Camera { yaw: 0.0, pitch: 0.0, distance: 4.0, ..Default::default() };
        let draws = if is_solid(extras.modes[0]) {
            vec![DrawItem {
                mesh: &gpu_mesh,
                model: Mat4::IDENTITY,
                color: [0.8, 0.8, 0.8, 1.0],
                selected: false,
                show_mask: false,
                patch_tint: false,
                roughness: 0.5,
            }]
        } else {
            Vec::new()
        };
        let frame = Frame { view_proj: cam.view_proj(1.0), eye: cam.eye(), clear: [0.0, 0.0, 0.0, 1.0], draws, lines: &extras.lines, overlay: &extras.overlay };
        r.render(&device, &queue, &target, &frame);
        Renderer::readback(&device, &queue, &target)
    }

    fn lit(px: &[u8], x: usize, y: usize) -> u32 {
        let i = (y * 64 + x) * 4;
        px[i] as u32 + px[i + 1] as u32 + px[i + 2] as u32
    }

    #[test]
    fn each_mode_draws_what_it_says_on_a_real_adapter() {
        let cube = || modelcraft_engine::ops::primitives::Primitive::Cube { size: Vec3::splat(1.0), segments: 1 }.build().unwrap();
        let Some(smooth) = picture(DisplayMode::Smooth, cube()) else { return };
        let flat = picture(DisplayMode::Flat, cube()).unwrap();
        let wire = picture(DisplayMode::Wireframe, cube()).unwrap();
        let see = picture(DisplayMode::SeeThrough, cube()).unwrap();
        let bbox = picture(DisplayMode::BoundingBox, cube()).unwrap();
        // The cube fills the middle of the picture; its outline is at about a quarter in.
        let centre = (32, 32);
        assert!(lit(&smooth, centre.0, centre.1) > 120, "smooth: a lit face in the middle");
        assert!(lit(&flat, centre.0, centre.1) > 120, "flat: a lit face in the middle");
        assert_eq!(lit(&wire, centre.0, centre.1), 0, "wireframe: nothing inside a face");
        assert_eq!(lit(&bbox, centre.0, centre.1), 0, "bounding box: nothing inside");
        let (s, w, b, t) = (lit_pixels(&smooth), lit_pixels(&wire), lit_pixels(&bbox), lit_pixels(&see));
        assert!(w > 20 && w < s * 6 / 10, "wireframe draws edges only: {w} lit pixels against {s} for the solid");
        assert!(b > 20 && b < s * 6 / 10, "the box draws edges only: {b}");
        let c = lit(&see, centre.0, centre.1);
        assert!(c > 0 && c < lit(&smooth, centre.0, centre.1) / 2, "see-through: a faint surface, {c}");
        assert!(t > w, "see-through covers more than the edges alone");
    }

    fn lit_pixels(px: &[u8]) -> usize {
        px.chunks(4).filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 12).count()
    }

    #[test]
    fn a_tag_beats_the_view_mode() {
        let mut s = Scene::new();
        let a = s.add("A", modelcraft_engine::scene::ObjectKind::Group, None).unwrap();
        assert_eq!(mode_of(&s, a, DisplayMode::Flat), DisplayMode::Flat);
        s.edit(a).unwrap().tags.push(modelcraft_engine::scene::Tag::Display { mode: DisplayMode::Wireframe });
        assert_eq!(mode_of(&s, a, DisplayMode::Flat), DisplayMode::Wireframe);
    }
}
