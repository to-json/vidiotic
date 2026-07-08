//! Bottom panel: shader picker/pin controls, pinned-shader chips, the audio
//! device combo, the spectrum + level meters, and the shader compile-error
//! drawer.

use crossbeam_channel::Sender;
use egui::Ui;

use super::theme::{PALETTE, SP_MD, SP_SM};
use super::widgets;
use super::{pick_file, PickKind};
use crate::commands::{Command, UiMirror};

/// The bottom panel: a left cluster (shader controls) and a right cluster
/// (audio device + meters) on one row, plus an animated error drawer below.
pub(super) fn show(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    egui::Panel::bottom(egui::Id::new("io")).show(ui, |ui| {
        ui.add_space(SP_SM);
        ui.horizontal(|ui| {
            if ui.button("Shader…").clicked() {
                pick_file(tx.clone(), PickKind::Shader);
            }
            let name_color = if m.shader_error.is_some() { PALETTE.error } else { PALETTE.fg_primary };
            ui.label(
                egui::RichText::new(m.shader_name.as_deref().unwrap_or("<none>"))
                    .monospace()
                    .color(name_color),
            );
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
            let mut unpinned = None;
            for s in &m.shader_pool {
                let resp = widgets::chip(ui, s.name.as_ref(), None, true);
                if resp.removed {
                    unpinned = Some(s.id);
                }
                ui.add_space(SP_SM);
            }
            if let Some(id) = unpinned {
                let _ = tx.send(Command::RemoveShader(id));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(err) = &m.audio_error {
                    let resp = widgets::chip(ui, "audio ⚠", Some(PALETTE.error), false);
                    ui.interact(resp.rect, ui.id().with("audio_error_hover"), egui::Sense::hover())
                        .on_hover_text(err.as_str());
                }
                level_bar(ui, m.level);
                spectrum(ui, m);
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
        });

        error_drawer(ui, m);
        ui.add_space(SP_SM);
    });
}

/// A vertical bar next to the spectrum, fed by `m.level`: `playing` under
/// half scale, `armed` past it, `error` near clipping.
fn level_bar(ui: &mut Ui, level: f32) {
    let p = &PALETTE;
    let height = 36.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(4.0, height), egui::Sense::hover());
    let mag = ((1.0 + level).ln() / 12.0).clamp(0.0, 1.0);
    let color = if mag > 0.85 {
        p.error
    } else if mag > 0.5 {
        p.armed
    } else {
        p.playing
    };
    let painter = ui.painter();
    painter.rect_filled(rect, egui::CornerRadius::same(1), p.bg_inset);
    let h = rect.height() * mag;
    let bar = egui::Rect::from_min_max(egui::pos2(rect.min.x, rect.max.y - h), rect.max);
    painter.rect_filled(bar, egui::CornerRadius::same(1), color);
}

/// Audio meter with a view toggle: the native 21 perceptual log bands (what
/// `fftBand`/the shaders react to) or the 512 linear bins the Shadertoy
/// `iChannel0` FFT row exposes. Toggle state lives in egui memory (display-only).
fn spectrum(ui: &mut Ui, m: &UiMirror) {
    let id = egui::Id::new("spectrum_linear_view");
    let mut linear = ui.data_mut(|d| d.get_temp::<bool>(id).unwrap_or(false));
    let col = PALETTE.accent;

    ui.horizontal(|ui| {
        let toggle = widgets::chip(ui, if linear { "512·lin" } else { "21·log" }, Some(col), false);
        ui.interact(toggle.rect, ui.id().with("spectrum_toggle_hover"), egui::Sense::hover())
            .on_hover_text(
                "spectrum view — 21 perceptual log bands (fftBand) \
                 vs 512 linear bins (iChannel0)",
            );
        if toggle.clicked {
            linear = !linear;
            ui.data_mut(|d| d.insert_temp(id, linear));
        }

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
    });
}

/// Slide-open drawer under the bar for the shader compile error: `error`-tinted
/// fill, a 2px `error` left border, monospace `fg_primary` text (the border
/// alone carries the red — a red-on-dark error wall was unreadable).
fn error_drawer(ui: &mut Ui, m: &UiMirror) {
    let openness = ui.ctx().animate_bool(egui::Id::new("shader_err_drawer"), m.shader_error.is_some());
    if openness <= 0.001 {
        return;
    }

    // Keep the last error text in temp memory so the drawer still has
    // something to show while it eases closed after the error clears.
    let text_id = egui::Id::new("shader_err_text");
    if let Some(err) = &m.shader_error {
        ui.data_mut(|d| d.insert_temp(text_id, err.to_string()));
    }
    let text = ui.data_mut(|d| d.get_temp::<String>(text_id)).unwrap_or_default();

    let frame = egui::Frame::new()
        .fill(PALETTE.error.linear_multiply(0.08))
        .inner_margin(egui::Margin::symmetric(SP_MD as i8, SP_SM as i8));
    let outer = frame.show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("shader_err").max_height(96.0 * openness).show(ui, |ui| {
            ui.label(egui::RichText::new(&text).monospace().color(PALETTE.fg_primary));
        });
    });
    let border = egui::Rect::from_min_size(outer.response.rect.min, egui::vec2(2.0, outer.response.rect.height()));
    ui.painter().rect_filled(border, egui::CornerRadius::ZERO, PALETTE.error);
}
