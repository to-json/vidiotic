//! The sequencer's playable content model. A `Cue` is a placement of a source
//! clip with trim points (in/out) and its own preserve-playhead override; a
//! `Bank` is an ordered set of cues. The sequencer advances through the *live*
//! bank's cues; other banks can be edited while one plays.

use std::sync::Arc;

use crate::commands::{ChainSlot, ClipId};

/// Identifies a cue. Distinct from `ClipId`: the same source clip can appear as
/// several cues (different trim / options), so decoders are keyed by cue.
pub type CueId = u32;

/// A per-cue parameter that keeps its value while switched off, so toggling a
/// knob off and back on restores what the user last dialed in. Advanced-mode
/// controls (offsets, speed multiplier) use this.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Toggle<T> {
    pub on: bool,
    pub val: T,
}

impl<T> Toggle<T> {
    /// A disabled toggle carrying `val` as its retained value.
    pub fn off(val: T) -> Self {
        Self { on: false, val }
    }
}

/// A camera cue's voluntary delay: how far behind the live edge it plays.
/// Dialed in seconds, or in beats (re-resolved against the live tempo every
/// tick). By default a change slews toward its new target; with `quantize` on
/// it re-targets exactly at loop-grid boundary crossings instead. Ignored for
/// file-backed cues.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamDelay {
    pub value: f64,
    /// `value` is in beats (`× 60 / bpm`) rather than seconds.
    pub beats: bool,
    /// Re-target at loop-grid boundaries instead of slewing continuously.
    pub quantize: bool,
}

impl CamDelay {
    /// The target delay in seconds at the given live tempo.
    pub fn seconds(&self, bpm: f64) -> f64 {
        if self.beats {
            self.value * 60.0 / bpm.max(1.0)
        } else {
            self.value
        }
    }
}

impl Default for CamDelay {
    fn default() -> Self {
        Self { value: 0.0, beats: false, quantize: false }
    }
}

/// A placement of a source clip: trim points plus per-cue playback overrides.
///
/// The advanced-mode fields (`dwell` through `speed_mul`) are always stored but
/// only take effect when the engine's advanced mode is on; see `App::advanced`.
#[derive(Clone, Debug)]
pub struct Cue {
    pub id: CueId,
    pub clip: ClipId,
    pub name: Arc<str>,
    /// In-point, seconds from the clip start: where playback and loop restarts
    /// seek to.
    pub in_sec: f64,
    /// Out-point, seconds; `None` = play to the clip's natural end.
    pub out_sec: Option<f64>,
    /// Per-cue override of the global preserve-playhead default; `None` inherits.
    pub preserve: Option<bool>,
    /// Per-cue effect chain: an ordered stack of shaders applied while this cue
    /// plays; each stage reads the previous stage's output via `prev()`. Empty =
    /// use whatever the live (livecoded) shader is. The live shader can appear as
    /// a slot (`SlotRef::Live`) anywhere in the stack.
    pub chain: Vec<ChainSlot>,
    /// Beats-until-advance, in 1/32-beat ticks (`LOOP_TICKS_PER_BEAT`); `None`
    /// inherits the global phrase length. How long this cue plays before the
    /// sequencer advances to the next.
    pub dwell: Option<u32>,
    /// Per-cue video re-loop grid in ticks: `None` inherits the global loop
    /// rate, `Some(0)` forces no re-loop, `Some(n)` loops every `n` ticks.
    /// Independent of `dwell`: a cue can dwell 16 beats but retrigger every 4.
    pub loop_len: Option<u32>,
    /// Micro-timing: shift this cue's loop-restart grid by signed ticks (swing).
    pub loop_phase: Toggle<i32>,
    /// Sample-start nudge: seconds added to `in_sec` on each (re)start.
    pub start_nudge: Toggle<f64>,
    /// Trig delay: ticks of lead-in the previous cue holds before this one starts.
    pub trig_delay: Toggle<u32>,
    /// Source-clip tempo metadata override; `None` inherits the clip's own BPM.
    pub bpm: Option<f64>,
    /// When on (and a source BPM is known), retime playback so the clip plays at
    /// the session tempo: `speed *= session_bpm / source_bpm`.
    pub bpm_sync_on: bool,
    /// User playback-speed multiplier, stacked on top of any BPM-sync factor.
    pub speed_mul: Toggle<f64>,
    /// Voluntary live delay, for camera-sourced cues only.
    pub delay: CamDelay,
}

impl Cue {
    /// A full-length cue: no trim, all overrides inherited, advanced knobs off.
    pub fn new(id: CueId, clip: ClipId, name: impl Into<Arc<str>>) -> Self {
        Self {
            id,
            clip,
            name: name.into(),
            in_sec: 0.0,
            out_sec: None,
            preserve: None,
            chain: Vec::new(),
            dwell: None,
            loop_len: None,
            loop_phase: Toggle::off(0),
            start_nudge: Toggle::off(0.0),
            trig_delay: Toggle::off(0),
            bpm: None,
            bpm_sync_on: false,
            speed_mul: Toggle::off(1.0),
            delay: CamDelay::default(),
        }
    }
}

/// An ordered, named set of cues; the play order is the vec order.
#[derive(Clone, Debug)]
pub struct Bank {
    pub name: Arc<str>,
    pub cues: Vec<Cue>,
}

impl Bank {
    /// An empty bank.
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            cues: Vec::new(),
        }
    }

    /// The cue with `id`, if it lives in this bank.
    pub fn cue(&self, id: CueId) -> Option<&Cue> {
        self.cues.iter().find(|c| c.id == id)
    }

    /// Mutable variant of [`Bank::cue`].
    pub fn cue_mut(&mut self, id: CueId) -> Option<&mut Cue> {
        self.cues.iter_mut().find(|c| c.id == id)
    }

    /// Cue ids in play order.
    pub fn ids(&self) -> Vec<CueId> {
        self.cues.iter().map(|c| c.id).collect()
    }
}
