//! The cue editor: the right-hand panel showing the selected cue's in/out
//! trim, preserve-playhead override, and shader override.

use crossbeam_channel::Sender;
use egui::Ui;

use super::fmt_time;
use super::theme::{PALETTE, SP_LG, SP_MD, SP_SM};
use super::widgets;
use crate::commands::{ClipRole, Command, CueView, UiMirror};

/// The right panel: the selected cue's fields, or an empty-state prompt.
pub(super) fn show(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    egui::Panel::right(egui::Id::new("cue_editor"))
        .resizable(true)
        .default_size(272.0)
        .min_size(210.0)
        .show(ui, |ui| {
            ui.add_space(SP_SM);
            if let Some(cue) = m.selected_cue.and_then(|id| m.cues.iter().find(|c| c.id == id)) {
                cue_editor(ui, m, cue, tx);
            } else {
                empty_state(ui);
            }
        });
}

/// Centered muted two-line prompt shown when no cue is selected, matching
/// the clip pool / cue list empty states.
fn empty_state(ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(SP_MD * 3.0);
        ui.label(egui::RichText::new("No cue selected").color(PALETTE.fg_muted));
        ui.label(
            egui::RichText::new("Double-click a clip to add a cue, then click it here to edit")
                .small()
                .color(PALETTE.fg_muted),
        );
        ui.add_space(SP_MD * 3.0);
    });
}

/// The fields for one cue: header, in/out trim, per-cue preserve, shader
/// override, and a bottom-anchored remove button.
fn cue_editor(ui: &mut Ui, m: &UiMirror, cue: &CueView, tx: &Sender<Command>) {
    ui.horizontal(|ui| {
        ui.strong(cue.name.as_ref());
        let (role_text, role_tint) = match cue.role {
            ClipRole::Playing => ("playing", Some(PALETTE.playing)),
            ClipRole::Armed => ("armed", Some(PALETTE.armed)),
            ClipRole::None => ("idle", None),
        };
        widgets::chip(ui, role_text, role_tint, false);
        widgets::chip(ui, &format!("#{}", cue.id), None, false);
    });
    ui.label(
        egui::RichText::new(format!("⏱ {}", fmt_time(m.playhead_sec)))
            .monospace()
            .color(PALETTE.fg_secondary),
    );
    ui.add_space(SP_MD);

    ui.style_mut().drag_value_text_style = egui::TextStyle::Monospace;
    egui::Grid::new("cue_trim").num_columns(2).spacing(egui::vec2(SP_SM, SP_SM)).show(ui, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new("In").color(PALETTE.fg_muted));
        });
        ui.horizontal(|ui| {
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
                .button(egui::RichText::new("⏺").color(PALETTE.accent))
                .on_hover_text("set in-point to the current playhead")
                .clicked()
            {
                let _ = tx.send(Command::SetCueInToPlayhead(cue.id));
            }
        });
        ui.end_row();

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new("Out").color(PALETTE.fg_muted));
        });
        ui.horizontal(|ui| {
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
                        .button(egui::RichText::new("⏺").color(PALETTE.accent))
                        .on_hover_text("set out-point to the current playhead")
                        .clicked()
                    {
                        let _ = tx.send(Command::SetCueOutToPlayhead(cue.id));
                    }
                }
                None => {
                    ui.label(egui::RichText::new("clip end").color(PALETTE.fg_muted));
                }
            }
        });
        ui.end_row();
    });

    ui.add_space(SP_LG);
    widgets::section_label(ui, "preserve playhead").on_hover_text(
        "On a cut, carry the playhead into this cue. Inherit follows the global toggle.",
    );
    let preserve_idx = match cue.preserve {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    };
    if let Some(i) = widgets::segmented(ui, "cue_preserve", &["Inherit", "On", "Off"], Some(preserve_idx))
    {
        let val = match i {
            0 => None,
            1 => Some(true),
            _ => Some(false),
        };
        let _ = tx.send(Command::SetCuePreserve(cue.id, val));
    }

    ui.add_space(SP_LG);
    widgets::section_label(ui, "shader").on_hover_text(
        "Render this cue with a pinned shader instead of the live one. Applies immediately while the cue plays.",
    );
    let selected_name = cue
        .shader
        .and_then(|id| m.shader_pool.iter().find(|s| s.id == id))
        .map(|s| &*s.name)
        .unwrap_or("Live shader");
    egui::ComboBox::from_id_salt("cue_shader").selected_text(selected_name).show_ui(ui, |ui| {
        if ui.selectable_label(cue.shader.is_none(), "Live shader").clicked() {
            let _ = tx.send(Command::SetCueShader(cue.id, None));
        }
        for s in &m.shader_pool {
            if ui.selectable_label(cue.shader == Some(s.id), s.name.as_ref()).clicked() {
                let _ = tx.send(Command::SetCueShader(cue.id, Some(s.id)));
            }
        }
    });
    if m.shader_pool.is_empty() {
        ui.label(
            egui::RichText::new("No pinned shaders yet — “📌 Pin” the current shader below.")
                .small()
                .color(PALETTE.fg_muted),
        );
    }

    ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
        let remove = egui::Button::new(egui::RichText::new("Remove cue").color(PALETTE.error))
            .fill(PALETTE.error.linear_multiply(0.12))
            .min_size(egui::vec2(ui.available_width(), 0.0));
        if ui.add(remove).clicked() {
            let _ = tx.send(Command::RemoveCue(cue.id));
        }
        ui.add_space(SP_SM);
        ui.label(
            egui::RichText::new("Trim & preserve apply the next time this cue is triggered.")
                .small()
                .color(PALETTE.fg_muted),
        );
        ui.add_space(SP_SM);
    });
}
