//! Application shell (M2): two windows (fullscreen output + egui control) on one
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
use crate::clippool::{self, Clip, Thumbnail};
use crate::clock::{ClockSource, InternalClock};
use crate::commands::{ClipEntry, ClipId, ClipRole, Command, SyncKind, UiMirror};
use crate::gfx::Graphics;
use crate::render::{Globals, Renderer};
use crate::sequencer::{Sequencer, SequencerEvent};
use crate::shader::lang_of;
use crate::shaderwatch::ShaderWatcher;
use crate::ui::EguiCtl;
use crate::video::decoder::{self, DecodeHandle};
use crate::video::frame::DecodedFrame;

const SHADER_DEBOUNCE: Duration = Duration::from_millis(75);

pub struct Boot {
    pub shader_path: PathBuf,
    pub windowed: bool,
    pub monitor: Option<usize>,
    pub bpm: f64,
    pub quantum: f64,
    pub phrase_len: u32,
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

pub struct App {
    // GPU / windows
    graphics: Option<Graphics>,
    renderer: Option<Renderer>,
    egui: Option<EguiCtl>,

    // engine state
    clock: InternalClock,
    sequencer: Sequencer,
    clips: Vec<Clip>,
    clip_dir: Option<PathBuf>,
    thumb_rx: Option<Receiver<Thumbnail>>,
    decoders: HashMap<ClipId, DecodeHandle>,
    current: Option<ClipId>,
    video_mode: i32,
    last_beat: f64,

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
}

impl App {
    pub fn new(boot: Boot) -> Self {
        let mut app = App {
            graphics: None,
            renderer: None,
            egui: None,
            clock: InternalClock::new(boot.bpm, boot.quantum),
            sequencer: Sequencer::new(boot.phrase_len as f64),
            clips: boot.clips,
            clip_dir: boot.clip_dir,
            thumb_rx: boot.thumb_rx,
            decoders: HashMap::new(),
            current: None,
            video_mode: 0,
            last_beat: 0.0,
            audio_out: boot.audio_out,
            audio_capture: boot.audio_capture,
            audio_err_rx: boot.audio_err_rx,
            audio_ctl_tx: boot.audio_ctl_tx,
            host: boot.host,
            audio_devices: boot.audio_devices,
            shader_path: boot.shader_path,
            watcher: None,
            dirty_at: None,
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
        };
        // Activate any clips requested at startup (e.g. from --clip).
        for id in boot.auto_active {
            let ev = app.sequencer.toggle_active(id, 0.0);
            app.apply_seq_events(ev);
        }
        app
    }

    fn clip_path(&self, id: ClipId) -> Option<PathBuf> {
        self.clips.iter().find(|c| c.id == id).map(|c| c.path.clone())
    }

    fn ensure_decoder(&mut self, id: ClipId) {
        if self.decoders.contains_key(&id) {
            return;
        }
        if let Some(path) = self.clip_path(id) {
            match decoder::spawn(path) {
                Ok(h) => {
                    self.decoders.insert(id, h);
                }
                Err(e) => log::error!("decode spawn for clip {id}: {e:#}"),
            }
        }
    }

