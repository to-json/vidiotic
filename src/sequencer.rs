//! Cue sequencer: auto-advances through the live bank's cues, cutting on phrase
//! boundaries and pre-arming the next cue's decoder one bar early. It is pure
//! logic over `ClockSnapshot`s keyed by `CueId` — the engine turns its events
//! into decoder spawn/swap/drop actions. `toggle_active` handles single-cue
//! add/remove (nuanced re-arm); `set_active_set` replaces the whole set on a
//! bank switch.

use crate::bank::CueId;
use crate::clock::{BoundaryTracker, ClockSnapshot};

#[derive(Clone, Copy, Debug, PartialEq)]
enum SeqState {
    Idle,
    Playing {
        cue: CueId,
    },
    PlayingArmed {
        cue: CueId,
        next: CueId,
        fire_at_beat: f64,
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

/// Phrase-quantized round-robin over the active cue set. See the module doc.
pub struct Sequencer {
    state: SeqState,
    active: Vec<CueId>, // insertion order == round-robin order
    phrase_len: f64,     // 16 or 32 beats
    bar: f64,            // 4 beats — arm lead time
    tracker: BoundaryTracker,
}

impl Sequencer {
    /// An idle sequencer with an empty active set.
    pub fn new(phrase_len: f64) -> Self {
        Self {
            state: SeqState::Idle,
            active: Vec::new(),
            phrase_len: phrase_len.max(1.0),
            bar: 4.0,
            tracker: BoundaryTracker::new(),
        }
    }

    /// Beats between auto-transitions.
    pub fn phrase_len(&self) -> f64 {
        self.phrase_len
    }

    /// Round-robin successor of `cur`, skipping it unless it's the only member.
    /// If `cur` is no longer active, fall back to the first active cue.
    fn pick_next(&self, cur: CueId) -> Option<CueId> {
        if self.active.is_empty() {
            return None;
        }
        match self.active.iter().position(|&c| c == cur) {
            Some(i) => Some(self.active[(i + 1) % self.active.len()]),
            None => Some(self.active[0]),
        }
    }

    /// Advance the state machine one frame: start playing when idle, arm the
    /// next cue a bar before the phrase boundary, and swap on the boundary.
    pub fn tick(&mut self, snap: &ClockSnapshot) -> Vec<SequencerEvent> {
        let mut ev = Vec::new();
        if !snap.is_playing {
            self.tracker.reset();
            return ev;
        }
        let boundary = self.tracker.crossed(snap.beat, self.phrase_len);

        match self.state {
            SeqState::Idle => {
                if let Some(&first) = self.active.first() {
                    ev.push(SequencerEvent::SwapTo(first));
                    self.state = SeqState::Playing { cue: first };
                }
            }
            SeqState::Playing { cue } => {
                let fire_at =
                    ((snap.beat / self.phrase_len).floor() + 1.0) * self.phrase_len;
                if snap.beat >= fire_at - self.bar {
                    if let Some(next) = self.pick_next(cue) {
                        if next != cue {
                            ev.push(SequencerEvent::ArmDecoder(next));
                            self.state = SeqState::PlayingArmed {
                                cue,
                                next,
                                fire_at_beat: fire_at,
                            };
                        }
                    }
                }
            }
            SeqState::PlayingArmed {
                cue,
                next,
                fire_at_beat,
            } => {
                if fire_at_beat - snap.beat > self.phrase_len {
                    // tap jumped us backwards past the arm point: retarget, keep armed
                    let fire_at =
                        ((snap.beat / self.phrase_len).floor() + 1.0) * self.phrase_len;
                    self.state = SeqState::PlayingArmed {
                        cue,
                        next,
                        fire_at_beat: fire_at,
                    };
                } else if boundary.is_some() || snap.beat >= fire_at_beat {
                    ev.push(SequencerEvent::SwapTo(next));
                    self.state = SeqState::Playing { cue: next };
                }
            }
        }
        ev
    }

    /// Toggle a cue's active-set membership. `beat` is the current beat, used to
    /// decide whether a removed armed cue can still be re-armed.
    pub fn toggle_active(&mut self, id: CueId, beat: f64) -> Vec<SequencerEvent> {
        let mut ev = Vec::new();
        if let Some(i) = self.active.iter().position(|&c| c == id) {
            self.active.remove(i);
            if let SeqState::PlayingArmed {
                cue,
                next,
                fire_at_beat,
            } = self.state
            {
                if next == id && fire_at_beat - beat >= self.bar {
                    ev.push(SequencerEvent::DisarmDecoder);
                    match self.pick_next(cue) {
                        Some(n2) if n2 != cue => {
                            ev.push(SequencerEvent::ArmDecoder(n2));
                            self.state = SeqState::PlayingArmed {
                                cue,
                                next: n2,
                                fire_at_beat,
                            };
                        }
                        _ => self.state = SeqState::Playing { cue },
                    }
                }
                // else <1 bar left: let it fire; resequences next phrase
            }
            // removing the playing cue: finish the phrase; pick_next falls back
            // to active[0] at the next arm point. Empty set: last cue loops.
        } else {
            self.active.push(id);
            if matches!(self.state, SeqState::Idle) {
                ev.push(SequencerEvent::SwapTo(id));
                self.state = SeqState::Playing { cue: id };
            }
        }
        ev
    }

    /// Replace the entire active set (a live-bank switch or bulk rebuild).
    /// Playback of a still-present cue continues; if the armed cue vanished it is
    /// disarmed (re-arms on the new set next arm window); an empty set stops
    /// advancing but leaves the current cue displayed.
    pub fn set_active_set(&mut self, ids: Vec<CueId>) -> Vec<SequencerEvent> {
        let mut ev = Vec::new();
        self.active = ids;
        match self.state {
            SeqState::Idle => {
                if let Some(&first) = self.active.first() {
                    ev.push(SequencerEvent::SwapTo(first));
                    self.state = SeqState::Playing { cue: first };
                }
            }
            SeqState::Playing { .. } => {}
            SeqState::PlayingArmed { cue, next, .. } => {
                if !self.active.contains(&next) {
                    ev.push(SequencerEvent::DisarmDecoder);
                    self.state = SeqState::Playing { cue };
                }
            }
        }
        self.tracker.reset();
        ev
    }

    /// Change the phrase length. Any armed cue is disarmed (its fire beat was
    /// computed against the old grid); it re-arms at the new grid's arm window.
    pub fn set_phrase_len(&mut self, beats: u32) -> Vec<SequencerEvent> {
        self.phrase_len = beats.max(1) as f64;
        self.tracker.reset();
        if let SeqState::PlayingArmed { cue, .. } = self.state {
            self.state = SeqState::Playing { cue };
            return vec![SequencerEvent::DisarmDecoder];
        }
        vec![]
    }

    /// Reset the phrase-boundary tracker (on a sync-source switch, where beat
    /// numbering may jump discontinuously).
    pub fn reset_boundary(&mut self) {
        self.tracker.reset();
    }

    /// Force the round-robin back to the first cue in the active set — a hard
    /// reset's "playlist position". Disarms any pending swap. A no-op if the
    /// active set is empty.
    pub fn reset_to_first(&mut self) -> Vec<SequencerEvent> {
        let mut ev = Vec::new();
        let Some(&first) = self.active.first() else {
            return ev;
        };
        if matches!(self.state, SeqState::PlayingArmed { .. }) {
            ev.push(SequencerEvent::DisarmDecoder);
        }
        if self.playing() != Some(first) {
            ev.push(SequencerEvent::SwapTo(first));
        }
        self.state = SeqState::Playing { cue: first };
        self.tracker.reset();
        ev
    }

    /// Currently displayed cue, if any.
    pub fn playing(&self) -> Option<CueId> {
        match self.state {
            SeqState::Playing { cue } | SeqState::PlayingArmed { cue, .. } => Some(cue),
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

    #[test]
    fn idle_starts_first_active_clip() {
        let mut s = Sequencer::new(16.0);
        assert_eq!(s.toggle_active(1, 0.0), vec![SequencerEvent::SwapTo(1)]);
        assert_eq!(s.playing(), Some(1));
    }

    #[test]
    fn arms_one_bar_early_then_cuts_on_boundary() {
        let mut s = Sequencer::new(16.0);
        s.toggle_active(1, 0.0);
        s.toggle_active(2, 0.0); // active = [1,2], playing 1
        // prime the boundary tracker mid-phrase
        assert!(s.tick(&snap(1.0)).is_empty());
        // one bar before phrase end (beat 12 = 16 - 4) -> arm clip 2
        assert_eq!(s.tick(&snap(12.0)), vec![SequencerEvent::ArmDecoder(2)]);
        assert_eq!(s.armed(), Some(2));
        // cross the phrase boundary at beat 16 -> swap to 2
        assert_eq!(s.tick(&snap(16.1)), vec![SequencerEvent::SwapTo(2)]);
        assert_eq!(s.playing(), Some(2));
    }

    #[test]
    fn solo_clip_loops_without_swap() {
        let mut s = Sequencer::new(16.0);
        s.toggle_active(1, 0.0);
        s.tick(&snap(1.0));
        assert!(s.tick(&snap(12.0)).is_empty()); // nothing to arm
        assert!(s.tick(&snap(16.1)).is_empty());
        assert_eq!(s.playing(), Some(1));
    }

    #[test]
    fn reset_to_first_disarms_a_pending_swap() {
        let mut s = Sequencer::new(16.0);
        for c in [1, 2, 3] {
            s.toggle_active(c, 0.0);
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
            s.toggle_active(c, 0.0);
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
        s.toggle_active(1, 0.0);
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
            s.toggle_active(c, 0.0);
        }
        s.tick(&snap(1.0));
        assert_eq!(s.tick(&snap(12.0)), vec![SequencerEvent::ArmDecoder(2)]);
        // remove the armed clip 2 with a full bar left -> disarm + arm 3
        let ev = s.toggle_active(2, 12.0);
        assert_eq!(
            ev,
            vec![SequencerEvent::DisarmDecoder, SequencerEvent::ArmDecoder(3)]
        );
        assert_eq!(s.armed(), Some(3));
    }
}
