use super::*;
use crate::Rng;
use proptest::prelude::{prop_assert, prop_assert_eq, proptest};

/// Points dropped from seeded heights that bounce on the ground plane y = 0.
struct Drops {
    n: usize,
    pos: Vec<Vec3>,
    vel: Vec<Vec3>,
    begins: Vec<f64>,
    steps_seen: u64,
}

impl Drops {
    fn new(n: usize) -> Drops {
        Drops { n, pos: Vec::new(), vel: Vec::new(), begins: Vec::new(), steps_seen: 0 }
    }
}

impl Solver for Drops {
    fn point_count(&self) -> usize {
        self.n
    }
    fn reset(&mut self, seed: u64) {
        let mut r = Rng::new(seed);
        self.pos = (0..self.n).map(|_| Vec3::new(r.range(-1.0, 1.0), r.range(1.0, 3.0), r.range(-1.0, 1.0))).collect();
        self.vel = vec![Vec3::ZERO; self.n];
        self.begins.clear();
        self.steps_seen = 0;
    }
    fn begin_frame(&mut self, frame: f64) {
        self.begins.push(frame);
    }
    fn step(&mut self, ctx: &StepContext) {
        self.steps_seen += 1;
        for (p, v) in self.pos.iter_mut().zip(&mut self.vel) {
            v.y -= 9.81 * ctx.dt;
            *p += *v * ctx.dt;
            if p.y < 0.0 {
                p.y = -p.y * 0.5;
                v.y = -v.y * 0.5;
            }
        }
    }
    fn write_points(&self, out: &mut Vec<Vec3>) {
        out.clear();
        out.extend_from_slice(&self.pos);
    }
}

fn config(start: f64, end: f64, substeps: u32) -> BakeConfig {
    BakeConfig { timing: SimTiming { start_frame: start, end_frame: end, fps: 24.0, substeps }, seed: 5, max_cache_bytes: DEFAULT_CACHE_BYTES }
}

#[test]
fn frame_zero_is_the_rest_state_and_every_later_frame_takes_substeps() {
    let mut s = Drops::new(3);
    let cfg = config(10.0, 14.0, 4);
    let cache = bake(&mut s, &cfg, &mut Silent).unwrap();
    assert_eq!((cache.frames(), cache.points(), cache.start, cache.fps), (5, 3, 10.0, 24.0));
    assert_eq!(s.steps_seen, 16, "4 frames after the first, 4 substeps each");
    assert_eq!(s.begins, vec![11.0, 12.0, 13.0, 14.0], "begin_frame runs for every frame but the start one");
    // The start frame is the seeded rest state, untouched by any step.
    let mut again = Drops::new(3);
    again.reset(5);
    assert_eq!(cache.frame(0).unwrap(), &again.pos[..]);
    assert!(cache.frame(1).unwrap().iter().zip(cache.frame(0).unwrap()).all(|(a, b)| a.y < b.y), "the points fall");
}

#[test]
fn the_same_seed_gives_the_same_cache_and_another_seed_does_not() {
    let a = bake(&mut Drops::new(50), &config(1.0, 40.0, 3), &mut Silent).unwrap();
    let b = bake(&mut Drops::new(50), &config(1.0, 40.0, 3), &mut Silent).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.checksum(), b.checksum());
    let mut cfg = config(1.0, 40.0, 3);
    cfg.seed = 6;
    assert_ne!(bake(&mut Drops::new(50), &cfg, &mut Silent).unwrap().checksum(), a.checksum());
    // Re-running the same solver object resets it: nothing carries over from the last run.
    let mut s = Drops::new(50);
    let first = bake(&mut s, &config(1.0, 40.0, 3), &mut Silent).unwrap();
    assert_eq!(bake(&mut s, &config(1.0, 40.0, 3), &mut Silent).unwrap(), first);
}

