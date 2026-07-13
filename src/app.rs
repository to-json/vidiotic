//! Application shell: two windows (fullscreen output + egui control) on one
//! shared Device/Queue, an engine tick that drains UI commands, runs the
//! sequencer, manages per-clip decoders, and feeds audio/beat uniforms to the
//! output shader.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, Modifiers, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId};

use crate::analysis::AudioFrame;
use crate::audio::{self, AudioCapture};
use crate::bank::{Bank, Cue, CueId};
use crate::clippool::{self, Clip, ClipBank, Thumbnail};
use crate::clock::{BoundaryTracker, ClockSource, InternalClock, LinkClock};
use crate::commands::{
    BankView, Cadence, ChainSlot, ClipBankView, ClipEntry, ClipId, ClipRole, Command, CueParam,
    CueView, SlotRef, SyncKind, TimeSig, UiMirror, LOOP_TICKS_PER_BEAT,
};
use crate::gfx::Graphics;
use crate::render::{Globals, Renderer};
use crate::sequencer::{CueStep, Sequencer, SequencerEvent};
use crate::shader::lang_of;
use crate::shaderwatch::ShaderWatcher;
use crate::ui::EguiCtl;
use crate::video::decoder;
use crate::video::SourceHandle;
use crate::video::frame::DecodedFrame;

const SHADER_DEBOUNCE: Duration = Duration::from_millis(75);

/// Tap-tempo: a gap longer than this starts a fresh measurement, and at most
/// this many recent taps are averaged.
const TAP_TIMEOUT: Duration = Duration::from_millis(2000);
const TAP_MAX: usize = 8;

/// Everything assembled at startup (CLI args, clip pool, audio plumbing) that
/// `App::new` takes ownership of.
pub struct Boot {
    pub shader_path: PathBuf,
    pub windowed: bool,
    pub monitor: Option<usize>,
    pub bpm: f64,
    pub time_sig: TimeSig,
    pub phrase_cadence: Cadence,
    pub initial_sync: SyncKind,
    pub clips: Vec<Clip>,
    /// Named groupings over `clips`; at least one bank should cover the pool or
    /// the grid shows nothing. The non-project path passes a single bank.
    pub clip_banks: Vec<ClipBank>,
    /// Cue banks to seed the sequencer with; empty ⇒ one default bank "A".
    pub cue_banks: Vec<Bank>,
    pub auto_active: Vec<ClipId>,
    /// Probe metadata (`fps`/`frames`/`duration_sec`/`source`) for loaded clips,
    /// keyed by id. The runtime `Clip` drops these, so they are retained here to
    /// round-trip through a save. Empty for the non-project path.
    pub clip_meta: HashMap<ClipId, crate::project::ClipMeta>,
    /// The `.viproj` this session was loaded from, if any — the default target
    /// for an in-place save. `None` when started from `--clip`/`--clip-dir`.
    pub project_path: Option<PathBuf>,
    /// Session playback defaults (project load overrides the CLI defaults).
    pub preserve_playhead: bool,
    pub loop_cadence: Option<Cadence>,
    pub advanced: bool,
    pub thumb_rx: Option<Receiver<Thumbnail>>,
    pub audio_out: triple_buffer::Output<AudioFrame>,
    pub audio_capture: AudioCapture,
    pub audio_err_rx: Receiver<cpal::Error>,
    pub audio_ctl_tx: Sender<crate::analysis::AudioCtl>,
    pub host: cpal::Host,
    pub audio_devices: Vec<Arc<str>>,
    pub cmd_tx: Sender<Command>,
    pub cmd_rx: Receiver<Command>,
}

/// The engine: owns the clock, sequencer, banks, decoders, renderer, and both
/// windows, and advances them all from one `update()` tick per event-loop pass.
pub struct App {
    // GPU / windows
    graphics: Option<Graphics>,
    renderer: Option<Renderer>,
    egui: Option<EguiCtl>,

    // engine state
    clock: Box<dyn ClockSource>,
    sync: SyncKind,
    time_sig: TimeSig,
    // Source-of-truth cadences: re-resolved against `time_sig` into the
    // sequencer's dwell / `loop_len` on every mutation (see `apply_cadences`).
    phrase_cadence: Cadence,
    loop_cadence: Option<Cadence>,
    sequencer: Sequencer,
    clips: Vec<Clip>,
    // Clip banks group the flat pool for the UI; `ClipId`s stay globally unique
    // so cues are unaffected. `active_clip_bank` is the one the grid shows.
    clip_banks: Vec<ClipBank>,
    active_clip_bank: usize,
    next_clip_id: ClipId,
    // Retained load-time clip probe metadata + the source `.viproj`, so a save can
    // round-trip data the runtime `Clip` doesn't hold and default to writing back
    // where the session was loaded from.
    clip_meta: HashMap<ClipId, crate::project::ClipMeta>,
    project_path: Option<PathBuf>,
    thumb_rx: Option<Receiver<Thumbnail>>,
    // Cue banks: the sequencer plays `live_bank`; the UI edits `edit_bank` (they
    // can differ so you play one set while modifying another). Decoders and
    // `current` are keyed by cue, not clip.
    banks: Vec<Bank>,
    live_bank: usize,
    edit_bank: usize,
    selected_cue: Option<CueId>,
    next_cue_id: CueId,
    decoders: HashMap<CueId, SourceHandle>,
    current: Option<CueId>,
    current_pts: f64, // playhead of the displayed clip, for set-in/out-to-playhead
    video_mode: i32,
    last_beat: f64,
    last_bpm: f64, // most recent snapshot tempo, for spawn-time BPM-synced speed
    // Advanced sequencer mode: when on, per-cue dwell/loop/offset/speed take
    // effect and the extended UI shows. Off (default) reproduces the simple
    // global-phrase behavior; per-cue edits are still stored, just inert.
    advanced: bool,
    // musical re-loop: force the current clip back to its start on a beat grid,
    // measured in 1/32-beat ticks — `loop_cadence` resolved against `time_sig`
    // by `apply_cadences`. None = let the clip loop on EOF only. In advanced
    // mode a per-cue rate/phase can override this for the playing cue.
    loop_len: Option<u32>,
    loop_tracker: BoundaryTracker,
    // on a cut, carry the outgoing playhead into the incoming clip (true, the
    // default — the armed decoder has been running since arm time so it cuts in
    // already advanced) or restart the incoming clip from its start (false).
    preserve_playhead: bool,
    // traditional tap-tempo: recent tap instants, averaged into a BPM.
    tap_times: Vec<Instant>,

    // audio
    audio_out: triple_buffer::Output<AudioFrame>,
    audio_capture: AudioCapture,
    audio_err_rx: Receiver<cpal::Error>,
    audio_ctl_tx: Sender<crate::analysis::AudioCtl>,
    host: cpal::Host,
    audio_devices: Vec<Arc<str>>,

    // shader
    shader_path: PathBuf,
    watcher: Option<ShaderWatcher>,
    dirty_at: Option<Instant>,
    // count of shaders pinned into the pool, for naming ("<stem> #N")
    shader_pin_count: u32,

    // ui plumbing
    cmd_tx: Sender<Command>,
    cmd_rx: Receiver<Command>,
    mirror: UiMirror,

    // window/input
    windowed: bool,
    monitor: Option<usize>,
    fullscreen_applied: bool,
    start: Instant,
    modifiers: Modifiers,
    bpm_entry: Option<String>,
    should_quit: bool,
    output_id: Option<WindowId>,
    control_id: Option<WindowId>,
    // While occluded (screen locked/asleep, window covered/minimized), the
    // compositor never hands back a drawable, so `get_current_texture()`
    // returns instantly instead of blocking on vsync. Poll-driving redraw
    // requests in that state spins the render loop at native CPU speed and
    // leaks GPU-side surface resources. Skip drawing entirely while occluded
    // instead.
    output_occluded: bool,
    control_occluded: bool,
}

