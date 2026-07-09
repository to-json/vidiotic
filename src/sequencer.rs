//! Cue sequencer: auto-advances through the live bank's cues, cutting when each
//! cue's own dwell window elapses and pre-arming the next cue's decoder one bar
//! early. It is pure logic over `ClockSnapshot`s keyed by `CueId` — the engine
//! turns its events into decoder spawn/swap/drop actions and supplies each
//! cue's dwell/trig-delay via [`CueStep`]. `toggle_active` handles single-cue
//! add/remove (nuanced re-arm); `set_active_set` replaces the whole set on a
//! bank switch.
//!
//! Timing is relative, not gridded: each cue's swap boundary is
//! `started + dwell` beats, so cues with different dwell lengths chain back to
//! back. In the simple (non-advanced) engine mode every `CueStep` carries the
//! same global dwell and zero trig-delay, which reproduces a fixed phrase grid.

use crate::bank::CueId;
use crate::clock::ClockSnapshot;

/// The timing a cue contributes to the rotation: how long it plays and how long
/// the previous cue holds before it cuts in. Beats, resolved by the engine from
/// the cue's (possibly inherited) tick values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CueStep {
    pub id: CueId,
    /// Beats this cue plays before the sequencer advances.
    pub dwell: f64,
    /// Beats of lead-in: the swap to this cue fires this long after the previous
    /// cue's dwell boundary (the previous cue holds through the delay).
    pub trig_delay: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SeqState {
    Idle,
    Playing {
        cue: CueId,
        /// Beat at which this cue's dwell window began.
        started: f64,
    },
    PlayingArmed {
        cue: CueId,
        started: f64,
        next: CueId,
        /// Beat at which the swap to `next` fires (dwell boundary + trig delay).
        fire_at: f64,
    },
}

/// Actions the engine must take in response to a sequencer state change.
#[derive(Clone, Debug, PartialEq)]
pub enum SequencerEvent {
    /// Spawn the cue's decoder ahead of the swap so its first frame is ready.
    ArmDecoder(CueId),
    /// Cut the output to this cue now.
    SwapTo(CueId),
    /// The armed cue was cancelled; unused decoders can be dropped.
    DisarmDecoder,
}

const EPS: f64 = 1e-6;

/// Dwell-quantized round-robin over the active cue set. See the module doc.
pub struct Sequencer {
    state: SeqState,
    active: Vec<CueStep>, // insertion order == round-robin order
    default_dwell: f64,   // beats a cue plays when its own dwell is inherited
    bar: f64,             // 4 beats — arm lead time
    /// Set after a beat-numbering discontinuity (pause, sync switch, bank start):
    /// the next tick re-anchors the current cue's dwell window to the live beat.
    reanchor: bool,
}

impl Sequencer {
    /// An idle sequencer with an empty active set. `default_dwell` is the global
    /// phrase length a cue uses when it inherits its dwell.
    pub fn new(default_dwell: f64) -> Self {
        Self {
            state: SeqState::Idle,
            active: Vec::new(),
            default_dwell: default_dwell.max(1.0),
            bar: 4.0,
            reanchor: false,
        }
    }

    /// The global default dwell (phrase length), in beats.
    pub fn phrase_len(&self) -> f64 {
        self.default_dwell
    }

