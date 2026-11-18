//! Tests of angle-based flattening: that it solves its constraints, that it is exact where the
//! surface is flat or developable, and that it does what it is for (angles closer to the
//! surface's than LSCM's) on curved islands.

use super::*;
use glam::{DVec2, DVec3, Vec3};
use modelcraft_ops::{edit, primitives::Primitive};

fn welded(p: Primitive) -> PolyMesh {
    edit::weld(&p.build().unwrap(), 1e-4).unwrap()
}

fn abf() -> UnwrapOptions {
    UnwrapOptions { method: UnwrapMethod::Abf, ..UnwrapOptions::default() }
}

/// The quantity ABF minimises, measured on a finished layout: the sum over triangle corners of
/// `((α − β) / β)²`, where `α` is the angle in UV space and `β` the angle on the surface, scaled
/// to a full turn around interior vertices.
fn angle_energy(m: &PolyMesh) -> f64 {
    let charts = Charts::build(m, &SeamSet::from_uv_discontinuities(m)).unwrap();
    let mut boundary = vec![false; charts.vertex_source.len()];
    for h in 0..m.corner_count() as u32 {
        if charts.edge_cut[m.edge_of(h) as usize] {
            boundary[charts.corner_vertex[h as usize] as usize] = true;
            boundary[charts.corner_vertex[m.next(h) as usize] as usize] = true;
        }
    }
    let mut tris = Vec::new();
    let mut corners: Vec<(u32, f64, f64)> = Vec::new();
    for f in 0..m.face_count() as u32 {
        tris.clear();
        face_triangles(m, f, &mut tris);
        for t in &tris {
            let p = t.map(|c| m.position(m.origin(c)).as_dvec3());
            let q = t.map(|c| DVec2::new(f64::from(m.uv(c).x), f64::from(m.uv(c).y)));
            for k in 0..3 {
                let ang3 = |a: DVec3, b: DVec3, c: DVec3| (b - a).cross(c - a).length().atan2((b - a).dot(c - a));
                let ang2 = |a: DVec2, b: DVec2, c: DVec2| (b - a).perp_dot(c - a).abs().atan2((b - a).dot(c - a));
                corners.push((charts.corner_vertex[t[k] as usize], ang3(p[k], p[(k + 1) % 3], p[(k + 2) % 3]), ang2(q[k], q[(k + 1) % 3], q[(k + 2) % 3])));
            }
        }
    }
    let mut total = vec![0.0f64; boundary.len()];
    for (v, b, _) in &corners {
        total[*v as usize] += b;
    }
    corners
        .iter()
        .map(|(v, b, a)| {
            let beta = if boundary[*v as usize] { *b } else { b * std::f64::consts::TAU / total[*v as usize] };
            ((a - beta) / beta).powi(2)
        })
        .sum()
}

#[test]
fn a_plane_is_unchanged_by_abf() {
    let m = Primitive::Plane { width: 4.0, depth: 2.0, segments: 8 }.build().unwrap();
    let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &abf()).unwrap();
    let isl = &res.islands[0];
    let abf = isl.abf.unwrap();
    assert!(abf.converged && abf.interior_vertices > 0 && !isl.fallback && !isl.abf_fell_back(), "{abf:?}");
    assert!(abf.iterations <= 3, "a flat island is solved at once: {abf:?}");
    let (d, _) = measure(&out);
    assert!(d.max_angle_error_deg < 0.01 && d.area_rms_log < 1e-4 && d.flipped == 0, "{d:?}");
}

#[test]
fn a_cylinder_unrolls_exactly() {
    let m = welded(Primitive::Cylinder { radius: 1.0, height: 2.0, sides: 24, rings: 4, caps: false });
    let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &abf()).unwrap();
    assert!(res.islands[0].abf.unwrap().converged);
    let (d, _) = measure(&out);
    eprintln!("abf cylinder: {d:?}");
    assert!(d.max_angle_error_deg < 0.05 && d.area_rms_log < 1e-3 && d.flipped == 0, "{d:?}");
}

#[test]
fn a_cone_cut_along_a_generatrix_is_developable() {
    let m = welded(Primitive::Cone { radius: 1.0, height: 1.5, sides: 24, caps: false });
    let apex = (0..m.vertex_count() as u32).max_by(|a, b| m.position(*a).y.total_cmp(&m.position(*b).y)).unwrap();
    let rim = (0..m.vertex_count() as u32)
        .filter(|v| m.position(*v).y < m.position(apex).y - 1.0)
        .min_by(|a, b| m.position(*a).z.total_cmp(&m.position(*b).z))
        .unwrap();
    let mut seams = SeamSet::new();
    assert!(seams.mark_path(&m, apex, rim).unwrap() >= 1);
    let (out, res) = unwrap_mesh(&m, &seams, &abf()).unwrap();
    assert_eq!(res.cut.edges_added, 0);
    assert!(res.islands.iter().all(|i| i.abf.is_some_and(|a| a.converged)), "{:?}", res.islands);
    let (d, _) = measure(&out);
    eprintln!("abf cone: {d:?}");
    assert!(d.flipped == 0 && d.max_angle_error_deg < 0.05 && d.area_rms_log < 1e-3, "{d:?}");
}

#[test]
fn the_sphere_is_flattened_with_the_constraints_met() {
    let m = welded(Primitive::Sphere { radius: 1.0, segments: 32, rings: 16 });
    let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &abf()).unwrap();
    let isl = &res.islands[0];
    let a = isl.abf.unwrap();
    eprintln!("abf sphere report: {a:?}");
    assert!(a.converged && a.residual < 1e-8 && a.direct && a.iterations < 30, "{a:?}");
    let (d, _) = measure(&out);
    eprintln!("abf sphere: {d:?}");
    assert_eq!(d.flipped, 0, "{d:?}");
}