impl App {
    pub fn new(boot: Boot) -> Self {
        // A loaded project seeds cue banks; otherwise start with one empty "A".
        let seeded = !boot.cue_banks.is_empty();
        let next_clip_id = boot.clips.iter().map(|c| c.id).max().map_or(0, |m| m + 1);
        let cue_banks = if seeded { boot.cue_banks } else { vec![Bank::new("A")] };
        let next_cue_id = cue_banks.iter().flat_map(Bank::ids).max().map_or(1, |m| m + 1);
        let mut app = Self {
            graphics: None,
            renderer: None,
            egui: None,
            clock: Box::new(InternalClock::new(boot.bpm, boot.time_sig.quantum())),
            sync: SyncKind::Internal,
            time_sig: boot.time_sig,
            phrase_cadence: boot.phrase_cadence,
            loop_cadence: boot.loop_cadence,
            sequencer: Sequencer::new(boot.phrase_cadence.beats(boot.time_sig)),
            clips: boot.clips,
            clip_banks: boot.clip_banks,
            active_clip_bank: 0,
            next_clip_id,
            clip_meta: boot.clip_meta,
            project_path: boot.project_path,
            thumb_rx: boot.thumb_rx,
            banks: cue_banks,
            live_bank: 0,
            edit_bank: 0,
            selected_cue: None,
            next_cue_id,
            decoders: HashMap::new(),
            current: None,
            current_pts: 0.0,
            video_mode: 0,
            last_beat: 0.0,
            last_bpm: boot.bpm,
            advanced: boot.advanced,
            loop_len: boot.loop_cadence.map(|c| c.ticks(boot.time_sig)),
            loop_tracker: BoundaryTracker::new(),
            preserve_playhead: boot.preserve_playhead,
            tap_times: Vec::new(),
            audio_out: boot.audio_out,
            audio_capture: boot.audio_capture,
            audio_err_rx: boot.audio_err_rx,
            audio_ctl_tx: boot.audio_ctl_tx,
            host: boot.host,
            audio_devices: boot.audio_devices,
            shader_path: boot.shader_path,
            watcher: None,
            dirty_at: None,
            shader_pin_count: 0,
            cmd_tx: boot.cmd_tx,
            cmd_rx: boot.cmd_rx,
            mirror: UiMirror::default(),
            windowed: boot.windowed,
            monitor: boot.monitor,
            fullscreen_applied: false,
            start: Instant::now(),
            modifiers: Modifiers::default(),
            bpm_entry: None,
            should_quit: false,
            output_id: None,
            control_id: None,
            output_occluded: false,
            control_occluded: false,
        };
        // Activate any clips requested at startup (e.g. from --clip): each becomes
        // a default full-length cue in the live bank. Skipped when a project
        // already seeded cue banks.
        if !seeded {
            for id in boot.auto_active {
                app.toggle_clip_active(id, 0.0);
            }
        }
        if boot.initial_sync == SyncKind::Link {
            app.set_sync_source(SyncKind::Link);
        }
        app
    }

    fn clip_path(&self, id: ClipId) -> Option<PathBuf> {
        self.clips.iter().find(|c| c.id == id).map(|c| c.path.clone())
    }

    fn clip_name(&self, id: ClipId) -> Arc<str> {
        self.clips
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    }

    /// Assemble the current session into a `Project` and write it to `path`. A
    /// failed write is logged, never fatal — losing a save must not kill the set.
    /// `SessionDefaults` are gathered from the live clock/sequencer here; the
    /// spec assembly itself lives in the shared [`crate::project`] module.
    fn save_project_to(&mut self, path: &Path) {
        use crate::project::{self, Project, SessionDefaults, SyncSpec};
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let defaults = SessionDefaults {
            bpm: self.clock.snapshot().bpm,
            quantum: self.time_sig.quantum(),
            phrase_len: self.sequencer.phrase_len().round() as u32,
            sync: match self.sync {
                SyncKind::Internal => SyncSpec::Internal,
                SyncKind::Link => SyncSpec::Link,
            },
            preserve_playhead: self.preserve_playhead,
            loop_len: self.loop_len,
            advanced: self.advanced,
            ts_num: self.time_sig.num,
            ts_den: self.time_sig.den,
            phrase_cadence: Some(self.phrase_cadence.into()),
            loop_cadence_set: true,
            loop_cadence: self.loop_cadence.map(Into::into),
            // Absolutize like clip paths: a CWD-relative `--shader` would resolve
            // against the save dir on load and be lost.
            shader_path: Some(project::relativize(dir, &project::absolutize(&self.shader_path))),
        };
        let proj = Project::from_runtime(
            dir,
            &self.clips,
            &self.clip_banks,
            &self.banks,
            &self.clip_meta,
            defaults,
        );
        match project::save(&proj, path) {
            Ok(()) => log::info!("saved project to {}", path.display()),
            Err(e) => log::error!("failed to save project to {}: {e:#}", path.display()),
        }
    }

    /// The cue with `id`, looked up in the live bank.
    fn live_cue(&self, id: CueId) -> Option<&Cue> {
        self.banks.get(self.live_bank).and_then(|b| b.cue(id))
    }

    fn alloc_cue_id(&mut self) -> CueId {
        let id = self.next_cue_id;
        self.next_cue_id += 1;
        id
    }

    fn ensure_decoder(&mut self, id: CueId) {
        if self.decoders.contains_key(&id) {
            return;
        }
        let Some(cue) = self.live_cue(id).cloned() else { return };
        let clip = cue.clip;
        // Advanced mode: sample-start nudge shifts the in-point, and playback
        // speed (BPM-sync × user multiplier) is baked in at spawn time.
        let nudge = if self.advanced && cue.start_nudge.on { cue.start_nudge.val } else { 0.0 };
        let in_sec = (cue.in_sec + nudge).max(0.0);
        let out_sec = cue.out_sec.filter(|&o| o > in_sec);
        let speed = self.effective_speed(&cue);
        if let Some(path) = self.clip_path(clip) {
            match decoder::spawn(path, in_sec, out_sec, speed) {
                Ok(h) => {
                    self.decoders.insert(id, SourceHandle::File(h));
                }
                Err(e) => log::error!("decode spawn for cue {id} (clip {clip}): {e:#}"),
            }
        }
    }

    /// This clip's source-tempo metadata, if set.
    fn clip_bpm(&self, id: ClipId) -> Option<f64> {
        self.clips.iter().find(|c| c.id == id).and_then(|c| c.bpm)
    }

    /// Effective playback speed for a cue: `1.0` unless advanced mode is on.
    fn effective_speed(&self, cue: &Cue) -> f64 {
        resolve_speed(self.advanced, self.last_bpm, cue, self.clip_bpm(cue.clip))
    }

    /// The sequencer timing a cue contributes, resolving inherited dwell against
    /// the global phrase length. In simple mode every cue uses the global dwell
    /// with no trig delay, reproducing a fixed phrase grid.
    fn step_for(&self, cue: &Cue) -> CueStep {
        let tpb = LOOP_TICKS_PER_BEAT as f64;
        let default = self.sequencer.phrase_len();
        if self.advanced {
            CueStep {
                id: cue.id,
                dwell: cue.dwell.map(|t| t as f64 / tpb).unwrap_or(default),
                trig_delay: if cue.trig_delay.on { cue.trig_delay.val as f64 / tpb } else { 0.0 },
            }
        } else {
            CueStep { id: cue.id, dwell: default, trig_delay: 0.0 }
        }
    }

