//! Event callbacks while the timeline plays: coalescing, the duty limit, the shared time budget.

use std::time::{Duration, Instant};

use modelcraft_engine::Document;
use modelcraft_script::{ErrorKind, Event, EventKind, EventPacer, Limits, Permissions, PlaybackPolicy, RunContext, Session};
use proptest::prelude::*;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn frames(events: &[Event]) -> Vec<(f64, u64)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::FrameChanged { frame, skipped } => Some((*frame, *skipped)),
            _ => None,
        })
        .collect()
}

#[test]
fn playback_events_are_coalesced_to_the_interval_and_carry_the_newest_frame() {
    let mut p = EventPacer::new(PlaybackPolicy { min_interval: ms(50), ..PlaybackPolicy::default() });
    let mut got = Vec::new();
    // 60 drawn frames at 16 ms: 0.96 s of playback.
    for i in 0..60u64 {
        let ev = p.offer(ms(i * 16), Some(i as f64), true);
        got.extend(ev);
    }
    let f = frames(&got);
    assert!(f.len() <= 20 && f.len() >= 15, "about one batch per 50 ms, got {}", f.len());
    assert_eq!(got.iter().filter(|e| **e == Event::SceneChanged).count(), f.len(), "a scene change travels with each batch");
    // The first one is delivered at once; the rest carry the newest frame and count what they folded.
    assert_eq!(f[0], (0.0, 0));
    assert!(f.windows(2).all(|w| w[1].0 > w[0].0), "frames only move forward: {f:?}");
    // Every drawn frame is delivered or folded into one that was, except those still held back.
    let folded: u64 = f.iter().map(|x| x.1).sum();
    let held = frames(&p.flush()).first().map_or(0, |x| 1 + x.1);
    assert_eq!(folded + f.len() as u64 + held, 60);
    assert!(p.coalesced > 0);
}

#[test]
fn a_costly_batch_delays_the_next_one_by_cost_over_duty() {
    let mut p = EventPacer::new(PlaybackPolicy { min_interval: ms(10), duty: 0.25, ..PlaybackPolicy::default() });
    assert_eq!(p.offer(ms(0), Some(1.0), false).len(), 1);
    // The callbacks took 20 ms: at 25 % duty the next batch waits 80 ms.
    p.record_cost(ms(0), ms(20));
    assert!(p.offer(ms(40), Some(2.0), false).is_empty());
    assert!(p.offer(ms(79), Some(3.0), false).is_empty());
    let ev = p.offer(ms(80), Some(4.0), false);
    assert_eq!(frames(&ev), vec![(4.0, 2)], "the newest frame, and the two folded ones counted");
    // A free batch only waits the minimum interval.
    p.record_cost(ms(80), Duration::ZERO);
    assert!(p.offer(ms(85), Some(5.0), false).is_empty());
    assert_eq!(p.offer(ms(90), Some(6.0), false).len(), 1);
}

#[test]
fn stopping_delivers_what_was_held_back_once() {
    let mut p = EventPacer::new(PlaybackPolicy { min_interval: ms(1000), ..PlaybackPolicy::default() });
    assert_eq!(p.offer(ms(0), Some(1.0), true).len(), 2);
    assert!(p.offer(ms(16), Some(2.0), true).is_empty());
    assert!(p.offer(ms(32), Some(3.0), false).is_empty());
    assert!(p.pending());
    let left = p.flush();
    assert_eq!(frames(&left), vec![(3.0, 1)]);
    assert!(left.contains(&Event::SceneChanged));
    assert!(!p.pending());
    assert!(p.flush().is_empty(), "nothing is delivered twice");
    // The next playback starts fresh: its first frame is delivered at once.
    assert_eq!(p.offer(ms(40), Some(10.0), false).len(), 1);
}

#[test]
fn a_policy_is_brought_into_range() {
    let wild = PlaybackPolicy { min_interval: Duration::MAX, frame_budget: Duration::ZERO, event_time: Duration::MAX, duty: f64::NAN };
    let c = wild.clamped();
    assert!(c.min_interval <= Duration::from_secs(5) && c.frame_budget >= ms(1) && c.event_time <= Duration::from_secs(2));
    assert_eq!(c.duty, 0.25);
    assert_eq!(PlaybackPolicy { duty: 0.0, ..PlaybackPolicy::default() }.clamped().duty, 0.01);
    assert_eq!(PlaybackPolicy { duty: 9.0, ..PlaybackPolicy::default() }.clamped().duty, 1.0);
    // A cost that would overflow a duration is bounded too.
    let mut p = EventPacer::new(wild);
    p.record_cost(ms(0), Duration::MAX);
    assert!(p.offer(ms(1), Some(1.0), false).is_empty());
}

