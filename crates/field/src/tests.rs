use super::*;
use proptest::prelude::*;

#[test]
fn every_shape_has_a_default_and_names_itself() {
    for k in SHAPES {
        assert_eq!(Field::default_of(k).expect("default").kind(), *k);
    }
    assert!(Field::default_of("hyperboloid").is_none());
}

#[test]
fn sphere_is_full_inside_and_falls_to_zero_at_its_radius() {
    let s = Shape::Sphere { radius: 2.0, inner: 0.5 };
    assert_eq!(s.value(Vec3::ZERO), 1.0);
    assert_eq!(s.value(Vec3::new(0.9, 0.0, 0.0)), 1.0, "inside the inner radius");
    assert!((s.value(Vec3::new(1.5, 0.0, 0.0)) - 0.5).abs() < 1e-5, "halfway between inner and outer");
    assert_eq!(s.value(Vec3::new(2.0, 0.0, 0.0)), 0.0);
    assert_eq!(s.value(Vec3::new(50.0, 0.0, 0.0)), 0.0);
}

#[test]
fn box_uses_the_furthest_axis() {
    let b = Shape::Box { size: Vec3::new(2.0, 4.0, 2.0), inner: 0.0 };
    assert_eq!(b.value(Vec3::ZERO), 1.0);
    assert!((b.value(Vec3::new(0.5, 0.0, 0.0)) - 0.5).abs() < 1e-5);
    assert!((b.value(Vec3::new(0.0, 1.0, 0.0)) - 0.5).abs() < 1e-5, "taller box, same fraction");
    assert_eq!(b.value(Vec3::new(0.0, 0.0, 1.5)), 0.0);
}

#[test]
fn linear_runs_along_y() {
    let l = Shape::Linear { length: 4.0 };
    assert_eq!(l.value(Vec3::new(9.0, -3.0, 9.0)), 1.0, "below the origin plane");
    assert!((l.value(Vec3::new(0.0, 1.0, 0.0)) - 0.75).abs() < 1e-5);
    assert_eq!(l.value(Vec3::new(0.0, 4.0, 0.0)), 0.0);
}

