use super::*;
use crate::{Cloner, Layout};
use proptest::prelude::*;

fn row(n: u32) -> CloneSet {
    Cloner { layout: Layout::Linear { count: n, offset: Vec3::new(1.0, 0.0, 0.0), rotation: Vec3::ZERO, scale: Vec3::ZERO }, seed: 3 }
        .generate(None)
        .expect("a row")
}

fn full(_: Vec3) -> f32 {
    1.0
}

fn ctx(time: f32, target: Option<Vec3>) -> Context<'static> {
    Context { seed: 7, time, target, weight: &full }
}

fn pos(set: &CloneSet, i: usize) -> Vec3 {
    set.instances[i].matrix.w_axis.truncate()
}

fn made(kind: &str) -> Modifier {
    Modifier::default_of(kind).expect("kind")
}

fn finite(set: &CloneSet) -> bool {
    set.instances.iter().all(|i| i.matrix.is_finite() && i.color.iter().all(|c| c.is_finite()))
}

#[test]
fn every_mode_has_a_default_names_itself_and_round_trips() {
    for k in MODES {
        let m = Modifier::default_of(k).expect("default");
        assert_eq!(m.kind(), *k);
        let json = serde_json::to_string(&m).expect("json");
        let back: Modifier = serde_json::from_str(&json).expect("back");
        assert_eq!(back, m, "{k}: {json}");
    }
    assert!(Modifier::default_of("shader").is_none());
    // Missing fields take their defaults.
    let m: Modifier = serde_json::from_str(r#"{"mode":"step"}"#).expect("sparse");
    assert_eq!(m.strength, 1.0);
    assert!(matches!(m.mode, Mode::Step { steps: 0, .. }));
}

#[test]
fn plain_moves_every_clone_by_the_same_amount() {
    let mut set = row(5);
    let before = set.clone();
    let mut m = made("plain");
    m.position = Vec3::new(0.0, 2.0, 0.0);
    m.apply(&mut set, &ctx(0.0, None));
    for i in 0..5 {
        assert!((pos(&set, i) - pos(&before, i) - Vec3::new(0.0, 2.0, 0.0)).length() < 1e-5);
        assert_eq!(set.instances[i].index, i as u32);
    }
}

#[test]
fn plain_turns_about_the_clones_own_axes_and_grows_them() {
    let mut set = row(2);
    let mut m = made("plain");
    m.position = Vec3::ZERO;
    m.rotation = Vec3::new(0.0, 90.0, 0.0);
    m.scale = Vec3::splat(1.0);
    m.uniform_scale = true;
    m.apply(&mut set, &ctx(0.0, None));
    let (s, q, t) = set.instances[1].matrix.to_scale_rotation_translation();
    assert!((s - Vec3::splat(2.0)).length() < 1e-4);
    assert!((q * Vec3::X - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-4);
    assert!((t - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5, "turning does not move the clone");
}

#[test]
fn strength_zero_and_zero_weight_change_nothing() {
    let before = row(6);
    let mut set = before.clone();
    let mut m = made("random");
    m.rotation = Vec3::splat(45.0);
    m.strength = 0.0;
    m.apply(&mut set, &ctx(0.0, None));
    assert_eq!(set, before);
    m.strength = 1.0;
    let none = |_: Vec3| 0.0;
    m.apply(&mut set, &Context { seed: 0, time: 0.0, target: None, weight: &none });
    assert_eq!(set, before);
}

#[test]
fn the_weight_scales_the_change_per_clone() {
    let mut set = row(4);
    let before = set.clone();
    let mut m = made("plain");
    m.position = Vec3::new(0.0, 4.0, 0.0);
    // Clones at x = 0, 1, 2, 3 get weights 0, 0.25, 0.5, 0.75.
    let ramp = |p: Vec3| p.x / 4.0;
    m.apply(&mut set, &Context { seed: 0, time: 0.0, target: None, weight: &ramp });
    for i in 0..4 {
        assert!((pos(&set, i).y - pos(&before, i).y - i as f32).abs() < 1e-5, "clone {i}");
    }
}

#[test]
fn random_is_seeded_bounded_and_stable_when_the_count_changes() {
    let mut a = row(8);
    let mut b = row(8);
    let mut m = made("random");
    m.position = Vec3::splat(2.0);
    m.apply(&mut a, &ctx(0.0, None));
    m.apply(&mut b, &ctx(0.0, None));
    assert_eq!(a, b, "the same seed gives the same clones");
    let base = row(8);
    let mut moved = 0;
    for i in 0..8 {
        let d = pos(&a, i) - pos(&base, i);
        assert!(d.abs().max_element() <= 2.0 + 1e-5);
        moved += usize::from(d.length() > 1e-4);
    }
    assert_eq!(moved, 8);
    // A different modifier seed gives different clones.
    m.mode = Mode::Random { seed: 99, signed: true };
    let mut c = row(8);
    m.apply(&mut c, &ctx(0.0, None));
    assert_ne!(a, c);
    // Fewer clones keep the numbers the first ones had.
    m.mode = Mode::Random { seed: 1, signed: true };
    let mut long = row(10);
    let mut short = row(4);
    m.apply(&mut long, &ctx(0.0, None));
    m.apply(&mut short, &ctx(0.0, None));
    for i in 0..4 {
        assert!((pos(&long, i) - pos(&short, i)).length() < 1e-6, "clone {i} changed with the count");
    }
    // Unsigned never goes backwards.
    m.mode = Mode::Random { seed: 5, signed: false };
    let mut u = row(20);
    m.apply(&mut u, &ctx(0.0, None));
    let base = row(20);
    assert!((0..20).all(|i| (pos(&u, i) - pos(&base, i)).min_element() >= -1e-6));
}

#[test]
fn the_generators_seed_changes_the_random_numbers_too() {
    let m = made("random");
    let (mut a, mut b) = (row(6), row(6));
    m.apply(&mut a, &Context { seed: 1, time: 0.0, target: None, weight: &full });
    m.apply(&mut b, &Context { seed: 2, time: 0.0, target: None, weight: &full });
    assert_ne!(a, b);
}

#[test]
fn step_ramps_along_the_clone_order() {
    let mut set = row(5);
    let mut m = made("step");
    m.position = Vec3::new(0.0, 4.0, 0.0);
    m.apply(&mut set, &ctx(0.0, None));
    for i in 0..5 {
        assert!((pos(&set, i).y - i as f32).abs() < 1e-5, "clone {i}");
    }
    // Reversed: the first clone gets the most.
    let mut set = row(5);
    m.mode = Mode::Step { steps: 0, ease: Ease::Linear, reverse: true };
    m.apply(&mut set, &ctx(0.0, None));
    assert!((pos(&set, 0).y - 4.0).abs() < 1e-5 && pos(&set, 4).y.abs() < 1e-5);
    // Cut into 3 levels: 0, 0, 0.5, 1, 1 of the move for 5 clones.
    let mut set = row(6);
    m.mode = Mode::Step { steps: 3, ease: Ease::Linear, reverse: false };
    m.apply(&mut set, &ctx(0.0, None));
    let ys: Vec<f32> = (0..6).map(|i| (pos(&set, i).y / 4.0 * 2.0).round() / 2.0).collect();
    assert_eq!(ys, vec![0.0, 0.0, 0.5, 0.5, 1.0, 1.0]);
    // One clone gets the full amount.
    let mut one_clone = row(1);
    m.mode = Mode::Step { steps: 0, ease: Ease::Linear, reverse: false };
    m.apply(&mut one_clone, &ctx(0.0, None));
    assert!((pos(&one_clone, 0).y - 4.0).abs() < 1e-5);
}

#[test]
fn delay_is_time_based_and_staggered() {
    let mut m = made("delay");
    m.position = Vec3::new(0.0, 1.0, 0.0);
    m.mode = Mode::Delay { start: 1.0, stagger: 0.5, duration: 1.0, ease: Ease::Linear, reverse: false };
    assert!(m.is_time_dependent());
    let at = |time: f32| {
        let mut set = row(3);
        m.apply(&mut set, &ctx(time, None));
        (0..3).map(|i| pos(&set, i).y).collect::<Vec<f32>>()
    };
    assert_eq!(at(0.0), vec![0.0, 0.0, 0.0], "nothing before the start");
    assert_eq!(at(1.0), vec![0.0, 0.0, 0.0]);
    let mid = at(1.5);
    assert!((mid[0] - 0.5).abs() < 1e-5 && mid[1].abs() < 1e-5 && mid[2].abs() < 1e-5, "{mid:?}");
    let late = at(2.0);
    assert!((late[0] - 1.0).abs() < 1e-5 && (late[1] - 0.5).abs() < 1e-5 && late[2].abs() < 1e-5, "{late:?}");
    // Past the last clone's end everything has arrived.
    assert_eq!(at(3.0), vec![1.0, 1.0, 1.0]);
    assert_eq!(at(1000.0), vec![1.0, 1.0, 1.0]);
    // Reversed: the last clone starts first.
    m.mode = Mode::Delay { start: 0.0, stagger: 1.0, duration: 1.0, ease: Ease::Linear, reverse: true };
    let mut set = row(3);
    m.apply(&mut set, &ctx(0.5, None));
    assert!((pos(&set, 2).y - 0.5).abs() < 1e-5 && pos(&set, 0).y.abs() < 1e-5);
    // A zero duration is a switch.
    m.mode = Mode::Delay { start: 1.0, stagger: 0.0, duration: 0.0, ease: Ease::Linear, reverse: false };
    let mut set = row(2);
    m.apply(&mut set, &ctx(0.9, None));
    assert_eq!(pos(&set, 0).y, 0.0);
    m.apply(&mut set, &ctx(1.0, None));
    assert_eq!(pos(&set, 0).y, 1.0);
}

#[test]
fn noise_is_deterministic_bounded_smooth_and_drifts_with_time() {
    let mut m = made("noise");
    m.position = Vec3::new(0.0, 1.0, 0.0);
    m.mode = Mode::Noise { seed: 1, frequency: 0.5, octaves: 2, speed: Vec3::ZERO, signed: true };
    assert!(!m.is_time_dependent());
    let (mut a, mut b) = (row(30), row(30));
    m.apply(&mut a, &ctx(0.0, None));
    m.apply(&mut b, &ctx(50.0, None));
    assert_eq!(a, b, "no drift: time does not matter");
    let base = row(30);
    assert!((0..30).all(|i| (pos(&a, i).y - pos(&base, i).y).abs() <= 1.0 + 1e-5));
    assert!((0..30).any(|i| pos(&a, i).y.abs() > 0.01), "it is not flat");
    // Smooth: neighbours a small step apart differ by little.
    let fine = Cloner { layout: Layout::Linear { count: 200, offset: Vec3::new(0.01, 0.0, 0.0), rotation: Vec3::ZERO, scale: Vec3::ZERO }, seed: 0 };
    let mut set = fine.generate(None).expect("fine");
    m.apply(&mut set, &ctx(0.0, None));
    assert!(set.instances.windows(2).all(|w| (w[0].matrix.w_axis.y - w[1].matrix.w_axis.y).abs() < 0.05));
    // With a drift it moves with time.
    m.mode = Mode::Noise { seed: 1, frequency: 0.5, octaves: 2, speed: Vec3::new(1.0, 0.0, 0.0), signed: true };
    assert!(m.is_time_dependent());
    let (mut c, mut d) = (row(30), row(30));
    m.apply(&mut c, &ctx(0.0, None));
    m.apply(&mut d, &ctx(1.3, None));
    assert_ne!(c, d);
    // Unsigned noise only pushes one way.
    m.mode = Mode::Noise { seed: 1, frequency: 0.5, octaves: 1, speed: Vec3::ZERO, signed: false };
    let mut e = row(30);
    m.apply(&mut e, &ctx(0.0, None));
    assert!((0..30).all(|i| pos(&e, i).y - pos(&base, i).y >= -1e-6));
}

#[test]
fn target_pulls_and_aims_clones_and_fades_with_distance() {
    let goal = Vec3::new(0.0, 5.0, 0.0);
    let mut m = made("target");
    m.position = Vec3::ZERO;
    m.mode = Mode::Target { pull: 1.0, aim: false, aim_axis: Axis::Y, reach: 0.0 };
    let mut set = row(3);
    m.apply(&mut set, &ctx(0.0, Some(goal)));
    assert!((0..3).all(|i| (pos(&set, i) - goal).length() < 1e-5), "a full pull lands on the target");
    // Aim: the chosen axis ends up pointing at the target.
    m.mode = Mode::Target { pull: 0.0, aim: true, aim_axis: Axis::Y, reach: 0.0 };
    let mut set = row(3);
    m.apply(&mut set, &ctx(0.0, Some(goal)));
    for i in 0..3 {
        let (_, q, t) = set.instances[i].matrix.to_scale_rotation_translation();
        let want = (goal - t).normalize();
        assert!((q * Vec3::Y - want).length() < 1e-4, "clone {i}");
    }
    // Reach: clones farther than the reach stay put, nearer ones are pulled more.
    m.mode = Mode::Target { pull: 1.0, aim: false, aim_axis: Axis::Y, reach: 6.0 };
    let base = row(3);
    let mut set = row(3);
    m.apply(&mut set, &ctx(0.0, Some(Vec3::new(0.0, 0.0, 0.0))));
    assert!((pos(&set, 0) - Vec3::ZERO).length() < 1e-5);
    assert!(pos(&set, 1).x < pos(&base, 1).x, "pulled toward the target");
    assert!(pos(&set, 1).x > pos(&set, 0).x);
    let mut far = row(3);
    m.apply(&mut far, &ctx(0.0, Some(Vec3::new(100.0, 0.0, 0.0))));
    assert_eq!(far, base, "out of reach");
    // Without a target point nothing happens.
    let mut none = row(3);
    m.apply(&mut none, &ctx(0.0, None));
    assert_eq!(none, base);
    assert!(m.needs_target() && !made("plain").needs_target());
}

#[test]
fn tint_moves_the_colour_toward_the_tint() {
    let mut set = row(3);
    let mut m = made("step");
    m.position = Vec3::ZERO;
    m.tint = [1.0, 0.0, 0.0];
    m.tint_mix = 1.0;
    m.apply(&mut set, &ctx(0.0, None));
    assert_eq!(set.instances[2].color, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(set.instances[0].color, [1.0; 4], "factor 0 keeps the colour");
    assert!(set.instances[1].color[1] > 0.4 && set.instances[1].color[1] < 0.6);
}

#[test]
fn modifiers_stack_in_order() {
    let mut set = row(3);
    let before = set.clone();
    let mut a = made("plain");
    a.position = Vec3::new(0.0, 1.0, 0.0);
    let mut b = made("step");
    b.position = Vec3::new(0.0, 2.0, 0.0);
    a.apply(&mut set, &ctx(0.0, None));
    b.apply(&mut set, &ctx(0.0, None));
    for i in 0..3 {
        assert!((pos(&set, i).y - pos(&before, i).y - (1.0 + i as f32)).abs() < 1e-5);
    }
}

#[test]
fn hostile_numbers_give_finite_clones() {
    let nan = f32::NAN;
    let inf = f32::INFINITY;
    for k in MODES {
        let mut m = made(k);
        m.strength = nan;
        m.position = Vec3::new(inf, nan, 1e30);
        m.rotation = Vec3::new(nan, inf, -inf);
        m.scale = Vec3::new(1e30, nan, -1e30);
        m.tint = [nan, inf, -1.0];
        m.tint_mix = inf;
        m.mode = match m.mode {
            Mode::Noise { seed, .. } => Mode::Noise { seed, frequency: nan, octaves: u32::MAX, speed: Vec3::splat(inf), signed: true },
            Mode::Delay { .. } => Mode::Delay { start: nan, stagger: inf, duration: nan, ease: Ease::Smooth, reverse: true },
            Mode::Step { .. } => Mode::Step { steps: u32::MAX, ease: Ease::Out, reverse: false },
            Mode::Target { aim_axis, .. } => Mode::Target { pull: inf, aim: true, aim_axis, reach: nan },
            other => other,
        };
        let mut set = row(40);
        m.apply(&mut set, &Context { seed: u32::MAX, time: nan, target: Some(Vec3::new(nan, 0.0, 0.0)), weight: &full });
        assert!(finite(&set), "{k}");
        assert_eq!(set.len(), 40);
        let mut set = row(40);
        m.apply(&mut set, &Context { seed: 1, time: 1e30, target: Some(Vec3::splat(1e30)), weight: &|_| f32::NAN });
        assert!(finite(&set), "{k} with a NaN weight");
    }
}

#[test]
fn ease_curves_keep_their_ends() {
    for e in [Ease::Linear, Ease::Smooth, Ease::In, Ease::Out] {
        assert_eq!(e.apply(0.0), 0.0);
        assert_eq!(e.apply(1.0), 1.0);
        assert_eq!(e.apply(-5.0), 0.0);
        assert_eq!(e.apply(f32::NAN), 0.0);
        assert_eq!(Ease::from_name(e.name()), Some(e));
        let mut prev = 0.0;
        for k in 0..=20 {
            let v = e.apply(k as f32 / 20.0);
            assert!(v >= prev - 1e-6);
            prev = v;
        }
    }
    assert!(Ease::from_name("bounce").is_none());
}

fn any_f32() -> impl Strategy<Value = f32> {
    prop_oneof![-1.0e6f32..1.0e6, Just(f32::NAN), Just(f32::INFINITY), Just(f32::NEG_INFINITY), Just(0.0f32), Just(1.0f32)]
}

fn any_vec() -> impl Strategy<Value = Vec3> {
    (any_f32(), any_f32(), any_f32()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_modifier_keeps_the_clones_finite_and_in_order(
        kind in 0usize..6,
        strength in any_f32(),
        position in any_vec(),
        rotation in any_vec(),
        scale in any_vec(),
        mix in any_f32(),
        p1 in any_f32(), p2 in any_f32(), p3 in any_f32(),
        time in any_f32(),
        seed in any::<u32>(),
        count in 0u32..60,
    ) {
        let mut m = made(MODES[kind]);
        m.strength = strength; m.position = position; m.rotation = rotation; m.scale = scale; m.tint_mix = mix;
        m.mode = match m.mode {
            Mode::Noise { .. } => Mode::Noise { seed, frequency: p1, octaves: seed % 9, speed: Vec3::new(p2, p3, 0.0), signed: seed % 2 == 0 },
            Mode::Delay { .. } => Mode::Delay { start: p1, stagger: p2, duration: p3, ease: Ease::Smooth, reverse: seed % 2 == 0 },
            Mode::Step { .. } => Mode::Step { steps: seed % 12, ease: Ease::In, reverse: seed % 2 == 0 },
            Mode::Target { aim_axis, .. } => Mode::Target { pull: p1, aim: true, aim_axis, reach: p2 },
            Mode::Random { .. } => Mode::Random { seed, signed: seed % 2 == 0 },
            Mode::Plain => Mode::Plain,
        };
        let mut set = row(count);
        let expected = set.len();
        let wt = |p: Vec3| p.x * 0.3;
        m.apply(&mut set, &Context { seed, time, target: Some(Vec3::new(p1, p2, p3)), weight: &wt });
        prop_assert!(finite(&set));
        prop_assert_eq!(set.len(), expected);
        for (i, inst) in set.instances.iter().enumerate() {
            prop_assert_eq!(inst.index, i as u32);
        }
        // Sanitizing is idempotent and the result round-trips through JSON.
        let s = m.sanitized();
        prop_assert_eq!(s.sanitized(), s);
        let json = serde_json::to_string(&s).expect("json");
        let back: Modifier = serde_json::from_str(&json).expect("back");
        prop_assert_eq!(back, s);
    }

    #[test]
    fn a_plain_modifier_with_no_strength_or_no_weight_is_the_identity(
        strength_zero in any::<bool>(), count in 0u32..40, position in any_vec(),
    ) {
        let before = row(count);
        let mut set = before.clone();
        let mut m = made("plain");
        m.position = position;
        let zero = |_: Vec3| 0.0;
        if strength_zero {
            m.strength = 0.0;
            m.apply(&mut set, &ctx(0.0, None));
        } else {
            m.apply(&mut set, &Context { seed: 0, time: 0.0, target: None, weight: &zero });
        }
        prop_assert_eq!(set, before);
    }

    #[test]
    fn step_factors_never_decrease_along_the_order(count in 2u32..50, steps in 0u32..9) {
        let mut set = row(count);
        let mut m = made("step");
        m.mode = Mode::Step { steps, ease: Ease::Smooth, reverse: false };
        m.position = Vec3::new(0.0, 1.0, 0.0);
        m.apply(&mut set, &ctx(0.0, None));
        for w in set.instances.windows(2) {
            prop_assert!(w[1].matrix.w_axis.y >= w[0].matrix.w_axis.y - 1e-6);
        }
        prop_assert!(set.instances.iter().all(|i| (-1e-6..=1.0 + 1e-6).contains(&i.matrix.w_axis.y)));
    }

    #[test]
    fn delay_never_goes_backwards_in_time(count in 1u32..20, t0 in 0.0f32..10.0, dt in 0.0f32..5.0) {
        let mut m = made("delay");
        m.position = Vec3::new(0.0, 1.0, 0.0);
        let run = |time: f32| { let mut s = row(count); m.apply(&mut s, &ctx(time, None)); s };
        let (a, b) = (run(t0), run(t0 + dt));
        for i in 0..count as usize {
            prop_assert!(b.instances[i].matrix.w_axis.y >= a.instances[i].matrix.w_axis.y - 1e-5);
        }
    }
}
