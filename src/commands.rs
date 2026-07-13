//! The UI↔engine contract: `Command`s flow from the control UI (and async file
//! pickers) to the engine; the engine publishes a `UiMirror` snapshot the UI
//! reads. Keeping these in one place lets other input sources (keys today,
//! MIDI eventually) map onto the same commands.

use std::path::PathBuf;
use std::sync::Arc;

use crate::bank::{CueId, Toggle};
use crate::isf::IsfValue;

/// Identifies a source clip in the pool (its scan index).
pub type ClipId = u32;

/// A compiled shader pinned into the pool. A cue can reference one as an override.
pub type ShaderId = u32;

/// Which shader runs at one position in a cue's effect chain.
///
/// `Builtin` carries the effect's stable name — the persistable handle written
/// into `.viproj`. `Pinned` is a runtime-only pool id (livecoded captures have
/// no stable source, so they are not serialized). `Live` is the current
/// livecoded shader, so it can sit anywhere in the stack. `Isf` carries the ISF
/// shader's file path (project-relative or absolute) — a persistable handle the
/// pool compiles on demand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlotRef {
    Live,
    Builtin(Arc<str>),
    Pinned(ShaderId),
    Isf(Arc<str>),
}

/// One entry in a cue's effect chain. `params` holds per-slot ISF input
/// overrides (empty for non-ISF slots, or for ISF inputs left at their schema
/// default); an input value carries an `f32`, so this is `PartialEq` but not `Eq`.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainSlot {
    pub shader: SlotRef,
    pub params: Vec<(Arc<str>, IsfValue)>,
}

impl ChainSlot {
    /// A slot referencing `shader` with default (no) parameters.
    pub fn new(shader: SlotRef) -> Self {
        Self { shader, params: Vec::new() }
    }

    /// The current value of an ISF input on this slot, if overridden.
    pub fn param(&self, name: &str) -> Option<&IsfValue> {
        self.params.iter().find(|(n, _)| n.as_ref() == name).map(|(_, v)| v)
    }

    /// Set (or replace) an ISF input override on this slot.
    pub fn set_param(&mut self, name: Arc<str>, value: IsfValue) {
        if let Some(slot) = self.params.iter_mut().find(|(n, _)| *n == name) {
            slot.1 = value;
        } else {
            self.params.push((name, value));
        }
    }
}

/// Resolution of the musical re-loop grid: 32 ticks per beat (quarter note), so
/// an eighth note is 16 ticks and a 4/4 bar is 128. Lets `SetLoopLen` stay an
/// integer while still expressing sub-beat divisions.
pub const LOOP_TICKS_PER_BEAT: u32 = 32;

/// Musical time signature. Tempo (BPM) always counts quarter notes — Link's
/// convention — so the signature only changes the bar length and beat
/// subdivision: `num` notes of `1/den` each per bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeSig {
    pub num: u8,
    pub den: u8,
}

/// Denominators the UI offers; a bar is always a whole number of
/// `LOOP_TICKS_PER_BEAT`-scaled ticks for each of these.
pub const TIME_SIG_DENS: [u8; 5] = [1, 2, 4, 8, 16];

impl TimeSig {
    /// Beats (quarter notes) per bar: `num * 4 / den`. Fractional for x/8, x/16
    /// signatures (e.g. 7/8 is 3.5 beats).
    pub fn quantum(self) -> f64 {
        self.num as f64 * 4.0 / self.den as f64
    }

    /// Ticks (`LOOP_TICKS_PER_BEAT`-scaled) per bar — always an integer since
    /// every allowed denominator divides `4 * LOOP_TICKS_PER_BEAT`.
    pub fn bar_ticks(self) -> u32 {
        self.num as u32 * (4 * LOOP_TICKS_PER_BEAT / self.den as u32)
    }

    /// Clamp to a valid signature: `num >= 1`, `den` snapped to the nearest
    /// allowed denominator.
    pub fn sanitized(self) -> Self {
        let num = self.num.max(1);
        let den = TIME_SIG_DENS
            .iter()
            .min_by_key(|&&d| (d as i32 - self.den as i32).abs())
            .copied()
            .unwrap_or(4);
        Self { num, den }
    }
}

impl Default for TimeSig {
    fn default() -> Self {
        Self { num: 4, den: 4 }
    }
}