proptest! {
    /// However the frames come, nothing is lost: the frames delivered plus the ones folded add up
    /// to the frames offered, the last frame offered is delivered by the end, and time never
    /// delivers two batches closer than the interval.
    #[test]
    fn nothing_is_lost_and_batches_keep_their_distance(steps in proptest::collection::vec((1u64..40, any::<bool>(), 0u64..30), 1..200)) {
        let mut p = EventPacer::new(PlaybackPolicy { min_interval: ms(50), ..PlaybackPolicy::default() });
        let mut now = 0u64;
        let mut offered = 0u64;
        let mut last_offered = 0.0;
        let mut delivered = 0u64;
        let mut last_at: Option<u64> = None;
        let mut last_frame = f64::NEG_INFINITY;
        for (i, (dt, scene, cost)) in steps.iter().enumerate() {
            now += dt;
            offered += 1;
            last_offered = i as f64;
            let ev = p.offer(ms(now), Some(last_offered), *scene);
            for (frame, skipped) in frames(&ev) {
                delivered += 1 + skipped;
                prop_assert!(frame > last_frame);
                last_frame = frame;
                if let Some(t) = last_at { prop_assert!(now - t >= 50, "{} after {}", now, t); }
                last_at = Some(now);
                p.record_cost(ms(now), ms(*cost));
            }
        }
        for (frame, skipped) in frames(&p.flush()) {
            delivered += 1 + skipped;
            last_frame = frame;
        }
        prop_assert_eq!(delivered, offered);
        prop_assert_eq!(last_frame, last_offered);
    }
}

fn session(src: &str) -> (Session, Document, RunContext) {
    let mut s = Session::new(Limits::default(), Permissions::none());
    let mut d = Document::new();
    let mut ctx = RunContext::default();
    let r = s.run(&mut d, &mut ctx, "w.rhai", src);
    assert!(r.ok(), "{:?}", r.error);
    (s, d, ctx)
}

#[test]
fn a_callback_sees_how_many_frames_were_folded_into_its_event() {
    let (mut s, mut d, mut ctx) = session("on_frame_changed(|e| { print(`frame ${e.frame} skipped ${e.skipped}`); });");
    let r = s.fire(&mut d, &mut ctx, &Event::FrameChanged { frame: 12.0, skipped: 3 });
    assert_eq!(r[0].lines[0].text, "frame 12.0 skipped 3");
}

#[test]
fn the_budget_is_shared_and_a_callback_cut_by_it_is_skipped_not_blamed() {
    // The operation limit is out of the way, so only the clock can stop the endless callback.
    let mut s = Session::new(Limits { max_operations: u64::MAX, ..Limits::default() }, Permissions::none());
    let (mut d, mut ctx) = (Document::new(), RunContext::default());
    let r = s.run(
        &mut d,
        &mut ctx,
        "w.rhai",
        "on_frame_changed(|e| { print(\"quick\"); }); on_frame_changed(|e| { loop { } }); on_frame_changed(|e| { print(\"never\"); });",
    );
    assert!(r.ok());
    let ev = Event::FrameChanged { frame: 1.0, skipped: 0 };
    let t = Instant::now();
    let fired = s.fire_within(&mut d, &mut ctx, &ev, ms(1500), ms(300));
    assert!(t.elapsed() < ms(1400), "the budget, not the 1.5 s limit, ended it: {:?}", t.elapsed());
    assert_eq!(fired.reports.len(), 2, "the third callback never started");
    assert_eq!(fired.reports[0].lines[0].text, "quick");
    assert!(fired.reports[1].error.is_none(), "cut by the budget is not an error: {:?}", fired.reports[1].error);
    assert_eq!(fired.skipped, 2);
    // Run out of budget many times: nobody is switched off for it.
    for _ in 0..6 {
        let _ = s.fire_within(&mut d, &mut ctx, &ev, ms(1500), ms(15));
    }
    assert_eq!(s.callback_count(EventKind::FrameChanged), 3);
}

#[test]
fn a_callback_that_overruns_its_own_limit_still_counts_as_failing() {
    let (mut s, mut d, mut ctx) = session("on_frame_changed(|e| { loop { } });");
    let ev = Event::FrameChanged { frame: 1.0, skipped: 0 };
    for i in 1..=3 {
        let fired = s.fire_within(&mut d, &mut ctx, &ev, ms(20), ms(500));
        assert_eq!(fired.reports[0].error.as_ref().map(|e| e.kind), Some(ErrorKind::Limit), "call {i}");
    }
    assert_eq!(s.callback_count(EventKind::FrameChanged), 0, "switched off after three");
    assert!(s.fire_within(&mut d, &mut ctx, &ev, ms(20), ms(500)).reports.is_empty());
}

#[test]
fn the_plain_fire_has_no_budget() {
    let (mut s, mut d, mut ctx) = session("on_scene_changed(|e| { print(\"a\"); }); on_scene_changed(|e| { print(\"b\"); });");
    let r = s.fire(&mut d, &mut ctx, &Event::SceneChanged);
    assert_eq!(r.len(), 2);
}
