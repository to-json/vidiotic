//! The UI↔engine contract: `Command`s flow from the control UI (and async file
//! pickers) to the engine; the engine publishes a `UiMirror` snapshot the UI
//! reads. Keeping these in one place lets MIDI (M4) map onto the same commands.

use std::path::PathBuf;

pub type ClipId = u32;

/// Resolution of the musical re-loop grid: 32 ticks per beat (quarter note), so
/// an eighth note is 16 ticks and a 4/4 bar is 128. Lets `SetLoopLen` stay an
/// integer while still expressing sub-beat divisions.
pub const LOOP_TICKS_PER_BEAT: u32 = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncKind {
    Internal,
    Link,
}

#[derive(Clone, Debug)]
pub enum Command {
    SetBpm(f64),
    BpmDelta(f64),   // ±1 from the +/- keys
    NudgeBpm(f64),   // ratio ±0.001 for the ±0.1% controls
    TapDownbeat,     // snap the downbeat phase to now (does not change tempo)
    TapTempo,        // derive BPM from the interval between successive taps
    SetSyncSource(SyncKind),
    SetPhraseLen(u32),          // beats between auto-transitions to the next clip
    SetLoopLen(Option<u32>),    // forced video re-loop grid, in 1/32-beat ticks (LOOP_TICKS_PER_BEAT); None = loop on EOF only
    ToggleClipActive(ClipId),
    SetClipDir(PathBuf),
    SetShaderPath(PathBuf),
    SetAudioDevice(Option<String>), // id key; None = default
    ToggleFullscreen,               // shell-intercepted
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipRole {
    None,
    Playing,
    Armed,
}

#[derive(Clone, Debug)]
pub struct ClipEntry {
    pub id: ClipId,
    pub name: String,
    pub active: bool,
    pub role: ClipRole,
    pub has_thumb: bool, // texture cached in the UI's thumbnail map
}

/// Read-only display state the engine republishes each tick for the control UI.
#[derive(Clone, Debug, Default)]
pub struct UiMirror {
    pub bpm: f64,
    pub beat: f64,
    pub phase: f64, // 0..quantum
    pub quantum: f64,
    pub bar_in_phrase: u32,
    pub bars_per_phrase: u32,
    pub phrase_len: u32,
    pub loop_len: Option<u32>, // forced re-loop grid in 1/32-beat ticks; None = EOF-only
    pub sync: Option<SyncKind>,
    pub peers: u64,
    pub audio_devices: Vec<(String, String)>, // (id key, name)
    pub current_device: Option<String>,
    pub audio_error: Option<String>,
    pub shader_name: Option<String>,
    pub shader_error: Option<String>,
    pub clip_dir: Option<String>,
    pub clips: Vec<ClipEntry>,
    pub levels: [f32; 21],
    pub level: f32,
    pub fullscreen: bool,
}

impl Default for SyncKind {
    fn default() -> Self {
        SyncKind::Internal
    }
}