    /// The [`CueStep`]s for a bank's cues, in play order.
    fn cue_steps(&self, bank: usize) -> Vec<CueStep> {
        self.banks
            .get(bank)
            .map(|b| b.cues.iter().map(|c| self.step_for(c)).collect())
            .unwrap_or_default()
    }

    /// The re-loop grid (ticks) and phase offset (beats) for the playing cue:
    /// per-cue in advanced mode, else the global loop setting.
    fn current_loop_params(&self) -> (Option<u32>, f64) {
        let global = self.loop_len;
        if !self.advanced {
            return (global, 0.0);
        }
        let Some(cue) = self.current.and_then(|c| self.live_cue(c)) else {
            return (global, 0.0);
        };
        let ticks = match cue.loop_len {
            Some(0) => None,      // per-cue: force no re-loop
            Some(t) => Some(t),   // per-cue rate
            None => global,       // inherit the global loop setting
        };
        let phase = if cue.loop_phase.on {
            cue.loop_phase.val as f64 / LOOP_TICKS_PER_BEAT as f64
        } else {
            0.0
        };
        (ticks, phase)
    }

    /// Add a full-length cue for `clip` to the live bank if none exists there,
    /// else remove it. Keeps the sequencer's active set in step. (The quick pool
    /// path; finer control comes from the bank editor.)
    fn toggle_clip_active(&mut self, clip: ClipId, beat: f64) {
        let existing = self.banks[self.live_bank]
            .cues
            .iter()
            .position(|c| c.clip == clip);
        if let Some(pos) = existing {
            let cue = self.banks[self.live_bank].cues[pos].clone();
            let step = self.step_for(&cue);
            let ev = self.sequencer.toggle_active(step, beat);
            self.banks[self.live_bank].cues.remove(pos);
            if self.selected_cue == Some(cue.id) {
                self.selected_cue = None;
            }
            self.apply_seq_events(ev);
        } else {
            let cue_id = self.alloc_cue_id();
            let name = self.clip_name(clip);
            let cue = Cue::new(cue_id, clip, name);
            let step = self.step_for(&cue);
            self.banks[self.live_bank].cues.push(cue);
            let ev = self.sequencer.toggle_active(step, beat);
            self.apply_seq_events(ev);
        }
    }

    /// Drop decoders that are neither playing nor armed.
    fn retain_decoders(&mut self) {
        let keep: Vec<CueId> = [self.current, self.sequencer.armed()]
            .into_iter()
            .flatten()
            .collect();
        self.decoders.retain(|k, _| keep.contains(k));
    }

    fn apply_seq_events(&mut self, events: Vec<SequencerEvent>) {
        for e in events {
            match e {
                SequencerEvent::ArmDecoder(c) => self.ensure_decoder(c),
                SequencerEvent::SwapTo(c) => {
                    self.ensure_decoder(c);
                    self.current = Some(c);
                    self.retain_decoders();
                    // With preserve off, the incoming clip should cut in from its
                    // in-point rather than the position it drifted to since arming.
                    // A cue's own `preserve` overrides the global default.
                    let preserve = self
                        .live_cue(c)
                        .and_then(|cue| cue.preserve)
                        .unwrap_or(self.preserve_playhead);
                    if !preserve {
                        if let Some(h) = self.decoders.get(&c) {
                            h.request_restart();
                        }
                    }
                    // A freshly swapped clip starts from the top; re-anchor the
                    // re-loop grid so it doesn't restart mid-clip immediately.
                    self.loop_tracker.reset();
                }
                SequencerEvent::DisarmDecoder => self.retain_decoders(),
            }
        }
    }

    /// If the edit bank is also live, rebuild the sequencer's active set from it
    /// (call after adding/removing a cue in the edit bank).
    fn resync_live_if_editing(&mut self) {
        if self.edit_bank == self.live_bank {
            let steps = self.cue_steps(self.live_bank);
            let ev = self.sequencer.set_active_set(steps);
            self.apply_seq_events(ev);
        }
    }

    fn add_cue(&mut self, clip: ClipId) {
        let cue_id = self.alloc_cue_id();
        let name = self.clip_name(clip);
        self.banks[self.edit_bank]
            .cues
            .push(Cue::new(cue_id, clip, name));
        self.selected_cue = Some(cue_id);
        self.resync_live_if_editing();
    }

    fn remove_cue(&mut self, id: CueId) {
        let Some(pos) = self.banks[self.edit_bank].cues.iter().position(|c| c.id == id) else {
            return;
        };
        self.banks[self.edit_bank].cues.remove(pos);
        if self.selected_cue == Some(id) {
            self.selected_cue = None;
        }
        self.resync_live_if_editing();
    }

    /// Mutate a cue in the edit bank. Trim/preserve changes take effect on the
    /// cue's next decoder spawn (they are read at spawn / swap time).
    fn edit_cue(&mut self, id: CueId, f: impl FnOnce(&mut Cue)) {
        if let Some(cue) = self.banks[self.edit_bank].cue_mut(id) {
            f(cue);
        }
    }

    /// Apply one advanced per-cue knob to the edit bank. Dwell/trig-delay change
    /// the rotation's timing, so those refresh the sequencer's active set; the
    /// rest are read at the cue's next decoder spawn or loop tick.
    fn set_cue_param(&mut self, id: CueId, p: CueParam) {
        self.edit_cue(id, |c| match p {
            CueParam::Dwell(v) => c.dwell = v,
            CueParam::Loop(v) => c.loop_len = v,
            CueParam::LoopPhase(t) => c.loop_phase = t,
            CueParam::StartNudge(t) => c.start_nudge = t,
            CueParam::TrigDelay(t) => c.trig_delay = t,
            CueParam::Bpm(v) => c.bpm = v,
            CueParam::BpmSync(on) => c.bpm_sync_on = on,
            CueParam::SpeedMul(t) => c.speed_mul = t,
        });
        if matches!(p, CueParam::Dwell(_) | CueParam::TrigDelay(_)) {
            self.resync_live_if_editing();
        }
    }

    /// Reorder a cue within the edit bank to `target`, then re-sync the live set.
    fn move_cue(&mut self, id: CueId, target: usize) {
        let cues = &mut self.banks[self.edit_bank].cues;
        let Some(from) = cues.iter().position(|c| c.id == id) else {
            return;
        };
        let cue = cues.remove(from);
        let to = target.min(cues.len());
        cues.insert(to, cue);
        self.resync_live_if_editing();
    }

    /// Set (or clear) a source clip's tempo metadata.
    fn set_clip_bpm(&mut self, id: ClipId, bpm: Option<f64>) {
        if let Some(c) = self.clips.iter_mut().find(|c| c.id == id) {
            c.bpm = bpm.filter(|b| b.is_finite() && *b > 0.0);
        }
    }

    /// Toggle advanced sequencer mode. Per-cue timing resolution changes for the
    /// whole rotation, so rebuild the active set and re-prime the loop grid.
    fn set_advanced(&mut self, on: bool) {
        if self.advanced == on {
            return;
        }
        self.advanced = on;
        self.loop_tracker.reset();
        self.resync_live_if_editing();
    }

    fn set_live_bank(&mut self, i: usize) {
        if i >= self.banks.len() || i == self.live_bank {
            return;
        }
        self.live_bank = i;
        // The new bank takes over: keep playing the current cue if it happens to
        // still resolve, otherwise the sequencer advances into the new set at the
        // next arm window.
        let steps = self.cue_steps(self.live_bank);
        let ev = self.sequencer.set_active_set(steps);
        self.apply_seq_events(ev);
    }

