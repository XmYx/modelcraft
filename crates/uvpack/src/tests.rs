use super::*;
use glam::{Mat4, Vec2, Vec3};
use modelcraft_mesh::PolyMesh;
use modelcraft_ops::{edit, primitives::Primitive};
use proptest::prelude::*;

fn rect(w: f32, h: f32, area_3d: f32) -> PackIsland {
    PackIsland { triangles: vec![[Vec2::ZERO, Vec2::new(w, 0.0), Vec2::new(w, h)], [Vec2::ZERO, Vec2::new(w, h), Vec2::new(0.0, h)]], area_3d }
}

fn bounds_of(i: &PackIsland, p: &Placement) -> (Vec2, Vec2) {
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for q in i.triangles.iter().flatten() {
        let q = p.apply(*q);
        lo = lo.min(q);
        hi = hi.max(q);
    }
    (lo, hi)
}

/// Every island inside the unit square, margin respected, no two overlapping.
fn assert_valid(islands: &[PackIsland], r: &PackResult, margin: f32) {
    let b: Vec<_> = islands.iter().zip(&r.placements).map(|(i, p)| bounds_of(i, p)).collect();
    let e = 1e-4;
    for (i, (lo, hi)) in b.iter().enumerate() {
        assert!(lo.min_element() >= margin * 0.5 - e && hi.max_element() <= 1.0 - margin * 0.5 + e, "island {i} leaves the square: {lo} {hi}");
        for (j, (lo2, hi2)) in b.iter().enumerate().skip(i + 1) {
            // Rotated islands may be packed by their hull, but their bounding boxes must still not overlap.
            let gap_x = (lo2.x - hi.x).max(lo.x - hi2.x);
            let gap_y = (lo2.y - hi.y).max(lo.y - hi2.y);
            assert!(gap_x.max(gap_y) >= margin - e, "islands {i} and {j} are {} apart, less than the margin {margin}", gap_x.max(gap_y));
        }
    }
}

#[test]
fn rectangles_fill_the_square_without_overlap() {
    let islands = vec![rect(3.0, 1.0, 3.0), rect(1.0, 1.0, 1.0), rect(2.0, 2.0, 4.0), rect(0.5, 4.0, 2.0), rect(1.0, 0.2, 0.2)];
    for rotation in [Rotation::None, Rotation::Quarter, Rotation::Fit] {
        let opts = PackOptions { margin: 0.01, rotation, density: Density::Keep };
        let r = pack(&islands, &opts).unwrap();
        assert_valid(&islands, &r, 0.01);
        eprintln!("{rotation:?}: utilisation {:.3}", r.utilisation);
        assert!(r.utilisation > 0.45, "{rotation:?}: {}", r.utilisation);
    }
}

#[test]
fn quarter_turns_help_long_islands() {
    // Eight 4x1 strips fill a 4x8 stack; without turning they still do, but mixed with tall ones
    // turning is what lets them share rows.
    let mut islands = vec![];
    for _ in 0..6 {
        islands.push(rect(4.0, 1.0, 4.0));
        islands.push(rect(1.0, 4.0, 4.0));
    }
    let none = pack(&islands, &PackOptions { margin: 0.0, rotation: Rotation::None, density: Density::Keep }).unwrap();
    let turn = pack(&islands, &PackOptions { margin: 0.0, rotation: Rotation::Quarter, density: Density::Keep }).unwrap();
    assert_valid(&islands, &turn, 0.0);
    assert!(turn.utilisation >= none.utilisation * 0.999, "none {} turn {}", none.utilisation, turn.utilisation);
    assert!(turn.utilisation > 0.7, "{}", turn.utilisation);
}

#[test]
fn fit_rotation_straightens_a_diagonal_strip() {
    let (c, s) = (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2);
    let rot = |p: Vec2| Vec2::new(c * p.x - s * p.y, s * p.x + c * p.y);
    let strip = |n: usize| {
        let mut i = rect(8.0, 1.0, 8.0);
        for t in &mut i.triangles {
            for p in t.iter_mut() {
                *p = rot(*p);
            }
        }
        let _ = n;
        i
    };
    let islands: Vec<PackIsland> = (0..4).map(strip).collect();
    let keep = pack(&islands, &PackOptions { margin: 0.0, rotation: Rotation::None, density: Density::Keep }).unwrap();
    let fit = pack(&islands, &PackOptions { margin: 0.0, rotation: Rotation::Fit, density: Density::Keep }).unwrap();
    assert!(fit.utilisation > 1.8 * keep.utilisation, "diagonal {} straightened {}", keep.utilisation, fit.utilisation);
    // The straightened strips are axis aligned: their bounding box is as large as they are.
    let (lo, hi) = bounds_of(&islands[0], &fit.placements[0]);
    let a = (hi.x - lo.x) * (hi.y - lo.y);
    assert!((a - islands[0].uv_area() * fit.placements[0].matrix.determinant().abs()).abs() < 1e-3 * a.max(1e-6), "bbox {a}");
}

