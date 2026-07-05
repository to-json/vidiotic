//! Control window: egui integrated as a library (not eframe). Owns the egui
//! context, winit input translation, the wgpu paint renderer, and the cached
//! clip thumbnails. `control_ui` reads a `UiMirror` and emits `Command`s.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use winit::window::Window;

use crate::commands::{ClipEntry, ClipId, ClipRole, Command, CueView, SyncKind, UiMirror};
use crate::gfx::WindowSurface;

pub struct EguiCtl {
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    thumbs: HashMap<ClipId, egui::TextureHandle>,
    pub repaint_at: Option<Instant>,
}

impl EguiCtl {
    pub fn new(window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ctx = egui::Context::default();
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let renderer = egui_wgpu::Renderer::new(
            device,
            format,
            egui_wgpu::RendererOptions {
                msaa_samples: 1,
                ..Default::default()
            },
        );
        EguiCtl {
            ctx,
            state,
            renderer,
            thumbs: HashMap::new(),
            repaint_at: None,
        }
    }

    pub fn has_thumb(&self, id: ClipId) -> bool {
        self.thumbs.contains_key(&id)
    }

    pub fn clear_thumbnails(&mut self) {
        self.thumbs.clear();
    }

    /// Feed a window event to egui. Returns (consumed, repaint).
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> (bool, bool) {
        let r = self.state.on_window_event(window, event);
        (r.consumed, r.repaint)
    }

    /// Cache a thumbnail as an egui texture (called once per clip when decoded).
    pub fn set_thumbnail(&mut self, id: ClipId, w: usize, h: usize, rgba: &[u8]) {
        let image = egui::ColorImage::from_rgba_unmultiplied([w, h], rgba);
        let handle = self
            .ctx
            .load_texture(format!("thumb:{id}"), image, egui::TextureOptions::LINEAR);
        self.thumbs.insert(id, handle);
    }

    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ws: &WindowSurface,
        mirror: &UiMirror,
        cmd_tx: &Sender<Command>,
    ) {
        let raw_input = self.state.take_egui_input(&ws.window);
        let ctx = self.ctx.clone();
        let thumbs = &self.thumbs;
        let full = ctx.run_ui(raw_input, |ui| control_ui(ui, mirror, cmd_tx, thumbs));
        self.state
            .handle_platform_output(&ws.window, full.platform_output);

        for (id, delta) in &full.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }
        let prims = self.ctx.tessellate(full.shapes, full.pixels_per_point);
        let sd = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [ws.config.width, ws.config.height],
            pixels_per_point: full.pixels_per_point,
        };

        if let Some(frame) = ws.acquire(device) {
            let view = frame.texture.create_view(&Default::default());
            let mut encoder = device.create_command_encoder(&Default::default());
            let user_bufs = self
                .renderer
                .update_buffers(device, queue, &mut encoder, &prims, &sd);
            {
                let mut pass = encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("egui"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.02,
                                    g: 0.02,
                                    b: 0.03,
                                    a: 1.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    })
                    .forget_lifetime();
                self.renderer.render(&mut pass, &prims, &sd);
            }
            queue.submit(user_bufs.into_iter().chain([encoder.finish()]));
            frame.present();
        }
        for id in &full.textures_delta.free {
            self.renderer.free_texture(id);
        }

        let delay = full
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.repaint_delay)
            .unwrap_or(Duration::MAX);
        if delay.is_zero() {
            ws.window.request_redraw();
        } else if delay < Duration::from_secs(3600) {
            self.repaint_at = Some(Instant::now() + delay);
        }
    }
}

/// Bar-based cadence choices for the "Next every" (sequencer) grid.
const CADENCE_BARS: [u32; 5] = [1, 2, 4, 8, 16];

/// "Loop every" cadence: (label, ticks) at 32 ticks/beat (LOOP_TICKS_PER_BEAT).
/// A beat is a quarter note (32), so an eighth note is 16 and a 4/4 bar is 128.
/// Goes sub-bar for beat-roll/stutter effects.
const LOOP_CADENCE: [(&str, u32); 8] = [
    ("1/8", 16),
    ("1/4", 32),
    ("1/2", 64),
    ("1 bar", 128),
    ("2 bars", 256),
    ("4 bars", 512),
    ("8 bars", 1024),
    ("16 bars", 2048),
];