/// A musical cadence length for the auto-advance / re-loop grids: either an
/// absolute note value, or a count of bars of the current [`TimeSig`] (so it
/// re-resolves automatically when the signature changes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cadence {
    /// Absolute length in `LOOP_TICKS_PER_BEAT`-scaled ticks.
    Note(u32),
    /// Whole bars of the current time signature.
    Bars(u32),
}

impl Cadence {
    /// Resolve to ticks against a time signature.
    pub fn ticks(self, ts: TimeSig) -> u32 {
        match self {
            Self::Note(t) => t,
            Self::Bars(n) => n * ts.bar_ticks(),
        }
    }

    /// Resolve to beats (quarter notes) against a time signature.
    pub fn beats(self, ts: TimeSig) -> f64 {
        self.ticks(ts) as f64 / LOOP_TICKS_PER_BEAT as f64
    }
}

impl Default for Cadence {
    /// Matches the pre-existing default phrase length of 16 beats (4 bars of 4/4).
    fn default() -> Self {
        Self::Bars(4)
    }
}

/// Which `ClockSource` drives the beat grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SyncKind {
    #[default]
    Internal,
    Link,
}

/// Everything an input surface (UI, keys, pickers) can ask the engine to do.
#[derive(Clone, Debug)]
pub enum Command {
    SetBpm(f64),
    BpmDelta(f64),   // ±1 from the +/- keys
    NudgeBpm(f64),   // ratio ±0.001 for the ±0.1% controls
    TapDownbeat,     // snap the downbeat phase to now (does not change tempo)
    TapTempo,        // derive BPM from the interval between successive taps
    SoftReset,       // reset the beat grid to bar 1 / beat 1 / phrase 1; playlist position and playhead untouched
    HardReset,       // soft reset, plus jump the playlist back to its first cue and restart its playhead
    SetSyncSource(SyncKind),
    SetTimeSig(TimeSig),
    SetPhraseCadence(Cadence),  // musical length between auto-transitions to the next clip
    SetLoopCadence(Option<Cadence>), // forced video re-loop grid; None = loop on EOF only
    SetPreservePlayhead(bool),  // on cut, carry the playhead over (true) or restart the incoming clip from its start (false)
    ToggleClipActive(ClipId),
    // Cue/bank editing. Cue mutations target the *edit* bank; `AddCue` also
    // selects the new cue. Trim/preserve edits take effect the next time the
    // cue's decoder spawns (re-trigger, or when its bank goes live).
    AddCue(ClipId),                    // add a full-length cue for this clip to the edit bank
    RemoveCue(CueId),
    SelectCue(Option<CueId>),
    SetCueIn(CueId, f64),              // in-point, seconds
    SetCueOut(CueId, Option<f64>),     // out-point, seconds; None = clip end
    SetCueInToPlayhead(CueId),         // snap in-point to the displayed playhead
    SetCueOutToPlayhead(CueId),        // snap out-point to the displayed playhead
    SetCuePreserve(CueId, Option<bool>), // per-cue preserve override; None = inherit global
    SetCueChain(CueId, Vec<ChainSlot>), // replace the cue's effect chain; empty = the live shader
    // Set one ISF input on one chain slot of a cue, without replacing the whole
    // chain (so a slider drag doesn't clobber the rest of the stack).
    SetChainParam { cue: CueId, slot: usize, name: Arc<str>, value: IsfValue },
    LoadIsf(PathBuf),                  // compile an ISF `.fs` into the pool and append it to the selected cue's chain
    SetCueParam(CueId, CueParam),      // one advanced per-cue timing/speed knob
    MoveCue(CueId, usize),             // reorder within the edit bank to a target index (drag / ◀▶)
    SetClipBpm(ClipId, Option<f64>),   // source-clip tempo metadata; None clears it
    SetAdvancedMode(bool),             // gate per-cue timing/speed resolution + the extended UI
    AddBank,
    SetLiveBank(usize),                // which bank the sequencer plays
    CycleLiveBank(i32),                // step the live bank by ±1, wrapping (keys , / .)
    SetEditBank(usize),                // which bank the UI edits
    // Shader pool: pin the current live shader's last-good compile so a cue can
    // use it while you keep livecoding the main shader.
    CaptureShader,                     // pin the current live shader into the pool
    RemoveShader(ShaderId),            // drop a pinned shader (cues fall back to the live shader)
    SetClipDir(PathBuf),                // replace the whole pool with one bank from this dir
    AddClipDirAsBank(PathBuf),          // append this dir as a new clip bank (keeps existing clips/cues)
    SetActiveClipBank(usize),           // which clip bank the pool grid shows
    SetShaderPath(PathBuf),
    SetAudioDevice(Option<String>), // id key; None = default
    ToggleFullscreen,               // shell-intercepted
    // Project persistence. `SaveProject` writes back to the loaded path (or opens
    // the picker if none), `SaveProjectAs` always opens the picker; both resolve
    // to a `SaveProjectTo` once a destination is known.
    SaveProject,
    SaveProjectAs,
    SaveProjectTo(PathBuf),
    Quit,
}