#[test]
fn uniform_density_equalises_texels_per_metre() {
    // Same UV size, but one island covers four times the surface.
    let islands = vec![rect(1.0, 1.0, 1.0), rect(1.0, 1.0, 4.0), rect(0.2, 0.2, 0.5)];
    let r = pack(&islands, &PackOptions { margin: 0.01, rotation: Rotation::None, density: Density::Uniform }).unwrap();
    assert_valid(&islands, &r, 0.01);
    let d: Vec<f32> = islands.iter().zip(&r.placements).map(|(i, p)| (i.uv_area() * p.matrix.determinant().abs() / i.area_3d).sqrt()).collect();
    assert!(d.iter().all(|x| (x - d[0]).abs() < 1e-4 * d[0]), "{d:?}");
    // Keeping the density leaves the ratio of the first two UV areas at 1.
    let k = pack(&islands, &PackOptions { margin: 0.01, rotation: Rotation::None, density: Density::Keep }).unwrap();
    let a: Vec<f32> = islands.iter().zip(&k.placements).map(|(i, p)| i.uv_area() * p.matrix.determinant().abs()).collect();
    assert!((a[0] - a[1]).abs() < 1e-5);
}

#[test]
fn pack_rejects_bad_input() {
    let mut bad = rect(1.0, 1.0, 1.0);
    bad.triangles[0][1] = Vec2::new(f32::NAN, 0.0);
    assert_eq!(pack(&[bad], &PackOptions::default()).unwrap_err(), PackError::NonFinite);
    assert_eq!(pack(&vec![PackIsland::default(); MAX_ISLANDS + 1], &PackOptions::default()).unwrap_err(), PackError::TooLarge(MAX_ISLANDS + 1));
    assert!(pack(&[], &PackOptions::default()).unwrap().placements.is_empty());
    // A NaN margin is treated as none, an empty island does not break the rest.
    let r = pack(&[PackIsland::default(), rect(1.0, 1.0, 1.0)], &PackOptions { margin: f32::NAN, ..PackOptions::default() }).unwrap();
    assert!(r.placements.iter().all(|p| p.offset.is_finite() && p.matrix.is_finite()));
}

fn cube() -> PolyMesh {
    Primitive::Cube { size: Vec3::splat(2.0), segments: 2 }.build().unwrap()
}

#[test]
fn mesh_islands_and_packing() {
    assert_eq!(uv_islands(&Primitive::Plane { width: 2.0, depth: 2.0, segments: 3 }.build().unwrap()).len(), 1);
    let m = cube();
    let groups = uv_islands(&m);
    assert_eq!(groups.len(), 6);
    assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), m.face_count());
    assert!(uv_islands(&PolyMesh::from_faces(vec![Vec3::ZERO, Vec3::X, Vec3::Y], &[vec![0, 1, 2]]).unwrap()).is_empty());
    assert_eq!(
        pack_mesh(&PolyMesh::from_faces(vec![Vec3::ZERO, Vec3::X, Vec3::Y], &[vec![0, 1, 2]]).unwrap(), &PackOptions::default()).unwrap_err(),
        PackError::NoUvs
    );

    let (uvs, r) = pack_mesh(&m, &PackOptions::default()).unwrap();
    assert_eq!(uvs.len(), m.corner_count());
    assert!(uvs.iter().all(|u| u.is_finite() && u.min_element() >= -1e-5 && u.max_element() <= 1.0 + 1e-5));
    // Six cube faces: even density, no overlap between island boxes.
    let mut packed = m.clone();
    packed.set_uvs(&uvs).unwrap();
    let dens = island_densities(&packed);
    assert_eq!(dens.len(), 6);
    assert!(dens.iter().all(|d| (d - dens[0]).abs() < 1e-3 * dens[0]), "{dens:?}");
    assert!(r.utilisation > 0.5, "{}", r.utilisation);
    let boxes: Vec<(Vec2, Vec2)> = uv_islands(&packed)
        .iter()
        .map(|fs| {
            let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for f in fs {
                for h in packed.face_half_edges(*f) {
                    lo = lo.min(packed.uv(h));
                    hi = hi.max(packed.uv(h));
                }
            }
            (lo, hi)
        })
        .collect();
    assert_eq!(boxes.len(), 6, "packing must not merge or split islands");
    for (i, (lo, hi)) in boxes.iter().enumerate() {
        for (lo2, hi2) in boxes.iter().skip(i + 1) {
            assert!(hi.x <= lo2.x || hi2.x <= lo.x || hi.y <= lo2.y || hi2.y <= lo.y, "overlap");
        }
    }
}