#[test]
fn more_substeps_change_the_step_not_the_frame_rate() {
    let mut s = Drops::new(1);
    let cache = bake(&mut s, &config(0.0, 24.0, 8), &mut Silent).unwrap();
    assert_eq!(cache.frames(), 25);
    assert_eq!(s.steps_seen, 24 * 8);
    // A point dropped from rest falls ½·g·t² in the first second, whatever the substeps.
    let mut one = Drops::new(1);
    let coarse = bake(&mut one, &config(0.0, 12.0, 1), &mut Silent).unwrap();
    let mut many = Drops::new(1);
    let fine = bake(&mut many, &config(0.0, 12.0, 16), &mut Silent).unwrap();
    let y0 = coarse.frame(0).unwrap()[0].y;
    let drop = |c: &FrameCache| y0 - c.frame(12).unwrap()[0].y;
    assert!((drop(&fine) - 0.5 * 9.81 * 0.25).abs() < 0.05, "half a second of free fall: {}", drop(&fine));
    assert!((drop(&coarse) - drop(&fine)).abs() < 0.2);
}

#[test]
fn cancelling_stops_between_frames() {
    struct StopAt(usize, bool);
    impl Observer for StopAt {
        fn progress(&mut self, p: Progress) {
            self.1 = p.done >= self.0;
        }
        fn cancelled(&self) -> bool {
            self.1
        }
    }
    let r = bake(&mut Drops::new(4), &config(1.0, 100.0, 2), &mut StopAt(7, false));
    assert_eq!(r, Err(SimError::Cancelled { done: 7 }));
    let flag = CancelFlag::new();
    flag.cancel();
    assert_eq!(bake(&mut Drops::new(4), &config(1.0, 100.0, 2), &mut flag.clone()), Err(SimError::Cancelled { done: 0 }));
}

#[test]
fn progress_counts_frames_and_steps() {
    struct Log(Vec<Progress>);
    impl Observer for Log {
        fn progress(&mut self, p: Progress) {
            self.0.push(p);
        }
    }
    let mut log = Log(Vec::new());
    bake(&mut Drops::new(2), &config(1.0, 3.0, 5), &mut log).unwrap();
    assert_eq!(log.0.iter().map(|p| (p.done, p.steps)).collect::<Vec<_>>(), vec![(1, 0), (2, 5), (3, 10)]);
    assert!(log.0.iter().all(|p| p.total == 3));
    assert_eq!(log.0.last().map(|p| p.fraction()), Some(1.0));
}

#[test]
fn memory_and_step_counts_are_bounded() {
    // 1000 points × 100 frames × 12 bytes = 1.2 MB does not fit a 1 MB budget: refused before running.
    let mut cfg = config(1.0, 100.0, 2);
    cfg.max_cache_bytes = 1 << 20;
    let mut s = Drops::new(1000);
    assert!(matches!(bake(&mut s, &cfg, &mut Silent), Err(SimError::CacheFull { .. })));
    assert_eq!(s.steps_seen, 0, "no step ran");
    // The point count is bounded.
    assert!(matches!(bake(&mut Drops::new(MAX_POINTS + 1), &config(1.0, 2.0, 1), &mut Silent), Err(SimError::Invalid(_))));
    // Timings: too many frames, too many substeps, too many steps, nonsense numbers.
    for t in [
        SimTiming { end_frame: 1.0e9, ..SimTiming::default() },
        SimTiming { substeps: 0, ..SimTiming::default() },
        SimTiming { substeps: MAX_SUBSTEPS + 1, ..SimTiming::default() },
        SimTiming { start_frame: f64::NAN, ..SimTiming::default() },
        SimTiming { fps: 0.0, ..SimTiming::default() },
        SimTiming { start_frame: 5.0, end_frame: 1.0, ..SimTiming::default() },
        SimTiming { start_frame: 0.0, end_frame: 99_990.0, substeps: 64, fps: 24.0 },
    ] {
        assert!(t.validate().is_err(), "{t:?}");
        assert!(t.sanitized().validate().is_ok(), "{t:?} sanitizes to {:?}", t.sanitized());
    }
}

#[test]
fn a_solver_that_diverges_stops_the_run() {
    struct Blow(f32);
    impl Solver for Blow {
        fn point_count(&self) -> usize {
            1
        }
        fn reset(&mut self, _: u64) {
            self.0 = 1.0;
        }
        fn step(&mut self, _: &StepContext) {
            self.0 *= 10.0;
        }
        fn write_points(&self, out: &mut Vec<Vec3>) {
            out.clear();
            out.push(Vec3::splat(if self.0 > 1e5 { f32::NAN } else { self.0 }));
        }
    }
    let r = bake(&mut Blow(1.0), &config(1.0, 50.0, 1), &mut Silent);
    assert_eq!(r, Err(SimError::Diverged { frame: 7.0 }));
}

