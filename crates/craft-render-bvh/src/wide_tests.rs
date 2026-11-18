//! The wide BVH and the PLOC build against brute force and against the binned build.

use glam::{Vec3, Vec3A};
use proptest::prelude::*;

use crate::wide::WIDTH;
use crate::{Aabb, Bvh, Hit, PrimInfo, Ray, TriAccel, TriangleBvh, hit_triangle};

fn brute(tris: &[[Vec3; 3]], ray: &Ray) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for (i, t) in tris.iter().enumerate() {
        let p0 = Vec3A::from(t[0]);
        let acc = TriAccel { p0, e1: Vec3A::from(t[1]) - p0, e2: Vec3A::from(t[2]) - p0 };
        if let Some((t, u, v)) = hit_triangle(&acc, ray, best.map_or(ray.tmax, |h| h.t)) {
            best = Some(Hit { t, u, v, prim: i as u32 });
        }
    }
    best
}

fn v3() -> impl Strategy<Value = Vec3> {
    (-10.0f32..10.0, -10.0f32..10.0, -10.0f32..10.0).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

fn grid(n: usize) -> Vec<[Vec3; 3]> {
    let mut tris = Vec::new();
    for y in 0..n {
        for x in 0..n {
            let p = Vec3::new(x as f32, y as f32, ((x * 7 + y * 13) % 5) as f32 * 0.1);
            tris.push([p, p + Vec3::X, p + Vec3::Y]);
            tris.push([p + Vec3::X, p + Vec3::X + Vec3::Y, p + Vec3::Y]);
        }
    }
    tris
}

/// Every primitive appears in exactly one leaf, children lie inside their parents.
fn check_binary(bvh: &Bvh, n: usize) {
    let mut seen = vec![0u32; n];
    let mut stack = vec![0usize];
    while let Some(at) = stack.pop() {
        let node = bvh.nodes[at];
        if node.count > 0 {
            for s in node.a..node.a + node.count {
                seen[bvh.prim_order[s as usize] as usize] += 1;
            }
        } else {
            for c in [at + 1, at + node.a as usize] {
                let ch = bvh.nodes[c];
                for k in 0..3 {
                    assert!(ch.min[k] >= node.min[k] - 1e-4 && ch.max[k] <= node.max[k] + 1e-4);
                }
                stack.push(c);
            }
        }
    }
    assert!(seen.iter().all(|c| *c == 1), "{:?}", seen.iter().filter(|c| **c != 1).count());
}

fn ploc_of(tris: &[[Vec3; 3]]) -> TriangleBvh {
    TriangleBvh::build_with(tris, crate::Builder::Ploc)
}

fn wide_of(tris: &[[Vec3; 3]], builder: crate::Builder) -> TriangleBvh {
    let mut b = TriangleBvh::build_with(tris, builder);
    assert!(b.enable_wide(), "the tree could be collapsed");
    b
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]
    #[test]
    fn wide_and_ploc_trees_match_brute_force(
        tris in prop::collection::vec((v3(), v3(), v3()), 1..300),
        rays in prop::collection::vec((v3(), v3()), 1..40),
    ) {
        let tris: Vec<[Vec3; 3]> = tris.into_iter().map(|(a, b, c)| [a, a + b * 0.2, a + c * 0.2]).collect();
        let (plain_wide, ploc, ploc_wide) = (wide_of(&tris, crate::Builder::BinnedSah), ploc_of(&tris), wide_of(&tris, crate::Builder::Ploc));
        check_binary(&ploc.bvh, tris.len());
        for (o, d) in rays {
            if d.length() < 1e-3 { continue; }
            let ray = Ray::new(o, d.normalize());
            let truth = brute(&tris, &ray);
            for (name, tree) in [("wide", &plain_wide), ("ploc", &ploc), ("ploc wide", &ploc_wide)] {
                prop_assert_eq!(tree.intersect(&ray).map(|h| h.t), truth.map(|h| h.t), "{}", name);
                prop_assert_eq!(tree.occluded(&ray), truth.is_some(), "{}", name);
            }
        }
    }
}