/// Faces whose UV triangle winds the wrong way for the surface (v points down).
fn mirrored_faces(m: &PolyMesh) -> usize {
    (0..m.face_count() as u32)
        .filter(|f| {
            let c: Vec<u32> = m.face_half_edges(*f).collect();
            let (a, b, d) = (m.uv(c[0]), m.uv(c[1]), m.uv(c[2]));
            (b - a).perp_dot(d - a) > 1e-9
        })
        .count()
}

fn span(m: &PolyMesh) -> (Vec2, Vec2) {
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for u in m.corner_uvs() {
        lo = lo.min(*u);
        hi = hi.max(*u);
    }
    (lo, hi)
}

#[test]
fn planar_projection_matches_the_plane_primitive() {
    let m = Primitive::Plane { width: 2.0, depth: 2.0, segments: 4 }.build().unwrap();
    let out = project_mesh(&m, &ProjectOptions::new(Projection::Planar { from: Axis::PosY })).unwrap();
    for (a, b) in out.corner_uvs().iter().zip(m.corner_uvs()) {
        assert!((*a - *b).length() < 1e-5, "{a} vs {b}");
    }
    assert_eq!(mirrored_faces(&out), 0);
    // Seen from below, the same plane is mirrored.
    let below = project_mesh(&m, &ProjectOptions::new(Projection::Planar { from: Axis::NegY })).unwrap();
    assert_eq!(mirrored_faces(&below), m.face_count());
    // Fit none keeps the model's own scale; stretch fills both sides.
    let raw = project_mesh(&m, &ProjectOptions { fit: Fit::None, ..ProjectOptions::new(Projection::Planar { from: Axis::PosY }) }).unwrap();
    assert!((span(&raw).1 - span(&raw).0 - Vec2::splat(2.0)).length() < 1e-5);
    let wide = Primitive::Plane { width: 4.0, depth: 1.0, segments: 2 }.build().unwrap();
    let st = project_mesh(&wide, &ProjectOptions { fit: Fit::Stretch, ..ProjectOptions::new(Projection::Planar { from: Axis::PosY }) }).unwrap();
    assert!((span(&st).1 - span(&st).0 - Vec2::ONE).length() < 1e-5);
    let un = project_mesh(&wide, &ProjectOptions::new(Projection::Planar { from: Axis::PosY })).unwrap();
    assert!((span(&un).1 - span(&un).0 - Vec2::new(1.0, 0.25)).length() < 1e-5);
}

#[test]
fn every_axis_projects_outward_faces_unmirrored() {
    let m = cube();
    for axis in Axis::ALL {
        let out = project_mesh(&m, &ProjectOptions::new(Projection::Planar { from: axis })).unwrap();
        let facing = (0..m.face_count() as u32).filter(|f| m.face_normal(*f).dot(axis.dir()) > 0.5).count();
        assert!(facing > 0);
        let flipped = (0..m.face_count() as u32)
            .filter(|f| m.face_normal(*f).dot(axis.dir()) > 0.5)
            .filter(|f| {
                let c: Vec<u32> = out.face_half_edges(*f).collect();
                (out.uv(c[1]) - out.uv(c[0])).perp_dot(out.uv(c[2]) - out.uv(c[0])) > 1e-9
            })
            .count();
        assert_eq!(flipped, 0, "{axis:?}");
    }
}