    /// The active cue's dwell in beats, falling back to the global default.
    fn dwell_of(&self, id: CueId) -> f64 {
        self.active
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.dwell)
            .unwrap_or(self.default_dwell)
            .max(1.0)
    }

    /// The trig-delay of the cue we're about to swap to, in beats.
    fn trig_delay_of(&self, id: CueId) -> f64 {
        self.active
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.trig_delay)
            .unwrap_or(0.0)
            .max(0.0)
    }

    /// Round-robin successor of `cur`, skipping it unless it's the only member.
    /// If `cur` is no longer active, fall back to the first active cue.
    fn pick_next(&self, cur: CueId) -> Option<CueId> {
        if self.active.is_empty() {
            return None;
        }
        match self.active.iter().position(|s| s.id == cur) {
            Some(i) => Some(self.active[(i + 1) % self.active.len()].id),
            None => Some(self.active[0].id),
        }
    }

    /// Advance the state machine one frame: start playing when idle, arm the
    /// next cue a bar before the current cue's dwell boundary, and swap on it.
    pub fn tick(&mut self, snap: &ClockSnapshot) -> Vec<SequencerEvent> {
        let mut ev = Vec::new();
        if !snap.is_playing {
            self.reanchor = true;
            return ev;
        }
        let beat = snap.beat;

        // A sync-source switch, pause, or bank start can make beat numbering jump;
        // re-anchor the current cue's dwell window to now and cancel a pending swap.
        if std::mem::take(&mut self.reanchor) {
            match self.state {
                SeqState::Playing { cue, .. } => {
                    self.state = SeqState::Playing { cue, started: beat };
                }
                SeqState::PlayingArmed { cue, .. } => {
                    self.state = SeqState::Playing { cue, started: beat };
                    ev.push(SequencerEvent::DisarmDecoder);
                }
                SeqState::Idle => {}
            }
        }

        match self.state {
            SeqState::Idle => {
                if let Some(first) = self.active.first().map(|s| s.id) {
                    ev.push(SequencerEvent::SwapTo(first));
                    self.state = SeqState::Playing { cue: first, started: beat };
                }
            }
            SeqState::Playing { cue, started } => {
                // A small backward tap: slide the window origin back so the dwell
                // isn't cut short.
                let started = started.min(beat);
                let boundary = started + self.dwell_of(cue);
                if beat >= boundary - self.bar {
                    match self.pick_next(cue) {
                        Some(next) if next != cue => {
                            ev.push(SequencerEvent::ArmDecoder(next));
                            let fire_at = boundary + self.trig_delay_of(next);
                            self.state = SeqState::PlayingArmed { cue, started, next, fire_at };
                        }
                        _ => self.state = SeqState::Playing { cue, started },
                    }
                } else {
                    self.state = SeqState::Playing { cue, started };
                }
            }
            SeqState::PlayingArmed { cue, started, next, fire_at } => {
                if beat + EPS < started {
                    // Backward jump past our window origin: cancel and re-anchor.
                    ev.push(SequencerEvent::DisarmDecoder);
                    self.state = SeqState::Playing { cue, started: beat };
                } else if beat >= fire_at {
                    ev.push(SequencerEvent::SwapTo(next));
                    self.state = SeqState::Playing { cue: next, started: fire_at };
                }
            }
        }
        ev
    }

    /// Toggle a cue's active-set membership. `beat` is the current beat, used to
    /// decide whether a removed armed cue can still be re-armed. Adding supplies
    /// the cue's [`CueStep`] timing; removal matches on `step.id`.
    pub fn toggle_active(&mut self, step: CueStep, beat: f64) -> Vec<SequencerEvent> {
        let mut ev = Vec::new();
        let id = step.id;
        if let Some(i) = self.active.iter().position(|s| s.id == id) {
            self.active.remove(i);
            if let SeqState::PlayingArmed { cue, next, started, fire_at } = self.state {
                if next == id && fire_at - beat >= self.bar {
                    ev.push(SequencerEvent::DisarmDecoder);
                    match self.pick_next(cue) {
                        Some(n2) if n2 != cue => {
                            ev.push(SequencerEvent::ArmDecoder(n2));
                            self.state = SeqState::PlayingArmed { cue, next: n2, started, fire_at };
                        }
                        _ => self.state = SeqState::Playing { cue, started },
                    }
                }
                // else <1 bar left: let it fire; resequences next window
            }
            // removing the playing cue: finish the window; pick_next falls back
            // to active[0] at the next arm point. Empty set: last cue loops.
        } else {
            self.active.push(step);
            if matches!(self.state, SeqState::Idle) {
                ev.push(SequencerEvent::SwapTo(id));
                self.state = SeqState::Playing { cue: id, started: beat };
            }
        }
        ev
    }

    /// Replace the entire active set (a live-bank switch or bulk rebuild).
    /// Playback of a still-present cue continues; if the armed cue vanished it is
    /// disarmed (re-arms on the new set next arm window); an empty set stops
    /// advancing but leaves the current cue displayed.
    pub fn set_active_set(&mut self, steps: Vec<CueStep>) -> Vec<SequencerEvent> {
        let mut ev = Vec::new();
        self.active = steps;
        match self.state {
            SeqState::Idle => {
                if let Some(first) = self.active.first().map(|s| s.id) {
                    ev.push(SequencerEvent::SwapTo(first));
                    self.state = SeqState::Playing { cue: first, started: 0.0 };
                    self.reanchor = true;
                }
            }
            SeqState::Playing { .. } => {}
            SeqState::PlayingArmed { cue, next, started, .. } => {
                if !self.active.iter().any(|s| s.id == next) {
                    ev.push(SequencerEvent::DisarmDecoder);
                    self.state = SeqState::Playing { cue, started };
                }
            }
        }
        ev
    }

    /// Change the global default dwell (the "next every" phrase length). Any
    /// armed cue is disarmed (its fire beat was computed against the old length);
    /// it re-arms at the new window's arm point.
    pub fn set_phrase_len(&mut self, beats: u32) -> Vec<SequencerEvent> {
        self.default_dwell = beats.max(1) as f64;
        if let SeqState::PlayingArmed { cue, started, .. } = self.state {
            self.state = SeqState::Playing { cue, started };
            return vec![SequencerEvent::DisarmDecoder];
        }
        vec![]
    }

    /// Re-anchor the current cue's dwell window on the next tick (on a
    /// sync-source switch, where beat numbering may jump discontinuously).
    pub fn reset_boundary(&mut self) {
        self.reanchor = true;
    }

    /// Force the round-robin back to the first cue in the active set — a hard
    /// reset's "playlist position". Disarms any pending swap. A no-op if the
    /// active set is empty.
    pub fn reset_to_first(&mut self) -> Vec<SequencerEvent> {
        let mut ev = Vec::new();
        let Some(first) = self.active.first().map(|s| s.id) else {
            return ev;
        };
        if matches!(self.state, SeqState::PlayingArmed { .. }) {
            ev.push(SequencerEvent::DisarmDecoder);
        }
        if self.playing() != Some(first) {
            ev.push(SequencerEvent::SwapTo(first));
        }
        self.state = SeqState::Playing { cue: first, started: 0.0 };
        self.reanchor = true;
        ev
    }

    /// Currently displayed cue, if any.
    pub fn playing(&self) -> Option<CueId> {
        match self.state {
            SeqState::Playing { cue, .. } | SeqState::PlayingArmed { cue, .. } => Some(cue),
            SeqState::Idle => None,
        }
    }

    /// Pre-armed next cue, if any.
    pub fn armed(&self) -> Option<CueId> {
        match self.state {
            SeqState::PlayingArmed { next, .. } => Some(next),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(beat: f64) -> ClockSnapshot {
        ClockSnapshot {
            bpm: 120.0,
            beat,
            phase: beat.rem_euclid(4.0),
            quantum: 4.0,
            is_playing: true,
        }
    }

    /// A step with the default 16-beat dwell and no trig delay.
    fn step(id: CueId) -> CueStep {
        CueStep { id, dwell: 16.0, trig_delay: 0.0 }
    }

    #[test]
    fn idle_starts_first_active_clip() {
        let mut s = Sequencer::new(16.0);
        assert_eq!(s.toggle_active(step(1), 0.0), vec![SequencerEvent::SwapTo(1)]);
        assert_eq!(s.playing(), Some(1));
    }

    #[test]
    fn arms_one_bar_early_then_cuts_on_boundary() {
        let mut s = Sequencer::new(16.0);
        s.toggle_active(step(1), 0.0);
        s.toggle_active(step(2), 0.0); // active = [1,2], playing 1 from beat 0
        // mid-window: nothing to do yet
        assert!(s.tick(&snap(1.0)).is_empty());
        // one bar before the boundary (beat 12 = 16 - 4) -> arm clip 2
        assert_eq!(s.tick(&snap(12.0)), vec![SequencerEvent::ArmDecoder(2)]);
        assert_eq!(s.armed(), Some(2));
        // cross the dwell boundary at beat 16 -> swap to 2
        assert_eq!(s.tick(&snap(16.1)), vec![SequencerEvent::SwapTo(2)]);
        assert_eq!(s.playing(), Some(2));
    }

    #[test]
    fn per_cue_dwell_controls_advance() {
        let mut s = Sequencer::new(16.0);
        // Two 8-beat cues override the 16-beat global default.
        s.toggle_active(CueStep { id: 1, dwell: 8.0, trig_delay: 0.0 }, 0.0);
        s.toggle_active(CueStep { id: 2, dwell: 8.0, trig_delay: 0.0 }, 0.0);
        assert!(s.tick(&snap(1.0)).is_empty());
        // arm a bar (4) before the 8-beat boundary
        assert_eq!(s.tick(&snap(4.0)), vec![SequencerEvent::ArmDecoder(2)]);
        assert_eq!(s.tick(&snap(8.1)), vec![SequencerEvent::SwapTo(2)]);
        assert_eq!(s.playing(), Some(2));
    }

    #[test]
    fn trig_delay_holds_previous_cue_past_the_boundary() {
        let mut s = Sequencer::new(16.0);
        s.toggle_active(CueStep { id: 1, dwell: 8.0, trig_delay: 0.0 }, 0.0);
        s.toggle_active(CueStep { id: 2, dwell: 8.0, trig_delay: 2.0 }, 0.0);
        s.tick(&snap(1.0));
        assert_eq!(s.tick(&snap(4.0)), vec![SequencerEvent::ArmDecoder(2)]);
        // past the boundary (8) but within cue 2's 2-beat trig delay: cue 1 holds
        assert!(s.tick(&snap(8.1)).is_empty());
        assert_eq!(s.playing(), Some(1));
        // after the delay (beat 10): swap fires
        assert_eq!(s.tick(&snap(10.1)), vec![SequencerEvent::SwapTo(2)]);
        assert_eq!(s.playing(), Some(2));
    }

    #[test]
    fn solo_clip_loops_without_swap() {
        let mut s = Sequencer::new(16.0);
        s.toggle_active(step(1), 0.0);
        s.tick(&snap(1.0));
        assert!(s.tick(&snap(12.0)).is_empty()); // nothing to arm
        assert!(s.tick(&snap(16.1)).is_empty());
        assert_eq!(s.playing(), Some(1));
    }

    #[test]
    fn reset_to_first_disarms_a_pending_swap() {
        let mut s = Sequencer::new(16.0);
        for c in [1, 2, 3] {
            s.toggle_active(step(c), 0.0);
        }
        s.tick(&snap(1.0));
        assert_eq!(s.tick(&snap(12.0)), vec![SequencerEvent::ArmDecoder(2)]);
        assert_eq!(s.armed(), Some(2));

        // Cue 1 (first) is already displaying; disarm the pending swap to 2
        // and cancel the round-robin advance, but no re-swap is needed.
        let ev = s.reset_to_first();
        assert_eq!(ev, vec![SequencerEvent::DisarmDecoder]);
        assert_eq!(s.playing(), Some(1));
        assert_eq!(s.armed(), None);
    }

    #[test]
    fn reset_to_first_swaps_back_from_a_later_cue() {
        let mut s = Sequencer::new(16.0);
        for c in [1, 2, 3] {
            s.toggle_active(step(c), 0.0);
        }
        s.tick(&snap(1.0));
        s.tick(&snap(12.0));
        assert_eq!(s.tick(&snap(16.1)), vec![SequencerEvent::SwapTo(2)]);
        assert_eq!(s.playing(), Some(2));

        let ev = s.reset_to_first();
        assert_eq!(ev, vec![SequencerEvent::SwapTo(1)]);
        assert_eq!(s.playing(), Some(1));
    }

    #[test]
    fn reset_to_first_on_first_cue_is_a_no_op() {
        let mut s = Sequencer::new(16.0);
        s.toggle_active(step(1), 0.0);
        assert_eq!(s.reset_to_first(), vec![]);
        assert_eq!(s.playing(), Some(1));
    }

    #[test]
    fn reset_to_first_with_no_active_cues_is_a_no_op() {
        let mut s = Sequencer::new(16.0);
        assert_eq!(s.reset_to_first(), vec![]);
        assert_eq!(s.playing(), None);
    }

    #[test]
    fn removing_armed_clip_rearms_replacement() {
        let mut s = Sequencer::new(16.0);
        for c in [1, 2, 3] {
            s.toggle_active(step(c), 0.0);
        }
        s.tick(&snap(1.0));
        assert_eq!(s.tick(&snap(12.0)), vec![SequencerEvent::ArmDecoder(2)]);
        // remove the armed clip 2 with a full bar left -> disarm + arm 3
        let ev = s.toggle_active(step(2), 12.0);
        assert_eq!(
            ev,
            vec![SequencerEvent::DisarmDecoder, SequencerEvent::ArmDecoder(3)]
        );
        assert_eq!(s.armed(), Some(3));
    }
}
