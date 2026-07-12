//! Bottom panel: shader picker/pin controls, pinned-shader tags, the audio
//! device combo, glyph spectrum + level meters, the shader compile-error
//! drawer, and the statusline — mode word, session summary, and the theme
//! switchboard (dark/light + hue rotation).

use crossbeam_channel::Sender;
use egui::Ui;

use phosphor::theme::{mono, palette, ROW, SP_MD, SP_SM};
use phosphor::widgets;

use super::{pick_file, PickKind};
use crate::commands::{Command, UiMirror};

/// The bottom panel: a left cluster (shader controls) and a right cluster
/// (audio device + meters) on one row, an animated error drawer below, and
/// the statusline at the very bottom.
pub(super) fn show(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    let p = palette();
    egui::Panel::bottom(egui::Id::new("io")).show(ui, |ui| {
        ui.add_space(SP_SM);
        ui.horizontal_wrapped(|ui| {
            if widgets::bracket_button(ui, "shader…", None, 0.0)
                .on_hover_text("Load a GLSL/WGSL shader file to livecode")
                .clicked()
            {
                pick_file(tx.clone(), PickKind::Shader);
            }
            let name_color = if m.shader_error.is_some() { p.error } else { p.fg_primary };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(m.shader_name.as_deref().unwrap_or("<none>"))
                        .monospace()
                        .color(name_color),
                )
                .truncate(),
            );
            if widgets::bracket_button(ui, "pin", None, 0.0)
                .on_hover_text(
                    "Pin the current shader's last good compile into the pool so a \
                     cue can use it while you keep livecoding this one. Key: c",
                )
                .clicked()
            {
                let _ = tx.send(Command::CaptureShader);
            }
            ui.add_space(SP_MD);
            let mut unpinned = None;
            for s in &m.shader_pool {
                let resp = widgets::chip(ui, s.name.as_ref(), None, true);
                ui.interact(resp.rect, ui.id().with(("pinned_shader_hover", s.id)), egui::Sense::hover())
                    .on_hover_text("Pinned shader — available to any cue's shader override. ✕ to unpin.");
                if resp.removed {
                    unpinned = Some(s.id);
                }
                ui.add_space(SP_SM);
            }
            if let Some(id) = unpinned {
                let _ = tx.send(Command::RemoveShader(id));
            }

            ui.add_space(SP_MD);
            if widgets::bracket_button(ui, "save", None, 0.0)
                .on_hover_text(
                    "Save the project (⌘/Ctrl+S). Writes back to the loaded file, or \
                     asks where to put it for a fresh session.",
                )
                .clicked()
            {
                let _ = tx.send(Command::SaveProject);
            }
            if widgets::bracket_button(ui, "save as…", None, 0.0)
                .on_hover_text("Save the project to a new .viproj file")
                .clicked()
            {
                let _ = tx.send(Command::SaveProjectAs);
            }
        });

        // Meters and audio input on their own row: right-justified against
        // the panel edge when they fit on one line, stacking left-to-right
        // (wrapped) on a narrow window.
        if ui.available_width() >= 480.0 {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    audio_error_tag(ui, m);
                    widgets::glyph_level(ui, compress(m.level), 8);
                    spectrum(ui, m);
                    device_combo(ui, m, tx);
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                device_combo(ui, m, tx);
                spectrum(ui, m);
                widgets::glyph_level(ui, compress(m.level), 8);
                audio_error_tag(ui, m);
            });
        }

        error_drawer(ui, m);
        ui.add_space(SP_SM);
        statusline(ui, m);
    });
}

/// Log-compress a raw FFT magnitude / level into `0..=1` for display.
fn compress(v: f32) -> f32 {
    ((1.0 + v).ln() / 12.0).clamp(0.0, 1.0)
}

/// Error tag shown while audio capture is failing; hover for the message.
fn audio_error_tag(ui: &mut Ui, m: &UiMirror) {
    if let Some(err) = &m.audio_error {
        let resp = widgets::chip(ui, "audio!", Some(palette().error), false);
        ui.interact(resp.rect, ui.id().with("audio_error_hover"), egui::Sense::hover())
            .on_hover_text(err.as_str());
    }
}

/// The audio input device picker.
fn device_combo(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    egui::ComboBox::from_id_salt("audio")
        .selected_text(m.current_device.as_deref().unwrap_or("default"))
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(false, "default")
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
        })
        .response
        .on_hover_text("Audio input device");
}

