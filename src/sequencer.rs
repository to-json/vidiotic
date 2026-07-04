//! Clip sequencer: the user toggles clips into an active set; the sequencer
//! auto-advances through them, cutting on phrase boundaries and pre-arming the
//! next clip's decoder one bar early. It is pure logic over `ClockSnapshot`s —
//! the engine turns its events into decoder spawn/swap/drop actions.

use crate::clock::{BoundaryTracker, ClockSnapshot};
use crate::commands::ClipId;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeqState {
    Idle,
    Playing {
        clip: ClipId,
    },
    PlayingArmed {
        clip: ClipId,
        next: ClipId,
        fire_at_beat: f64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum SequencerEvent {
    ArmDecoder(ClipId),
    SwapTo(ClipId),
    DisarmDecoder,
}

pub struct Sequencer {
    pub state: SeqState,
    active: Vec<ClipId>, // insertion order == round-robin order
    phrase_len: f64,     // 16 or 32 beats
    bar: f64,            // 4 beats — arm lead time
    tracker: BoundaryTracker,
}

impl Sequencer {
    pub fn new(phrase_len: f64) -> Self {
        Sequencer {
            state: SeqState::Idle,
            active: Vec::new(),
            phrase_len,
            bar: 4.0,
            tracker: BoundaryTracker::new(),
        }
    }

    pub fn active(&self) -> &[ClipId] {
        &self.active
    }

    pub fn phrase_len(&self) -> f64 {
        self.phrase_len
    }

    pub fn is_active(&self, id: ClipId) -> bool {
        self.active.contains(&id)
    }

    /// Round-robin successor of `cur`, skipping it unless it's the only member.
    /// If `cur` is no longer active, fall back to the first active clip.
    fn pick_next(&self, cur: ClipId) -> Option<ClipId> {
        if self.active.is_empty() {
            return None;
        }
        match self.active.iter().position(|&c| c == cur) {
            Some(i) => Some(self.active[(i + 1) % self.active.len()]),
            None => Some(self.active[0]),
        }
    }

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
                    self.state = SeqState::Playing { clip: first };
                }
            }
            SeqState::Playing { clip } => {
                let fire_at =
                    ((snap.beat / self.phrase_len).floor() + 1.0) * self.phrase_len;
                if snap.beat >= fire_at - self.bar {
                    if let Some(next) = self.pick_next(clip) {
                        if next != clip {
                            ev.push(SequencerEvent::ArmDecoder(next));
                            self.state = SeqState::PlayingArmed {
                                clip,
                                next,
                                fire_at_beat: fire_at,
                            };
                        }
                    }
                }
            }
            SeqState::PlayingArmed {
                clip,
                next,
                fire_at_beat,
            } => {
                if fire_at_beat - snap.beat > self.phrase_len {
                    // tap jumped us backwards past the arm point: retarget, keep armed
                    let fire_at =
                        ((snap.beat / self.phrase_len).floor() + 1.0) * self.phrase_len;
                    self.state = SeqState::PlayingArmed {
                        clip,
                        next,
                        fire_at_beat: fire_at,
                    };
                } else if boundary.is_some() || snap.beat >= fire_at_beat {
                    ev.push(SequencerEvent::SwapTo(next));
                    self.state = SeqState::Playing { clip: next };
                }
            }
        }
        ev
    }

    /// Toggle a clip's active-set membership. `beat` is the current beat, used to
    /// decide whether a removed armed clip can still be re-armed.
    pub fn toggle_active(&mut self, id: ClipId, beat: f64) -> Vec<SequencerEvent> {
        let mut ev = Vec::new();
        if let Some(i) = self.active.iter().position(|&c| c == id) {
            self.active.remove(i);
            if let SeqState::PlayingArmed {
                clip,
                next,
                fire_at_beat,
            } = self.state
            {
                if next == id {
                    if fire_at_beat - beat >= self.bar {
                        ev.push(SequencerEvent::DisarmDecoder);
                        match self.pick_next(clip) {
                            Some(n2) if n2 != clip => {
                                ev.push(SequencerEvent::ArmDecoder(n2));
                                self.state = SeqState::PlayingArmed {
                                    clip,
                                    next: n2,
                                    fire_at_beat,
                                };
                            }
                            _ => self.state = SeqState::Playing { clip },
                        }
                    }
                    // else <1 bar left: let it fire; resequences next phrase
                }
            }
            // removing the playing clip: finish the phrase; pick_next falls back
            // to active[0] at the next arm point. Empty set: last clip loops.
        } else {
            self.active.push(id);
            if matches!(self.state, SeqState::Idle) {
                ev.push(SequencerEvent::SwapTo(id));
                self.state = SeqState::Playing { clip: id };
            }
        }
        ev
    }

    pub fn set_phrase_len(&mut self, beats: u32) -> Vec<SequencerEvent> {
        self.phrase_len = beats as f64;
        self.tracker.reset();
        if let SeqState::PlayingArmed { clip, .. } = self.state {
            self.state = SeqState::Playing { clip };
            return vec![SequencerEvent::DisarmDecoder];
        }
        vec![]
    }

    /// Currently displayed clip, if any.
    pub fn playing(&self) -> Option<ClipId> {
        match self.state {
            SeqState::Playing { clip } | SeqState::PlayingArmed { clip, .. } => Some(clip),
            SeqState::Idle => None,
        }
    }

    /// Pre-armed next clip, if any.
    pub fn armed(&self) -> Option<ClipId> {
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
