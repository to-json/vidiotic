//! The UI↔engine contract: `Command`s flow from the control UI (and async file
//! pickers) to the engine; the engine publishes a `UiMirror` snapshot the UI
//! reads. Keeping these in one place lets other input sources (keys today,
//! MIDI eventually) map onto the same commands.

use std::path::PathBuf;
use std::sync::Arc;

use crate::bank::CueId;

/// Identifies a source clip in the pool (its scan index).
pub type ClipId = u32;

/// A compiled shader pinned into the pool. A cue can reference one as an override.
pub type ShaderId = u32;

/// Resolution of the musical re-loop grid: 32 ticks per beat (quarter note), so
/// an eighth note is 16 ticks and a 4/4 bar is 128. Lets `SetLoopLen` stay an
/// integer while still expressing sub-beat divisions.
pub const LOOP_TICKS_PER_BEAT: u32 = 32;

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
    SetPhraseLen(u32),          // beats between auto-transitions to the next clip
    SetLoopLen(Option<u32>),    // forced video re-loop grid, in 1/32-beat ticks (LOOP_TICKS_PER_BEAT); None = loop on EOF only
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
    SetCueShader(CueId, Option<ShaderId>), // per-cue shader override; None = the live shader
    AddBank,
    SetLiveBank(usize),                // which bank the sequencer plays
    SetEditBank(usize),                // which bank the UI edits
    // Shader pool: pin the current live shader's last-good compile so a cue can
    // use it while you keep livecoding the main shader.
    CaptureShader,                     // pin the current live shader into the pool
    RemoveShader(ShaderId),            // drop a pinned shader (cues fall back to the live shader)
    SetClipDir(PathBuf),
    SetShaderPath(PathBuf),
    SetAudioDevice(Option<String>), // id key; None = default
    ToggleFullscreen,               // shell-intercepted
    Quit,
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
    pub shader: Option<ShaderId>, // per-cue shader override; None = the live shader
    pub role: ClipRole, // Playing/Armed if this cue is the live bank's current/next
    pub has_thumb: bool,
}

/// A bank's identity for the bank bar.
#[derive(Clone, Debug)]
pub struct BankView {
    pub name: Arc<str>,
    pub cue_count: usize,
}

/// A pinned shader in the pool, as shown in the shader picker / cue editor.
#[derive(Clone, Debug)]
pub struct ShaderPoolView {
    pub id: ShaderId,
    pub name: Arc<str>,
}

/// Read-only display state the engine republishes each tick for the control UI.
#[derive(Clone, Debug, Default)]
pub struct UiMirror {
    pub bpm: f64,
    pub bpm_entry: Option<String>, // pending keyboard BPM entry, digits typed so far
    pub beat: f64,
    pub phase: f64, // 0..quantum
    pub quantum: f64,
    pub bar_in_phrase: u32,
    pub bars_per_phrase: u32,
    pub phrase_len: u32,
    pub loop_len: Option<u32>, // forced re-loop grid in 1/32-beat ticks; None = EOF-only
    pub preserve_playhead: bool, // carry the playhead over on a cut vs. restart the incoming clip
    pub sync: Option<SyncKind>,
    pub peers: u64,
    pub audio_devices: Vec<Arc<str>>, // device names; the name doubles as the selection key
    pub current_device: Option<Arc<str>>,
    pub audio_error: Option<String>,
    pub shader_name: Option<String>,
    pub shader_error: Option<Arc<str>>,
    pub clip_dir: Option<String>,
    pub clips: Vec<ClipEntry>,
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