fn bars_label(bars: u32) -> String {
    if bars == 1 {
        "1 bar".to_string()
    } else {
        format!("{bars} bars")
    }
}

fn control_ui(
    ui: &mut egui::Ui,
    m: &UiMirror,
    tx: &Sender<Command>,
    thumbs: &HashMap<ClipId, egui::TextureHandle>,
) {
    egui::Panel::top(egui::Id::new("transport")).show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!("{:6.1}", m.bpm))
                    .monospace()
                    .size(40.0)
                    .strong(),
            );
            ui.vertical(|ui| {
                let mut bpm = m.bpm;
                if ui
                    .add(
                        egui::DragValue::new(&mut bpm)
                            .speed(0.1)
                            .range(20.0..=300.0)
                            .fixed_decimals(1),
                    )
                    .changed()
                {
                    let _ = tx.send(Command::SetBpm(bpm));
                }
                ui.horizontal(|ui| {
                    if ui.button("−0.1%").clicked() {
                        let _ = tx.send(Command::NudgeBpm(-0.001));
                    }
                    if ui.button("+0.1%").clicked() {
                        let _ = tx.send(Command::NudgeBpm(0.001));
                    }
                });
            });
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("DOWNBEAT").size(17.0))
                        .min_size(egui::vec2(80.0, 46.0)),
                )
                .on_hover_text("Snap the downbeat to now (phase only, nearest bar). Key: t / Space")
                .clicked()
                || ui.input(|i| i.key_pressed(egui::Key::Space))
            {
                let _ = tx.send(Command::TapDownbeat);
            }
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("RESET").size(18.0))
                        .min_size(egui::vec2(64.0, 46.0)),
                )
                .on_hover_text("Reset the clock to bar 1, beat 1 (phrase 1). Tempo unchanged.")
                .clicked()
            {
                let _ = tx.send(Command::ResetClock);
            }
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("TEMPO").size(20.0))
                        .min_size(egui::vec2(72.0, 46.0)),
                )
                .on_hover_text("Tap 2+ times to set BPM from the interval. Key: b")
                .clicked()
            {
                let _ = tx.send(Command::TapTempo);
            }
            ui.separator();
            ui.vertical(|ui| {
                ui.label(format!(
                    "bar {}/{}",
                    m.bar_in_phrase + 1,
                    m.bars_per_phrase.max(1)
                ));
                // beat pulse dot
                let pulse = 1.0 - (m.phase / m.quantum.max(1.0)) as f32;
                let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                let g = (0.25 + 0.75 * pulse).clamp(0.0, 1.0);
                ui.painter()
                    .circle_filled(r.center(), 7.0, egui::Color32::from_rgb(40, (g * 220.0) as u8, 90));
            });
            ui.separator();
            egui::ComboBox::from_id_salt("sync")
                .selected_text(match m.sync.unwrap_or(SyncKind::Internal) {
                    SyncKind::Internal => "Internal",
                    SyncKind::Link => "Link",
                })
                .show_ui(ui, |ui| {
                    let cur = m.sync.unwrap_or(SyncKind::Internal);
                    if ui.selectable_label(cur == SyncKind::Internal, "Internal").clicked() {
                        let _ = tx.send(Command::SetSyncSource(SyncKind::Internal));
                    }
                    if ui.selectable_label(cur == SyncKind::Link, "Link").clicked() {
                        let _ = tx.send(Command::SetSyncSource(SyncKind::Link));
                    }
                });
        });
        ui.add_space(4.0);
        // Cadence controls, in bars. `Next` = how often the sequencer advances to
        // the next active clip; `Loop` = how often the current clip restarts.
        ui.horizontal(|ui| {
            ui.label("Next every:")
                .on_hover_text("Beats between auto-transitions to the next active clip");
            for bars in CADENCE_BARS {
                let beats = bars * 4;
                if ui
                    .selectable_label(m.phrase_len == beats, bars_label(bars))
                    .clicked()
                {
                    let _ = tx.send(Command::SetPhraseLen(beats));
                }
            }
            ui.separator();
            ui.label("Loop every:")
                .on_hover_text("Force the current clip back to its start on this beat grid");
            if ui.selectable_label(m.loop_len.is_none(), "off").clicked() {
                let _ = tx.send(Command::SetLoopLen(None));
            }
            for (label, beats) in LOOP_CADENCE {
                if ui
                    .selectable_label(m.loop_len == Some(beats), label)
                    .clicked()
                {
                    let _ = tx.send(Command::SetLoopLen(Some(beats)));
                }
            }
            ui.separator();
            let mut preserve = m.preserve_playhead;
            if ui
                .checkbox(&mut preserve, "preserve playhead")
                .on_hover_text(
                    "On a cut, carry the playhead into the next clip (it comes in \
                     already running). Off: the next clip restarts from its start.",
                )
                .changed()
            {
                let _ = tx.send(Command::SetPreservePlayhead(preserve));
            }
        });
        ui.add_space(4.0);
    });

    egui::Panel::bottom(egui::Id::new("io")).show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Shader…").clicked() {
                pick_file(tx.clone(), PickKind::Shader);
            }
            ui.monospace(m.shader_name.as_deref().unwrap_or("<none>"));
            if ui
                .button("📌 Pin")
                .on_hover_text(
                    "Pin the current shader's last good compile into the pool so a \
                     cue can use it while you keep livecoding this one.",
                )
                .clicked()
            {
                let _ = tx.send(Command::CaptureShader);
            }
            ui.separator();
            egui::ComboBox::from_id_salt("audio")
                .selected_text(m.current_device.as_deref().unwrap_or("default"))
                .show_ui(ui, |ui| {
                    for (id, name) in &m.audio_devices {
                        if ui
                            .selectable_label(m.current_device.as_deref() == Some(id), name)
                            .clicked()
                        {
                            let _ = tx.send(Command::SetAudioDevice(Some(id.clone())));
                        }
                    }
                });
        });
        if !m.shader_pool.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.weak("Pinned:");
                for s in &m.shader_pool {
                    ui.label(egui::RichText::new(&s.name).small());
                    if ui
                        .small_button("✕")
                        .on_hover_text("remove this pinned shader")
                        .clicked()
                    {
                        let _ = tx.send(Command::RemoveShader(s.id));
                    }
                    ui.separator();
                }
            });
        }
        spectrum(ui, m);
        if let Some(err) = &m.shader_error {
            egui::ScrollArea::vertical()
                .id_salt("shader_err")
                .max_height(96.0)
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(err)
                            .monospace()
                            .color(egui::Color32::from_rgb(255, 90, 90)),
                    );
                });
        }
        ui.add_space(4.0);
    });

    // Cue editor: edits the selected cue of the edit bank.
    egui::Panel::right(egui::Id::new("cue_editor"))
        .resizable(true)
        .default_size(272.0)
        .min_size(210.0)
        .show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("Cue");
                ui.weak(format!("⏱ {}", fmt_time(m.playhead_sec)));
            });
            ui.separator();
            match m.selected_cue.and_then(|id| m.cues.iter().find(|c| c.id == id)) {
                Some(cue) => cue_editor(ui, m, cue, tx),
                None => {
                    ui.add_space(8.0);
                    ui.weak("No cue selected.");
                    ui.add_space(4.0);
                    ui.weak("Double-click a clip to add a cue to the edit bank, then click the cue to edit it here.");
                }
            }
        });

    egui::CentralPanel::default().show(ui, |ui| {
        // Source clip pool.
        ui.horizontal(|ui| {
            ui.heading("Clips");
            if ui.button("Folder…").clicked() {
                pick_file(tx.clone(), PickKind::ClipDir);
            }
            if let Some(d) = &m.clip_dir {
                ui.weak(d);
            }
        });
        ui.weak("double-click a clip to add it as a cue to the edit bank");
        egui::ScrollArea::vertical()
            .id_salt("clip_pool")
            .max_height(190.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for clip in &m.clips {
                        clip_tile(ui, clip, thumbs, tx);
                    }
                });
            });

        ui.separator();
        bank_bar(ui, m, tx);
        egui::ScrollArea::vertical()
            .id_salt("cue_list")
            .show(ui, |ui| {
                if m.cues.is_empty() {
                    ui.add_space(6.0);
                    ui.weak("Empty bank — double-click a clip above to add a cue.");
                }
                ui.horizontal_wrapped(|ui| {
                    for cue in &m.cues {
                        cue_chip(ui, m, cue, thumbs, tx);
                    }
                });
            });
    });
}