#[test]
fn box_projection_puts_each_side_in_its_own_cell() {
    let m = cube();
    let out = project_mesh(&m, &ProjectOptions::new(Projection::Box)).unwrap();
    assert_eq!(mirrored_faces(&out), 0);
    assert!(out.corner_uvs().iter().all(|u| u.min_element() >= -1e-5 && u.max_element() <= 1.0 + 1e-5));
    let groups = uv_islands(&out);
    assert_eq!(groups.len(), 6, "one island per side of the cube");
    // No two sides overlap.
    let boxes: Vec<(Vec2, Vec2)> = groups
        .iter()
        .map(|fs| {
            let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for f in fs {
                for h in out.face_half_edges(*f) {
                    lo = lo.min(out.uv(h));
                    hi = hi.max(out.uv(h));
                }
            }
            (lo, hi)
        })
        .collect();
    for (i, (lo, hi)) in boxes.iter().enumerate() {
        for (lo2, hi2) in boxes.iter().skip(i + 1) {
            assert!(hi.x <= lo2.x + 1e-6 || hi2.x <= lo.x + 1e-6 || hi.y <= lo2.y + 1e-6 || hi2.y <= lo.y + 1e-6);
        }
    }
}

#[test]
fn cylindrical_projection_unrolls_a_cylinder() {
    let m = edit::weld(&Primitive::Cylinder { radius: 1.0, height: 2.0, sides: 24, rings: 3, caps: false }.build().unwrap(), 1e-4).unwrap();
    let out = project_mesh(&m, &ProjectOptions { fit: Fit::None, ..ProjectOptions::new(Projection::Cylindrical { axis: Axis::PosY }) }).unwrap();
    assert_eq!(mirrored_faces(&out), 0, "seen from outside");
    // u goes once around: width 1 plus one face that crosses the cut; v spans the height / size = 1.
    let (lo, hi) = span(&out);
    assert!((hi.y - lo.y - 1.0).abs() < 1e-4, "{lo} {hi}");
    assert!(hi.x - lo.x > 0.99 && hi.x - lo.x < 1.1, "{lo} {hi}");
    // No face is wider than one side's share of the turn (no wrap across the cut).
    for f in 0..out.face_count() as u32 {
        let us: Vec<f32> = out.face_half_edges(f).map(|h| out.uv(h).x).collect();
        let w = us.iter().copied().fold(f32::MIN, f32::max) - us.iter().copied().fold(f32::MAX, f32::min);
        assert!(w < 1.5 / 24.0, "face {f} is {w} wide");
    }
}

#[test]
fn spherical_projection_handles_the_poles() {
    let m = edit::weld(&Primitive::Sphere { radius: 1.0, segments: 16, rings: 8 }.build().unwrap(), 1e-4).unwrap();
    let out = project_mesh(&m, &ProjectOptions::new(Projection::Spherical { axis: Axis::PosY })).unwrap();
    assert!(out.corner_uvs().iter().all(|u| u.is_finite() && u.min_element() >= -1e-5 && u.max_element() <= 1.0 + 1e-5));
    assert_eq!(mirrored_faces(&out), 0);
    for f in 0..out.face_count() as u32 {
        let us: Vec<f32> = out.face_half_edges(f).map(|h| out.uv(h).x).collect();
        let w = us.iter().copied().fold(f32::MIN, f32::max) - us.iter().copied().fold(f32::MAX, f32::min);
        assert!(w < 0.2, "face {f} is {w} wide");
    }
    // Every other axis works too.
    for axis in [Axis::PosX, Axis::PosZ] {
        let o = project_mesh(&m, &ProjectOptions::new(Projection::Spherical { axis })).unwrap();
        assert!(o.corner_uvs().iter().all(|u| u.is_finite()));
    }
}

#[test]
fn camera_projection_follows_the_view() {
    let m = Primitive::Plane { width: 2.0, depth: 2.0, segments: 2 }.build().unwrap();
    // Look straight down at the plane, +X to the right and -Z up on screen, like a planar projection.
    let view = Mat4::look_at_rh(Vec3::new(0.0, 5.0, 0.0), Vec3::ZERO, Vec3::NEG_Z);
    let proj = Mat4::orthographic_rh(-1.0, 1.0, -1.0, 1.0, 0.1, 10.0);
    let cam = ProjectOptions { fit: Fit::None, ..ProjectOptions::new(Projection::Camera { view_proj: (proj * view).to_cols_array() }) };
    let out = project_mesh(&m, &cam).unwrap();
    let planar = project_mesh(&m, &ProjectOptions::new(Projection::Planar { from: Axis::PosY })).unwrap();
    for (a, b) in out.corner_uvs().iter().zip(planar.corner_uvs()) {
        assert!((*a - *b).length() < 1e-4, "{a} vs {b}");
    }
    let mut nan = (proj * view).to_cols_array();
    nan[3] = f32::NAN;
    assert_eq!(project(&m, &ProjectOptions::new(Projection::Camera { view_proj: nan })).unwrap_err(), PackError::NonFinite);
}

