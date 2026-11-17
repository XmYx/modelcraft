//! Scene time: the frame rate, the playback range and playback itself.

use serde::{Deserialize, Serialize};

pub const MIN_FPS: f64 = 1.0;
pub const MAX_FPS: f64 = 1000.0;
/// Frames stay within ± this bound (about 11 hours at 24 fps), so frame arithmetic stays exact.
pub const MAX_FRAME: f64 = 1_000_000.0;

fn default_fps() -> f64 {
    24.0
}
fn default_end() -> f64 {
    120.0
}

/// Frame rate and playback range of a scene.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimeSettings {
    /// Frames per second.
    #[serde(default = "default_fps")]
    pub fps: f64,
    /// First frame of the playback range.
    #[serde(default)]
    pub start: f64,
    /// Last frame of the playback range (never before `start`).
    #[serde(default = "default_end")]
    pub end: f64,
}

impl Default for TimeSettings {
    fn default() -> Self {
        TimeSettings { fps: default_fps(), start: 0.0, end: default_end() }
    }
}

impl TimeSettings {
    /// Replace non-finite or out-of-range parts.
    pub fn sanitized(mut self) -> TimeSettings {
        self.fps = if self.fps.is_finite() { self.fps.clamp(MIN_FPS, MAX_FPS) } else { default_fps() };
        self.start = clamp_frame(self.start);
        self.end = clamp_frame(self.end).max(self.start);
        self
    }
    pub fn seconds(&self, frame: f64) -> f64 {
        frame / self.fps.max(MIN_FPS)
    }
    pub fn frame_at(&self, seconds: f64) -> f64 {
        seconds * self.fps
    }
    /// Length of the range in frames.
    pub fn span(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }
    pub fn clamp(&self, frame: f64) -> f64 {
        clamp_frame(frame).clamp(self.start, self.end.max(self.start))
    }
}

/// A finite frame within ±[`MAX_FRAME`] (0 for a non-finite input).
pub fn clamp_frame(frame: f64) -> f64 {
    if frame.is_finite() { frame.clamp(-MAX_FRAME, MAX_FRAME) } else { 0.0 }
}

/// What playback does at the end of the range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopMode {
    /// Stop on the last frame.
    Once,
    /// Jump back to the first frame.
    #[default]
    Loop,
    /// Turn around and play backwards.
    PingPong,
}

/// Playback state. [`Playback::advance`] moves a frame by elapsed wall-clock time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Playback {
    pub playing: bool,
    /// `1.0` forwards, `-1.0` backwards; other magnitudes change the speed.
    pub rate: f64,
    pub mode: LoopMode,
    /// Show whole frames only (the usual way to play keyed animation).
    pub whole_frames: bool,
    /// Time not yet turned into a whole frame.
    carry: f64,
}

impl Default for Playback {
    fn default() -> Self {
        Playback { playing: false, rate: 1.0, mode: LoopMode::Loop, whole_frames: true, carry: 0.0 }
    }
}

impl Playback {
    pub fn play(&mut self) {
        self.playing = true;
        self.carry = 0.0;
        if self.rate == 0.0 || !self.rate.is_finite() {
            self.rate = 1.0;
        }
    }
    pub fn stop(&mut self) {
        self.playing = false;
        self.carry = 0.0;
    }
    pub fn toggle(&mut self) {
        if self.playing { self.stop() } else { self.play() }
    }

