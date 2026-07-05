//! The sequencer's playable content model. A `Cue` is a placement of a source
//! clip with trim points (in/out) and its own preserve-playhead override; a
//! `Bank` is an ordered set of cues. The sequencer advances through the *live*
//! bank's cues; other banks can be edited while one plays.

use crate::commands::{ClipId, ShaderId};

/// Identifies a cue. Distinct from `ClipId`: the same source clip can appear as
/// several cues (different trim / options), so decoders are keyed by cue.
pub type CueId = u32;

#[derive(Clone, Debug)]
pub struct Cue {
    pub id: CueId,
    pub clip: ClipId,
    pub name: String,
    /// In-point, seconds from the clip start: where playback and loop restarts
    /// seek to.
    pub in_sec: f64,
    /// Out-point, seconds; `None` = play to the clip's natural end.
    pub out_sec: Option<f64>,
    /// Per-cue override of the global preserve-playhead default; `None` inherits.
    pub preserve: Option<bool>,
    /// Per-cue shader override: a pinned pool shader used while this cue plays.
    /// `None` = use whatever the live (livecoded) shader is.
    pub shader: Option<ShaderId>,
}

impl Cue {
    pub fn new(id: CueId, clip: ClipId, name: String) -> Self {
        Cue {
            id,
            clip,
            name,
            in_sec: 0.0,
            out_sec: None,
            preserve: None,
            shader: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Bank {
    pub name: String,
    pub cues: Vec<Cue>,
}

impl Bank {
    pub fn new(name: impl Into<String>) -> Self {
        Bank {
            name: name.into(),
            cues: Vec::new(),
        }
    }

    pub fn cue(&self, id: CueId) -> Option<&Cue> {
        self.cues.iter().find(|c| c.id == id)
    }

    pub fn cue_mut(&mut self, id: CueId) -> Option<&mut Cue> {
        self.cues.iter_mut().find(|c| c.id == id)
    }

    /// Cue ids in play order.
    pub fn ids(&self) -> Vec<CueId> {
        self.cues.iter().map(|c| c.id).collect()
    }
}