#[test]
fn quantised_boxes_contain_the_boxes_they_stand_for() {
    let tris = grid(60);
    let tree = wide_of(&tris, crate::Builder::BinnedSah);
    let wide = tree.wide.as_ref().unwrap();
    // Walk the wide tree; every leaf child's box must contain its triangles, every interior child's
    // box the boxes of the child node's own children.
    let mut stack = vec![0usize];
    let mut leaf_prims = 0usize;
    while let Some(at) = stack.pop() {
        let node = wide.nodes[at];
        for k in 0..WIDTH {
            let b = node.child_box(k);
            match node.meta[k] {
                0 => {}
                1 => {
                    let child = wide.nodes[node.data[k] as usize];
                    for j in 0..WIDTH {
                        if child.meta[j] != 0 {
                            let cb = child.child_box(j);
                            for a in 0..3 {
                                assert!(
                                    cb.min[a] >= b.min[a] - 1e-3 && cb.max[a] <= b.max[a] + 1e-3,
                                    "child {j} of node {} escapes its parent's box",
                                    node.data[k]
                                );
                            }
                        }
                    }
                    stack.push(node.data[k] as usize);
                }
                m => {
                    let (first, count) = (node.data[k] as usize, (m & 0x7f) as usize);
                    for slot in first..first + count {
                        let t = tree.bvh.prim_order[slot] as usize;
                        for c in tris[t] {
                            for a in 0..3 {
                                assert!(c[a] >= b.min[a] && c[a] <= b.max[a], "a triangle corner lies outside its leaf's quantised box");
                            }
                        }
                    }
                    leaf_prims += count;
                }
            }
        }
    }
    assert_eq!(leaf_prims, tris.len());
    // A node holds up to eight children and the tree needs far fewer nodes than the binary one.
    assert!(wide.nodes.len() * 3 < tree.bvh.nodes.len(), "{} wide nodes for {} binary", wide.nodes.len(), tree.bvh.nodes.len());
}

#[test]
fn ploc_builds_a_tree_nearly_as_good_as_the_binned_build_and_survives_hostile_input() {
    let tris = grid(120);
    let ploc = ploc_of(&tris);
    check_binary(&ploc.bvh, tris.len());
    assert!(ploc.bvh.stats().max_depth <= crate::MAX_DEPTH + 1);
    // Quality on a triangle soup, where the Morton order follows the geometry: within 40 % of the
    // binned build's expected traversal cost (measured 15 % worse). A regular grid is the hard case
    // for an ordering by Morton code and is about 2.8 times worse.
    let mut seed = 7u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32
    };
    let soup: Vec<[Vec3; 3]> = (0..20000)
        .map(|_| {
            let a = Vec3::new(rnd(), rnd(), rnd()) * 100.0;
            [a, a + Vec3::new(rnd(), rnd(), rnd()), a + Vec3::new(rnd(), rnd(), rnd())]
        })
        .collect();
    let (a, b) = (TriangleBvh::build(&soup).bvh.stats(), ploc_of(&soup).bvh.stats());
    eprintln!("soup, binned SAH: {a:?}\nsoup, PLOC:       {b:?}");
    assert!(b.sah_cost < a.sah_cost * 1.4, "SAH cost {} against {}", b.sah_cost, a.sah_cost);
    let ray = Ray::new(Vec3::new(50.3, 70.2, 10.0), -Vec3::Z);
    assert_eq!(ploc.intersect(&ray).map(|h| h.t), brute(&tris, &ray).map(|h| h.t));
    // Empty, one primitive, bad primitives, coincident primitives.
    assert!(ploc_of(&[]).intersect(&Ray::new(Vec3::ZERO, Vec3::X)).is_none());
    assert!(ploc_of(&[[Vec3::new(1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, -1.0), Vec3::new(1.0, 0.0, 1.0)]]).intersect(&Ray::new(Vec3::ZERO, Vec3::X)).is_some());
    let bad = [
        [Vec3::splat(f32::NAN); 3],
        [Vec3::new(1.0, -1.0, -1.0), Vec3::new(1.0, 1.0, -1.0), Vec3::new(1.0, 0.0, 1.0)],
        [Vec3::ZERO, Vec3::splat(f32::INFINITY), Vec3::X],
    ];
    assert_eq!(ploc_of(&bad).triangle_count(), 1);
    let same = vec![[Vec3::ZERO, Vec3::X, Vec3::Y]; 5000];
    let tree = ploc_of(&same);
    assert!(tree.bvh.stats().max_depth <= crate::MAX_DEPTH + 1);
    assert!(tree.intersect(&Ray::new(Vec3::new(0.2, 0.2, 1.0), -Vec3::Z)).is_some());
    let mut empty = TriangleBvh::build(&[]);
    assert!(empty.enable_wide() && empty.intersect(&Ray::new(Vec3::ZERO, Vec3::X)).is_none());
    // A ray with a NaN direction finds nothing and does not hang.
    let w = wide_of(&same, crate::Builder::Ploc);
    assert!(w.intersect(&Ray::new(Vec3::ZERO, Vec3::splat(f32::NAN))).is_none());
    let _ = (Aabb::EMPTY, PrimInfo { bounds: Aabb::EMPTY, centroid: Vec3A::ZERO });
}

