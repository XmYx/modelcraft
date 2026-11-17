use super::*;
use proptest::prelude::*;

fn cube() -> PolyMesh {
    let p = vec![
        Vec3::new(-0.5, -0.5, -0.5),
        Vec3::new(0.5, -0.5, -0.5),
        Vec3::new(0.5, 0.5, -0.5),
        Vec3::new(-0.5, 0.5, -0.5),
        Vec3::new(-0.5, -0.5, 0.5),
        Vec3::new(0.5, -0.5, 0.5),
        Vec3::new(0.5, 0.5, 0.5),
        Vec3::new(-0.5, 0.5, 0.5),
    ];
    let f: Vec<Vec<u32>> = vec![vec![0, 3, 2, 1], vec![4, 5, 6, 7], vec![0, 1, 5, 4], vec![2, 3, 7, 6], vec![1, 2, 6, 5], vec![3, 0, 4, 7]];
    PolyMesh::from_faces(p, &f).expect("a cube")
}

fn lay(kind: &str, target: Option<&PolyMesh>) -> CloneSet {
    Cloner::default_of(kind).expect("kind").generate(target).expect("layout")
}

fn translation(set: &CloneSet, i: usize) -> Vec3 {
    set.instances[i].matrix.w_axis.truncate()
}

#[test]
fn every_kind_has_a_default_and_names_itself() {
    for k in KINDS {
        let c = Cloner::default_of(k).expect("default");
        assert_eq!(c.kind(), *k);
    }
    assert!(Cloner::default_of("honeycomb").is_none());
}

#[test]
fn linear_steps_offset_rotation_and_scale() {
    let c =
        Cloner { layout: Layout::Linear { count: 4, offset: Vec3::new(2.0, 0.0, 0.0), rotation: Vec3::new(0.0, 30.0, 0.0), scale: Vec3::splat(0.5) }, seed: 0 };
    let set = c.generate(None).expect("linear");
    assert_eq!(set.len(), 4);
    assert!((translation(&set, 3) - Vec3::new(6.0, 0.0, 0.0)).length() < 1e-5);
    let (s, r, _) = set.instances[2].matrix.to_scale_rotation_translation();
    assert!((s - Vec3::splat(2.0)).length() < 1e-5);
    assert!((r.to_euler(EulerRot::XYZ).1.to_degrees() - 60.0).abs() < 1e-3);
    assert_eq!(set.instances[0].index, 0);
    assert!(!set.truncated);
}

#[test]
fn radial_full_turn_does_not_repeat_the_first_clone_and_faces_outwards() {
    let set = lay("radial", None);
    assert_eq!(set.len(), 8);
    for i in 0..8 {
        assert!((translation(&set, i).length() - 2.0).abs() < 1e-4, "on the ring");
        // The clone's +X points away from the centre.
        let out = set.instances[i].matrix.transform_vector3(Vec3::X);
        assert!(out.dot(translation(&set, i).normalize()) > 0.999);
    }
    assert!((translation(&set, 0) - translation(&set, 7)).length() > 0.5);
    // Not a full turn: an arc includes both ends.
    let arc = Cloner { layout: Layout::Radial { count: 3, radius: 1.0, axis: Axis::Z, start: 0.0, end: 180.0, align: false }, seed: 0 };
    let set = arc.generate(None).expect("arc");
    assert!((translation(&set, 0) - Vec3::X).length() < 1e-5);
    assert!((translation(&set, 2) + Vec3::X).length() < 1e-4);
    assert!(translation(&set, 1).z.abs() < 1e-5);
}

#[test]
fn grid_is_centred_and_counts_the_product() {
    let c = Cloner { layout: Layout::Grid { count: [3, 2, 2], spacing: Vec3::new(1.0, 2.0, 3.0) }, seed: 0 };
    let set = c.generate(None).expect("grid");
    assert_eq!(set.len(), 12);
    let sum: Vec3 = (0..12).map(|i| translation(&set, i)).sum();
    assert!(sum.length() < 1e-4, "centred on the origin");
    assert!(translation(&set, 0).x < 0.0);
}

#[test]
fn a_layout_past_the_cap_stops_and_says_so() {
    let c = Cloner { layout: Layout::Grid { count: [u32::MAX, u32::MAX, u32::MAX], spacing: Vec3::ONE }, seed: 0 };
    let set = c.generate(None).expect("grid");
    assert_eq!(set.len(), MAX_CLONES);
    assert!(set.truncated);
    let c = Cloner { layout: Layout::Scatter { count: u32::MAX, extent: Vec3::ONE }, seed: 0 };
    let set = c.generate(None).expect("scatter");
    assert_eq!(set.len(), MAX_CLONES);
    assert!(set.truncated);
}

#[test]
fn scatter_is_seeded_and_stays_in_its_box() {
    let mut a = Cloner::default_of("scatter").expect("scatter");
    let first = a.generate(None).expect("a");
    assert_eq!(first, a.generate(None).expect("again"), "same seed, same clones");
    a.seed = 2;
    assert_ne!(first, a.generate(None).expect("b"), "another seed, other clones");
    for i in 0..first.len() {
        assert!(translation(&first, i).abs().max_element() <= 2.0 + 1e-5);
    }
    // Spread over the box rather than bunched.
    let mean: Vec3 = (0..first.len()).map(|i| translation(&first, i)).sum::<Vec3>() / first.len() as f32;
    assert!(mean.length() < 1.2);
}