    /// Drop decoders that are neither playing nor armed.
    fn retain_decoders(&mut self) {
        let keep: Vec<ClipId> = [self.current, self.sequencer.armed()]
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
                }
                SequencerEvent::DisarmDecoder => self.retain_decoders(),
            }
        }
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

    fn set_clip_dir(&mut self, dir: PathBuf) {
        let clips = clippool::scan(&dir);
        log::info!("clip pool: {} clips in {}", clips.len(), dir.display());
        self.thumb_rx = Some(clippool::spawn_thumbnailer(clips.clone()));
        self.clips = clips;
        self.clip_dir = Some(dir);
        self.decoders.clear();
        self.current = None;
        self.sequencer = Sequencer::new(self.sequencer.phrase_len());
        if let Some(egui) = self.egui.as_mut() {
            egui.clear_thumbnails();
        }
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

    fn apply_command(&mut self, cmd: Command, event_loop: &ActiveEventLoop) {
        match cmd {
            Command::SetBpm(b) => self.clock.set_bpm(b),
            Command::BpmDelta(d) => {
                let b = self.clock.snapshot().bpm + d;
                self.clock.set_bpm(b);
            }
            Command::NudgeBpm(r) => self.clock.nudge_bpm(r),
            Command::TapDownbeat => self.clock.tap_downbeat(),
            Command::SetSyncSource(SyncKind::Internal) => {}
            Command::SetSyncSource(SyncKind::Link) => {
                log::info!("Link sync arrives in M3; staying on internal clock")
            }
            Command::SetPhraseLen(n) => {
                let ev = self.sequencer.set_phrase_len(n);
                self.apply_seq_events(ev);
            }
            Command::ToggleClipActive(id) => {
                let ev = self.sequencer.toggle_active(id, self.last_beat);
                self.apply_seq_events(ev);
            }
            Command::SetClipDir(dir) => self.set_clip_dir(dir),
            Command::SetShaderPath(p) => {
                self.shader_path = p;
                self.watcher = ShaderWatcher::new(&self.shader_path).ok();
                self.load_shader();
            }
            Command::SetAudioDevice(name) => self.switch_audio_device(name),
            Command::ToggleFullscreen => self.toggle_fullscreen(event_loop),
            Command::Quit => self.should_quit = true,
        }
    }

    fn update(&mut self, event_loop: &ActiveEventLoop) {
        // 1. Commands (from UI + async pickers + keys).
        let cmds: Vec<Command> = self.cmd_rx.try_iter().collect();
        for c in cmds {
            self.apply_command(c, event_loop);
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

        // 6. Pull the newest frame from the current clip and upload it.
        if let Some(cur) = self.current {
            let mut newest: Option<DecodedFrame> = None;
            if let Some(h) = self.decoders.get(&cur) {
                while let Ok(f) = h.frames.try_recv() {
                    newest = Some(f);
                }
            }
            if let (Some(frame), Some(g), Some(r)) =
                (newest, self.graphics.as_ref(), self.renderer.as_mut())
            {
                self.video_mode = frame.pixels.video_mode();
                r.upload_frame(&g.device, &g.queue, &frame);
            }
        }

        // 7. Uniforms.
        let audio: AudioFrame = *self.audio_out.read();
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
        }

        // 8. Publish the mirror for the control window.
        self.build_mirror(&snap, &audio);

        // 9. Redraw scheduling.
        if let Some(g) = self.graphics.as_ref() {
            g.output.window.request_redraw();
            let repaint_due = self
                .egui
                .as_ref()
                .and_then(|e| e.repaint_at)
                .is_some_and(|t| Instant::now() >= t);
            // control repaints ~each tick while shown (cheap; it clears on occlusion)
            g.control.window.request_redraw();
            let _ = repaint_due;
        }

        if self.should_quit {
            event_loop.exit();
        }
    }

    fn build_mirror(&mut self, snap: &crate::clock::ClockSnapshot, audio: &AudioFrame) {
        let phrase = self.sequencer.phrase_len();
        let playing = self.sequencer.playing();
        let armed = self.sequencer.armed();
        let has_thumb = |id: ClipId| self.egui.as_ref().is_some_and(|e| e.has_thumb(id));

        self.mirror.bpm = snap.bpm;
        self.mirror.beat = snap.beat;
        self.mirror.phase = snap.phase;
        self.mirror.quantum = snap.quantum;
        self.mirror.phrase_len = phrase as u32;
        self.mirror.bars_per_phrase = (phrase / 4.0) as u32;
        self.mirror.bar_in_phrase = (snap.beat.rem_euclid(phrase) / 4.0) as u32;
        self.mirror.sync = Some(SyncKind::Internal);
        self.mirror.peers = 0;
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
                active: self.sequencer.is_active(c.id),
                role: if playing == Some(c.id) {
                    ClipRole::Playing
                } else if armed == Some(c.id) {
                    ClipRole::Armed
                } else {
                    ClipRole::None
                },
                has_thumb: has_thumb(c.id),
            })
            .collect();
        self.mirror.levels = audio.bands;
        self.mirror.level = audio.level;
        self.mirror.fullscreen = self.fullscreen;
    }

    fn render_output(&mut self) {
        let (Some(g), Some(r)) = (self.graphics.as_ref(), self.renderer.as_ref()) else {
            return;
        };
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

    fn toggle_fullscreen(&mut self, _event_loop: &ActiveEventLoop) {
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
            WindowEvent::RedrawRequested if is_output => self.render_output(),
            WindowEvent::RedrawRequested if is_control => self.render_control(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.update(event_loop);
    }
}

pub fn run(boot: Boot) -> anyhow::Result<()> {
    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(boot);
    event_loop.run_app(&mut app)?;
    Ok(())
}