/// A source-clip tile in the pool. Double-click adds a cue to the edit bank.
fn clip_tile(
    ui: &mut egui::Ui,
    clip: &ClipEntry,
    thumbs: &HashMap<ClipId, egui::TextureHandle>,
    tx: &Sender<Command>,
) {
    ui.allocate_ui(egui::vec2(128.0, 104.0), |ui| {
        ui.vertical(|ui| {
            let resp = if let Some(tex) = thumbs.get(&clip.id) {
                let img = egui::Image::new((tex.id(), egui::vec2(120.0, 68.0)));
                ui.add(egui::Button::image(img).selected(clip.active))
            } else {
                ui.add_sized(
                    egui::vec2(120.0, 68.0),
                    egui::Button::new("…").selected(clip.active),
                )
            };
            let resp = resp.on_hover_text("double-click: add a cue to the edit bank");
            let marker = match clip.role {
                ClipRole::Playing => "▶ ",
                ClipRole::Armed => "○ ",
                ClipRole::None => "",
            };
            let color = match clip.role {
                ClipRole::Playing => egui::Color32::LIGHT_GREEN,
                ClipRole::Armed => egui::Color32::YELLOW,
                ClipRole::None if clip.active => egui::Color32::GOLD,
                ClipRole::None => egui::Color32::GRAY,
            };
            ui.colored_label(color, format!("{marker}{}", ellipsize(&clip.name, 16)));
            if resp.double_clicked() {
                let _ = tx.send(Command::AddCue(clip.id));
            }
        });
    });
}