#[test]
fn dead_points_are_allowed_only_when_the_solver_says_so() {
    struct Pool(bool);
    impl Solver for Pool {
        fn point_count(&self) -> usize {
            2
        }
        fn reset(&mut self, _: u64) {}
        fn step(&mut self, _: &StepContext) {}
        fn write_points(&self, out: &mut Vec<Vec3>) {
            out.clear();
            out.extend([Vec3::ONE, DEAD]);
        }
        fn allows_dead(&self) -> bool {
            self.0
        }
    }
    assert!(matches!(bake(&mut Pool(false), &config(1.0, 2.0, 1), &mut Silent), Err(SimError::Diverged { .. })));
    let c = bake(&mut Pool(true), &config(1.0, 2.0, 1), &mut Silent).unwrap();
    assert!(!is_live(c.frame(1).unwrap()[1]));
}

#[test]
fn a_solver_that_writes_the_wrong_count_is_an_error_not_a_panic() {
    struct Wrong;
    impl Solver for Wrong {
        fn point_count(&self) -> usize {
            3
        }
        fn reset(&mut self, _: u64) {}
        fn step(&mut self, _: &StepContext) {}
        fn write_points(&self, out: &mut Vec<Vec3>) {
            out.clear();
        }
    }
    assert!(matches!(bake(&mut Wrong, &config(1.0, 2.0, 1), &mut Silent), Err(SimError::Invalid(_))));
}

#[test]
fn playback_samples_the_cache_by_scene_frame() {
    let cache = bake(&mut Drops::new(2), &config(10.0, 20.0, 4), &mut Silent).unwrap();
    let mut out = Vec::new();
    for f in 10..=20 {
        assert!(cache.sample(f64::from(f), &mut out));
        assert_eq!(&out[..], cache.frame((f - 10) as usize).unwrap());
    }
    cache.sample(10.5, &mut out);
    let (a, b) = (cache.frame(0).unwrap()[0], cache.frame(1).unwrap()[0]);
    assert!((out[0] - (a + b) * 0.5).length() < 1e-6);
}

#[test]
fn step_context_alpha_ends_at_one() {
    let c = |s| StepContext { dt: 0.01, substep: s, substeps: 4, frame: 1.0 };
    assert_eq!((c(0).alpha(), c(3).alpha()), (0.25, 1.0));
}

proptest! {
    #[test]
    fn any_timing_either_validates_or_sanitizes_into_one_that_does(
        start in -1e6f64..1e6, end in -1e6f64..1e6, fps in -10.0f64..2000.0, substeps in 0u32..200,
    ) {
        let t = SimTiming { start_frame: start, end_frame: end, fps, substeps };
        let s = t.sanitized();
        prop_assert!(s.validate().is_ok(), "{t:?} -> {s:?}");
        if t.validate().is_ok() {
            prop_assert_eq!(t.frame_count(), s.frame_count());
        }
    }

    #[test]
    fn random_runs_stay_within_budget_and_sample_without_panicking(
        n in 0usize..40, frames in 1u32..30, substeps in 1u32..6, limit in 0usize..20_000, at in -50.0f64..80.0,
    ) {
        let mut cfg = config(1.0, f64::from(frames), substeps);
        cfg.max_cache_bytes = limit;
        match bake(&mut Drops::new(n), &cfg, &mut Silent) {
            Ok(cache) => {
                prop_assert!(cache.bytes() <= limit);
                let mut out = Vec::new();
                let ok = cache.sample(at, &mut out);
                prop_assert_eq!(ok, !cache.is_empty());
                prop_assert!(out.is_empty() || out.len() == n);
            }
            Err(SimError::CacheFull { .. }) => {}
            Err(e) => prop_assert!(false, "{e}"),
        }
    }

    #[test]
    fn cancelling_at_any_frame_reports_how_far_it_got(stop in 0usize..30) {
        struct StopAt(usize, usize);
        impl Observer for StopAt {
            fn progress(&mut self, p: Progress) { self.1 = p.done; }
            fn cancelled(&self) -> bool { self.1 >= self.0 }
        }
        let r = bake(&mut Drops::new(3), &config(1.0, 60.0, 1), &mut StopAt(stop, 0));
        prop_assert_eq!(r, Err(SimError::Cancelled { done: stop }));
    }
}