    /// Step the live bank by `delta`, wrapping around. No-op with fewer than
    /// two banks. `set_live_bank` ignores a same-index target, so wrap is safe.
    fn cycle_live_bank(&mut self, delta: i32) {
        let n = self.banks.len();
        if n < 2 {
            return;
        }
        let next = (self.live_bank as i32 + delta).rem_euclid(n as i32) as usize;
        self.set_live_bank(next);
    }

    fn set_edit_bank(&mut self, i: usize) {
        if i >= self.banks.len() {
            return;
        }
        self.edit_bank = i;
        self.selected_cue = None;
    }

    fn add_bank(&mut self) {
        // Name banks A, B, C, … by count; past Z, suffix a number (A1, B1, …).
        let n = self.banks.len();
        let name = if n < 26 {
            ((b'A' + n as u8) as char).to_string()
        } else {
            format!("{}{}", (b'A' + (n % 26) as u8) as char, n / 26)
        };
        self.banks.push(Bank::new(name));
    }

    fn load_shader(&mut self) {
        let (Some(g), Some(r)) = (self.graphics.as_ref(), self.renderer.as_mut()) else {
            return;
        };
        match std::fs::read_to_string(&self.shader_path) {
            Ok(src) => {
                r.set_shader(&g.device, &src, lang_of(&self.shader_path));
                match r.shader_error() {
                    Some(e) => log::warn!("shader error:\n{e}"),
                    None => log::info!("shader loaded: {}", self.shader_path.display()),
                }
            }
            Err(e) => log::warn!("cannot read shader {}: {e}", self.shader_path.display()),
        }
    }

    /// Pin the current live shader's last-good compile into the renderer's pool,
    /// named after the shader file plus a running count.
    fn capture_shader(&mut self) {
        let Some(g) = self.graphics.as_ref() else { return };
        let stem = self
            .shader_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("shader")
            .to_string();
        self.shader_pin_count += 1;
        let name = format!("{stem} #{}", self.shader_pin_count);
        if let Some(r) = self.renderer.as_mut() {
            if let Some(id) = r.capture_current(&g.device, name) { log::info!("pinned shader {id}") } else {
                self.shader_pin_count -= 1;
                log::warn!("no compiled shader to pin");
            }
        }
    }

    /// Drop a pinned shader and clear any cue references to it (they fall back to
    /// the live shader).
    fn remove_shader(&mut self, id: crate::commands::ShaderId) {
        if let Some(r) = self.renderer.as_mut() {
            r.remove_pool_shader(id);
        }
        for bank in &mut self.banks {
            for cue in &mut bank.cues {
                cue.chain
                    .retain(|slot| slot.shader != crate::commands::SlotRef::Pinned(id));
            }
        }
    }

