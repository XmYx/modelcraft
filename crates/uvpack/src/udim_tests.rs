//! Tests of UDIM tiles: numbering, packing into several tiles, and what the packer promises.

use super::*;
use glam::{Vec2, Vec3};
use modelcraft_ops::primitives::Primitive;
use proptest::prelude::*;

fn rect(w: f32, h: f32) -> PackIsland {
    PackIsland { triangles: vec![[Vec2::ZERO, Vec2::new(w, 0.0), Vec2::new(w, h)], [Vec2::ZERO, Vec2::new(w, h), Vec2::new(0.0, h)]], area_3d: w * h }
}

fn lcg(n: usize, seed: u64) -> Vec<PackIsland> {
    let mut state = seed;
    let mut rnd = move || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    (0..n).map(|_| rect(0.1 + 0.5 * rnd(), 0.1 + 0.5 * rnd())).collect()
}

/// Axis-aligned box of an island's placed triangles.
fn placed_box(i: &PackIsland, p: &Placement) -> (Vec2, Vec2) {
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for q in i.triangles.iter().flatten() {
        let q = p.apply(*q);
        lo = lo.min(q);
        hi = hi.max(q);
    }
    (lo, hi)
}

#[test]
fn tile_numbers_follow_the_udim_convention() {
    assert_eq!(tile_of(Vec2::new(0.5, 0.5)), (0, 0));
    assert_eq!(udim_number(0, 0), Some(1001));
    assert_eq!(udim_number(9, 0), Some(1010));
    assert_eq!(udim_number(0, 1), Some(1011));
    assert_eq!(udim_number(10, 0), None);
    assert_eq!(udim_number(-1, 0), None);
    // v is stored downwards: the tile above the unit square has negative v.
    assert_eq!(tile_of(Vec2::new(1.5, -0.5)), (1, 1));
    assert_eq!(udim_number(1, 1), Some(1012));
    assert_eq!(tile_of(Vec2::new(0.2, 1.5)), (0, -1));
    assert_eq!(tile_of_number(1012), Some((1, 1)));
    assert_eq!(tile_of_number(1000), None);
    assert_eq!(tile_of_number(1101), None);
    for k in 1001..1101 {
        let (c, r) = tile_of_number(k).unwrap();
        assert_eq!(udim_number(c, r), Some(k));
    }
}

#[test]
fn islands_land_inside_their_tile_with_one_density_everywhere() {
    let islands = lcg(40, 7);
    let opts = UdimOptions { margin: 0.004, rotation: Rotation::None, density: Density::Uniform, tiles: UdimTiles::Count(4) };
    let r = pack_udim(&islands, &opts).unwrap();
    assert_eq!(r.placements.len(), 40);
    assert_eq!(r.tiles_used, vec![1001, 1002, 1003, 1004]);
    let boxes: Vec<(Vec2, Vec2)> = islands.iter().zip(&r.placements).map(|(i, p)| placed_box(i, p)).collect();
    for (k, (lo, hi)) in boxes.iter().enumerate() {
        let (col, row) = ((r.tile[k] % TILE_COLUMNS) as f32, (r.tile[k] / TILE_COLUMNS) as f32);
        assert!(
            lo.x >= col - 1e-5 && hi.x <= col + 1.0 + 1e-5 && lo.y >= -row - 1e-5 && hi.y <= 1.0 - row + 1e-5,
            "island {k} {lo:?}..{hi:?} leaves tile {}",
            r.tile[k]
        );
    }
    // No two islands of one tile overlap.
    for a in 0..boxes.len() {
        for b in a + 1..boxes.len() {
            if r.tile[a] == r.tile[b] {
                let (x, y) = (
                    boxes[a].0.x < boxes[b].1.x - 1e-6 && boxes[b].0.x < boxes[a].1.x - 1e-6,
                    boxes[a].0.y < boxes[b].1.y - 1e-6 && boxes[b].0.y < boxes[a].1.y - 1e-6,
                );
                assert!(!(x && y), "islands {a} and {b} overlap");
            }
        }
    }
    // The same texel density in every tile: placed UV area over surface area.
    let dens: Vec<f32> = islands.iter().zip(&r.placements).map(|(i, p)| (i.uv_area() * p.matrix.determinant().abs() / i.area_3d).sqrt()).collect();
    let (lo, hi) = dens.iter().fold((f32::MAX, f32::MIN), |(l, h), d| (l.min(*d), h.max(*d)));
    assert!((hi - lo) / hi < 1e-3, "density {lo} to {hi}");
    // Four tiles give about twice the linear scale of one.
    let one = pack_udim(&islands, &UdimOptions { tiles: UdimTiles::Count(1), ..opts }).unwrap();
    eprintln!("scale on 1 tile {}, on 4 tiles {}; utilisation {:?}", one.scale, r.scale, r.utilisation);
    assert!(r.scale > one.scale * 1.7, "{} vs {}", r.scale, one.scale);
    assert!(r.utilisation.iter().all(|u| *u > 0.4 && *u <= 1.0), "{:?}", r.utilisation);
}

