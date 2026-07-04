//! Single-window application shell (M1): owns the winit event loop, drives the
//! per-frame engine tick (shader reload, video upload, audio+clock uniforms,
//! render), and handles output-window hotkeys. The control window and command
//! channel arrive in M2.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, Modifiers, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId};

use crate::analysis::{AudioFrame, NUM_BANDS};
use crate::audio::AudioCapture;
use crate::clock::{ClockSource, InternalClock};
use crate::gfx::Gfx;
use crate::render::{Globals, Renderer};
use crate::shader::lang_of;
use crate::shaderwatch::ShaderWatcher;
use crate::video::decoder::DecodeHandle;
use crate::video::frame::DecodedFrame;

const PHRASE_LEN: f64 = 16.0; // M1 default; configurable in M2
const SHADER_DEBOUNCE: Duration = Duration::from_millis(75);

pub struct Boot {
    pub shader_path: PathBuf,
    pub windowed: bool,
    pub monitor: Option<usize>,
    pub clock: InternalClock,
    pub audio_out: triple_buffer::Output<AudioFrame>,
    pub audio_capture: AudioCapture,
    pub audio_err_rx: crossbeam_channel::Receiver<cpal::Error>,
    /// Kept alive so the analysis thread never sees its control channel disconnect.
    pub audio_ctl_tx: crossbeam_channel::Sender<crate::analysis::AudioCtl>,
    pub decode: DecodeHandle,
}

pub struct App {
    boot: Boot,
    gfx: Option<Gfx>,
    renderer: Option<Renderer>,
    watcher: Option<ShaderWatcher>,

    start: Instant,
    dirty_at: Option<Instant>,
    modifiers: Modifiers,
    bpm_entry: Option<String>,
    fullscreen_applied: bool,
    video_mode: i32,
    frames: u64,
}

impl App {
    pub fn new(boot: Boot) -> Self {
        App {
            boot,
            gfx: None,
            renderer: None,
            watcher: None,
            start: Instant::now(),
            dirty_at: None,
            modifiers: Modifiers::default(),
            bpm_entry: None,
            fullscreen_applied: false,
            video_mode: 0,
            frames: 0,
        }
    }

    fn load_shader(&mut self) {
        let (Some(gfx), Some(renderer)) = (self.gfx.as_ref(), self.renderer.as_mut()) else {
            return;
        };
        let path = &self.boot.shader_path;
        match std::fs::read_to_string(path) {
            Ok(src) => {
                renderer.set_shader(&gfx.device, &src, lang_of(path));
                match renderer.shader_error() {
                    Some(e) => log::warn!("shader error:\n{e}"),
                    None => log::info!("shader loaded: {}", path.display()),
                }
            }
            Err(e) => log::warn!("cannot read shader {}: {e}", path.display()),
        }
    }

    /// One engine tick + render.
    fn tick_and_render(&mut self) {
        // 1. Shader hot-reload (debounced).
        if self.watcher.as_ref().is_some_and(|w| w.dirty()) {
            self.dirty_at = Some(Instant::now());
        }
        if self.dirty_at.is_some_and(|t| t.elapsed() >= SHADER_DEBOUNCE) {
            self.dirty_at = None;
            self.load_shader();
        }

        // 2. Upload the newest decoded frame (drop any backlog).
        let mut newest: Option<DecodedFrame> = None;
        while let Ok(f) = self.boot.decode.frames.try_recv() {
            newest = Some(f);
        }
        if let (Some(frame), Some(gfx), Some(renderer)) =
            (newest, self.gfx.as_ref(), self.renderer.as_mut())
        {
            self.video_mode = frame.pixels.video_mode();
            renderer.upload_frame(&gfx.device, &gfx.queue, &frame);
        }

        // 3. Audio + clock -> uniforms.
        let audio: AudioFrame = *self.boot.audio_out.read();
        let snap = self.boot.clock.snapshot();
        let (Some(gfx), Some(renderer)) = (self.gfx.as_ref(), self.renderer.as_ref()) else {
            return;
        };
        let mut g = Globals {
            resolution: [gfx.config.width as f32, gfx.config.height as f32],
            mouse: [0.0, 0.0],
            time: self.start.elapsed().as_secs_f32(),
            lvl: audio.level,
            beat: (snap.beat.rem_euclid(16384.0)) as f32,
            bar_phase: (snap.phase / snap.quantum) as f32,
            phrase_phase: (snap.beat.rem_euclid(PHRASE_LEN) / PHRASE_LEN) as f32,
            bpm: snap.bpm as f32,
            video_mode: self.video_mode,
            _pad0: 0.0,
            freqs: [[0.0; 4]; 6],
        };
        debug_assert_eq!(NUM_BANDS, 21);
        g.set_bands(&audio.bands);
        renderer.update_globals(&gfx.queue, &g);

        // 4. Render + present.
        if let Some(frame) = gfx.acquire() {
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = gfx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            renderer.render(&mut encoder, &view);
            gfx.queue.submit([encoder.finish()]);
            frame.present();
            self.frames += 1;
            if self.frames == 1 {
                log::info!("first frame presented");
            }
        }
    }