/// Audio meter with a view toggle: the native 21 perceptual log bands (what
/// `fftBand`/the shaders react to) or the 512 linear bins the Shadertoy
/// `iChannel0` FFT row exposes, pooled to 48 columns. Toggle state lives in
/// egui memory (display-only).
fn spectrum(ui: &mut Ui, m: &UiMirror) {
    let id = egui::Id::new("spectrum_linear_view");
    let mut linear = ui.data_mut(|d| d.get_temp::<bool>(id).unwrap_or(false));

    ui.horizontal(|ui| {
        let toggle = widgets::chip(ui, if linear { "512·lin" } else { "21·log" }, Some(palette().blue), false);
        ui.interact(toggle.rect, ui.id().with("spectrum_toggle_hover"), egui::Sense::hover())
            .on_hover_text(
                "spectrum view — 21 perceptual log bands (fftBand) \
                 vs 512 linear bins (iChannel0)",
            );
        if toggle.clicked {
            linear = !linear;
            ui.data_mut(|d| d.insert_temp(id, linear));
        }

        if linear {
            // 512-bin linear spectrum (iChannel0 row 0), already 0..1. One
            // glyph column per pooled cell, taking the max bin it covers.
            const COLS: usize = 48;
            let spec = &m.spectrum_linear;
            let n = spec.len();
            let mut mags = [0.0_f32; COLS];
            if n > 0 {
                for (cx, mag) in mags.iter_mut().enumerate() {
                    let lo = cx * n / COLS;
                    let hi = ((cx + 1) * n / COLS).clamp(lo + 1, n);
                    *mag = spec[lo..hi].iter().copied().fold(0.0_f32, f32::max);
                }
            }
            widgets::glyph_fft(ui, &mags);
        } else {
            // Bands are large FFT magnitudes; log-compress for display.
            let mags: Vec<f32> = m.levels.iter().map(|v| compress(*v)).collect();
            widgets::glyph_fft(ui, &mags);
        }
    });
}

/// Slide-open drawer under the bar for the shader compile error: `error`-tinted
/// fill, a 2px `error` left border, monospace `fg_primary` text (the border
/// alone carries the red — a red-on-dark error wall was unreadable).
fn error_drawer(ui: &mut Ui, m: &UiMirror) {
    let p = palette();
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
        .fill(p.error.linear_multiply(0.08))
        .inner_margin(egui::Margin::symmetric(SP_MD as i8, SP_SM as i8));
    let outer = frame.show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("shader_err").max_height(96.0 * openness).show(ui, |ui| {
            ui.label(egui::RichText::new(&text).monospace().color(p.fg_primary));
        });
    });
    let border = egui::Rect::from_min_size(outer.response.rect.min, egui::vec2(2.0, outer.response.rect.height()));
    ui.painter().rect_filled(border, egui::CornerRadius::ZERO, p.error);
}

/// The statusline: a full-width `select`-filled strip with the mode word
/// (NORMAL / ENTRY while typing a BPM / ERROR on a failed compile), the
/// loaded shader, a session summary, and the theme switchboard on the right.
fn statusline(ui: &mut Ui, m: &UiMirror) {
    let p = palette();
    let cw = widgets::cell_width(ui);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, egui::CornerRadius::ZERO, p.accent_dim);

    // Mode segment: its own fill when something is happening.
    let (mode, mode_bg) = if m.shader_error.is_some() {
        ("ERROR", Some(p.error))
    } else if m.bpm_entry.is_some() {
        ("ENTRY", Some(p.accent))
    } else {
        ("NORMAL", None)
    };
    let mode_cells = mode.chars().count() as f32 + 2.0;
    if let Some(bg) = mode_bg {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(cw * mode_cells, rect.height())),
            egui::CornerRadius::ZERO,
            bg,
        );
    }
    let mode_fg = if mode_bg.is_some() { p.bg_inset } else { p.fg_primary };
    painter.text(
        egui::pos2(rect.min.x + cw, rect.center().y),
        egui::Align2::LEFT_CENTER,
        mode,
        mono(),
        mode_fg,
    );

    let summary = format!(
        "{}   {} clips · {} cues · {:.1} bpm",
        m.shader_name.as_deref().unwrap_or("—"),
        m.clips.len(),
        m.cues.len(),
        m.bpm,
    );
    // Clip the summary short of the theme switchboard so a narrow window
    // truncates it instead of running the two together.
    let summary_clip = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.max.x - cw * widgets::THEME_CELLS, rect.max.y),
    );
    painter.with_clip_rect(summary_clip).text(
        egui::pos2(rect.min.x + cw * (mode_cells + 2.0), rect.center().y),
        egui::Align2::LEFT_CENTER,
        summary,
        mono(),
        p.fg_secondary,
    );

    widgets::theme_controls(ui, rect);
}
