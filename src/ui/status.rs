//! Bottom panel: shader picker/pin controls, pinned-shader chips, the audio
//! device combo, the spectrum meter, and the shader compile-error drawer.

use crossbeam_channel::Sender;
use egui::Ui;

use super::theme::{PALETTE, SP_MD, SP_SM};
use super::{pick_file, PickKind};
use crate::commands::{Command, UiMirror};

/// The bottom panel.
pub(super) fn show(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    egui::Panel::bottom(egui::Id::new("io")).show(ui, |ui| {
        ui.add_space(SP_SM);
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
            ui.add_space(SP_MD);
            egui::ComboBox::from_id_salt("audio")
                .selected_text(m.current_device.as_deref().unwrap_or("default"))
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(false, "Default")
                        .on_hover_text("system default input")
                        .clicked()
                    {
                        let _ = tx.send(Command::SetAudioDevice(None));
                    }
                    for name in &m.audio_devices {
                        if ui
                            .selectable_label(m.current_device.as_ref() == Some(name), name.as_ref())
                            .clicked()
                        {
                            let _ = tx.send(Command::SetAudioDevice(Some(name.to_string())));
                        }
                    }
                });
        });
        if !m.shader_pool.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.weak("Pinned:");
                for s in &m.shader_pool {
                    ui.label(egui::RichText::new(s.name.as_ref()).small());
                    if ui
                        .small_button("✕")
                        .on_hover_text("remove this pinned shader")
                        .clicked()
                    {
                        let _ = tx.send(Command::RemoveShader(s.id));
                    }
                    ui.add_space(SP_SM);
                }
            });
        }
        // The one boundary in this panel worth a real rule: controls above,
        // meters below.
        ui.separator();
        spectrum(ui, m);
        if let Some(err) = &m.shader_error {
            egui::ScrollArea::vertical()
                .id_salt("shader_err")
                .max_height(96.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(err.as_ref()).monospace().color(PALETTE.error));
                });
        }
        ui.add_space(SP_SM);
    });
}

/// Audio meter with a view toggle: the native 21 perceptual log bands (what
/// `fftBand`/the shaders react to) or the 512 linear bins the Shadertoy
/// `iChannel0` FFT row exposes. Toggle state lives in egui memory (display-only).
fn spectrum(ui: &mut Ui, m: &UiMirror) {
    let id = egui::Id::new("spectrum_linear_view");
    let mut linear = ui.data_mut(|d| d.get_temp::<bool>(id).unwrap_or(false));
    let col = PALETTE.accent;

    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(21.0 * 8.0, 36.0), egui::Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, egui::CornerRadius::same(2), PALETTE.bg_inset);
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