#[test]
fn a_texel_density_decides_how_many_tiles() {
    // Eighteen 0.3 x 0.3 islands of 0.09 square metres: at density 1 they cover 1.62 tiles.
    let islands: Vec<PackIsland> = (0..18).map(|_| rect(0.3, 0.3)).collect();
    let at = |d: f32| UdimOptions {
        margin: 0.002,
        rotation: Rotation::Quarter,
        density: Density::Uniform,
        tiles: UdimTiles::Density { texel_density: d, max_tiles: 20 },
    };
    let two = pack_udim(&islands, &at(1.0)).unwrap();
    assert_eq!(two.tiles_used.len(), 2, "{:?}", two.tiles_used);
    // The density is exactly what was asked.
    for (i, p) in islands.iter().zip(&two.placements) {
        let d = (i.uv_area() * p.matrix.determinant().abs() / i.area_3d).sqrt();
        assert!((d - 1.0).abs() < 1e-3, "{d}");
    }
    let one = pack_udim(&islands, &at(0.5)).unwrap();
    assert_eq!(one.tiles_used, vec![1001]);
    let many = pack_udim(&islands, &at(3.0)).unwrap();
    assert!(many.tiles_used.len() >= 14, "{:?}", many.tiles_used);
    // Too many tiles asked for, and a density without surface areas, are errors, not hangs.
    assert_eq!(
        pack_udim(&islands, &UdimOptions { tiles: UdimTiles::Density { texel_density: 30.0, max_tiles: 5 }, ..at(1.0) }).unwrap_err(),
        PackError::NoRoom
    );
    assert!(matches!(pack_udim(&islands, &UdimOptions { density: Density::Keep, ..at(1.0) }), Err(PackError::Invalid(_))));
    assert!(pack_udim(&islands, &at(f32::NAN)).is_err());
}

#[test]
fn a_mesh_is_spread_over_tiles_and_reports_them() {
    let m = Primitive::Cube { size: Vec3::splat(2.0), segments: 2 }.build().unwrap();
    let (uvs, r) = pack_udim_mesh(&m, &UdimOptions { tiles: UdimTiles::Count(3), ..UdimOptions::default() }).unwrap();
    assert_eq!(r.placements.len(), 6);
    let mut out = m.clone();
    out.set_uvs(&uvs).unwrap();
    let used = udim_tiles(&out);
    assert_eq!(used, r.tiles_used);
    assert_eq!(used, vec![1001, 1002, 1003]);
    assert!(uvs.iter().all(|u| u.is_finite()));
    let mut bare = m.clone();
    bare.clear_uvs();
    assert_eq!(pack_udim_mesh(&bare, &UdimOptions::default()).unwrap_err(), PackError::NoUvs);
    assert!(udim_tiles(&bare).is_empty());
    // One tile is the ordinary packer's job: everything in the unit square.
    let (one, _) = pack_udim_mesh(&m, &UdimOptions::default()).unwrap();
    assert!(one.iter().all(|u| u.min_element() >= -1e-4 && u.max_element() <= 1.0 + 1e-4));
}

proptest! {
    #[test]
    fn udim_packing_never_panics_and_stays_in_the_tiles(
        n in 0usize..30,
        seed in any::<u64>(),
        tiles in 0u32..120,
        margin in -0.1f32..0.3,
        rot in 0u8..3,
        density in any::<bool>(),
    ) {
        let islands = lcg(n, seed);
        let rotation = [Rotation::None, Rotation::Quarter, Rotation::Fit][rot as usize];
        let opts = UdimOptions { margin, rotation, density: if density { Density::Uniform } else { Density::Keep }, tiles: UdimTiles::Count(tiles) };
        match pack_udim(&islands, &opts) {
            Ok(r) => {
                prop_assert_eq!(r.placements.len(), n);
                for (i, p) in islands.iter().zip(&r.placements) {
                    let (lo, hi) = placed_box(i, p);
                    prop_assert!(lo.is_finite() && hi.is_finite());
                    prop_assert!(lo.x >= -1e-3 && hi.x <= 10.0 + 1e-3 && hi.y <= 1.0 + 1e-3 && lo.y >= -9.0 - 1e-3);
                }
            }
            Err(e) => prop_assert!(matches!(e, PackError::NoRoom | PackError::TooLarge(_))),
        }
    }
}