#[test]
fn surface_clones_sit_on_the_target_and_stand_on_it() {
    let cube = cube();
    let set = lay("surface", Some(&cube));
    assert_eq!(set.len(), 60);
    for i in 0..set.len() {
        let p = translation(&set, i);
        assert!((p.abs().max_element() - 0.5).abs() < 1e-4, "on a face of the cube: {p:?}");
        // +Y follows a face normal: one of the axes, towards the side the point is on.
        let up = set.instances[i].matrix.transform_vector3(Vec3::Y);
        let on_face = [Vec3::X, Vec3::Y, Vec3::Z].into_iter().any(|a| (p.dot(a).abs() - 0.5).abs() < 1e-3 && up.dot(a * p.dot(a).signum()) > 0.99);
        assert!(on_face, "clone {i} at {p:?} stands along {up:?}");
    }
}

#[test]
fn vertices_puts_one_clone_on_each_vertex() {
    let cube = cube();
    let set = lay("vertices", Some(&cube));
    assert_eq!(set.len(), 8);
    for (i, p) in cube.positions().iter().enumerate() {
        assert!((translation(&set, i) - *p).length() < 1e-6);
    }
}

#[test]
fn target_layouts_report_a_missing_or_empty_target() {
    let c = Cloner::default_of("surface").expect("surface");
    assert_eq!(c.generate(None), Err(CloneError::NeedsTarget));
    assert_eq!(c.generate(Some(&PolyMesh::default())), Err(CloneError::EmptyTarget));
    let v = Cloner::default_of("vertices").expect("vertices");
    assert_eq!(v.generate(None), Err(CloneError::NeedsTarget));
    assert_eq!(v.generate(Some(&PolyMesh::default())), Err(CloneError::EmptyTarget));
}

#[test]
fn clones_are_data_not_meshes() {
    let set = Cloner { layout: Layout::Grid { count: [100, 1, 100], spacing: Vec3::ONE }, seed: 0 }.generate(None).expect("grid");
    assert_eq!(set.len(), MAX_CLONES);
    assert!(std::mem::size_of::<CloneInstance>() <= 96);
    assert!(set.len() * std::mem::size_of::<CloneInstance>() < 1_000_000);
}

#[test]
fn cloners_round_trip_through_json() {
    for k in KINDS {
        let c = Cloner::default_of(k).expect("default");
        let json = serde_json::to_string(&c).expect("serialize");
        assert!(json.contains(&format!("\"mode\":\"{k}\"")), "{json}");
        assert_eq!(serde_json::from_str::<Cloner>(&json).expect("parse"), c);
    }
    // A file without a seed still reads.
    let c: Cloner = serde_json::from_str(r#"{"mode":"vertices","align":false}"#).expect("no seed");
    assert_eq!(c.seed, 0);
}

fn any_f32() -> impl Strategy<Value = f32> {
    prop_oneof![proptest::num::f32::ANY, -1000.0f32..1000.0, Just(f32::NAN), Just(f32::INFINITY), Just(0.0)]
}

fn any_vec3() -> impl Strategy<Value = Vec3> {
    (any_f32(), any_f32(), any_f32()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

fn any_axis() -> impl Strategy<Value = Axis> {
    prop_oneof![Just(Axis::X), Just(Axis::Y), Just(Axis::Z)]
}

fn any_layout() -> impl Strategy<Value = Layout> {
    prop_oneof![
        (any::<u32>(), any_vec3(), any_vec3(), any_vec3()).prop_map(|(count, offset, rotation, scale)| Layout::Linear { count, offset, rotation, scale }),
        (any::<u32>(), any_f32(), any_axis(), any_f32(), any_f32(), any::<bool>()).prop_map(|(count, radius, axis, start, end, align)| Layout::Radial {
            count,
            radius,
            axis,
            start,
            end,
            align
        }),
        (any::<[u32; 3]>(), any_vec3()).prop_map(|(count, spacing)| Layout::Grid { count, spacing }),
        (any::<u32>(), any_vec3()).prop_map(|(count, extent)| Layout::Scatter { count, extent }),
        (any::<u32>(), any::<bool>()).prop_map(|(count, align)| Layout::Surface { count, align }),
        any::<bool>().prop_map(|align| Layout::Vertices { align }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the numbers (NaN, infinite, huge, zero): the clones are finite, never more than the
    /// cap, deterministic, and a layout that asked for more says it was cut.
    #[test]
    fn any_layout_gives_finite_bounded_deterministic_clones(layout in any_layout(), seed in any::<u32>()) {
        let c = Cloner { layout, seed };
        let target = cube();
        let a = c.generate(Some(&target)).expect("a cube is a valid target");
        prop_assert!(a.len() <= MAX_CLONES);
        prop_assert!(!a.is_empty());
        prop_assert!(a.instances.iter().all(|i| i.matrix.is_finite() && i.color.iter().all(|c| c.is_finite())));
        prop_assert_eq!(&a, &c.generate(Some(&target)).expect("again"));
        let asked = c.layout.requested(target.vertex_count());
        prop_assert_eq!(a.truncated, asked > a.len() as u64);
        for (n, i) in a.instances.iter().enumerate() {
            prop_assert_eq!(i.index as usize, n);
        }
    }

    /// The scale of a clone never reaches zero, so no clone is singular.
    #[test]
    fn clones_never_collapse(layout in any_layout()) {
        let set = Cloner { layout, seed: 3 }.generate(Some(&cube())).expect("generate");
        for i in &set.instances {
            let (s, _, _) = i.matrix.to_scale_rotation_translation();
            prop_assert!(s.is_finite() && s.min_element() > 0.0);
        }
    }
}