/// One advanced per-cue knob, edited via [`Command::SetCueParam`]. Mirrors the
/// advanced fields on [`crate::bank::Cue`]; ticks are 1/32-beat
/// ([`LOOP_TICKS_PER_BEAT`]).
#[derive(Clone, Copy, Debug)]
pub enum CueParam {
    Dwell(Option<u32>),        // beats-until-advance in ticks; None = inherit global
    Loop(Option<u32>),         // re-loop grid in ticks; None = inherit global
    LoopPhase(Toggle<i32>),    // loop-grid micro-timing, signed ticks
    StartNudge(Toggle<f64>),   // in-point nudge, seconds
    TrigDelay(Toggle<u32>),    // swap-in lead-in, ticks
    Bpm(Option<f64>),          // source tempo override; None = inherit clip
    BpmSync(bool),             // retime to session tempo
    SpeedMul(Toggle<f64>),     // user speed multiplier
}

/// A clip/cue's live-playback role, for UI markers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipRole {
    None,
    Playing,
    Armed,
}

/// One source clip as shown in the pool grid.
#[derive(Clone, Debug)]
pub struct ClipEntry {
    pub id: ClipId,
    pub name: Arc<str>,
    pub active: bool,
    pub role: ClipRole,
    pub has_thumb: bool, // texture cached in the UI's thumbnail map
    pub bpm: Option<f64>, // source tempo metadata, if set
    pub bank: usize,     // the clip bank this entry is shown under
}

/// A clip bank's identity for the clip-bank bar above the pool grid.
#[derive(Clone, Debug)]
pub struct ClipBankView {
    pub name: Arc<str>,
    pub clip_count: usize,
}

/// One cue of the edit bank, as shown in the sequencer section / editor.
#[derive(Clone, Debug)]
pub struct CueView {
    pub id: CueId,
    pub clip: ClipId,
    pub name: Arc<str>,
    pub in_sec: f64,
    pub out_sec: Option<f64>,
    pub preserve: Option<bool>,
    pub chain: Vec<ChainSlot>, // per-cue effect chain; empty = the live shader
    pub role: ClipRole, // Playing/Armed if this cue is the live bank's current/next
    pub has_thumb: bool,
    // Advanced-mode timing/speed (see `crate::bank::Cue`). Ticks are 1/32-beat.
    pub dwell: Option<u32>,
    pub loop_len: Option<u32>,
    pub loop_phase: Toggle<i32>,
    pub start_nudge: Toggle<f64>,
    pub trig_delay: Toggle<u32>,
    pub bpm: Option<f64>,       // this cue's source-tempo override
    pub clip_bpm: Option<f64>,  // the source clip's own BPM (the inherited value)
    pub bpm_sync_on: bool,
    pub speed_mul: Toggle<f64>,
    pub speed: f64,             // resolved effective playback speed (for the readout)
}

/// A bank's identity for the bank bar.
#[derive(Clone, Debug)]
pub struct BankView {
    pub name: Arc<str>,
    pub cue_count: usize,
}

/// A pool shader, as shown in the shader picker / cue editor. `builtin` entries
/// are bundled effects addressable by stable name (and persistable); non-builtin
/// entries are livecoded pins (runtime-only).
#[derive(Clone, Debug)]
pub struct ShaderPoolView {
    pub id: ShaderId,
    pub name: Arc<str>,
    pub builtin: bool,
    /// ISF input schema (min/max/default/labels) for the param editor; empty for
    /// non-ISF pool entries.
    pub inputs: Vec<crate::isf::IsfInput>,
}