    /// Compile an ISF `.fs` into the shader pool and append it to the selected
    /// cue's effect chain. No-op (logged) if no cue is selected or the file can't
    /// be read/compiled.
    fn load_isf(&mut self, path: PathBuf) {
        let Some(cue) = self.selected_cue else {
            log::warn!("Load ISF: no cue selected");
            return;
        };
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("Load ISF: read {}: {e}", path.display());
                return;
            }
        };
        let name: Arc<str> = path.to_string_lossy().into();
        let result = if let (Some(g), Some(r)) = (self.graphics.as_ref(), self.renderer.as_mut()) {
            Some(r.load_isf(&g.device, &g.queue, name.clone(), &src))
        } else {
            None
        };
        match result {
            Some(Ok(_)) => {
                log::info!("loaded ISF {}", path.display());
                self.edit_cue(cue, |c| c.chain.push(ChainSlot::new(SlotRef::Isf(name))));
            }
            Some(Err(e)) => log::error!("Load ISF {}: {e}", path.display()),
            None => {}
        }
    }

    /// Compile every ISF shader referenced by a cue chain into the pool, so
    /// project-loaded `SlotRef::Isf` slots resolve. Called once the renderer
    /// exists; missing/broken files are logged and the slot renders as a no-op.
    fn load_referenced_isf(&mut self) {
        let mut paths: Vec<Arc<str>> = Vec::new();
        for bank in &self.banks {
            for cue in &bank.cues {
                for slot in &cue.chain {
                    if let SlotRef::Isf(p) = &slot.shader {
                        if !paths.iter().any(|q| q == p) {
                            paths.push(p.clone());
                        }
                    }
                }
            }
        }
        for p in paths {
            let src = match std::fs::read_to_string(p.as_ref()) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("ISF {p}: {e}");
                    continue;
                }
            };
            if let (Some(g), Some(r)) = (self.graphics.as_ref(), self.renderer.as_mut()) {
                if let Err(e) = r.load_isf(&g.device, &g.queue, p.clone(), &src) {
                    log::error!("ISF {p}: {e}");
                }
            }
        }
    }

    /// Replace the entire pool with a single clip bank scanned from `dir`. Cues
    /// referenced the old pool, so they are cleared. (The `＋` in the clip-bank
    /// bar appends instead — see [`Self::add_clip_dir_as_bank`].)
    fn set_clip_dir(&mut self, dir: PathBuf) {
        let clips = clippool::scan(&dir);
        log::info!("clip pool: {} clips in {}", clips.len(), dir.display());
        self.thumb_rx = Some(clippool::spawn_thumbnailer(clips.clone()));
        let clip_ids = clips.iter().map(|c| c.id).collect();
        self.next_clip_id = clips.len() as ClipId;
        self.clips = clips;
        let name = dir_bank_name(&dir);
        self.clip_banks = vec![ClipBank {
            name,
            dir: Some(dir),
            clip_ids,
        }];
        self.active_clip_bank = 0;
        self.decoders.clear();
        self.current = None;
        // Cues referenced clips from the old pool; start fresh.
        self.banks = vec![Bank::new("A")];
        self.live_bank = 0;
        self.edit_bank = 0;
        self.selected_cue = None;
        self.next_cue_id = 1;
        self.sequencer = Sequencer::new(self.sequencer.phrase_len());
        if let Some(egui) = self.egui.as_mut() {
            egui.clear_thumbnails();
        }
    }

    /// Append `dir` as a new clip bank, extending the flat pool with fresh global
    /// ids and thumbnailing only the added clips. Existing clips, cues, and
    /// thumbnails are untouched; the new bank becomes active.
    fn add_clip_dir_as_bank(&mut self, dir: PathBuf) {
        let new = clippool::scan_from(&dir, self.next_clip_id);
        if new.is_empty() {
            log::warn!("no clips found in {}", dir.display());
            return;
        }
        log::info!("clip bank: +{} clips from {}", new.len(), dir.display());
        let clip_ids: Vec<ClipId> = new.iter().map(|c| c.id).collect();
        self.next_clip_id = clip_ids.iter().max().map_or(self.next_clip_id, |m| m + 1);
        // A single thumb_rx is polled each tick; the new receiver carries only the
        // added clips, and already-cached thumbnails are kept (not cleared).
        self.thumb_rx = Some(clippool::spawn_thumbnailer(new.clone()));
        self.clips.extend(new);
        let name = dir_bank_name(&dir);
        self.clip_banks.push(ClipBank {
            name,
            dir: Some(dir),
            clip_ids,
        });
        self.active_clip_bank = self.clip_banks.len() - 1;
    }

    fn set_active_clip_bank(&mut self, i: usize) {
        if i < self.clip_banks.len() {
            self.active_clip_bank = i;
        }
    }

    fn set_sync_source(&mut self, kind: SyncKind) {
        if kind == self.sync {
            return;
        }
        let snap = self.clock.snapshot();
        self.clock = match kind {
            SyncKind::Internal => Box::new(InternalClock::from_snapshot(&snap)),
            SyncKind::Link => Box::new(LinkClock::new(snap.bpm, self.time_sig.quantum())),
        };
        self.sync = kind;
        self.sequencer.reset_boundary(); // beat numbering may jump on switch
        self.loop_tracker.reset();
        log::info!("sync source: {kind:?}");
    }

    fn switch_audio_device(&mut self, name: Option<String>) {
        let (err_tx, err_rx) = crossbeam_channel::bounded::<cpal::Error>(8);
        match audio::build_capture(
            &self.host,
            None,
            name.as_deref(),
            &self.audio_ctl_tx,
            err_tx,
        ) {
            Ok(cap) => {
                log::info!("audio switched to '{}'", cap.device_name);
                self.audio_capture = cap;
                self.audio_err_rx = err_rx;
            }
            Err(e) => log::warn!("audio device switch failed: {e:#}"),
        }
    }

    /// Derive BPM from the spacing of recent taps. A gap over `TAP_TIMEOUT`
    /// starts a fresh measurement; the last `TAP_MAX` taps are averaged.
    fn tap_tempo(&mut self) {
        let now = Instant::now();
        if self
            .tap_times
            .last()
            .is_some_and(|&last| now.duration_since(last) > TAP_TIMEOUT)
        {
            self.tap_times.clear();
        }
        self.tap_times.push(now);
        if self.tap_times.len() > TAP_MAX {
            let excess = self.tap_times.len() - TAP_MAX;
            self.tap_times.drain(0..excess);
        }
        if self.tap_times.len() >= 2 {
            let first = self.tap_times[0];
            let intervals = (self.tap_times.len() - 1) as f64;
            let avg = now.duration_since(first).as_secs_f64() / intervals;
            if avg > 0.0 {
                let bpm = (60.0 / avg).clamp(20.0, 1000.0);
                self.clock.set_bpm(bpm);
            }
        }
    }

    fn set_loop_len(&mut self, ticks: Option<u32>) {
        self.loop_len = ticks;
        self.loop_tracker.reset();
    }

    /// Re-resolve `phrase_cadence`/`loop_cadence` against the current
    /// `time_sig` and push the concrete lengths into the sequencer and the
    /// loop grid. Called after any edit to the cadences or the signature.
    fn apply_cadences(&mut self) {
        let ev = self.sequencer.set_phrase_len(self.phrase_cadence.beats(self.time_sig));
        self.apply_seq_events(ev);
        self.set_loop_len(self.loop_cadence.map(|c| c.ticks(self.time_sig)));
    }

    /// Reset the beat grid to its origin (bar 1, beat 1, phrase 1) and re-prime
    /// the phrase/loop boundary trackers so nothing misfires on the backward
    /// jump. Playlist position and playhead are untouched.
    fn soft_reset(&mut self) {
        self.clock.reset();
        self.loop_tracker.reset();
        self.sequencer.reset_boundary();
    }

    /// Soft reset, plus jump the playlist back to its first cue and restart
    /// that cue's playhead from its in-point — regardless of the preserve-
    /// playhead setting, since a hard reset means "start over".
    fn hard_reset(&mut self) {
        self.soft_reset();
        let ev = self.sequencer.reset_to_first();
        self.apply_seq_events(ev);
        if let Some(cur) = self.current {
            if let Some(h) = self.decoders.get(&cur) {
                h.request_restart();
            }
        }
    }

    fn apply_command(&mut self, cmd: Command) {
        match cmd {
            Command::SetBpm(b) => self.clock.set_bpm(b),
            Command::BpmDelta(d) => {
                let b = self.clock.snapshot().bpm + d;
                self.clock.set_bpm(b);
            }
            Command::NudgeBpm(r) => self.clock.nudge_bpm(r),
            Command::TapDownbeat => self.clock.tap_downbeat(),
            Command::TapTempo => self.tap_tempo(),
            Command::SoftReset => self.soft_reset(),
            Command::HardReset => self.hard_reset(),
            Command::SetSyncSource(kind) => self.set_sync_source(kind),
            Command::SetTimeSig(ts) => {
                self.time_sig = ts.sanitized();
                self.clock.set_quantum(self.time_sig.quantum());
                self.apply_cadences();
                self.sequencer.reset_boundary();
            }
            Command::SetPhraseCadence(c) => {
                self.phrase_cadence = c;
                self.apply_cadences();
            }
            Command::SetLoopCadence(c) => {
                self.loop_cadence = c;
                self.apply_cadences();
            }
            Command::SetPreservePlayhead(on) => self.preserve_playhead = on,
            Command::ToggleClipActive(id) => self.toggle_clip_active(id, self.last_beat),
            Command::AddCue(clip) => self.add_cue(clip),
            Command::RemoveCue(id) => self.remove_cue(id),
            Command::SelectCue(id) => self.selected_cue = id,
            Command::SetCueIn(id, s) => {
                self.edit_cue(id, |c| { c.in_sec = s.max(0.0); normalize_cue_trim(c); });
            }
            Command::SetCueOut(id, s) => {
                self.edit_cue(id, |c| { c.out_sec = s; normalize_cue_trim(c); });
            }
            Command::SetCueInToPlayhead(id) => {
                if self.current == Some(id) {
                    let p = self.current_pts.max(0.0);
                    self.edit_cue(id, |c| { c.in_sec = p; normalize_cue_trim(c); });
                }
            }
            Command::SetCueOutToPlayhead(id) => {
                if self.current == Some(id) {
                    let p = self.current_pts.max(0.0);
                    self.edit_cue(id, |c| { c.out_sec = Some(p); normalize_cue_trim(c); });
                }
            }
            Command::SetCuePreserve(id, v) => self.edit_cue(id, |c| c.preserve = v),
            Command::SetCueChain(id, chain) => self.edit_cue(id, |c| c.chain = chain),
            Command::SetChainParam { cue, slot, name, value } => {
                self.edit_cue(cue, |c| {
                    if let Some(s) = c.chain.get_mut(slot) {
                        s.set_param(name, value);
                    }
                });
            }
            Command::LoadIsf(path) => self.load_isf(path),
            Command::SetCueParam(id, p) => self.set_cue_param(id, p),
            Command::MoveCue(id, to) => self.move_cue(id, to),
            Command::SetClipBpm(id, bpm) => self.set_clip_bpm(id, bpm),
            Command::SetAdvancedMode(on) => self.set_advanced(on),
            Command::CaptureShader => self.capture_shader(),
            Command::RemoveShader(id) => self.remove_shader(id),
            Command::AddBank => self.add_bank(),
            Command::SetLiveBank(i) => self.set_live_bank(i),
            Command::CycleLiveBank(d) => self.cycle_live_bank(d),
            Command::SetEditBank(i) => self.set_edit_bank(i),
            Command::SetClipDir(dir) => self.set_clip_dir(dir),
            Command::AddClipDirAsBank(dir) => self.add_clip_dir_as_bank(dir),
            Command::SetActiveClipBank(i) => self.set_active_clip_bank(i),
            Command::SetShaderPath(p) => {
                self.shader_path = p;
                self.watcher = ShaderWatcher::new(&self.shader_path).ok();
                self.load_shader();
            }
            Command::SetAudioDevice(name) => self.switch_audio_device(name),
            Command::ToggleFullscreen => self.toggle_fullscreen(),
            Command::SaveProject => {
                if let Some(p) = self.project_path.clone() {
                    self.save_project_to(&p);
                } else {
                    crate::ui::pick_file(
                        self.cmd_tx.clone(),
                        crate::ui::PickKind::SaveProject(None),
                    );
                }
            }
            Command::SaveProjectAs => {
                crate::ui::pick_file(
                    self.cmd_tx.clone(),
                    crate::ui::PickKind::SaveProject(self.project_path.clone()),
                );
            }
            Command::SaveProjectTo(p) => {
                self.save_project_to(&p);
                self.project_path = Some(p);
            }
            Command::Quit => self.should_quit = true,
        }
    }

    fn update(&mut self, event_loop: &ActiveEventLoop) {
        // 1. Commands (from UI + async pickers + keys).
        let cmds: Vec<Command> = self.cmd_rx.try_iter().collect();
        for c in cmds {
            self.apply_command(c);
        }

        // 2. Thumbnails.
        if let Some(rx) = &self.thumb_rx {
            let thumbs: Vec<Thumbnail> = rx.try_iter().collect();
            if let Some(egui) = self.egui.as_mut() {
                for t in thumbs {
                    egui.set_thumbnail(t.id, t.w, t.h, &t.rgba);
                }
            }
        }

        // 3. Audio errors.
        while let Ok(e) = self.audio_err_rx.try_recv() {
            log::warn!("audio stream error: {e}");
            self.mirror.audio_error = Some(e.to_string());
        }

        // 4. Shader hot-reload (debounced).
        if self.watcher.as_ref().is_some_and(|w| w.dirty()) {
            self.dirty_at = Some(Instant::now());
        }
        if self.dirty_at.is_some_and(|t| t.elapsed() >= SHADER_DEBOUNCE) {
            self.dirty_at = None;
            self.load_shader();
        }

        // 5. Clock + sequencer.
        let snap = self.clock.snapshot();
        self.last_beat = snap.beat;
        self.last_bpm = snap.bpm;
        let ev = self.sequencer.tick(&snap);
        self.apply_seq_events(ev);

        // 5b. Musical re-loop: on each grid boundary (in 1/32-beat ticks), seek
        // the current clip back to its start so it restarts on the beat. The
        // rate/phase come from the playing cue in advanced mode, else the global
        // loop setting; a loop phase shifts the grid for swing/micro-timing.
        let (loop_ticks, loop_phase) = self.current_loop_params();
        if let (Some(ticks), Some(cur)) = (loop_ticks, self.current) {
            let grid = ticks as f64 / LOOP_TICKS_PER_BEAT as f64;
            if snap.is_playing && self.loop_tracker.crossed(snap.beat - loop_phase, grid).is_some() {
                if let Some(h) = self.decoders.get(&cur) {
                    h.request_restart();
                }
            }
        } else {
            self.loop_tracker.reset();
        }

        // 6. Pull the newest frame from the current source and upload it.
        if let Some(cur) = self.current {
            let newest: Option<DecodedFrame> = self
                .decoders
                .get_mut(&cur)
                .and_then(|h| h.poll_newest(Instant::now()));
            if let Some(frame) = newest {
                self.current_pts = frame.pts_sec;
                self.video_mode = frame.pixels.video_mode();
                // Skipped while occluded — nothing will ever present this
                // texture, and the write_texture call leaks GPU-side staging
                // memory when no frame is ever submitted to reclaim it.
                if !self.output_occluded {
                    if let (Some(g), Some(r)) = (self.graphics.as_ref(), self.renderer.as_mut()) {
                        r.upload_frame(&g.device, &g.queue, &frame);
                    }
                }
            }
        }

        // 6b. Point the renderer at the playing cue's effect chain, or an empty
        // chain (the live shader) when the cue has none.
        let chain = self
            .current
            .and_then(|c| self.live_cue(c))
            .map(|cue| cue.chain.clone())
            .unwrap_or_default();
        if let Some(r) = self.renderer.as_mut() {
            r.set_active_chain(chain);
        }

        // 7. Uniforms. Skipped while occluded: these are two GPU queue writes
        // (globals buffer + audio texture) that would otherwise fire on every
        // Poll-loop tick with no frame ever submitted to reclaim them, leaking
        // GPU-side staging memory.
        let audio: AudioFrame = *self.audio_out.read();
        if !self.output_occluded {
            if let (Some(g), Some(r)) = (self.graphics.as_ref(), self.renderer.as_ref()) {
                let phrase = self.sequencer.phrase_len();
                let mut globals = Globals {
                    resolution: [g.output.config.width as f32, g.output.config.height as f32],
                    mouse: [0.0, 0.0],
                    time: self.start.elapsed().as_secs_f32(),
                    lvl: audio.level,
                    beat: snap.beat.rem_euclid(16384.0) as f32,
                    bar_phase: (snap.phase / snap.quantum) as f32,
                    phrase_phase: (snap.beat.rem_euclid(phrase) / phrase) as f32,
                    bpm: snap.bpm as f32,
                    video_mode: self.video_mode,
                    _pad0: 0.0,
                    freqs: [[0.0; 4]; 6],
                };
                globals.set_bands(&audio.bands);
                r.update_globals(&g.queue, &globals);
                r.upload_audio(&g.queue, &audio.audio_tex);
            }
        }

        // 8. Publish the mirror for the control window.
        self.build_mirror(&snap, &audio);

        // 9. Redraw scheduling. Skip windows that are occluded (screen locked/
        // asleep, covered, minimized): the compositor has no drawable to hand
        // back, so `get_current_texture()` returns instantly instead of
        // blocking on vsync, and polling it in that state spins the loop at
        // full CPU speed leaking GPU-side surface resources.
        if let Some(g) = self.graphics.as_ref() {
            if !self.output_occluded {
                g.output.window.request_redraw();
            }
            // control repaints ~each tick while shown (cheap; it clears on occlusion)
            if !self.control_occluded {
                g.control.window.request_redraw();
            }
        }

        if self.should_quit {
            event_loop.exit();
        }

        // Nothing paces `ControlFlow::Poll` while both windows are occluded
        // (no vsync wait, no redraw request) — without this the loop free-spins
        // at raw CPU speed. A short sleep is a cheap, robust backstop regardless
        // of what future per-tick work might get added.
        if self.output_occluded && self.control_occluded {
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    fn build_mirror(&mut self, snap: &crate::clock::ClockSnapshot, audio: &AudioFrame) {
        let phrase = self.sequencer.phrase_len();
        // Resolve the playing/armed cues to their source clips so the pool grid
        // can mark them. `active` = the clip has a cue in the live bank.
        let armed = self.sequencer.armed();
        let live = &self.banks[self.live_bank];
        let active_clips: std::collections::HashSet<ClipId> =
            live.cues.iter().map(|c| c.clip).collect();
        let playing_clip = self.current.and_then(|cid| live.cue(cid)).map(|c| c.clip);
        let armed_clip = armed.and_then(|cid| live.cue(cid)).map(|c| c.clip);
        let has_thumb = |id: ClipId| self.egui.as_ref().is_some_and(|e| e.has_thumb(id));

        self.mirror.bpm = snap.bpm;
        self.mirror.bpm_entry = self.bpm_entry.clone();
        self.mirror.beat = snap.beat;
        self.mirror.phase = snap.phase;
        self.mirror.quantum = snap.quantum;
        self.mirror.time_sig = self.time_sig;
        self.mirror.phrase_cadence = self.phrase_cadence;
        self.mirror.loop_cadence = self.loop_cadence;
        self.mirror.phrase_beats = phrase;
        self.mirror.loop_len = self.loop_len;
        self.mirror.preserve_playhead = self.preserve_playhead;
        self.mirror.advanced = self.advanced;
        let q = snap.quantum.max(0.25);
        self.mirror.bars_per_phrase = (phrase / q).round().max(1.0) as u32;
        self.mirror.bar_in_phrase = (snap.beat.rem_euclid(phrase) / q) as u32;
        self.mirror.sync = Some(self.sync);
        let caps = self.clock.caps();
        self.mirror.peers = caps.peers;
        self.mirror.can_set_tempo = caps.can_set_tempo;
        self.mirror.can_set_phase = caps.can_set_phase;
        self.mirror.audio_devices = self.audio_devices.clone();
        self.mirror.current_device = Some(self.audio_capture.device_name.clone());
        self.mirror.shader_name = self
            .shader_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string);
        self.mirror.shader_error = self
            .renderer
            .as_ref()
            .and_then(|r| r.shader_error())
            .cloned();
        // The pool grid shows one clip bank at a time; the clip-bank bar lists
        // them all. Cues still resolve against the full flat pool (via ClipId),
        // so playing/armed marking works across banks.
        let active = self.active_clip_bank;
        let clip_ids: Vec<ClipId> = self
            .clip_banks
            .get(active)
            .map(|b| b.clip_ids.clone())
            .unwrap_or_default();
        self.mirror.clip_dir = self
            .clip_banks
            .get(active)
            .and_then(|b| b.dir.as_ref())
            .map(|d| d.display().to_string());
        self.mirror.clip_banks = self
            .clip_banks
            .iter()
            .map(|b| ClipBankView {
                name: b.name.clone(),
                clip_count: b.clip_ids.len(),
            })
            .collect();
        self.mirror.active_clip_bank = active;
        self.mirror.clips = clip_ids
            .iter()
            .filter_map(|&id| self.clips.iter().find(|c| c.id == id))
            .map(|c| ClipEntry {
                id: c.id,
                name: c.name.clone(),
                active: active_clips.contains(&c.id),
                role: if playing_clip == Some(c.id) {
                    ClipRole::Playing
                } else if armed_clip == Some(c.id) {
                    ClipRole::Armed
                } else {
                    ClipRole::None
                },
                has_thumb: has_thumb(c.id),
                bpm: c.bpm,
                bank: active,
            })
            .collect();
        // Cue banks: the bank bar, and the edit bank's cues (with live roles).
        let armed_cue = self.sequencer.armed();
        let playing_cue = self.current;
        self.mirror.banks = self
            .banks
            .iter()
            .map(|b| BankView {
                name: b.name.clone(),
                cue_count: b.cues.len(),
            })
            .collect();
        self.mirror.live_bank = self.live_bank;
        self.mirror.edit_bank = self.edit_bank;
        self.mirror.selected_cue = self.selected_cue;
        self.mirror.shader_pool = self
            .renderer
            .as_ref()
            .map(|r| r.pool_view())
            .unwrap_or_default();
        self.mirror.playhead_sec = self.current_pts;
        // Locals captured by the map so it borrows only disjoint fields of `self`
        // (banks/clips/egui) and can be assigned back into `self.mirror`.
        let advanced = self.advanced;
        let last_bpm = self.last_bpm;
        let clips = &self.clips;
        let clip_bpm = |id: ClipId| clips.iter().find(|c| c.id == id).and_then(|c| c.bpm);
        self.mirror.cues = self.banks[self.edit_bank]
            .cues
            .iter()
            .map(|c| {
                let clip_bpm = clip_bpm(c.clip);
                CueView {
                    id: c.id,
                    clip: c.clip,
                    name: c.name.clone(),
                    in_sec: c.in_sec,
                    out_sec: c.out_sec,
                    preserve: c.preserve,
                    chain: c.chain.clone(),
                    role: if playing_cue == Some(c.id) {
                        ClipRole::Playing
                    } else if armed_cue == Some(c.id) {
                        ClipRole::Armed
                    } else {
                        ClipRole::None
                    },
                    has_thumb: has_thumb(c.clip),
                    dwell: c.dwell,
                    loop_len: c.loop_len,
                    loop_phase: c.loop_phase,
                    start_nudge: c.start_nudge,
                    trig_delay: c.trig_delay,
                    bpm: c.bpm,
                    clip_bpm,
                    bpm_sync_on: c.bpm_sync_on,
                    speed_mul: c.speed_mul,
                    speed: resolve_speed(advanced, last_bpm, c, clip_bpm),
                }
            })
            .collect();

        self.mirror.levels = audio.bands;
        // The 512-bin linear FFT row of the iChannel0 texture, already 0..1.
        self.mirror.spectrum_linear.clear();
        self.mirror.spectrum_linear.extend(
            audio.audio_tex[..crate::analysis::AUDIO_TEX_W]
                .iter()
                .map(|&b| b as f32 / 255.0),
        );
        self.mirror.level = audio.level;
        self.mirror.fullscreen = self
            .graphics
            .as_ref()
            .is_some_and(|g| g.output.window.fullscreen().is_some());
    }

    fn render_output(&mut self) {
        let (Some(g), Some(r)) = (self.graphics.as_ref(), self.renderer.as_mut()) else {
            return;
        };
        if self.output_occluded {
            return;
        }
        let (w, h) = (g.output.config.width, g.output.config.height);
        if let Some(frame) = g.output.acquire(&g.device) {
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = g
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            r.render(&g.device, &g.queue, &mut encoder, &view, w, h);
            g.queue.submit([encoder.finish()]);
            frame.present();
        }
        if !self.fullscreen_applied {
            self.fullscreen_applied = true;
            self.apply_fullscreen_initial();
        }
    }

    fn render_control(&mut self) {
        let (Some(g), Some(egui)) = (self.graphics.as_ref(), self.egui.as_mut()) else {
            return;
        };
        if self.control_occluded {
            return;
        }
        egui.render(&g.device, &g.queue, &g.control, &self.mirror, &self.cmd_tx);
    }

    fn apply_fullscreen_initial(&mut self) {
        if self.windowed {
            return;
        }
        if let Some(g) = self.graphics.as_ref() {
            let monitor = pick_monitor_from_window(&g.output.window, self.monitor);
            g.output
                .window
                .set_fullscreen(Some(Fullscreen::Borderless(monitor)));
        }
    }

    fn toggle_fullscreen(&mut self) {
        if let Some(g) = self.graphics.as_ref() {
            if g.output.window.fullscreen().is_some() {
                g.output.window.set_fullscreen(None);
            } else {
                let monitor = pick_monitor_from_window(&g.output.window, self.monitor);
                g.output
                    .window
                    .set_fullscreen(Some(Fullscreen::Borderless(monitor)));
            }
        }
    }

    fn handle_key(&mut self, ev: &KeyEvent) {
        if ev.state != ElementState::Pressed {
            return;
        }
        let tx = &self.cmd_tx;
        match &ev.logical_key {
            Key::Character(c) => match c.as_str() {
                "t" if !ev.repeat => {
                    let _ = tx.send(Command::TapDownbeat);
                }
                "b" if !ev.repeat => {
                    let _ = tx.send(Command::TapTempo);
                }
                "+" | "=" => {
                    let _ = tx.send(Command::BpmDelta(1.0));
                }
                "-" => {
                    let _ = tx.send(Command::BpmDelta(-1.0));
                }
                "[" => {
                    let _ = tx.send(Command::NudgeBpm(-0.001));
                }
                "]" => {
                    let _ = tx.send(Command::NudgeBpm(0.001));
                }
                "r" if !ev.repeat => {
                    let _ = tx.send(Command::SoftReset);
                }
                "R" if !ev.repeat => {
                    let _ = tx.send(Command::HardReset);
                }
                "c" if !ev.repeat => {
                    let _ = tx.send(Command::CaptureShader);
                }
                "," if !ev.repeat => {
                    let _ = tx.send(Command::CycleLiveBank(-1));
                }
                "." if !ev.repeat => {
                    let _ = tx.send(Command::CycleLiveBank(1));
                }
                "f" if !ev.repeat => {
                    let _ = tx.send(Command::ToggleFullscreen);
                }
                "q" if self.modifiers.state().super_key() => {
                    let _ = tx.send(Command::Quit);
                }
                "s" if self.modifiers.state().super_key()
                    || self.modifiers.state().control_key() =>
                {
                    let _ = tx.send(Command::SaveProject);
                }
                d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                    let e = self.bpm_entry.get_or_insert_with(String::new);
                    if e.len() < 5 {
                        e.push_str(d);
                    }
                }
                _ => {}
            },
            Key::Named(NamedKey::Enter) => {
                if let Some(s) = self.bpm_entry.take() {
                    if let Ok(b) = s.parse::<f64>() {
                        if (20.0..=1000.0).contains(&b) {
                            let _ = self.cmd_tx.send(Command::SetBpm(b));
                        }
                    }
                }
            }
            Key::Named(NamedKey::Escape) => self.bpm_entry = None,
            _ => {}
        }
    }
}