/// A timing, not a check: `cargo test --release -p craft-render-bvh -- --ignored --nocapture bench_wide`.
#[test]
#[ignore = "benchmark"]
fn bench_wide_and_ploc() {
    let n = 1000;
    let mut tris = Vec::with_capacity(n * n * 2);
    for y in 0..n {
        for x in 0..n {
            let h = |x: usize, y: usize| ((x as f32 * 0.05).sin() + (y as f32 * 0.07).cos()) * 3.0;
            let p = |x: usize, y: usize| Vec3::new(x as f32 * 0.1, h(x, y), y as f32 * 0.1);
            tris.push([p(x, y), p(x + 1, y), p(x, y + 1)]);
            tris.push([p(x + 1, y), p(x + 1, y + 1), p(x, y + 1)]);
        }
    }
    // Incoherent rays: random origins above the field, random downward directions.
    let mut seed = 12345u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32
    };
    let rays: Vec<Ray> =
        (0..1_000_000).map(|_| Ray::new(Vec3::new(rnd() * 100.0, 8.0, rnd() * 100.0), Vec3::new(rnd() - 0.5, -0.3 - rnd(), rnd() - 0.5).normalize())).collect();
    for (label, builder) in [("binned SAH", crate::Builder::BinnedSah), ("PLOC", crate::Builder::Ploc)] {
        let t = std::time::Instant::now();
        let mut tree = TriangleBvh::build_with(&tris, builder);
        let build = t.elapsed();
        let stats = tree.bvh.stats();
        let t = std::time::Instant::now();
        let ok = tree.enable_wide();
        let collapse = t.elapsed();
        for wide in [false, true] {
            if wide && !ok {
                continue;
            }
            if !wide {
                tree.wide = None;
            } else {
                tree.enable_wide();
            }
            let t = std::time::Instant::now();
            let hits = rays.iter().filter(|r| tree.intersect(r).is_some()).count();
            let secs = t.elapsed().as_secs_f64();
            eprintln!(
                "{label:10} {:6}: build {:.0} ms{}, {} binary nodes, SAH {:.1}; {} rays on one thread {:.0} ms = {:.2} M rays/s, {hits} hits",
                if wide { "8-wide" } else { "binary" },
                build.as_secs_f64() * 1e3,
                if wide { format!(" + collapse {:.0} ms", collapse.as_secs_f64() * 1e3) } else { String::new() },
                stats.nodes,
                stats.sah_cost,
                rays.len(),
                secs * 1e3,
                rays.len() as f64 / secs / 1e6
            );
        }
    }
}