#[test]
fn noise_is_seeded_smooth_and_spread() {
    let a = Shape::Noise { scale: 1.0, octaves: 3, seed: 7 };
    let b = Shape::Noise { scale: 1.0, octaves: 3, seed: 8 };
    let pts: Vec<Vec3> = (0..200).map(|i| Vec3::new(i as f32 * 0.37, (i as f32 * 0.11).sin() * 3.0, i as f32 * 0.05)).collect();
    let va: Vec<f32> = pts.iter().map(|p| a.value(*p)).collect();
    assert_eq!(va, pts.iter().map(|p| a.value(*p)).collect::<Vec<_>>(), "deterministic");
    assert_ne!(va, pts.iter().map(|p| b.value(*p)).collect::<Vec<_>>(), "the seed changes it");
    let (lo, hi) = va.iter().fold((1.0f32, 0.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
    assert!(lo < 0.3 && hi > 0.7, "uses the range: {lo}..{hi}");
    // Smooth: a tiny step changes the value a tiny amount.
    for p in pts.iter().take(50) {
        assert!((a.value(*p) - a.value(*p + Vec3::splat(1e-3))).abs() < 0.02);
    }
}

#[test]
fn presets_are_monotone_and_hit_both_ends() {
    for n in CURVE_PRESETS {
        let c = Curve::preset(n).expect("preset");
        assert_eq!(c.preset_name(), Some(*n));
        let v: Vec<f32> = (0..=100).map(|i| c.eval(i as f32 / 100.0)).collect();
        assert!(v[0].abs() < 1e-6 && (v[100] - 1.0).abs() < 1e-6, "{n}");
        assert!(v.windows(2).all(|w| w[1] >= w[0] - 1e-5), "{n} is monotone");
    }
    assert!((Curve::linear().eval(0.3) - 0.3).abs() < 1e-5);
    assert!(Curve::preset("ease_in").expect("p").eval(0.5) < 0.5);
    assert!(Curve::preset("ease_out").expect("p").eval(0.5) > 0.5);
}

#[test]
fn a_hostile_curve_is_repaired() {
    let c = Curve { points: vec![[f32::NAN, 0.5], [3.0, -2.0], [0.2, f32::INFINITY], [0.2, 0.9]] };
    let s = c.sanitized();
    assert!(s.points.len() >= 2);
    assert!(s.points.windows(2).all(|w| w[0][0] < w[1][0]));
    assert!(Curve { points: vec![] }.eval(0.5) == 0.5, "empty falls back to the line");
    let many = Curve { points: (0..100).map(|i| [i as f32 / 100.0, 0.5]).collect() };
    assert_eq!(many.sanitized().points.len(), Curve::MAX_POINTS);
}

#[test]
fn remap_inverts_and_scales() {
    let r = Remap { curve: Curve::linear(), invert: true, strength: 0.5 };
    assert!((r.apply(0.0) - 0.5).abs() < 1e-5);
    assert!(r.apply(1.0).abs() < 1e-5);
    assert_eq!(Remap { strength: 0.0, ..Remap::default() }.apply(0.8), 0.0);
}

#[test]
fn stacks_blend_in_their_own_frames() {
    let mut s = FieldStack::new();
    assert_eq!(s.weight(Vec3::splat(5.0)), 1.0, "no fields, no falloff");
    // A sphere at x = 4: the stack asks the field about p - (4, 0, 0).
    s.push(Mat4::from_translation(Vec3::new(-4.0, 0.0, 0.0)), Field::default_of("sphere").expect("sphere"));
    assert_eq!(s.weight(Vec3::new(4.0, 0.0, 0.0)), 1.0);
    assert_eq!(s.weight(Vec3::ZERO), 0.0);
    // Subtract a smaller sphere from the middle: a ring-like hole.
    let mut hole = Field::default_of("sphere").expect("sphere");
    hole.shape = Shape::Sphere { radius: 0.5, inner: 0.0 };
    hole.blend = Blend::Subtract;
    s.push(Mat4::from_translation(Vec3::new(-4.0, 0.0, 0.0)), hole);
    assert_eq!(s.weight(Vec3::new(4.0, 0.0, 0.0)), 0.0);
    assert!(s.weight(Vec3::new(4.8, 0.0, 0.0)) > 0.1);
    // Max joins two spheres.
    let mut t = FieldStack::new();
    t.push(Mat4::from_translation(Vec3::new(2.0, 0.0, 0.0)), Field::default_of("sphere").expect("s"));
    let mut other = Field::default_of("sphere").expect("s");
    other.blend = Blend::Max;
    t.push(Mat4::from_translation(Vec3::new(-2.0, 0.0, 0.0)), other);
    assert_eq!(t.weight(Vec3::new(-2.0, 0.0, 0.0)), 1.0);
    assert_eq!(t.weight(Vec3::new(2.0, 0.0, 0.0)), 1.0);
    // A field scaled to nothing covers nothing: weight 0, and with Max it leaves the rest alone.
    let before = t.len();
    let mut nothing = Field::default_of("box").expect("b");
    nothing.blend = Blend::Max;
    t.push(Mat4::ZERO, nothing);
    assert_eq!(t.len(), before + 1);
    assert_eq!(t.weight(Vec3::new(2.0, 0.0, 0.0)), 1.0);
    let mut alone = FieldStack::new();
    alone.push(Mat4::from_scale(Vec3::ZERO).inverse(), Field::default_of("sphere").expect("s"));
    assert_eq!(alone.weight(Vec3::ZERO), 0.0, "a lone field with no extent weighs nothing, rather than everything");
}

#[test]
fn fields_round_trip_through_json_and_old_files_still_read() {
    for k in SHAPES {
        let f = Field::default_of(k).expect("default");
        let json = serde_json::to_string(&f).expect("ser");
        assert!(json.contains(&format!("\"shape\":\"{k}\"")), "{json}");
        assert_eq!(serde_json::from_str::<Field>(&json).expect("de"), f);
    }
    let f: Field = serde_json::from_str(r#"{"shape":"sphere","radius":3.0,"inner":0.1}"#).expect("minimal");
    assert_eq!(f.blend, Blend::Multiply);
    assert_eq!(f.remap, Remap::default());
}

fn any_f32() -> impl Strategy<Value = f32> {
    prop_oneof![proptest::num::f32::ANY, -100.0f32..100.0, Just(f32::NAN), Just(f32::INFINITY), Just(0.0)]
}

fn any_vec3() -> impl Strategy<Value = Vec3> {
    (any_f32(), any_f32(), any_f32()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

fn any_expr() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("x + y".to_string()),
        Just("sin(x * 3) * 0.5 + 0.5".to_string()),
        Just("1 / x".to_string()),
        Just("sqrt(0 - r)".to_string()),
        Just("not a formula (".to_string()),
        Just("q".to_string()),
        Just("9".repeat(3000)),
        any::<String>(),
    ]
}

fn any_shape() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (any_f32(), any_f32()).prop_map(|(angle, inner)| Shape::Radial { angle, inner }),
        (any_f32(), any_f32(), any_f32()).prop_map(|(radius, height, inner)| Shape::Cylinder { radius, height, inner }),
        (any_f32(), any_f32(), any_f32()).prop_map(|(radius, height, inner)| Shape::Cone { radius, height, inner }),
        (any_f32(), any_f32(), any_f32()).prop_map(|(radius, length, inner)| Shape::Capsule { radius, length, inner }),
        (any_f32(), any_f32(), any_f32()).prop_map(|(radius, tube, inner)| Shape::Torus { radius, tube, inner }),
        (any_f32(), any::<u32>()).prop_map(|(scale, seed)| Shape::Random { scale, seed }),
        any_expr().prop_map(|expr| Shape::Formula { expr }),
        (any_f32(), any_f32()).prop_map(|(radius, inner)| Shape::Sphere { radius, inner }),
        (any_vec3(), any_f32()).prop_map(|(size, inner)| Shape::Box { size, inner }),
        any_f32().prop_map(|length| Shape::Linear { length }),
        (any_f32(), any::<u32>(), any::<u32>()).prop_map(|(scale, octaves, seed)| Shape::Noise { scale, octaves, seed }),
    ]
}

fn any_blend() -> impl Strategy<Value = Blend> {
    prop_oneof![Just(Blend::Multiply), Just(Blend::Add), Just(Blend::Subtract), Just(Blend::Min), Just(Blend::Max)]
}

fn any_curve() -> impl Strategy<Value = Curve> {
    proptest::collection::vec((any_f32(), any_f32()).prop_map(|(x, y)| [x, y]), 0..40).prop_map(|points| Curve { points })
}

fn any_field() -> impl Strategy<Value = Field> {
    (any_shape(), any_curve(), any::<bool>(), any_f32(), any_blend(), any_f32(), any::<bool>()).prop_map(
        |(shape, curve, invert, strength, blend, opacity, enabled)| Field { shape, remap: Remap { curve, invert, strength }, blend, opacity, enabled },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Whatever the parameters and the point (NaN, infinite, huge): the weight is finite and in 0..=1.
    #[test]
    fn any_field_gives_a_finite_weight_in_range(f in any_field(), p in any_vec3()) {
        let w = f.weight(p);
        prop_assert!(w.is_finite() && (0.0..=1.0).contains(&w), "{w}");
    }

    /// A stack of any fields in any (even singular) frames stays finite and in range.
    #[test]
    fn any_stack_stays_in_range(fields in proptest::collection::vec(any_field(), 0..40), p in any_vec3(), t in any_vec3()) {
        let mut s = FieldStack::new();
        for f in fields {
            s.push(Mat4::from_translation(if t.is_finite() { t } else { Vec3::ZERO }), f);
        }
        prop_assert!(s.len() <= MAX_LAYERS);
        let w = s.weight(p);
        prop_assert!(w.is_finite() && (0.0..=1.0).contains(&w));
    }

    /// A sphere never gets stronger further from its centre.
    #[test]
    fn sphere_weight_falls_with_distance(radius in 0.1f32..50.0, inner in 0.0f32..0.99, d1 in 0.0f32..100.0, d2 in 0.0f32..100.0, dir in any_vec3()) {
        let dir = dir.normalize_or(Vec3::X);
        let s = Shape::Sphere { radius, inner };
        let (a, b) = if d1 <= d2 { (d1, d2) } else { (d2, d1) };
        prop_assert!(s.value(dir * a) >= s.value(dir * b) - 1e-5);
    }

    /// A curve through increasing points never overshoots 0..1 and stays monotone when its points are.
    #[test]
    fn monotone_curves_stay_monotone(ys in proptest::collection::vec(0.0f32..1.0, 2..10)) {
        let mut ys = ys;
        ys.sort_by(f32::total_cmp);
        let n = ys.len();
        let c = Curve { points: ys.iter().enumerate().map(|(i, y)| [i as f32 / (n - 1) as f32, *y]).collect() };
        let cc = c.compile();
        let v: Vec<f32> = (0..=200).map(|i| cc.eval(i as f32 / 200.0)).collect();
        prop_assert!(v.iter().all(|x| (0.0..=1.0).contains(x)));
        prop_assert!(v.windows(2).all(|w| w[1] >= w[0] - 1e-4));
    }
}

// ---------------------------------------------------------------- the second set of shapes

#[test]
fn radial_sweeps_around_y_from_plus_x() {
    let r = Shape::Radial { angle: 180.0, inner: 0.0 };
    assert_eq!(r.value(Vec3::new(3.0, 0.0, 0.0)), 1.0, "along +X");
    assert!((r.value(Vec3::new(1.0, 0.0, -1.0)) - 0.5).abs() < 1e-4, "half way to the edge of the sweep (45 of 90 degrees)");
    assert!((r.value(Vec3::new(1.0, 5.0, 1.0)) - 0.5).abs() < 1e-4, "height does not matter, either way round");
    assert_eq!(r.value(Vec3::new(0.0, 0.0, -1.0)), 0.0, "a quarter turn is the edge of a half-turn sweep");
    assert_eq!(r.value(Vec3::new(-1.0, 0.0, 0.0)), 0.0, "opposite is outside a half-turn sweep");
    let wide = Shape::Radial { angle: 360.0, inner: 0.0 };
    assert!(wide.value(Vec3::new(-1.0, 0.0, 0.0)) < 1e-4);
    assert!(wide.value(Vec3::new(0.0, 0.0, 1.0)) > 0.4);
}

#[test]
fn cylinder_cone_capsule_and_torus_measure_their_own_distance() {
    let c = Shape::Cylinder { radius: 1.0, height: 4.0, inner: 0.0 };
    assert_eq!(c.value(Vec3::ZERO), 1.0);
    assert!((c.value(Vec3::new(0.5, 0.0, 0.0)) - 0.5).abs() < 1e-5);
    assert!((c.value(Vec3::new(0.0, 1.0, 0.0)) - 0.5).abs() < 1e-5, "half the half-height");
    assert_eq!(c.value(Vec3::new(0.0, 2.5, 0.0)), 0.0);
    assert_eq!(c.value(Vec3::new(1.5, 0.0, 0.0)), 0.0);

    // A cone is wide at the bottom and thin at the top.
    let k = Shape::Cone { radius: 1.0, height: 2.0, inner: 0.0 };
    let low = k.value(Vec3::new(0.6, -0.8, 0.0));
    let high = k.value(Vec3::new(0.6, 0.8, 0.0));
    assert!(low > high, "{low} vs {high}");
    assert_eq!(k.value(Vec3::new(0.6, 0.95, 0.0)), 0.0, "outside near the tip");
    assert_eq!(k.value(Vec3::new(0.0, -1.5, 0.0)), 0.0, "below the base");

    // A capsule is full along its segment and round at the ends.
    let cap = Shape::Capsule { radius: 1.0, length: 4.0, inner: 0.0 };
    assert_eq!(cap.value(Vec3::new(0.0, 1.9, 0.0)), 1.0, "on the segment");
    assert!((cap.value(Vec3::new(0.5, 1.0, 0.0)) - 0.5).abs() < 1e-5);
    assert!((cap.value(Vec3::new(0.0, 2.5, 0.0)) - 0.5).abs() < 1e-5, "half a radius beyond the end");
    assert_eq!(cap.value(Vec3::new(0.0, 3.5, 0.0)), 0.0);

    // A torus is full on its ring and empty at the hole.
    let t = Shape::Torus { radius: 2.0, tube: 0.5, inner: 0.0 };
    assert_eq!(t.value(Vec3::new(2.0, 0.0, 0.0)), 1.0);
    assert_eq!(t.value(Vec3::new(0.0, 0.0, -2.0)), 1.0);
    assert!((t.value(Vec3::new(2.25, 0.0, 0.0)) - 0.5).abs() < 1e-5);
    assert_eq!(t.value(Vec3::ZERO), 0.0);
    assert_eq!(t.value(Vec3::new(2.0, 1.0, 0.0)), 0.0);
}

#[test]
fn random_is_blocky_seeded_and_spread() {
    let a = Shape::Random { scale: 1.0, seed: 3 };
    assert_eq!(a.value(Vec3::new(0.1, 0.2, 0.3)), a.value(Vec3::new(0.9, 0.8, 0.7)), "one cell, one value");
    let b = Shape::Random { scale: 1.0, seed: 4 };
    let pts: Vec<Vec3> = (0..64).map(|i| Vec3::new(i as f32 + 0.5, 0.5, 0.5)).collect();
    let va: Vec<f32> = pts.iter().map(|p| a.value(*p)).collect();
    assert_ne!(va, pts.iter().map(|p| b.value(*p)).collect::<Vec<_>>());
    let (lo, hi) = va.iter().fold((1.0f32, 0.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
    assert!(lo < 0.2 && hi > 0.8, "{lo}..{hi}");
}

#[test]
fn a_formula_field_reads_the_point_and_survives_bad_formulas() {
    let f = Shape::Formula { expr: "1 - r / 2".into() };
    assert_eq!(f.value(Vec3::ZERO), 1.0);
    assert!((f.value(Vec3::new(1.0, 0.0, 0.0)) - 0.5).abs() < 1e-6);
    let g = Shape::Formula { expr: "clamp(y + 0.5, 0, 1)".into() };
    assert!((g.value(Vec3::new(9.0, 0.25, 0.0)) - 0.75).abs() < 1e-6);
    // A formula that does not parse, names something unknown, or divides by zero weighs 0.
    for bad in ["1 +", "wobble * 2", "1 / (x - x)", ""] {
        assert_eq!(Shape::Formula { expr: bad.into() }.value(Vec3::ONE), 0.0, "{bad:?}");
    }
    // The stack parses it once and gets the same answer.
    let mut s = FieldStack::new();
    s.push(Mat4::IDENTITY, Field { shape: f.clone(), ..Field::default_of("sphere").expect("s") });
    assert!((s.weight(Vec3::new(1.0, 0.0, 0.0)) - 0.5).abs() < 1e-6);
    // An absurdly long formula is cut to the language's limit.
    let long = Shape::Formula { expr: "1+".repeat(2000) + "1" }.sanitized();
    assert!(matches!(&long, Shape::Formula { expr } if expr.chars().count() == MAX_FORMULA));
}

#[test]
fn opacity_fades_a_field_between_leaving_the_stack_alone_and_applying_it() {
    let sphere = |blend, opacity| Field { blend, opacity, ..Field::default_of("sphere").expect("s") };
    let mut full = FieldStack::new();
    full.push(Mat4::IDENTITY, sphere(Blend::Multiply, 1.0));
    full.push(Mat4::from_translation(Vec3::new(-1.0, 0.0, 0.0)), sphere(Blend::Subtract, 1.0));
    let mut half = FieldStack::new();
    half.push(Mat4::IDENTITY, sphere(Blend::Multiply, 1.0));
    half.push(Mat4::from_translation(Vec3::new(-1.0, 0.0, 0.0)), sphere(Blend::Subtract, 0.5));
    let mut none = FieldStack::new();
    none.push(Mat4::IDENTITY, sphere(Blend::Multiply, 1.0));
    none.push(Mat4::from_translation(Vec3::new(-1.0, 0.0, 0.0)), sphere(Blend::Subtract, 0.0));
    let p = Vec3::new(0.5, 0.0, 0.0);
    let (a, b, c) = (full.weight(p), half.weight(p), none.weight(p));
    assert!(a < b && b < c, "{a} < {b} < {c}");
    assert!((b - (a + c) / 2.0).abs() < 1e-5, "halfway between");
    // The first field with opacity 0 is no falloff at all.
    let mut first = FieldStack::new();
    first.push(Mat4::IDENTITY, sphere(Blend::Multiply, 0.0));
    assert_eq!(first.weight(Vec3::splat(50.0)), 1.0);
}

#[test]
fn a_disabled_field_is_left_out_of_the_stack() {
    let mut s = FieldStack::new();
    s.push(Mat4::IDENTITY, Field { enabled: false, ..Field::default_of("sphere").expect("s") });
    assert!(s.is_empty());
    assert_eq!(s.weight(Vec3::splat(9.0)), 1.0);
    s.push(Mat4::IDENTITY, Field::default_of("sphere").expect("s"));
    assert_eq!(s.weight(Vec3::splat(9.0)), 0.0);
}

#[test]
fn old_fields_without_opacity_or_switch_read_as_full_and_on() {
    let f: Field = serde_json::from_str(r#"{"shape":"noise","scale":2.0,"octaves":2,"seed":5,"blend":"add"}"#).expect("old");
    assert_eq!(f.opacity, 1.0);
    assert!(f.enabled);
    let f: Field = serde_json::from_str(r#"{"shape":"formula","expr":"x"}"#).expect("formula");
    assert_eq!(f.kind(), "formula");
}