/// The bank bar: pick which bank to edit, send one live, add a bank. `●` marks
/// the live bank; the selected tab is the edit bank shown in the list below.
fn bank_bar(ui: &mut egui::Ui, m: &UiMirror, tx: &Sender<Command>) {
    ui.horizontal(|ui| {
        ui.strong("Banks");
        for (i, b) in m.banks.iter().enumerate() {
            let live = i == m.live_bank;
            let label = format!("{}{} ({})", if live { "● " } else { "" }, b.name, b.cue_count);
            if ui
                .selectable_label(i == m.edit_bank, label)
                .on_hover_text("edit this bank (shown below)")
                .clicked()
            {
                let _ = tx.send(Command::SetEditBank(i));
            }
            if !live
                && ui
                    .small_button("▶")
                    .on_hover_text("play this bank (it takes over at the next phrase)")
                    .clicked()
            {
                let _ = tx.send(Command::SetLiveBank(i));
            }
        }
        if ui.button("＋").on_hover_text("add a bank").clicked() {
            let _ = tx.send(Command::AddBank);
        }
    });
}

/// A cue tile in the edit bank's list. Click selects it for the editor.
fn cue_chip(
    ui: &mut egui::Ui,
    m: &UiMirror,
    cue: &CueView,
    thumbs: &HashMap<ClipId, egui::TextureHandle>,
    tx: &Sender<Command>,
) {
    let selected = m.selected_cue == Some(cue.id);
    ui.allocate_ui(egui::vec2(146.0, 116.0), |ui| {
        let stroke = if selected {
            egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 170, 255))
        } else {
            egui::Stroke::new(1.0, egui::Color32::from_gray(60))
        };
        egui::Frame::group(ui.style()).stroke(stroke).show(ui, |ui| {
            ui.vertical(|ui| {
                let resp = if let Some(tex) = thumbs.get(&cue.clip) {
                    let img = egui::Image::new((tex.id(), egui::vec2(120.0, 56.0)));
                    ui.add(egui::Button::image(img).selected(selected))
                } else {
                    ui.add_sized(
                        egui::vec2(120.0, 56.0),
                        egui::Button::new(ellipsize(&cue.name, 10)).selected(selected),
                    )
                };
                if resp.clicked() {
                    let _ = tx.send(Command::SelectCue(Some(cue.id)));
                }
                ui.horizontal(|ui| {
                    let (marker, color) = match cue.role {
                        ClipRole::Playing => ("▶", egui::Color32::LIGHT_GREEN),
                        ClipRole::Armed => ("○", egui::Color32::YELLOW),
                        ClipRole::None => (" ", egui::Color32::GRAY),
                    };
                    ui.colored_label(color, marker);
                    ui.label(ellipsize(&cue.name, 12));
                });
                ui.horizontal(|ui| {
                    ui.small(trim_label(cue));
                    if cue.preserve.is_some() {
                        ui.small(if cue.preserve == Some(true) { "·keep" } else { "·cut" });
                    }
                    if cue.shader.is_some() {
                        ui.small("·fx").on_hover_text("has a shader override");
                    }
                    if ui.small_button("✕").on_hover_text("remove cue").clicked() {
                        let _ = tx.send(Command::RemoveCue(cue.id));
                    }
                });
            });
        });
    });
}

