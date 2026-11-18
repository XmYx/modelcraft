use glam::{Mat4, Vec3};
use modelcraft_ops::primitives::Primitive;
use modelcraft_sim::{CancelFlag, SimTiming};
use proptest::prelude::*;

use super::*;

fn sphere_surface() -> Surface {
    let mesh = Primitive::Sphere { radius: 1.0, segments: 24, rings: 12 }.build().unwrap();
    Surface::from_mesh(&mesh).unwrap()
}

fn guides(s: &Surface, n: usize, length: f32, seed: u64) -> Guides {
    let spec = GrowSpec { amount: Amount::Count(n), length, points_per_strand: 8, seed, ..GrowSpec::default() };
    grow_guides(s, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap()
}

fn timing(frames: u32) -> SimTiming {
    SimTiming { start_frame: 1.0, end_frame: frames as f64, fps: 24.0, substeps: 4 }
}

fn bake_with(g: &Guides, s: &Surface, set: &DynamicsSettings, frames: u32) -> modelcraft_sim::FrameCache {
    bake_guides(g, s, set, timing(frames), 1, 256 << 20, &mut modelcraft_sim::Silent).unwrap()
}

fn strand_of(cache: &modelcraft_sim::FrameCache, frame: usize, i: usize, pps: usize) -> Vec<Vec3> {
    cache.frame(frame).unwrap()[i * pps..(i + 1) * pps].to_vec()
}

#[test]
fn without_forces_the_hair_stays_where_it_was_groomed() {
    let s = sphere_surface();
    let g = guides(&s, 40, 0.3, 1);
    let calm = DynamicsSettings { gravity: Vec3::ZERO, wind: Vec3::ZERO, ..DynamicsSettings::default() };
    let cache = bake_with(&g, &s, &calm, 10);
    assert_eq!((cache.frames(), cache.points()), (10, 40 * 8));
    for f in 0..cache.frames() {
        for (a, b) in cache.frame(f).unwrap().iter().zip(g.all_points()) {
            assert!(a.distance(*b) < 1e-4, "frame {f}");
        }
    }
}

#[test]
fn roots_stay_pinned_and_lengths_hold_under_gravity() {
    let s = sphere_surface();
    let g = guides(&s, 60, 0.35, 2);
    let set = DynamicsSettings { stiffness: 0.0, collide: false, ..DynamicsSettings::default() };
    let cache = bake_with(&g, &s, &set, 40);
    let rest: Vec<f32> = (0..g.len()).map(|i| g.strand_length(i)).collect();
    for f in 0..cache.frames() {
        for i in 0..g.len() {
            let strand = strand_of(&cache, f, i, 8);
            assert_eq!(strand[0], g.strand(i)[0], "roots are pinned");
            let len: f32 = strand.windows(2).map(|w| w[0].distance(w[1])).sum();
            assert!((len - rest[i]).abs() < 0.05 * rest[i], "frame {f} strand {i}: {len} against {}", rest[i]);
        }
    }
}

#[test]
fn gravity_makes_hair_hang_and_stiffness_resists_it() {
    // Strands standing out sideways from a wall of a flat sheet droop under gravity, less when stiff.
    let positions = vec![Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(-1.0, 0.0, 1.0)];
    let s = Surface::from_triangles(positions, vec![[0, 3, 1], [1, 3, 2]]).unwrap();
    let spec = GrowSpec { amount: Amount::Count(20), length: 0.5, points_per_strand: 8, lean: 1.0, lean_toward: Vec3::X, ..GrowSpec::default() };
    let g = grow_guides(&s, &spec, &SurfaceMaps::default(), &Limits::default()).unwrap();
    let tip_y = |stiffness: f32| {
        let set = DynamicsSettings { stiffness, collide: false, ..DynamicsSettings::default() };
        let cache = bake_with(&g, &s, &set, 60);
        let last = cache.frames() - 1;
        (0..g.len()).map(|i| strand_of(&cache, last, i, 8)[7].y).sum::<f32>() / g.len() as f32
    };
    let rest_y = (0..g.len()).map(|i| g.strand(i)[7].y).sum::<f32>() / g.len() as f32;
    let (limp, stiff) = (tip_y(0.0), tip_y(0.9));
    assert!(limp < rest_y - 0.15, "limp hair droops: {limp} from {rest_y}");
    assert!(stiff > limp + 0.05 && stiff < rest_y + 1e-3, "stiff hair droops less: {stiff} (limp {limp})");
    // Wind pushes sideways.
    let windy = DynamicsSettings { gravity: Vec3::ZERO, wind: Vec3::new(0.0, 0.0, 20.0), stiffness: 0.0, collide: false, ..DynamicsSettings::default() };
    let cache = bake_with(&g, &s, &windy, 30);
    let z = (0..g.len()).map(|i| strand_of(&cache, cache.frames() - 1, i, 8)[7].z - g.strand(i)[7].z).sum::<f32>() / g.len() as f32;
    assert!(z > 0.1, "wind blows the tips along +z: {z}");
}

#[test]
fn hair_lands_on_the_emitter_instead_of_going_through_it() {
    // Long hair on a sphere falls toward the ground; the half under the equator must rest on or
    // outside the surface, never inside it.
    let s = sphere_surface();
    let g = guides(&s, 200, 0.5, 3);
    let set = DynamicsSettings { stiffness: 0.0, ..DynamicsSettings::default() };
    let clearance = set.clearance;
    let cache = bake_with(&g, &s, &set, 60);
    let mut worst = f32::INFINITY;
    for f in 0..cache.frames() {
        for p in cache.frame(f).unwrap() {
            worst = worst.min(p.length());
        }
    }
    // The faceted sphere's inscribed radius is a little under one: allow its facets.
    assert!(worst > 0.97 + clearance * 0.5, "no point goes into the head: {worst}");
    // Without collision the same bake goes through it.
    let ghost = DynamicsSettings { collide: false, ..set };
    let cache = bake_with(&g, &s, &ghost, 60);
    let through = cache.positions().iter().map(|p| p.length()).fold(f32::INFINITY, f32::min);
    assert!(through < 0.9, "collision is what keeps the hair out: {through}");
}

#[test]
fn a_bake_is_deterministic_and_cancellable_and_bounded() {
    let s = sphere_surface();
    let g = guides(&s, 30, 0.3, 4);
    let set = DynamicsSettings::default();
    let a = bake_with(&g, &s, &set, 12);
    let b = bake_with(&g, &s, &set, 12);
    assert_eq!(a.checksum(), b.checksum(), "the same bake twice gives the same bytes");
    let flag = CancelFlag::new();
    flag.cancel();
    let r = bake_guides(&g, &s, &set, timing(12), 1, 256 << 20, &mut flag.clone());
    assert!(matches!(r, Err(HairError::Simulation(m)) if m.contains("cancelled")));
    // A cache that cannot fit fails before any step.
    assert!(bake_guides(&g, &s, &set, timing(12), 1, 1000, &mut modelcraft_sim::Silent).is_err());
}

#[test]
fn bad_settings_are_errors() {
    let s = sphere_surface();
    let g = guides(&s, 5, 0.3, 5);
    for bad in [
        DynamicsSettings { gravity: Vec3::NAN, ..DynamicsSettings::default() },
        DynamicsSettings { gravity: Vec3::splat(1.0e9), ..DynamicsSettings::default() },
        DynamicsSettings { damping: 2.0, ..DynamicsSettings::default() },
        DynamicsSettings { stiffness: -1.0, ..DynamicsSettings::default() },
        DynamicsSettings { iterations: 0, ..DynamicsSettings::default() },
        DynamicsSettings { iterations: 1000, ..DynamicsSettings::default() },
        DynamicsSettings { clearance: f32::INFINITY, ..DynamicsSettings::default() },
    ] {
        assert!(bake_guides(&g, &s, &bad, timing(3), 1, 1 << 20, &mut modelcraft_sim::Silent).is_err(), "{bad:?}");
    }
    assert!(
        bake_guides(
            &g,
            &s,
            &DynamicsSettings::default(),
            SimTiming { start_frame: 5.0, end_frame: 1.0, fps: 24.0, substeps: 4 },
            1,
            1 << 20,
            &mut modelcraft_sim::Silent
        )
        .is_err()
    );
}

#[test]
fn a_frame_of_the_cache_is_a_valid_set_of_guides() {
    let s = sphere_surface();
    let g = guides(&s, 10, 0.3, 6);
    let cache = bake_with(&g, &s, &DynamicsSettings::default(), 5);
    let at = guides_from_points(&g, cache.frame(3).unwrap(), &Limits::default()).unwrap();
    assert_eq!((at.len(), at.points_per_strand(), at.roots()), (g.len(), 8, g.roots()));
    assert!(guides_from_points(&g, &[Vec3::ZERO; 3], &Limits::default()).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Whatever the forces, baked points are finite, roots never move and no segment stretches wildly.
    #[test]
    fn bakes_stay_finite_and_attached(
        seed in any::<u64>(),
        gravity in (-30.0f32..30.0, -30.0f32..30.0, -30.0f32..30.0),
        wind in (-30.0f32..30.0, -30.0f32..30.0, -30.0f32..30.0),
        damping in 0.0f32..1.0,
        stiffness in 0.0f32..1.0,
        iterations in 1u32..16,
        collide in any::<bool>(),
    ) {
        let s = sphere_surface();
        let g = guides(&s, 12, 0.3, seed);
        let set = DynamicsSettings { gravity: Vec3::new(gravity.0, gravity.1, gravity.2), wind: Vec3::new(wind.0, wind.1, wind.2), damping, stiffness, iterations, collide, clearance: 0.004 };
        let cache = bake_with(&g, &s, &set, 12);
        for f in 0..cache.frames() {
            for i in 0..g.len() {
                let strand = strand_of(&cache, f, i, 8);
                prop_assert!(strand.iter().all(|p| p.is_finite()));
                prop_assert_eq!(strand[0], g.strand(i)[0]);
                let rest = g.strand_length(i);
                let len: f32 = strand.windows(2).map(|w| w[0].distance(w[1])).sum();
                prop_assert!(len < rest * 1.6 + 0.01, "stretched {len} from {rest}");
            }
        }
    }
}

// ---- glTF export ----

fn parse_glb(bytes: &[u8]) -> (serde_json::Value, Vec<u8>) {
    assert_eq!(&bytes[0..4], b"glTF");
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize, bytes.len(), "the header length is the file length");
    let jl = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    assert_eq!(&bytes[16..20], b"JSON");
    assert_eq!(jl % 4, 0);
    let json: serde_json::Value = serde_json::from_slice(&bytes[20..20 + jl]).unwrap();
    let at = 20 + jl;
    let bl = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    assert_eq!(&bytes[at + 4..at + 8], b"BIN\0");
    assert_eq!(bl % 4, 0);
    assert_eq!(at + 8 + bl, bytes.len());
    (json, bytes[at + 8..].to_vec())
}

#[test]
fn curves_export_as_a_gltf_of_lines() {
    let s = sphere_surface();
    let g = guides(&s, 25, 0.3, 7);
    let widths: Vec<f32> = (0..g.all_points().len()).map(|i| 0.01 - 0.001 * (i % 8) as f32).collect();
    let bytes = to_glb(&CurveExport { points: g.all_points(), points_per_strand: 8, widths: Some(&widths) }, "Hair", Mat4::IDENTITY).unwrap();
    let (json, bin) = parse_glb(&bytes);
    assert_eq!(json["asset"]["version"], "2.0");
    let prim = &json["meshes"][0]["primitives"][0];
    assert_eq!(prim["mode"], 1, "LINES");
    assert_eq!(json["meshes"][0]["extras"]["modelcraft"]["strands"], 25);
    assert_eq!(json["meshes"][0]["extras"]["modelcraft"]["pointsPerStrand"], 8);
    let acc = |i: usize| &json["accessors"][i];
    let (pos, wid, idx) = (
        &acc(prim["attributes"]["POSITION"].as_u64().unwrap() as usize),
        &acc(prim["attributes"]["_WIDTH"].as_u64().unwrap() as usize),
        &acc(prim["indices"].as_u64().unwrap() as usize),
    );
    assert_eq!((pos["count"].as_u64(), wid["count"].as_u64(), idx["count"].as_u64()), (Some(200), Some(200), Some(25 * 7 * 2)));
    // Every view lies inside the buffer and the positions are the points.
    for v in json["bufferViews"].as_array().unwrap() {
        assert!(v["byteOffset"].as_u64().unwrap() + v["byteLength"].as_u64().unwrap() <= bin.len() as u64);
    }
    let f = |o: usize| f32::from_le_bytes(bin[o..o + 4].try_into().unwrap());
    for (i, p) in g.all_points().iter().enumerate() {
        assert_eq!([f(i * 12), f(i * 12 + 4), f(i * 12 + 8)], p.to_array());
    }
    // The bounds are exact and the width at the root is the first one given.
    let lo = g.all_points().iter().fold(Vec3::splat(f32::INFINITY), |a, p| a.min(*p));
    for k in 0..3 {
        assert_eq!(pos["min"][k].as_f64().unwrap() as f32, lo[k]);
    }
    assert_eq!(f(200 * 12), 0.01);
    // Indices pair consecutive points of one strand and stay in range.
    let io = 200 * 12 + 200 * 4;
    let u = |o: usize| u32::from_le_bytes(bin[o..o + 4].try_into().unwrap());
    for k in 0..25 * 7 {
        let (a, b) = (u(io + k * 8), u(io + k * 8 + 4));
        assert_eq!(b, a + 1);
        assert_eq!(a as usize / 8, (b as usize) / 8, "a segment does not cross strands");
        assert!(b < 200);
    }
}

#[test]
fn export_places_the_curves_and_refuses_bad_input() {
    let pts = [Vec3::ZERO, Vec3::Y, Vec3::X, Vec3::X + Vec3::Y];
    let moved = to_glb(&CurveExport { points: &pts, points_per_strand: 2, widths: None }, "x", Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0))).unwrap();
    let (json, bin) = parse_glb(&moved);
    assert_eq!(json["accessors"][0]["min"], serde_json::json!([5.0, 0.0, 0.0]));
    assert!(json["meshes"][0]["primitives"][0]["attributes"].get("_WIDTH").is_none());
    assert_eq!(f32::from_le_bytes(bin[0..4].try_into().unwrap()), 5.0);
    for bad in [
        CurveExport { points: &[], points_per_strand: 2, widths: None },
        CurveExport { points: &pts, points_per_strand: 1, widths: None },
        CurveExport { points: &pts, points_per_strand: 3, widths: None },
        CurveExport { points: &pts, points_per_strand: 2, widths: Some(&[1.0]) },
        CurveExport { points: &[Vec3::NAN, Vec3::X], points_per_strand: 2, widths: None },
    ] {
        assert!(to_glb(&bad, "x", Mat4::IDENTITY).is_err());
    }
    assert!(to_glb(&CurveExport { points: &pts, points_per_strand: 2, widths: None }, "x", Mat4::from_scale(Vec3::splat(f32::NAN))).is_err());
}
