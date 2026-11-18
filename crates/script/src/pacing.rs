//! Pacing of event callbacks while the timeline plays.
//!
//! During playback the scene changes every drawn frame (the pose moves) and the frame number
//! changes with it, so a callback per event would run dozens of times a second on the UI thread.
//! The [`EventPacer`] coalesces those two events instead: at most one batch is delivered per
//! [`PlaybackPolicy::min_interval`], carrying the newest frame; the batch is not delivered again
//! until the time the last batch cost, divided by [`PlaybackPolicy::duty`], has passed (so
//! callbacks never take more than that share of the time); and when playback stops, whatever was
//! held back is delivered once. The pacer is a plain value driven by a clock the caller passes
//! in, so it is testable without sleeping.

use std::time::Duration;

use crate::api::events::Event;

/// What callbacks may spend while the timeline plays.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackPolicy {
    /// Shortest time between two batches of frame and scene events.
    pub min_interval: Duration,
    /// Time all callbacks of one batch may use together; a callback that has not started when it
    /// is spent is skipped, and the callback running when it ends is stopped.
    pub frame_budget: Duration,
    /// Limit of one callback during playback (never more than the usual event limit).
    pub event_time: Duration,
    /// The share of the time callbacks may use, in `(0, 1]`: after a batch that cost `c`, the next
    /// one waits at least `c / duty`.
    pub duty: f64,
}

impl Default for PlaybackPolicy {
    fn default() -> Self {
        PlaybackPolicy { min_interval: Duration::from_millis(50), frame_budget: Duration::from_millis(8), event_time: Duration::from_millis(25), duty: 0.25 }
    }
}

impl PlaybackPolicy {
    /// The policy with every field brought into its valid range.
    pub fn clamped(&self) -> PlaybackPolicy {
        PlaybackPolicy {
            min_interval: self.min_interval.min(Duration::from_secs(5)),
            frame_budget: self.frame_budget.max(Duration::from_millis(1)).min(Duration::from_secs(1)),
            event_time: self.event_time.max(Duration::from_millis(1)).min(Duration::from_secs(2)),
            duty: if self.duty.is_finite() { self.duty.clamp(0.01, 1.0) } else { 0.25 },
        }
    }
}

/// Coalesces `frame_changed` and `scene_changed` events during playback.
#[derive(Clone, Debug)]
pub struct EventPacer {
    policy: PlaybackPolicy,
    /// The earliest time the next batch may be delivered.
    not_before: Duration,
    /// The newest frame not yet delivered.
    frame: Option<f64>,
    scene: bool,
    /// Frame changes folded into the pending one (reported to the callback as `skipped`).
    skipped: u64,
    /// Batches delivered and events held back, for the status line and tests.
    pub delivered: u64,
    pub coalesced: u64,
}

impl EventPacer {
    pub fn new(policy: PlaybackPolicy) -> Self {
        EventPacer { policy: policy.clamped(), not_before: Duration::ZERO, frame: None, scene: false, skipped: 0, delivered: 0, coalesced: 0 }
    }

    pub fn policy(&self) -> &PlaybackPolicy {
        &self.policy
    }

    pub fn set_policy(&mut self, policy: PlaybackPolicy) {
        self.policy = policy.clamped();
    }

    /// Whether events are held back right now.
    pub fn pending(&self) -> bool {
        self.frame.is_some() || self.scene
    }

    /// Report what changed in this drawn frame while the timeline plays, at time `now` (any
    /// monotonic clock). Returns the events to deliver now: the newest frame and a scene change
    /// when one happened since the last batch, or nothing while the batch is not due.
    pub fn offer(&mut self, now: Duration, frame: Option<f64>, scene_changed: bool) -> Vec<Event> {
        if let Some(f) = frame {
            if self.frame.is_some() {
                self.skipped += 1;
                self.coalesced += 1;
            }
            self.frame = Some(f);
        }
        if scene_changed {
            if self.scene {
                self.coalesced += 1;
            }
            self.scene = true;
        }
        if !self.pending() || now < self.not_before {
            return Vec::new();
        }
        self.take(now)
    }

    /// Playback stopped: deliver what was held back, once, whatever the clock says.
    pub fn flush(&mut self) -> Vec<Event> {
        let events = self.drain();
        self.not_before = Duration::ZERO;
        events
    }

    /// Tell the pacer what the last batch cost, so the next one keeps callbacks within the duty.
    pub fn record_cost(&mut self, now: Duration, cost: Duration) {
        let wait = Duration::from_secs_f64((cost.as_secs_f64() / self.policy.duty).min(30.0));
        self.not_before = now.saturating_add(wait.max(self.policy.min_interval));
    }

    fn take(&mut self, now: Duration) -> Vec<Event> {
        self.not_before = now.saturating_add(self.policy.min_interval);
        let events = self.drain();
        if !events.is_empty() {
            self.delivered += 1;
        }
        events
    }

    fn drain(&mut self) -> Vec<Event> {
        // The scene first, then the frame, as when no playback holds anything back.
        let mut events = Vec::new();
        if std::mem::take(&mut self.scene) {
            events.push(Event::SceneChanged);
        }
        if let Some(frame) = self.frame.take() {
            events.push(Event::FrameChanged { frame, skipped: std::mem::take(&mut self.skipped) });
        }
        events
    }
}
