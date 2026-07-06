//! Application shell: two windows (fullscreen output + egui control) on one
//! shared Device/Queue, an engine tick that drains UI commands, runs the
//! sequencer, manages per-clip decoders, and feeds audio/beat uniforms to the
//! output shader.

use std::collections::HashMap;
use std::path::PathBuf;
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
use crate::clippool::{self, Clip, Thumbnail};
use crate::clock::{BoundaryTracker, ClockSource, InternalClock, LinkClock};
use crate::commands::{
    BankView, ClipEntry, ClipId, ClipRole, Command, CueView, ShaderPoolView, SyncKind, UiMirror,
};
use crate::gfx::Graphics;
use crate::render::{Globals, Renderer};
use crate::sequencer::{Sequencer, SequencerEvent};
use crate::shader::lang_of;
use crate::shaderwatch::ShaderWatcher;
use crate::ui::EguiCtl;
use crate::video::decoder::{self, DecodeHandle};
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
    pub quantum: f64,
    pub phrase_len: u32,
    pub initial_sync: SyncKind,
    pub clip_dir: Option<PathBuf>,
    pub clips: Vec<Clip>,
    pub auto_active: Vec<ClipId>,
    pub thumb_rx: Option<Receiver<Thumbnail>>,
    pub audio_out: triple_buffer::Output<AudioFrame>,
    pub audio_capture: AudioCapture,
    pub audio_err_rx: Receiver<cpal::Error>,
    pub audio_ctl_tx: Sender<crate::analysis::AudioCtl>,
    pub host: cpal::Host,
    pub audio_devices: Vec<String>,
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
    quantum: f64,
    sequencer: Sequencer,
    clips: Vec<Clip>,
    clip_dir: Option<PathBuf>,
    thumb_rx: Option<Receiver<Thumbnail>>,
    // Cue banks: the sequencer plays `live_bank`; the UI edits `edit_bank` (they
    // can differ so you play one set while modifying another). Decoders and
    // `current` are keyed by cue, not clip.
    banks: Vec<Bank>,
    live_bank: usize,
    edit_bank: usize,
    selected_cue: Option<CueId>,
    next_cue_id: CueId,
    decoders: HashMap<CueId, DecodeHandle>,
    current: Option<CueId>,
    current_pts: f64, // playhead of the displayed clip, for set-in/out-to-playhead
    video_mode: i32,
    last_beat: f64,
    // musical re-loop: force the current clip back to its start on a beat grid,
    // measured in 1/32-beat ticks. None = let the clip loop on EOF only.
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
    audio_devices: Vec<String>,

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
    fullscreen: bool,
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
        let mut app = App {
            graphics: None,
            renderer: None,
            egui: None,
            clock: Box::new(InternalClock::new(boot.bpm, boot.quantum)),
            sync: SyncKind::Internal,
            quantum: boot.quantum,
            sequencer: Sequencer::new(boot.phrase_len as f64),
            clips: boot.clips,
            clip_dir: boot.clip_dir,
            thumb_rx: boot.thumb_rx,
            banks: vec![Bank::new("A")],
            live_bank: 0,
            edit_bank: 0,
            selected_cue: None,
            next_cue_id: 1,
            decoders: HashMap::new(),
            current: None,
            current_pts: 0.0,
            video_mode: 0,
            last_beat: 0.0,
            loop_len: None,
            loop_tracker: BoundaryTracker::new(),
            preserve_playhead: true,
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
            fullscreen: false,
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
        // a default full-length cue in the live bank.
        for id in boot.auto_active {
            app.toggle_clip_active(id, 0.0);
        }
        if boot.initial_sync == SyncKind::Link {
            app.set_sync_source(SyncKind::Link);
        }
        app
    }

    fn clip_path(&self, id: ClipId) -> Option<PathBuf> {
        self.clips.iter().find(|c| c.id == id).map(|c| c.path.clone())
    }

    fn clip_name(&self, id: ClipId) -> String {
        self.clips
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.clone())
            .unwrap_or_default()
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
        let Some(cue) = self.live_cue(id) else { return };
        let clip = cue.clip;
        let in_sec = cue.in_sec.max(0.0);
        let out_sec = cue.out_sec.filter(|&o| o > in_sec);
        if let Some(path) = self.clip_path(clip) {
            match decoder::spawn(path, in_sec, out_sec) {
                Ok(h) => {
                    self.decoders.insert(id, h);
                }
                Err(e) => log::error!("decode spawn for cue {id} (clip {clip}): {e:#}"),
            }
        }
    }

    /// Add a full-length cue for `clip` to the live bank if none exists there,
    /// else remove it. Keeps the sequencer's active set in step. (The quick pool
    /// path; finer control comes from the bank editor.)
    fn toggle_clip_active(&mut self, clip: ClipId, beat: f64) {
        let existing = self.banks[self.live_bank]
            .cues
            .iter()
            .position(|c| c.clip == clip);
        match existing {
            Some(pos) => {
                let cue_id = self.banks[self.live_bank].cues[pos].id;
                let ev = self.sequencer.toggle_active(cue_id, beat);
                self.banks[self.live_bank].cues.remove(pos);
                if self.selected_cue == Some(cue_id) {
                    self.selected_cue = None;
                }
                self.apply_seq_events(ev);
            }
            None => {
                let cue_id = self.alloc_cue_id();
                let name = self.clip_name(clip);
                self.banks[self.live_bank]
                    .cues
                    .push(Cue::new(cue_id, clip, name));
                let ev = self.sequencer.toggle_active(cue_id, beat);
                self.apply_seq_events(ev);
            }
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
            let ids = self.banks[self.live_bank].ids();
            let ev = self.sequencer.set_active_set(ids);
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

    fn set_live_bank(&mut self, i: usize) {
        if i >= self.banks.len() || i == self.live_bank {
            return;
        }
        self.live_bank = i;
        // The new bank takes over: keep playing the current cue if it happens to
        // still resolve, otherwise the sequencer advances into the new set at the
        // next arm window.
        let ids = self.banks[self.live_bank].ids();
        let ev = self.sequencer.set_active_set(ids);
        self.apply_seq_events(ev);
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
            match r.capture_current(&g.device, name) {
                Some(id) => log::info!("pinned shader {id}"),
                None => {
                    self.shader_pin_count -= 1;
                    log::warn!("no compiled shader to pin");
                }
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
                if cue.shader == Some(id) {
                    cue.shader = None;
                }
            }
        }
    }

    fn set_clip_dir(&mut self, dir: PathBuf) {
        let clips = clippool::scan(&dir);
        log::info!("clip pool: {} clips in {}", clips.len(), dir.display());
        self.thumb_rx = Some(clippool::spawn_thumbnailer(clips.clone()));
        self.clips = clips;
        self.clip_dir = Some(dir);
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

    fn set_sync_source(&mut self, kind: SyncKind) {
        if kind == self.sync {
            return;
        }
        let snap = self.clock.snapshot();
        self.clock = match kind {
            SyncKind::Internal => Box::new(InternalClock::from_snapshot(&snap)),
            SyncKind::Link => Box::new(LinkClock::new(snap.bpm, self.quantum)),
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
                let bpm = (60.0 / avg).clamp(20.0, 300.0);
                self.clock.set_bpm(bpm);
            }
        }
    }

    fn set_loop_len(&mut self, ticks: Option<u32>) {
        self.loop_len = ticks;
        self.loop_tracker.reset();
    }

    /// Hard-reset the beat grid to its origin (bar 1, beat 1, phrase 1) and
    /// re-prime the phrase/loop boundary trackers so nothing misfires on the
    /// backward jump.
    fn reset_clock(&mut self) {
        self.clock.reset();
        self.loop_tracker.reset();
        self.sequencer.reset_boundary();
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
            Command::ResetClock => self.reset_clock(),
            Command::SetSyncSource(kind) => self.set_sync_source(kind),
            Command::SetPhraseLen(n) => {
                let ev = self.sequencer.set_phrase_len(n);
                self.apply_seq_events(ev);
            }
            Command::SetLoopLen(beats) => self.set_loop_len(beats),
            Command::SetPreservePlayhead(on) => self.preserve_playhead = on,
            Command::ToggleClipActive(id) => self.toggle_clip_active(id, self.last_beat),
            Command::AddCue(clip) => self.add_cue(clip),
            Command::RemoveCue(id) => self.remove_cue(id),
            Command::SelectCue(id) => self.selected_cue = id,
            Command::SetCueIn(id, s) => {
                self.edit_cue(id, |c| { c.in_sec = s.max(0.0); normalize_cue_trim(c); })
            }
            Command::SetCueOut(id, s) => {
                self.edit_cue(id, |c| { c.out_sec = s; normalize_cue_trim(c); })
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
            Command::SetCueShader(id, s) => self.edit_cue(id, |c| c.shader = s),
            Command::CaptureShader => self.capture_shader(),
            Command::RemoveShader(id) => self.remove_shader(id),
            Command::AddBank => self.add_bank(),
            Command::SetLiveBank(i) => self.set_live_bank(i),
            Command::SetEditBank(i) => self.set_edit_bank(i),
            Command::SetClipDir(dir) => self.set_clip_dir(dir),
            Command::SetShaderPath(p) => {
                self.shader_path = p;
                self.watcher = ShaderWatcher::new(&self.shader_path).ok();
                self.load_shader();
            }
            Command::SetAudioDevice(name) => self.switch_audio_device(name),
            Command::ToggleFullscreen => self.toggle_fullscreen(),
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
        let ev = self.sequencer.tick(&snap);
        self.apply_seq_events(ev);

        // 5b. Musical re-loop: on each grid boundary of `loop_len` (in 1/32-beat
        // ticks), seek the current clip back to its start so it restarts on the beat.
        if let (Some(ticks), Some(cur)) = (self.loop_len, self.current) {
            let grid = ticks as f64 / crate::commands::LOOP_TICKS_PER_BEAT as f64;
            if snap.is_playing && self.loop_tracker.crossed(snap.beat, grid).is_some() {
                if let Some(h) = self.decoders.get(&cur) {
                    h.request_restart();
                }
            }
        } else {
            self.loop_tracker.reset();
        }

        // 6. Pull the newest frame from the current clip and upload it.
        if let Some(cur) = self.current {
            let mut newest: Option<DecodedFrame> = None;
            if let Some(h) = self.decoders.get(&cur) {
                while let Ok(f) = h.frames.try_recv() {
                    newest = Some(f);
                }
            }
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

        // 6b. Point the renderer at the playing cue's shader override (a pinned
        // pool shader), or back to the live shader when the cue has none.
        let override_shader = self
            .current
            .and_then(|c| self.live_cue(c))
            .and_then(|cue| cue.shader);
        if let Some(r) = self.renderer.as_mut() {
            r.set_active_shader(override_shader);
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
        self.mirror.beat = snap.beat;
        self.mirror.phase = snap.phase;
        self.mirror.quantum = snap.quantum;
        self.mirror.phrase_len = phrase as u32;
        self.mirror.loop_len = self.loop_len;
        self.mirror.preserve_playhead = self.preserve_playhead;
        self.mirror.bars_per_phrase = (phrase / 4.0) as u32;
        self.mirror.bar_in_phrase = (snap.beat.rem_euclid(phrase) / 4.0) as u32;
        self.mirror.sync = Some(self.sync);
        self.mirror.peers = self.clock.caps().peers;
        self.mirror.audio_devices = self
            .audio_devices
            .iter()
            .map(|n| (n.clone(), n.clone()))
            .collect();
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
            .map(str::to_string);
        self.mirror.clip_dir = self
            .clip_dir
            .as_ref()
            .map(|d| d.display().to_string());
        self.mirror.clips = self
            .clips
            .iter()
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
            .map(|r| {
                r.pool_view()
                    .into_iter()
                    .map(|(id, name)| ShaderPoolView { id, name })
                    .collect()
            })
            .unwrap_or_default();
        self.mirror.playhead_sec = self.current_pts;
        self.mirror.cues = self.banks[self.edit_bank]
            .cues
            .iter()
            .map(|c| CueView {
                id: c.id,
                clip: c.clip,
                name: c.name.clone(),
                in_sec: c.in_sec,
                out_sec: c.out_sec,
                preserve: c.preserve,
                shader: c.shader,
                role: if playing_cue == Some(c.id) {
                    ClipRole::Playing
                } else if armed_cue == Some(c.id) {
                    ClipRole::Armed
                } else {
                    ClipRole::None
                },
                has_thumb: has_thumb(c.clip),
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
        self.mirror.fullscreen = self.fullscreen;
    }

    fn render_output(&mut self) {
        let (Some(g), Some(r)) = (self.graphics.as_ref(), self.renderer.as_ref()) else {
            return;
        };
        if self.output_occluded {
            return;
        }
        if let Some(frame) = g.output.acquire(&g.device) {
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = g
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            r.render(&mut encoder, &view);
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
            self.fullscreen = true;
        }
    }

    fn toggle_fullscreen(&mut self) {
        if let Some(g) = self.graphics.as_ref() {
            if g.output.window.fullscreen().is_some() {
                g.output.window.set_fullscreen(None);
                self.fullscreen = false;
            } else {
                let monitor = pick_monitor_from_window(&g.output.window, self.monitor);
                g.output
                    .window
                    .set_fullscreen(Some(Fullscreen::Borderless(monitor)));
                self.fullscreen = true;
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
                "f" if !ev.repeat => {
                    let _ = tx.send(Command::ToggleFullscreen);
                }
                "q" if self.modifiers.state().super_key() => {
                    let _ = tx.send(Command::Quit);
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
                        if (20.0..=300.0).contains(&b) {
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

/// Keep stored trim consistent with the decoder's rule (`ensure_decoder` only
/// honors an out-point strictly after the in-point): collapse an out ≤ in to
/// "untrimmed" so the editor never shows a trim that playback ignores.
fn normalize_cue_trim(cue: &mut Cue) {
    if cue.out_sec.is_some_and(|o| o <= cue.in_sec) {
        cue.out_sec = None;
    }
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
        let make = |title: &str, w: f64, h: f64| {
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(title)
                    .with_inner_size(winit::dpi::LogicalSize::new(w, h)),
            )
        };
        let (output_win, control_win) = match (make("vidiotic output", 1280.0, 720.0), make("vidiotic control", 1000.0, 720.0)) {
            (Ok(o), Ok(c)) => (Arc::new(o), Arc::new(c)),
            _ => {
                log::error!("failed to create windows");
                event_loop.exit();
                return;
            }
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
            WindowEvent::ModifiersChanged(m) if is_output => self.modifiers = m,
            WindowEvent::KeyboardInput { event, .. } if is_output => self.handle_key(&event),
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
pub fn run(boot: Boot) -> anyhow::Result<()> {
    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(boot);
    event_loop.run_app(&mut app)?;
    Ok(())
}
