//! Bottom panel: shader picker/pin controls, pinned-shader tags, the audio
//! device combo, glyph spectrum + level meters, the shader compile-error
//! drawer, and the statusline — mode word, session summary, and the theme
//! switchboard (dark/light + hue rotation).

use crossbeam_channel::Sender;
use egui::Ui;

use phosphor::icon;
use phosphor::theme::{palette, SP_MD, SP_SM};
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
            if widgets::bracket_button(ui, &format!("{} shader…", icon::FOLDER), None, 0.0)
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
            if widgets::bracket_button(ui, &format!("{} pin", icon::PIN), None, 0.0)
                .on_hover_text(
                    "Pin the current shader's last good compile into the pool so a \
                     cue can use it while you keep livecoding this one. Key: c",
                )
                .clicked()
            {
                let _ = tx.send(Command::CaptureShader);
            }
            if !m.shader_pool.is_empty() {
                ui.add_space(SP_MD);
                pinned_shaders(ui, m, tx);
            }

            ui.add_space(SP_MD);
            if widgets::bracket_button(ui, &format!("{} save", icon::SAVE), None, 0.0)
                .on_hover_text(
                    "Save the project (⌘/Ctrl+S). Writes back to the loaded file, or \
                     asks where to put it for a fresh session.",
                )
                .clicked()
            {
                let _ = tx.send(Command::SaveProject);
            }
            if widgets::bracket_button(ui, &format!("{} save as…", icon::SAVE), None, 0.0)
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

        ui.add_space(SP_SM);
        statusline(ui, m);
        error_window(ui, m);
    });
}

/// Collapsed toggle for the pinned-shader pool: `[N pinned]`, opening a popup
/// with one row per pinned shader (name + delete button) on click, instead
/// of chips always eating a full row's width as the pool grows.
fn pinned_shaders(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    let p = palette();
    let resp = widgets::bracket_button(ui, &format!("{} pinned", m.shader_pool.len()), None, 0.0)
        .on_hover_text("Pinned shaders — available to any cue's shader override");
    egui::Popup::menu(&resp).show(|ui| {
        for s in &m.shader_pool {
            ui.horizontal(|ui| {
                // Plain label + a full-widget delete button, not `chip`'s
                // carved-out close sub-rect: that math assumes a stable
                // horizontal flow and doesn't hold up inside the popup's
                // justified vertical layout (the click landed in the popup,
                // closing it, but not on the sub-rect chip expected).
                ui.label(egui::RichText::new(s.name.as_ref()).monospace().color(p.fg_secondary));
                if !s.builtin
                    && widgets::bracket_button(ui, icon::DELETE, Some(p.error), 0.0)
                        .on_hover_text("unpin")
                        .clicked()
                {
                    let _ = tx.send(Command::RemoveShader(s.id));
                }
            });
        }
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

/// Floating, resizable window with the full shader compile-error text.
/// Opened via the indicator in the statusline; stays open (showing the last
/// error) until the user closes it, even after the error clears.
fn error_window(ui: &mut Ui, m: &UiMirror) {
    let p = palette();

    // Keep the last error text in memory so the window still has something
    // to show if it's left open after the error clears.
    let text_id = egui::Id::new("shader_err_text");
    if let Some(err) = &m.shader_error {
        ui.data_mut(|d| d.insert_temp(text_id, err.to_string()));
    }
    let text = ui.data_mut(|d| d.get_temp::<String>(text_id)).unwrap_or_default();

    let open_id = egui::Id::new("shader_err_window_open");
    let mut open = ui.data_mut(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    if !open {
        return;
    }
    egui::Window::new("Shader error")
        .id(egui::Id::new("shader_err_window"))
        .open(&mut open)
        .default_size(egui::vec2(480.0, 240.0))
        .resizable(true)
        .show(ui.ctx(), |ui| {
            egui::ScrollArea::vertical().id_salt("shader_err").show(ui, |ui| {
                ui.label(egui::RichText::new(&text).monospace().color(p.fg_primary));
            });
        });
    ui.data_mut(|d| d.insert_temp(open_id, open));
}

/// The statusline: a full-width `select`-filled strip with the mode word
/// (NORMAL / ENTRY while typing a BPM / ERROR on a failed compile), the
/// loaded shader, a session summary, and the collapsed theme toggle on the
/// right. When the mode is ERROR, its segment doubles as the indicator that
/// opens [`error_window`] with the full compile-error text.
fn statusline(ui: &mut Ui, m: &UiMirror) {
    let p = palette();
    let has_error = m.shader_error.is_some();
    let mode = if has_error {
        ("ERROR", Some(p.error))
    } else if m.bpm_entry.is_some() {
        ("ENTRY", Some(p.accent))
    } else {
        ("NORMAL", None)
    };
    let summary = format!(
        "{}   {} clips · {} cues · {:.1} bpm",
        m.shader_name.as_deref().unwrap_or("—"),
        m.clips.len(),
        m.cues.len(),
        m.bpm,
    );
    let mode_clicked = widgets::statusline(ui, mode, &summary);
    if mode_clicked && has_error {
        let open_id = egui::Id::new("shader_err_window_open");
        ui.data_mut(|d| d.insert_temp(open_id, true));
    }
}
