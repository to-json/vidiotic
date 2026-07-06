//! Beat clock. Everything the app quantizes to comes from a `ClockSource`:
//! the internal host-time clock now, Ableton Link (M3) and Pro DJ Link (later)
//! behind the same small trait. All timing is derived from host time, never
//! frame counts, so it survives frame-rate variation and can be quantized.

use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub struct ClockSnapshot {
    pub bpm: f64,
    /// Continuous beats since the anchor. Only jumps backwards on a tap/phase reset.
    pub beat: f64,
    /// Position within the quantum: `beat.rem_euclid(quantum)`.
    pub phase: f64,
    /// Beats per cycle the source aligns to (a bar = 4).
    pub quantum: f64,
    pub is_playing: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ClockCaps {
    pub can_set_tempo: bool,
    pub can_set_phase: bool,
    pub peers: u64,
}

pub trait ClockSource {
    fn snapshot(&mut self) -> ClockSnapshot;
    fn set_bpm(&mut self, bpm: f64);
    /// Multiply tempo by `1 + ratio`; `ratio = ±0.001` for the ±0.1% controls.
    fn nudge_bpm(&mut self, ratio: f64);
    /// Make "now" an exact quantum (bar) boundary — sets the downbeat anchor.
    fn tap_downbeat(&mut self);
    /// Reset the grid to its origin: `beat = 0` — note one of bar one, phrase one.
    fn reset(&mut self);
    fn caps(&self) -> ClockCaps;
}

const BPM_MIN: f64 = 20.0;
const BPM_MAX: f64 = 999.0;

pub struct InternalClock {
    anchor: Instant,
    bpm: f64,
    beats_at_anchor: f64,
    quantum: f64,
}

impl InternalClock {
    pub fn new(bpm: f64, quantum: f64) -> Self {
        Self {
            anchor: Instant::now(),
            bpm,
            beats_at_anchor: 0.0,
            quantum,
        }
    }

    /// Seed from another clock's snapshot when switching sync source (continuity).
    pub fn from_snapshot(s: &ClockSnapshot) -> Self {
        Self {
            anchor: Instant::now(),
            bpm: s.bpm,
            beats_at_anchor: s.beat,
            quantum: s.quantum,
        }
    }

    fn beat_now(&self) -> f64 {
        self.beats_at_anchor + self.anchor.elapsed().as_secs_f64() * self.bpm / 60.0
    }

    /// Fold elapsed time into `beats_at_anchor` at the OLD tempo, then move the
    /// anchor to now. A subsequent bpm change then cannot re-price already-elapsed
    /// time, so `beat` stays continuous across tempo changes.
    fn reanchor(&mut self) {
        let now = Instant::now();
        self.beats_at_anchor += now.duration_since(self.anchor).as_secs_f64() * self.bpm / 60.0;
        self.anchor = now;
    }
}

impl ClockSource for InternalClock {
    fn snapshot(&mut self) -> ClockSnapshot {
        let beat = self.beat_now();
        ClockSnapshot {
            bpm: self.bpm,
            beat,
            phase: beat.rem_euclid(self.quantum),
            quantum: self.quantum,
            is_playing: true,
        }
    }

    fn set_bpm(&mut self, bpm: f64) {
        self.reanchor();
        self.bpm = bpm.clamp(BPM_MIN, BPM_MAX);
    }

    fn nudge_bpm(&mut self, ratio: f64) {
        self.reanchor();
        self.bpm = (self.bpm * (1.0 + ratio)).clamp(BPM_MIN, BPM_MAX);
    }

    /// Round the current beat to the NEAREST quantum multiple: a tap 0.3 beats
    /// after the true downbeat snaps back -0.3 rather than jumping +3.7 forward.
    /// Worst-case correction is quantum/2, and `beat` may step backwards by up to
    /// that much — `BoundaryTracker` absorbs it without firing a transition.
    fn tap_downbeat(&mut self) {
        self.reanchor();
        self.beats_at_anchor = (self.beats_at_anchor / self.quantum).round() * self.quantum;
    }

    fn reset(&mut self) {
        self.anchor = Instant::now();
        self.beats_at_anchor = 0.0;
    }

    fn caps(&self) -> ClockCaps {
        ClockCaps {
            can_set_tempo: true,
            can_set_phase: true,
            peers: 0,
        }
    }
}

/// Ableton Link clock: follows a shared session's tempo and phase. rekordbox 6+
/// in Performance mode speaks Link, as do Ableton Live and many apps. We always
/// report `is_playing = true` — VJ visuals should keep running regardless of the
/// session's transport (start/stop) state.
pub struct LinkClock {
    link: rusty_link::AblLink,
    state: rusty_link::SessionState, // reusable scratch; capture fills it in place
    quantum: f64,
}

impl LinkClock {
    pub fn new(initial_bpm: f64, quantum: f64) -> Self {
        let link = rusty_link::AblLink::new(initial_bpm);
        link.enable_start_stop_sync(true);
        link.enable(true); // begins peer discovery
        LinkClock {
            link,
            state: rusty_link::SessionState::new(),
            quantum,
        }
    }
}

impl ClockSource for LinkClock {
    fn snapshot(&mut self) -> ClockSnapshot {
        let t = self.link.clock_micros();
        self.link.capture_app_session_state(&mut self.state);
        ClockSnapshot {
            bpm: self.state.tempo(),
            beat: self.state.beat_at_time(t, self.quantum),
            phase: self.state.phase_at_time(t, self.quantum),
            quantum: self.quantum,
            is_playing: true,
        }
    }

    fn set_bpm(&mut self, bpm: f64) {
        let t = self.link.clock_micros();
        self.link.capture_app_session_state(&mut self.state);
        self.state.set_tempo(bpm.clamp(BPM_MIN, BPM_MAX), t);
        self.link.commit_app_session_state(&self.state);
    }

    fn nudge_bpm(&mut self, ratio: f64) {
        let t = self.link.clock_micros();
        self.link.capture_app_session_state(&mut self.state);
        let new = (self.state.tempo() * (1.0 + ratio)).clamp(BPM_MIN, BPM_MAX);
        self.state.set_tempo(new, t);
        self.link.commit_app_session_state(&self.state);
    }

    fn tap_downbeat(&mut self) {
        let t = self.link.clock_micros();
        self.link.capture_app_session_state(&mut self.state);
        let beat = self.state.beat_at_time(t, self.quantum);
        let target = (beat / self.quantum).round() * self.quantum;
        self.state.request_beat_at_time(target, t, self.quantum);
        self.link.commit_app_session_state(&self.state);
    }

    fn reset(&mut self) {
        // Request the current instant be beat 0. With peers this shifts the
        // whole session's grid — that's the intended meaning of a manual reset.
        let t = self.link.clock_micros();
        self.link.capture_app_session_state(&mut self.state);
        self.state.request_beat_at_time(0.0, t, self.quantum);
        self.link.commit_app_session_state(&self.state);
    }

    fn caps(&self) -> ClockCaps {
        ClockCaps {
            can_set_tempo: true,
            can_set_phase: true,
            peers: self.link.num_peers(),
        }
    }
}

impl Drop for LinkClock {
    fn drop(&mut self) {
        self.link.enable(false);
    }
}

/// Detects when the beat clock crosses a phrase boundary, tolerating the
/// backwards jumps a tap can cause.
pub struct BoundaryTracker {
    prev_beat: Option<f64>,
}

const BACKWARD_EPS: f64 = 1e-6;

impl BoundaryTracker {
    pub fn new() -> Self {
        Self { prev_beat: None }
    }

    /// Returns `Some(phrase_index)` exactly once when a phrase boundary is crossed.
    pub fn crossed(&mut self, cur_beat: f64, phrase_len: f64) -> Option<u64> {
        // first frame: prime only, never fire
        let prev = self.prev_beat.replace(cur_beat)?;
        if cur_beat < prev - BACKWARD_EPS {
            // tap round-down / phase renegotiation / rewind: resync silently
            return None;
        }
        let prev_idx = (prev / phrase_len).floor() as i64;
        let cur_idx = (cur_beat / phrase_len).floor() as i64;
        (cur_idx > prev_idx).then_some(cur_idx as u64)
    }

    /// Call on pause, sync-source switch, or phrase-length change.
    pub fn reset(&mut self) {
        self.prev_beat = None;
    }
}

impl Default for BoundaryTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_frame_never_fires() {
        let mut t = BoundaryTracker::new();
        assert_eq!(t.crossed(0.0, 16.0), None);
    }

    #[test]
    fn forward_cross_fires_once() {
        let mut t = BoundaryTracker::new();
        assert_eq!(t.crossed(15.0, 16.0), None); // prime
        assert_eq!(t.crossed(15.9, 16.0), None); // same phrase 0
        assert_eq!(t.crossed(16.1, 16.0), Some(1)); // into phrase 1
        assert_eq!(t.crossed(16.5, 16.0), None); // still phrase 1
    }

    #[test]
    fn backward_jump_does_not_fire() {
        let mut t = BoundaryTracker::new();
        assert_eq!(t.crossed(31.5, 16.0), None); // prime, phrase 1
        // tap snaps beat back to 30.0 (still phrase 1) — must NOT fire
        assert_eq!(t.crossed(30.0, 16.0), None);
        // and continuing forward from 30 into 32 fires once
        assert_eq!(t.crossed(32.2, 16.0), Some(2));
    }

    #[test]
    fn multi_phrase_skip_fires_once() {
        let mut t = BoundaryTracker::new();
        assert_eq!(t.crossed(1.0, 16.0), None); // prime
        // a frame hitch skips from phrase 0 to phrase 3 — fires once
        assert_eq!(t.crossed(50.0, 16.0), Some(3));
    }

    #[test]
    fn internal_clock_bpm_change_is_continuous() {
        let mut c = InternalClock::new(120.0, 4.0);
        let b0 = c.snapshot().beat;
        c.set_bpm(174.0);
        let b1 = c.snapshot().beat;
        // beat must not jump on a tempo change (allow tiny elapsed advance)
        assert!((b1 - b0).abs() < 0.05, "beat jumped by {}", b1 - b0);
        assert_eq!(c.snapshot().bpm, 174.0);
    }

    #[test]
    fn link_clock_constructs_and_snapshots() {
        // Proves the Ableton Link FFI binding works: construct, follow tempo,
        // read peers. (No assertion on exact tempo — another Link app on the LAN
        // could negotiate it; peers may be >0 for the same reason.)
        let mut c = LinkClock::new(128.0, 4.0);
        let s = c.snapshot();
        assert!(s.bpm.is_finite() && s.bpm > 0.0);
        assert!(s.is_playing);
        assert_eq!(s.quantum, 4.0);
        let _ = c.caps().peers;
    }

    #[test]
    fn tap_snaps_phase_to_boundary() {
        let mut c = InternalClock::new(120.0, 4.0);
        // advance a bit, then tap: phase should be ~0 right after.
        std::thread::sleep(std::time::Duration::from_millis(5));
        c.tap_downbeat();
        let phase = c.snapshot().phase;
        assert!(phase < 0.05 || phase > 3.95, "phase not near boundary: {phase}");
    }

    #[test]
    fn reset_returns_to_grid_origin() {
        // A high tempo so a few beats accrue quickly, then reset → beat ~0.
        let mut c = InternalClock::new(600.0, 4.0);
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(c.snapshot().beat > 0.1, "expected the beat to advance first");
        c.reset();
        let beat = c.snapshot().beat;
        assert!(beat < 0.05, "beat not at origin after reset: {beat}");
    }
}
