//! Control window: egui integrated as a library (not eframe). Owns the egui
//! context, winit input translation, the wgpu paint renderer, and the cached
//! clip thumbnails. `control_ui` reads a `UiMirror` and emits `Command`s.
//!
//! Layout is split by panel: [`transport`] (top), [`status`] (bottom),
//! [`editor`] (right, the selected cue's fields), and [`library`] (center,
//! the clip pool and cue banks). [`theme`] holds the palette and spacing
//! scale; [`widgets`] holds shared custom-painted controls.

mod editor;
mod library;
mod status;
mod theme;
mod transport;

use std::collections::HashMap;

use crossbeam_channel::Sender;
use winit::window::Window;

use crate::commands::{ClipId, Command, UiMirror};
use crate::gfx::WindowSurface;

/// The control window's egui stack: context, winit input translation, wgpu
/// paint renderer, and the cached clip thumbnails.
pub struct EguiCtl {
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    thumbs: HashMap<ClipId, egui::TextureHandle>,
}

impl EguiCtl {
    /// Set up egui for the control window, painting to surfaces of `format`.
    pub fn new(window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
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
        }
    }

    /// Whether a thumbnail texture is cached for this clip.
    pub fn has_thumb(&self, id: ClipId) -> bool {
        self.thumbs.contains_key(&id)
    }

    /// Drop all cached thumbnails (the clip pool was replaced).
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

    /// Run `control_ui` for one frame and paint it into the control surface.
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
                                load: wgpu::LoadOp::Clear(theme::wgpu_clear_color(theme::PALETTE.bg_base)),
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
        // No repaint scheduling: the engine requests a control redraw every
        // tick anyway (see `App::update`), which outpaces egui's repaint_delay.
    }
}

fn control_ui(
    ui: &mut egui::Ui,
    m: &UiMirror,
    tx: &Sender<Command>,
    thumbs: &HashMap<ClipId, egui::TextureHandle>,
) {
    transport::show(ui, m, tx);
    status::show(ui, m, tx);
    editor::show(ui, m, tx);
    library::show(ui, m, tx, thumbs);
}

/// Format a seconds count as `m:ss.cc`.
fn fmt_time(secs: f64) -> String {
    let secs = secs.max(0.0);
    let mins = (secs / 60.0).floor() as u64;
    let rem = secs - mins as f64 * 60.0;
    format!("{mins}:{rem:05.2}")
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