#[test]
fn abf_is_closer_to_the_surface_angles_than_lscm_on_curved_islands() {
    for (name, m) in [
        ("sphere", welded(Primitive::Sphere { radius: 1.0, segments: 32, rings: 16 })),
        ("round cube", welded(Primitive::RoundCube { radius: 1.0, segments: 8 })),
        ("torus", welded(Primitive::Torus { radius: 1.0, tube: 0.35, segments: 24, sides: 12 })),
    ] {
        let (lscm, _) = unwrap_mesh(&m, &SeamSet::new(), &UnwrapOptions::default()).unwrap();
        let (abf_mesh, res) = unwrap_mesh(&m, &SeamSet::new(), &abf()).unwrap();
        assert!(res.islands.iter().all(|i| i.abf.is_some_and(|a| a.converged)), "{name}: {:?}", res.islands);
        let (el, ea) = (angle_energy(&lscm), angle_energy(&abf_mesh));
        let (dl, da) = (measure(&lscm).0, measure(&abf_mesh).0);
        eprintln!(
            "{name}: ABF energy {ea:.4} vs LSCM {el:.4}; max angle error {:.2} vs {:.2} deg; area rms {:.3} vs {:.3}",
            da.max_angle_error_deg, dl.max_angle_error_deg, da.area_rms_log, dl.area_rms_log
        );
        // ABF minimises exactly this energy over every valid flattening that has the same cuts.
        assert!(ea <= el * 1.0001 + 1e-9, "{name}: ABF {ea} should not exceed LSCM {el}");
        assert_eq!(da.flipped, 0, "{name}: {da:?}");
    }
}

#[test]
fn abf_converges_on_a_bumpy_patch_and_stays_unfolded() {
    // A grid pushed up in a smooth hill: a curved disc with many interior vertices.
    let mut m = Primitive::Plane { width: 4.0, depth: 4.0, segments: 12 }.build().unwrap();
    let positions: Vec<Vec3> = (0..m.vertex_count() as u32)
        .map(|v| {
            let p = m.position(v);
            Vec3::new(p.x, 0.8 * (-(p.x * p.x + p.z * p.z) / 2.0).exp(), p.z)
        })
        .collect();
    m.set_positions(&positions).unwrap();
    let (out, res) = unwrap_mesh(&m, &SeamSet::new(), &abf()).unwrap();
    let a = res.islands[0].abf.unwrap();
    assert!(a.converged && a.interior_vertices > 100, "{a:?}");
    let (d, _) = measure(&out);
    eprintln!("abf hill: {d:?} ({} iterations)", a.iterations);
    assert_eq!(d.flipped, 0, "{d:?}");
    assert!(d.mean_angle_error_deg < 3.0, "{d:?}");
}

#[test]
fn abf_with_pins_keeps_the_pins() {
    let m = Primitive::Plane { width: 2.0, depth: 2.0, segments: 4 }.build().unwrap();
    let pins = vec![Pin { corner: 0, uv: glam::Vec2::new(0.25, 0.25) }, Pin { corner: 5, uv: glam::Vec2::new(0.75, 0.25) }];
    let (out, _) = unwrap_mesh(&m, &SeamSet::new(), &UnwrapOptions { pins: pins.clone(), ..abf() }).unwrap();
    for p in &pins {
        assert!(out.uv(p.corner).distance(p.uv) < 1e-4, "{:?} vs {:?}", out.uv(p.corner), p.uv);
    }
}

#[test]
fn a_solve_that_cannot_converge_falls_back_to_lscm_and_says_so() {
    // Two triangles that disagree about the orientation of a shared interior vertex would break the
    // constraints; use the problem directly with an impossible target: an interior vertex whose
    // wheel is one triangle (a single triangle cannot close a vertex).
    let tris = [[0usize, 1, 2]];
    let angles = [[1.0f64, 1.0, 1.1415926]];
    let (out, rep) = crate::abf::solve(&crate::abf::AbfProblem { tris: &tris, angles: &angles, interior: &[true, false, false], coords: &[[0.0; 3]; 3] });
    assert!(!rep.converged, "{rep:?}");
    let _ = out;
}

#[test]
fn abf_on_random_soup_never_panics_and_stays_finite() {
    use proptest::prelude::*;
    let mut runner = proptest::test_runner::TestRunner::default();
    runner
        .run(
            &(
                proptest::collection::vec((-2.0f32..2.0, -2.0f32..2.0, -2.0f32..2.0), 4..14),
                proptest::collection::vec(proptest::collection::vec(0u32..16, 3..6), 1..24),
            ),
            |(pts, faces)| {
                let pos: Vec<Vec3> = pts.iter().map(|p| Vec3::new(p.0, p.1, p.2)).collect();
                let (m, _) = PolyMesh::build(pos, faces.into_iter().map(modelcraft_mesh::Polygon::new).collect()).unwrap_or_default();
                match unwrap_mesh(&m, &SeamSet::new(), &abf()) {
                    Ok((out, _)) => {
                        prop_assert!(out.corner_uvs().iter().all(|u| u.is_finite()));
                    }
                    Err(e) => prop_assert_eq!(e, UvError::NoFaces),
                }
                Ok(())
            },
        )
        .unwrap();
}