/// Resolve a cue's effective playback speed: `1.0` in simple mode; in advanced
/// mode a BPM-sync factor (`session_bpm / source_bpm`, when synced and a source
/// tempo is known) stacked with the user multiplier, clamped to a sane range.
fn resolve_speed(advanced: bool, session_bpm: f64, cue: &Cue, clip_bpm: Option<f64>) -> f64 {
    if !advanced {
        return 1.0;
    }
    let sync = if cue.bpm_sync_on {
        match cue.bpm.or(clip_bpm) {
            Some(src) if src > 0.0 => session_bpm / src,
            _ => 1.0,
        }
    } else {
        1.0
    };
    let mul = if cue.speed_mul.on { cue.speed_mul.val } else { 1.0 };
    (sync * mul).clamp(0.05, 20.0)
}

/// Keep stored trim consistent with the decoder's rule (`ensure_decoder` only
/// honors an out-point strictly after the in-point): collapse an out ≤ in to
/// "untrimmed" so the editor never shows a trim that playback ignores.
fn normalize_cue_trim(cue: &mut Cue) {
    if cue.out_sec.is_some_and(|o| o <= cue.in_sec) {
        cue.out_sec = None;
    }
}

/// A clip bank's display name from its source directory (the folder's own name).
fn dir_bank_name(dir: &std::path::Path) -> std::sync::Arc<str> {
    dir.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("clips")
        .into()
}