    /// The frame after `dt` seconds of playback from `frame`. Stops itself at the end of the
    /// range in [`LoopMode::Once`]. A long pause (a stalled window) never skips more than one
    /// pass over the range.
    pub fn advance(&mut self, frame: f64, dt: f64, time: &TimeSettings) -> f64 {
        if !self.playing || !dt.is_finite() || dt <= 0.0 {
            return frame;
        }
        let time = time.sanitized();
        let span = time.span();
        if span <= 0.0 {
            self.stop();
            return time.start;
        }
        let mut step = dt * time.fps * self.rate + self.carry;
        if self.whole_frames {
            let whole = step.trunc();
            self.carry = step - whole;
            step = whole;
            if step == 0.0 {
                return frame;
            }
        } else {
            self.carry = 0.0;
        }
        // In a range of `span` frames, frame `end` and frame `start` are distinct when looping:
        // a loop is `span + 1` whole frames long, a ping-pong `2·span`.
        let x = clamp_frame(frame) - time.start + step.clamp(-4.0 * span - 4.0, 4.0 * span + 4.0);
        match self.mode {
            LoopMode::Once => {
                if x >= span || x <= 0.0 {
                    let end = if x >= span { time.end } else { time.start };
                    if (self.rate > 0.0) == (x >= span) {
                        self.stop();
                    }
                    return end;
                }
                time.start + x
            }
            LoopMode::Loop => {
                let period = if self.whole_frames { span + 1.0 } else { span };
                time.start + x.rem_euclid(period)
            }
            LoopMode::PingPong => {
                let y = x.rem_euclid(2.0 * span);
                if y > span {
                    self.rate = -self.rate;
                    time.start + 2.0 * span - y
                } else {
                    if x < 0.0 || x >= 2.0 * span {
                        // Bounced off the start (or went all the way round): direction unchanged
                        // only after an even number of bounces.
                        let bounces = (x / span).floor() as i64;
                        if bounces.rem_euclid(2) != 0 {
                            self.rate = -self.rate;
                        }
                    }
                    time.start + y
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_repaired() {
        let t = TimeSettings { fps: f64::NAN, start: 50.0, end: 10.0 }.sanitized();
        assert_eq!((t.fps, t.start, t.end), (24.0, 50.0, 50.0));
        let t = TimeSettings { fps: 1e9, start: f64::INFINITY, end: -f64::INFINITY }.sanitized();
        assert_eq!((t.fps, t.start, t.end), (MAX_FPS, 0.0, 0.0));
        let t = TimeSettings::default();
        assert_eq!(t.seconds(48.0), 2.0);
        assert_eq!(t.frame_at(0.5), 12.0);
        assert_eq!(t.clamp(500.0), 120.0);
    }

    #[test]
    fn playback_runs_at_the_scene_rate_in_whole_frames() {
        let time = TimeSettings { fps: 24.0, start: 0.0, end: 48.0 };
        let mut p = Playback::default();
        assert_eq!(p.advance(0.0, 1.0, &time), 0.0, "stopped");
        p.play();
        let mut f = 0.0;
        // 60 display frames = one second = 24 scene frames.
        for _ in 0..60 {
            f = p.advance(f, 1.0 / 60.0, &time);
            assert_eq!(f, f.round());
        }
        assert!((f - 24.0).abs() <= 1.0, "{f}");
        p.whole_frames = false;
        assert!((p.advance(10.0, 0.01, &time) - 10.24).abs() < 1e-9);
    }

    #[test]
    fn loop_modes() {
        let time = TimeSettings { fps: 10.0, start: 10.0, end: 20.0 };
        let mut p = Playback::default();
        p.play();
        // Looping shows frame 20, then frame 10.
        assert_eq!(p.advance(19.0, 0.1, &time), 20.0);
        assert_eq!(p.advance(20.0, 0.1, &time), 10.0);
        p.mode = LoopMode::Once;
        assert_eq!(p.advance(19.0, 0.5, &time), 20.0);
        assert!(!p.playing, "stops at the end");
        p.mode = LoopMode::PingPong;
        p.play();
        assert_eq!(p.advance(19.0, 0.3, &time), 18.0);
        assert_eq!(p.rate, -1.0);
        assert_eq!(p.advance(11.0, 0.3, &time), 12.0);
        assert_eq!(p.rate, 1.0);
        // A stalled frame or an empty range cannot hang or escape the range.
        let f = p.advance(15.0, 1e12, &time);
        assert!((10.0..=20.0).contains(&f));
        let empty = TimeSettings { fps: 24.0, start: 5.0, end: 5.0 };
        assert_eq!(p.advance(5.0, 1.0, &empty), 5.0);
        assert!(!p.playing);
    }
}
