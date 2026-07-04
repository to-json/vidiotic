//! Control window: egui integrated as a library (not eframe). Owns the egui
//! context, winit input translation, the wgpu paint renderer, and the cached
//! clip thumbnails. `control_ui` reads a `UiMirror` and emits `Command`s.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use winit::window::Window;

use crate::commands::{ClipId, ClipRole, Command, SyncKind, UiMirror};
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
                    egui::Button::new(egui::RichText::new("TAP").size(22.0))
                        .min_size(egui::vec2(88.0, 46.0)),
                )
                .clicked()
                || ui.input(|i| i.key_pressed(egui::Key::Space))
            {
                let _ = tx.send(Command::TapDownbeat);
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
            for len in [16u32, 32] {
                if ui.selectable_label(m.phrase_len == len, format!("{len}")).clicked() {
                    let _ = tx.send(Command::SetPhraseLen(len));
                }
            }
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
    });

    egui::Panel::bottom(egui::Id::new("io")).show(ui, |ui| {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Shader…").clicked() {
                pick_file(tx.clone(), PickKind::Shader);
            }
            ui.monospace(m.shader_name.as_deref().unwrap_or("<none>"));
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
        spectrum(ui, &m.levels);
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

    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading("Clips");
            if ui.button("Folder…").clicked() {
                pick_file(tx.clone(), PickKind::ClipDir);
            }
            if let Some(d) = &m.clip_dir {
                ui.weak(d);
            }
            ui.weak("— click to toggle in/out of the loop");
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for clip in &m.clips {
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
                            let marker = match clip.role {
                                ClipRole::Playing => "▶ ",
                                ClipRole::Armed => "○ ",
                                ClipRole::None => "",
                            };
                            let name = if clip.name.len() > 16 {
                                format!("{}…", &clip.name[..15])
                            } else {
                                clip.name.clone()
                            };
                            let color = match clip.role {
                                ClipRole::Playing => egui::Color32::LIGHT_GREEN,
                                ClipRole::Armed => egui::Color32::YELLOW,
                                ClipRole::None if clip.active => egui::Color32::GOLD,
                                ClipRole::None => egui::Color32::GRAY,
                            };
                            ui.colored_label(color, format!("{marker}{name}"));
                            if resp.clicked() {
                                let _ = tx.send(Command::ToggleClipActive(clip.id));
                            }
                        });
                    });
                }
            });
        });
    });
}

fn spectrum(ui: &mut egui::Ui, bands: &[f32; 21]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(21.0 * 8.0, 36.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, egui::CornerRadius::same(2), egui::Color32::from_gray(20));
    let w = rect.width() / 21.0;
    for (i, v) in bands.iter().enumerate() {
        // bands are large FFT magnitudes; log-compress for display
        let mag = (1.0 + v).ln() / 12.0;
        let h = rect.height() * mag.clamp(0.0, 1.0);
        let bar = egui::Rect::from_min_max(
            egui::pos2(rect.min.x + i as f32 * w + 1.0, rect.max.y - h),
            egui::pos2(rect.min.x + (i as f32 + 1.0) * w - 1.0, rect.max.y),
        );
        p.rect_filled(bar, egui::CornerRadius::ZERO, egui::Color32::from_rgb(90, 200, 130));
    }
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