fn pick_monitor_from_window(window: &Window, index: Option<usize>) -> Option<MonitorHandle> {
    let monitors: Vec<MonitorHandle> = window.available_monitors().collect();
    let primary = window.primary_monitor();
    match index {
        Some(i) => monitors.get(i).cloned(),
        None => monitors
            .iter()
            .find(|m| primary.as_ref() != Some(*m))
            .cloned()
            .or(primary),
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.graphics.is_some() {
            return;
        }
        let make = |title: &str, w: f64, h: f64, min_w: f64, min_h: f64| {
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(title)
                    .with_inner_size(winit::dpi::LogicalSize::new(w, h))
                    .with_min_inner_size(winit::dpi::LogicalSize::new(min_w, min_h)),
            )
        };
        // The control layout is designed to stack down to ~420 px wide;
        // below that, rows would clip rather than wrap.
        let (output_win, control_win) = if let (Ok(o), Ok(c)) = (make("vidiotic output", 1280.0, 720.0, 160.0, 90.0), make("vidiotic control", 1000.0, 720.0, 420.0, 480.0)) { (Arc::new(o), Arc::new(c)) } else {
            log::error!("failed to create windows");
            event_loop.exit();
            return;
        };
        self.output_id = Some(output_win.id());
        self.control_id = Some(control_win.id());
        let graphics = match Graphics::new(output_win, control_win) {
            Ok(g) => g,
            Err(e) => {
                log::error!("gpu init: {e:#}");
                event_loop.exit();
                return;
            }
        };
        let renderer = Renderer::new(&graphics.device, graphics.output.config.format);
        let egui = EguiCtl::new(
            &graphics.control.window,
            &graphics.device,
            graphics.control.config.format,
        );
        self.graphics = Some(graphics);
        self.renderer = Some(renderer);
        self.egui = Some(egui);
        self.watcher = ShaderWatcher::new(&self.shader_path).ok();
        self.load_shader();
        self.load_referenced_isf();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let is_control = self.control_id == Some(id);
        let is_output = self.output_id == Some(id);

        if is_control {
            if let (Some(g), Some(egui)) = (self.graphics.as_ref(), self.egui.as_mut()) {
                if !matches!(event, WindowEvent::RedrawRequested) {
                    let win = g.control.window.clone();
                    let (consumed, _repaint) = egui.on_window_event(&win, &event);
                    if consumed {
                        return;
                    }
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = self.graphics.as_mut() {
                    if is_output {
                        g.output.resize(&g.device, size.width, size.height);
                    } else if is_control {
                        g.control.resize(&g.device, size.width, size.height);
                    }
                }
            }
            // Keyboard shortcuts are honored from either window. Control-window
            // key events only reach here when egui didn't consume them above
            // (i.e. no text field is focused), so typing still wins.
            WindowEvent::ModifiersChanged(m) if is_output || is_control => self.modifiers = m,
            WindowEvent::KeyboardInput { event, .. } if is_output || is_control => {
                self.handle_key(&event)
            }
            WindowEvent::Occluded(occluded) if is_output => self.output_occluded = occluded,
            WindowEvent::Occluded(occluded) if is_control => self.control_occluded = occluded,
            WindowEvent::RedrawRequested if is_output => self.render_output(),
            WindowEvent::RedrawRequested if is_control => self.render_control(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.update(event_loop);
    }
}

/// Run the player until quit: builds the `App` and drives the winit event loop.
///
/// # Errors
/// Propagates failure to create or run the winit event loop.
pub fn run(boot: Boot) -> anyhow::Result<()> {
    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(boot);
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::Toggle;

    fn cue() -> Cue {
        Cue::new(1, 0, "c")
    }

    #[test]
    fn speed_is_unity_in_simple_mode() {
        let mut c = cue();
        c.bpm_sync_on = true;
        c.speed_mul = Toggle { on: true, val: 2.0 };
        // advanced = false: every knob is inert, playback is native speed.
        assert_eq!(resolve_speed(false, 140.0, &c, Some(70.0)), 1.0);
    }

    #[test]
    fn bpm_sync_uses_session_over_source() {
        let mut c = cue();
        c.bpm_sync_on = true;
        // clip authored at 70 bpm, session at 140 -> play twice as fast
        assert_eq!(resolve_speed(true, 140.0, &c, Some(70.0)), 2.0);
        // cue-level bpm overrides the clip's
        c.bpm = Some(140.0);
        assert_eq!(resolve_speed(true, 140.0, &c, Some(70.0)), 1.0);
    }

    #[test]
    fn bpm_sync_without_source_is_unity() {
        let mut c = cue();
        c.bpm_sync_on = true;
        assert_eq!(resolve_speed(true, 140.0, &c, None), 1.0);
    }

    #[test]
    fn sync_and_multiplier_stack() {
        let mut c = cue();
        c.bpm_sync_on = true;
        c.speed_mul = Toggle { on: true, val: 1.5 };
        // (140/70) * 1.5 = 3.0
        assert_eq!(resolve_speed(true, 140.0, &c, Some(70.0)), 3.0);
        // multiplier alone, no sync
        c.bpm_sync_on = false;
        assert_eq!(resolve_speed(true, 140.0, &c, Some(70.0)), 1.5);
    }
}
