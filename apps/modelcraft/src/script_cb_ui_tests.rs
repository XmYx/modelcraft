//! Script event callbacks while the timeline plays (egui_kittest): coalesced frame and scene
//! events, a time budget for slow callbacks, and what is held back arriving when playback stops.

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use modelcraft_script::PlaybackPolicy;

use crate::app::ModelApp;
use crate::args::Cli;
use crate::script_ui::ConsoleKind;

fn harness() -> Harness<'static, ModelApp> {
    let ctx = egui::Context::default();
    let app = ModelApp::with_context(&ctx, Cli::default());
    Harness::builder().with_size(egui::vec2(1500.0, 950.0)).build_ui_state(|ui, app: &mut ModelApp| app.draw(ui), app)
}

fn install(h: &mut Harness<'static, ModelApp>, src: &str) {
    h.run();
    assert!(crate::script_ui::execute(h.state_mut(), "cb.rhai", src, None), "{:?}", h.state().script.console);
    h.run();
}

fn prints(h: &Harness<'static, ModelApp>, starting: &str) -> Vec<String> {
    h.state().script.console.iter().filter(|l| l.kind == ConsoleKind::Print && l.text.starts_with(starting)).map(|l| l.text.clone()).collect()
}

fn play(h: &mut Harness<'static, ModelApp>, frames: usize) {
    h.state_mut().anim.playback.whole_frames = false;
    h.state_mut().anim.playback.play();
    for _ in 0..frames {
        std::thread::sleep(Duration::from_millis(3));
        h.step();
    }
}

fn stop(h: &mut Harness<'static, ModelApp>) {
    h.state_mut().anim.playback.stop();
    h.step();
    h.step();
}

#[test]
fn frame_callbacks_are_merged_while_playing_and_the_last_frame_arrives_when_it_stops() {
    let mut h = harness();
    install(&mut h, "on_frame_changed(|e| { print(`frame ${e.frame} skipped ${e.skipped}`); });");
    // One batch every ten seconds: only the first frame gets through while playing.
    h.state_mut().script.pacer.set_policy(PlaybackPolicy { min_interval: Duration::from_secs(10), ..PlaybackPolicy::default() });
    play(&mut h, 25);
    let during = prints(&h, "frame");
    assert_eq!(during.len(), 1, "{during:?}");
    assert!(h.state().script.pacer.pending(), "the rest is held back, not lost");
    stop(&mut h);
    let all = prints(&h, "frame");
    assert_eq!(all.len(), 2, "the held-back frames arrive once when playback stops: {all:?}");
    let last = h.state().doc.scene.anim.frame;
    assert!(all[1].starts_with(&format!("frame {last:?} ")), "{} vs frame {last}", all[1]);
    assert!(!all[1].ends_with("skipped 0"), "it says it folded frames: {}", all[1]);
    // Nothing is replayed afterwards.
    h.run();
    h.run();
    assert_eq!(prints(&h, "frame").len(), 2);
    assert!(!h.state().script.pacer.pending());
}

#[test]
fn scene_callbacks_are_merged_while_playing_too() {
    let mut h = harness();
    h.run();
    h.state_mut().doc.execute(modelcraft_engine::Command::AddPrimitive { kind: "cube".into(), name: Some("A".into()), parent: None }).unwrap();
    h.run();
    install(&mut h, "on_scene_changed(|e| { print(\"scene\"); });");
    h.state_mut().script.pacer.set_policy(PlaybackPolicy { min_interval: Duration::from_secs(10), ..PlaybackPolicy::default() });
    h.state_mut().anim.playback.whole_frames = false;
    h.state_mut().anim.playback.play();
    for i in 0..20 {
        // The scene changes in every drawn frame, as a posed scene does.
        h.state_mut().doc.execute(modelcraft_engine::Command::AddGroup { name: Some(format!("g{i}")) }).unwrap();
        std::thread::sleep(Duration::from_millis(2));
        h.step();
    }
    assert_eq!(prints(&h, "scene").len(), 1, "one batch while playing");
    stop(&mut h);
    assert_eq!(prints(&h, "scene").len(), 2, "and one more when it stops");
}

#[test]
fn a_slow_callback_cannot_stall_playback() {
    let mut h = harness();
    h.run();
    // Only the clock can stop the endless callback here, not the operation limit.
    h.state_mut().script.session = Some(modelcraft_script::Session::new(
        modelcraft_script::Limits { max_operations: u64::MAX, max_event_time: Duration::from_millis(10), ..Default::default() },
        modelcraft_script::Permissions::none(),
    ));
    h.state_mut().script.pacer.set_policy(PlaybackPolicy {
        min_interval: Duration::ZERO,
        frame_budget: Duration::from_millis(10),
        event_time: Duration::from_millis(10),
        duty: 1.0,
    });
    install(&mut h, "on_frame_changed(|e| { loop { } });");
    let t = Instant::now();
    play(&mut h, 60);
    let took = t.elapsed();
    stop(&mut h);
    let errors = h.state().script.console.iter().filter(|l| l.kind == ConsoleKind::Error).count();
    assert_eq!(errors, 3, "stopped by its time limit, and switched off after three: {:?}", h.state().script.console);
    assert!(h.state().script.console.iter().any(|l| l.text.contains("switched off")));
    assert!(took < Duration::from_millis(1500), "60 playing frames took {took:?} (three stopped callbacks of 10 ms each)");
}

#[test]
fn without_playback_every_change_is_reported_as_before() {
    let mut h = harness();
    install(&mut h, "on_frame_changed(|e| { print(`frame ${e.frame} skipped ${e.skipped}`); });");
    for f in [3.0, 4.0, 5.0] {
        h.state_mut().doc.set_frame(f);
        h.step();
    }
    assert_eq!(prints(&h, "frame"), vec!["frame 3.0 skipped 0", "frame 4.0 skipped 0", "frame 5.0 skipped 0"]);
}