    fn apply_fullscreen(&mut self, event_loop: &ActiveEventLoop) {
        if self.boot.windowed {
            return;
        }
        let Some(gfx) = self.gfx.as_ref() else { return };
        let monitor = pick_monitor(event_loop, self.boot.monitor);
        gfx.window
            .set_fullscreen(Some(Fullscreen::Borderless(monitor)));
    }

    fn toggle_fullscreen(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gfx) = self.gfx.as_ref() else { return };
        if gfx.window.fullscreen().is_some() {
            gfx.window.set_fullscreen(None);
        } else {
            let monitor = pick_monitor(event_loop, self.boot.monitor);
            gfx.window
                .set_fullscreen(Some(Fullscreen::Borderless(monitor)));
        }
    }

    fn handle_key(&mut self, event_loop: &ActiveEventLoop, ev: &KeyEvent) {
        if ev.state != ElementState::Pressed {
            return;
        }
        match &ev.logical_key {
            Key::Character(c) => match c.as_str() {
                "t" if !ev.repeat => self.boot.clock.tap_downbeat(),
                "+" | "=" => {
                    let bpm = self.boot.clock.snapshot().bpm + 1.0;
                    self.boot.clock.set_bpm(bpm);
                }
                "-" => {
                    let bpm = self.boot.clock.snapshot().bpm - 1.0;
                    self.boot.clock.set_bpm(bpm);
                }
                "[" => self.boot.clock.nudge_bpm(-0.001),
                "]" => self.boot.clock.nudge_bpm(0.001),
                "f" if !ev.repeat => self.toggle_fullscreen(event_loop),
                "q" if self.modifiers.state().super_key() => event_loop.exit(),
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
                            self.boot.clock.set_bpm(b);
                            log::info!("bpm set to {b}");
                        }
                    }
                }
            }
            Key::Named(NamedKey::Escape) => self.bpm_entry = None,
            _ => {}
        }
    }
}

fn pick_monitor(event_loop: &ActiveEventLoop, index: Option<usize>) -> Option<MonitorHandle> {
    let monitors: Vec<MonitorHandle> = event_loop.available_monitors().collect();
    let primary = event_loop.primary_monitor();
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
        if self.gfx.is_some() {
            return;
        }
        let window = match event_loop.create_window(
            Window::default_attributes()
                .with_title("vidiotic")
                .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0)),
        ) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("create_window: {e}");
                event_loop.exit();
                return;
            }
        };
        let gfx = match Gfx::new(window) {
            Ok(g) => g,
            Err(e) => {
                log::error!("gpu init: {e:#}");
                event_loop.exit();
                return;
            }
        };
        let renderer = Renderer::new(&gfx.device, gfx.config.format);
        self.gfx = Some(gfx);
        self.renderer = Some(renderer);

        match ShaderWatcher::new(&self.boot.shader_path) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => log::warn!("shader watcher disabled: {e}"),
        }
        self.load_shader();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gfx) = self.gfx.as_mut() {
                    gfx.resize(size.width, size.height);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m,
            WindowEvent::KeyboardInput { event, .. } => self.handle_key(event_loop, &event),
            WindowEvent::RedrawRequested => {
                self.tick_and_render();
                if self.frames >= 1 && !self.fullscreen_applied {
                    self.fullscreen_applied = true;
                    self.apply_fullscreen(event_loop);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Drain audio errors (device unplug etc.) — surfaced properly in M2.
        while let Ok(e) = self.boot.audio_err_rx.try_recv() {
            log::warn!("audio stream error: {e}");
        }
        if let Some(gfx) = self.gfx.as_ref() {
            gfx.window.request_redraw(); // Fifo present paces the loop
        }
    }
}

pub fn run(boot: Boot) -> anyhow::Result<()> {
    let event_loop = winit::event_loop::EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(boot);
    event_loop.run_app(&mut app)?;
    Ok(())
}
