//! The cue editor: the right-hand panel showing the selected cue's in/out
//! trim, preserve-playhead override, and shader override.

use crossbeam_channel::Sender;
use egui::Ui;

use super::fmt_time;
use super::theme::{SP_LG, SP_MD, SP_SM};
use crate::commands::{ClipRole, Command, CueView, UiMirror};

/// The right panel: header (cue name + playhead) plus the selected cue's
/// fields, or an empty-state prompt when nothing is selected.
pub(super) fn show(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    egui::Panel::right(egui::Id::new("cue_editor"))
        .resizable(true)
        .default_size(272.0)
        .min_size(210.0)
        .show(ui, |ui| {
            ui.add_space(SP_SM);
            ui.horizontal(|ui| {
                ui.heading("Cue");
                ui.weak(format!("⏱ {}", fmt_time(m.playhead_sec)));
            });
            ui.add_space(SP_MD);
            if let Some(cue) = m.selected_cue.and_then(|id| m.cues.iter().find(|c| c.id == id)) { cue_editor(ui, m, cue, tx) } else {
                ui.add_space(SP_MD);
                ui.weak("No cue selected.");
                ui.add_space(SP_SM);
                ui.weak("Double-click a clip to add a cue to the edit bank, then click the cue to edit it here.");
            }
        });
}

/// The fields for one cue: in/out trim, per-cue preserve, shader override.
fn cue_editor(ui: &mut Ui, m: &UiMirror, cue: &CueView, tx: &Sender<Command>) {
    ui.add_space(SP_SM);
    ui.strong(cue.name.as_ref());
    let role = match cue.role {
        ClipRole::Playing => "playing",
        ClipRole::Armed => "armed",
        ClipRole::None => "idle",
    };
    ui.weak(format!("cue #{} · {role}", cue.id));
    ui.add_space(SP_MD);

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

    ui.add_space(SP_LG);
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

    ui.add_space(SP_LG);
    ui.label("Shader")
        .on_hover_text("Render this cue with a pinned shader instead of the live one. Applies immediately while the cue plays.");
    let selected_name = cue
        .shader
        .and_then(|id| m.shader_pool.iter().find(|s| s.id == id))
        .map(|s| &*s.name)
        .unwrap_or("Live shader");
    egui::ComboBox::from_id_salt("cue_shader")
        .selected_text(selected_name)
        .show_ui(ui, |ui| {
            if ui.selectable_label(cue.shader.is_none(), "Live shader").clicked() {
                let _ = tx.send(Command::SetCueShader(cue.id, None));
            }
            for s in &m.shader_pool {
                if ui
                    .selectable_label(cue.shader == Some(s.id), s.name.as_ref())
                    .clicked()
                {
                    let _ = tx.send(Command::SetCueShader(cue.id, Some(s.id)));
                }
            }
        });
    if m.shader_pool.is_empty() {
        ui.weak("No pinned shaders yet — “📌 Pin” the current shader below.");
    }

    ui.add_space(SP_LG);
    ui.weak("Trim & preserve apply the next time this cue is triggered.");
    ui.add_space(SP_SM);
    if ui.button("Remove cue").clicked() {
        let _ = tx.send(Command::RemoveCue(cue.id));
    }
}