/// The side-panel editor for one cue: in/out trim, per-cue preserve, shader override.
fn cue_editor(ui: &mut egui::Ui, m: &UiMirror, cue: &CueView, tx: &Sender<Command>) {
    ui.add_space(4.0);
    ui.strong(&cue.name);
    let role = match cue.role {
        ClipRole::Playing => "playing",
        ClipRole::Armed => "armed",
        ClipRole::None => "idle",
    };
    ui.weak(format!("cue #{} · {role}", cue.id));
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        ui.label("In ");
        let mut v = cue.in_sec;
        if ui
            .add(
                egui::DragValue::new(&mut v)
                    .speed(0.05)
                    .range(0.0..=f64::MAX)
                    .suffix(" s")
                    .fixed_decimals(2),
            )
            .changed()
        {
            let _ = tx.send(Command::SetCueIn(cue.id, v));
        }
        if ui
            .button("⏺")
            .on_hover_text("set in-point to the current playhead")
            .clicked()
        {
            let _ = tx.send(Command::SetCueInToPlayhead(cue.id));
        }
    });

    ui.horizontal(|ui| {
        ui.label("Out");
        let mut trimmed = cue.out_sec.is_some();
        if ui
            .checkbox(&mut trimmed, "")
            .on_hover_text("trim the end (off = play to the clip's natural end)")
            .changed()
        {
            let out = trimmed.then_some(cue.in_sec + 1.0);
            let _ = tx.send(Command::SetCueOut(cue.id, out));
        }
        match cue.out_sec {
            Some(out) => {
                let mut v = out;
                if ui
                    .add(
                        egui::DragValue::new(&mut v)
                            .speed(0.05)
                            .range(0.0..=f64::MAX)
                            .suffix(" s")
                            .fixed_decimals(2),
                    )
                    .changed()
                {
                    let _ = tx.send(Command::SetCueOut(cue.id, Some(v)));
                }
                if ui
                    .button("⏺")
                    .on_hover_text("set out-point to the current playhead")
                    .clicked()
                {
                    let _ = tx.send(Command::SetCueOutToPlayhead(cue.id));
                }
            }
            None => {
                ui.weak("clip end");
            }
        }
    });

    ui.add_space(10.0);
    ui.label("Preserve playhead")
        .on_hover_text("On a cut, carry the playhead into this cue. Inherit follows the global toggle.");
    ui.horizontal(|ui| {
        if ui.selectable_label(cue.preserve.is_none(), "Inherit").clicked() {
            let _ = tx.send(Command::SetCuePreserve(cue.id, None));
        }
        if ui.selectable_label(cue.preserve == Some(true), "On").clicked() {
            let _ = tx.send(Command::SetCuePreserve(cue.id, Some(true)));
        }
        if ui.selectable_label(cue.preserve == Some(false), "Off").clicked() {
            let _ = tx.send(Command::SetCuePreserve(cue.id, Some(false)));
        }
    });

    ui.add_space(10.0);
    ui.label("Shader")
        .on_hover_text("Render this cue with a pinned shader instead of the live one. Applies immediately while the cue plays.");
    let selected_name = cue
        .shader
        .and_then(|id| m.shader_pool.iter().find(|s| s.id == id))
        .map(|s| s.name.as_str())
        .unwrap_or("Live shader");
    egui::ComboBox::from_id_salt("cue_shader")
        .selected_text(selected_name)
        .show_ui(ui, |ui| {
            if ui.selectable_label(cue.shader.is_none(), "Live shader").clicked() {
                let _ = tx.send(Command::SetCueShader(cue.id, None));
            }
            for s in &m.shader_pool {
                if ui
                    .selectable_label(cue.shader == Some(s.id), &s.name)
                    .clicked()
                {
                    let _ = tx.send(Command::SetCueShader(cue.id, Some(s.id)));
                }
            }
        });
    if m.shader_pool.is_empty() {
        ui.weak("No pinned shaders yet — “📌 Pin” the current shader below.");
    }

    ui.add_space(10.0);
    ui.weak("Trim & preserve apply the next time this cue is triggered.");
    ui.add_space(4.0);
    if ui.button("Remove cue").clicked() {
        let _ = tx.send(Command::RemoveCue(cue.id));
    }
}

fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        s.to_string()
    }
}

fn fmt_time(secs: f64) -> String {
    let secs = secs.max(0.0);
    let mins = (secs / 60.0).floor() as u64;
    let rem = secs - mins as f64 * 60.0;
    format!("{mins}:{rem:05.2}")
}

fn trim_label(cue: &CueView) -> String {
    let out = cue.out_sec.map(fmt_time).unwrap_or_else(|| "end".to_string());
    format!("{}–{}", fmt_time(cue.in_sec), out)
}

/// Audio meter with a view toggle: the native 21 perceptual log bands (what
/// `fftBand`/the shaders react to) or the 512 linear bins the Shadertoy
/// `iChannel0` FFT row exposes. Toggle state lives in egui memory (display-only).
fn spectrum(ui: &mut egui::Ui, m: &UiMirror) {
    let id = egui::Id::new("spectrum_linear_view");
    let mut linear = ui.data_mut(|d| d.get_temp::<bool>(id).unwrap_or(false));
    let col = egui::Color32::from_rgb(90, 200, 130);

    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(21.0 * 8.0, 36.0), egui::Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, egui::CornerRadius::same(2), egui::Color32::from_gray(20));
        if linear {
            // 512-bin linear spectrum (iChannel0 row 0), already 0..1. One
            // vertical line per pixel column, taking the max bin it covers.
            let spec = &m.spectrum_linear;
            let n = spec.len();
            let cols = rect.width() as usize;
            if n > 0 && cols > 0 {
                for cx in 0..cols {
                    let lo = cx * n / cols;
                    let hi = ((cx + 1) * n / cols).clamp(lo + 1, n);
                    let v = spec[lo..hi].iter().copied().fold(0.0_f32, f32::max);
                    let h = rect.height() * v.clamp(0.0, 1.0);
                    let x = rect.min.x + cx as f32;
                    p.line_segment(
                        [egui::pos2(x, rect.max.y), egui::pos2(x, rect.max.y - h)],
                        egui::Stroke::new(1.0, col),
                    );
                }
            }
        } else {
            let w = rect.width() / 21.0;
            for (i, v) in m.levels.iter().enumerate() {
                // bands are large FFT magnitudes; log-compress for display
                let mag = (1.0 + v).ln() / 12.0;
                let h = rect.height() * mag.clamp(0.0, 1.0);
                let bar = egui::Rect::from_min_max(
                    egui::pos2(rect.min.x + i as f32 * w + 1.0, rect.max.y - h),
                    egui::pos2(rect.min.x + (i as f32 + 1.0) * w - 1.0, rect.max.y),
                );
                p.rect_filled(bar, egui::CornerRadius::ZERO, col);
            }
        }
        if ui
            .selectable_label(linear, if linear { "512·lin" } else { "21·log" })
            .on_hover_text(
                "spectrum view — 21 perceptual log bands (fftBand) \
                 vs 512 linear bins (iChannel0)",
            )
            .clicked()
        {
            linear = !linear;
            ui.data_mut(|d| d.insert_temp(id, linear));
        }
    });
}

enum PickKind {
    ClipDir,
    Shader,
}

/// Open a native picker on a worker thread and deliver the choice as a Command.
/// (NSOpenPanel is main-thread-only for blocking dialogs, so use the async API.)
fn pick_file(tx: Sender<Command>, kind: PickKind) {
    match kind {
        PickKind::ClipDir => {
            let fut = rfd::AsyncFileDialog::new().pick_folder();
            std::thread::spawn(move || {
                if let Some(h) = pollster::block_on(fut) {
                    let _ = tx.send(Command::SetClipDir(h.path().to_path_buf()));
                }
            });
        }
        PickKind::Shader => {
            let fut = rfd::AsyncFileDialog::new()
                .add_filter("shaders", &["frag", "fs", "glsl", "wgsl"])
                .pick_file();
            std::thread::spawn(move || {
                if let Some(h) = pollster::block_on(fut) {
                    let _ = tx.send(Command::SetShaderPath(h.path().to_path_buf()));
                }
            });
        }
    }
}

/// Wrap an Arc<Window> for reuse elsewhere (kept for symmetry with output side).
pub type SharedWindow = Arc<Window>;