#[test]
fn projecting_selected_faces_leaves_the_others() {
    let m = cube();
    let out = project_mesh(&m, &ProjectOptions { faces: vec![0, 1, 999], ..ProjectOptions::new(Projection::Planar { from: Axis::PosY }) }).unwrap();
    for f in 2..m.face_count() as u32 {
        for h in m.face_half_edges(f) {
            assert_eq!(out.uv(h), m.uv(h));
        }
    }
    assert_ne!(out.corner_uvs(), m.corner_uvs());
    // No UVs to start with: the rest is zero.
    let bare = PolyMesh::from_faces(vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z], &[vec![0, 1, 2], vec![0, 2, 3]]).unwrap();
    let o = project(&bare, &ProjectOptions { faces: vec![0], ..ProjectOptions::new(Projection::Planar { from: Axis::PosZ }) }).unwrap();
    assert_eq!(o.len(), 6);
    assert_eq!(&o[3..], &[Vec2::ZERO; 3]);
    assert_eq!(project(&PolyMesh::default(), &ProjectOptions::new(Projection::Box)).unwrap_err(), PackError::NoFaces);
    let json = serde_json::to_string(&Projection::Cylindrical { axis: Axis::NegZ }).unwrap();
    assert_eq!(serde_json::from_str::<Projection>(&json).unwrap(), Projection::Cylindrical { axis: Axis::NegZ });
}

proptest! {
    #[test]
    fn random_rectangles_always_pack_cleanly(
        sizes in proptest::collection::vec((0.05f32..5.0, 0.05f32..5.0), 1..40),
        margin in 0.0f32..0.03,
        rot in 0u8..3,
    ) {
        let islands: Vec<PackIsland> = sizes.iter().map(|(w, h)| rect(*w, *h, w * h)).collect();
        let rotation = [Rotation::None, Rotation::Quarter, Rotation::Fit][rot as usize];
        let opts = PackOptions { margin, rotation, density: Density::Keep };
        let r = pack(&islands, &opts).unwrap();
        let b: Vec<_> = islands.iter().zip(&r.placements).map(|(i, p)| bounds_of(i, p)).collect();
        for (i, (lo, hi)) in b.iter().enumerate() {
            prop_assert!(lo.min_element() >= margin * 0.5 - 1e-3 && hi.max_element() <= 1.0 - margin * 0.5 + 1e-3);
            for (lo2, hi2) in b.iter().skip(i + 1) {
                let gap = ((lo2.x - hi.x).max(lo.x - hi2.x)).max((lo2.y - hi.y).max(lo.y - hi2.y));
                prop_assert!(gap >= margin - 1e-3, "gap {gap} < margin {margin}");
            }
        }
    }

    #[test]
    fn projections_of_perturbed_meshes_are_finite(seed in 0u32..500, kind in 0u8..5, fit in 0u8..3) {
        let mut m = edit::weld(&Primitive::Sphere { radius: 1.0, segments: 6, rings: 4 }.build().unwrap(), 1e-4).unwrap();
        m.map_positions(|i, p| {
            let h = |k: u32| ((i.wrapping_mul(2654435761).wrapping_add(seed.wrapping_mul(40503)).wrapping_add(k * 97)) % 1000) as f32 / 1000.0 - 0.5;
            p + Vec3::new(h(1), h(2), h(3)) * 0.4
        });
        let projection = match kind {
            0 => Projection::Planar { from: Axis::ALL[(seed % 6) as usize] },
            1 => Projection::Box,
            2 => Projection::Cylindrical { axis: Axis::ALL[(seed % 6) as usize] },
            3 => Projection::Spherical { axis: Axis::ALL[(seed % 6) as usize] },
            _ => Projection::Camera { view_proj: Mat4::perspective_rh(1.0, 1.0, 0.1, 100.0).to_cols_array() },
        };
        let fit = [Fit::None, Fit::Uniform, Fit::Stretch][fit as usize];
        match project(&m, &ProjectOptions { projection, fit, faces: vec![] }) {
            Ok(uvs) => prop_assert!(uvs.iter().all(|u| u.is_finite())),
            Err(e) => prop_assert_eq!(e, PackError::NonFinite),
        }
    }
}