/// Read-only display state the engine republishes each tick for the control UI.
#[derive(Clone, Debug, Default)]
pub struct UiMirror {
    pub bpm: f64,
    pub bpm_entry: Option<String>, // pending keyboard BPM entry, digits typed so far
    pub beat: f64,
    pub phase: f64, // 0..quantum
    pub quantum: f64,
    pub time_sig: TimeSig,
    pub phrase_cadence: Cadence,      // source-of-truth "next every" length
    pub loop_cadence: Option<Cadence>, // source-of-truth "loop every" length; None = EOF-only
    pub bar_in_phrase: u32,
    pub bars_per_phrase: u32,
    pub phrase_beats: f64,           // phrase_cadence resolved against time_sig, in beats
    pub loop_len: Option<u32>, // forced re-loop grid in 1/32-beat ticks; None = EOF-only
    pub preserve_playhead: bool, // carry the playhead over on a cut vs. restart the incoming clip
    pub advanced: bool, // advanced sequencer mode: per-cue timing/speed + extended UI
    pub sync: Option<SyncKind>,
    pub peers: u64,
    /// Whether the active clock source accepts tempo/phase edits. Link is
    /// listen-only, so its controls (BPM, nudge, tap, downbeat) grey out.
    pub can_set_tempo: bool,
    pub can_set_phase: bool,
    pub audio_devices: Vec<Arc<str>>, // device names; the name doubles as the selection key
    pub current_device: Option<Arc<str>>,
    pub audio_error: Option<String>,
    pub shader_name: Option<String>,
    pub shader_error: Option<Arc<str>>,
    pub clip_dir: Option<String>, // the active clip bank's source dir, for the header
    pub clip_banks: Vec<ClipBankView>,
    pub active_clip_bank: usize,
    pub clips: Vec<ClipEntry>, // the active clip bank's clips, in id order
    // Cue banks.
    pub banks: Vec<BankView>,
    pub live_bank: usize,
    pub edit_bank: usize,
    pub cues: Vec<CueView>, // the edit bank's cues, in order
    pub selected_cue: Option<CueId>,
    pub shader_pool: Vec<ShaderPoolView>, // pinned shaders a cue can override with
    pub playhead_sec: f64, // position of the currently displayed clip
    pub levels: [f32; 21],       // 21 perceptual log bands (native fftBand view)
    pub spectrum_linear: Vec<f32>, // 512 linear bins 0..1 — the iChannel0 FFT row
    pub level: f32,
    pub fullscreen: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_sig_quantum_and_bar_ticks() {
        let cases = [
            (TimeSig { num: 4, den: 4 }, 4.0, 128),
            (TimeSig { num: 7, den: 8 }, 3.5, 112),
            (TimeSig { num: 3, den: 4 }, 3.0, 96),
            (TimeSig { num: 5, den: 16 }, 1.25, 40),
            (TimeSig { num: 2, den: 1 }, 8.0, 256),
        ];
        for (ts, quantum, ticks) in cases {
            assert_eq!(ts.quantum(), quantum, "{ts:?} quantum");
            assert_eq!(ts.bar_ticks(), ticks, "{ts:?} bar_ticks");
        }
    }

    #[test]
    fn time_sig_sanitized_clamps() {
        assert_eq!(TimeSig { num: 0, den: 4 }.sanitized(), TimeSig { num: 1, den: 4 });
        assert_eq!(TimeSig { num: 4, den: 3 }.sanitized().den, 2);
        assert_eq!(TimeSig { num: 4, den: 32 }.sanitized().den, 16);
        assert_eq!(TimeSig { num: 4, den: 0 }.sanitized().den, 1);
    }

    #[test]
    fn cadence_note_is_signature_independent() {
        let sig_4_4 = TimeSig { num: 4, den: 4 };
        let sig_7_8 = TimeSig { num: 7, den: 8 };
        assert_eq!(Cadence::Note(32).ticks(sig_4_4), 32);
        assert_eq!(Cadence::Note(32).ticks(sig_7_8), 32);
    }

    #[test]
    fn cadence_bars_tracks_signature() {
        let sig_7_8 = TimeSig { num: 7, den: 8 };
        assert_eq!(Cadence::Bars(2).ticks(sig_7_8), 224);
        assert_eq!(Cadence::Bars(2).beats(sig_7_8), 7.0);
    }
}

