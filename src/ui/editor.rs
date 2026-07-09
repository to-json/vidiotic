//! The cue editor: the right-hand panel showing the selected cue's in/out
//! trim, preserve-playhead override, and shader override.

use crossbeam_channel::Sender;
use egui::Ui;

use super::theme::{PALETTE, SP_LG, SP_MD, SP_SM};
use super::widgets;
use super::{fmt_time, LOOP_CADENCE};
use crate::bank::Toggle;
use crate::commands::{ClipRole, Command, CueParam, CueView, UiMirror, LOOP_TICKS_PER_BEAT};

/// Ticks per beat, as a float for beat↔tick conversions in the advanced rows.
const TPB: f64 = LOOP_TICKS_PER_BEAT as f64;

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
    // The fields scroll: advanced mode adds enough rows to overflow a short window.
    egui::ScrollArea::vertical()
        .id_salt("cue_fields")
        .auto_shrink([false, false])
        .show(ui, |ui| cue_fields(ui, m, cue, tx));
}

/// The scrollable field stack: trim, preserve, shader, the advanced sections
/// (when enabled), and the remove button.
fn cue_fields(ui: &mut Ui, m: &UiMirror, cue: &CueView, tx: &Sender<Command>) {
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
    egui::ComboBox::from_id_salt("cue_shader")
        .selected_text(selected_name)
        .show_ui(ui, |ui| {
            if ui.selectable_label(cue.shader.is_none(), "Live shader").clicked() {
                let _ = tx.send(Command::SetCueShader(cue.id, None));
            }
            for s in &m.shader_pool {
                if ui.selectable_label(cue.shader == Some(s.id), s.name.as_ref()).clicked() {
                    let _ = tx.send(Command::SetCueShader(cue.id, Some(s.id)));
                }
            }
        })
        .response
        .on_hover_text(
            "Override this cue's shader while it plays. Live shader follows whatever you're livecoding.",
        );
    if m.shader_pool.is_empty() {
        ui.label(
            egui::RichText::new("No pinned shaders yet — “📌 Pin” the current shader below.")
                .small()
                .color(PALETTE.fg_muted),
        );
    }

    if m.advanced {
        advanced_sections(ui, m, cue, tx);
    }

    ui.add_space(SP_LG);
    let remove = egui::Button::new(egui::RichText::new("Remove cue").color(PALETTE.error))
        .fill(PALETTE.error.linear_multiply(0.12))
        .min_size(egui::vec2(ui.available_width(), 0.0));
    if ui.add(remove).on_hover_text("Remove this cue from the bank").clicked() {
        let _ = tx.send(Command::RemoveCue(cue.id));
    }
    ui.add_space(SP_SM);
    ui.label(
        egui::RichText::new("Trim, timing & speed apply the next time this cue is triggered.")
            .small()
            .color(PALETTE.fg_muted),
    );
    ui.add_space(SP_SM);
}

/// The advanced per-cue sections: dwell, loop rate, timing offsets, and speed.
/// Only shown when advanced mode is on (`UiMirror::advanced`).
fn advanced_sections(ui: &mut Ui, m: &UiMirror, cue: &CueView, tx: &Sender<Command>) {
    ui.add_space(SP_LG);
    widgets::section_label(ui, "dwell").on_hover_text(
        "Beats this cue plays before advancing. Inherit follows the global “next every”.",
    );
    dwell_row(ui, m, cue, tx);

    ui.add_space(SP_MD);
    widgets::section_label(ui, "loop rate").on_hover_text(
        "Retrigger this cue's video on this grid while it plays. Inherit follows the \
         global “loop every”; off never re-loops.",
    );
    loop_row(ui, cue, tx);

    ui.add_space(SP_LG);
    widgets::section_label(ui, "offsets");
    if let Some((on, v)) = param_row(
        ui,
        "swing",
        "Shift the loop-restart grid by ± ticks (32/beat) for swing / micro-timing.",
        cue.loop_phase.on,
        cue.loop_phase.val as f64,
        1.0,
        -256.0..=256.0,
        " tk",
        0,
    ) {
        let val = v.round() as i32;
        let _ = tx.send(Command::SetCueParam(cue.id, CueParam::LoopPhase(Toggle { on, val })));
    }
    if let Some((on, v)) = param_row(
        ui,
        "nudge",
        "Add seconds to the in-point on each (re)start — sweep which frames show.",
        cue.start_nudge.on,
        cue.start_nudge.val,
        0.01,
        -600.0..=600.0,
        " s",
        2,
    ) {
        let _ = tx.send(Command::SetCueParam(cue.id, CueParam::StartNudge(Toggle { on, val: v })));
    }
    if let Some((on, v)) = param_row(
        ui,
        "delay",
        "Hold the previous cue this many beats before this one cuts in.",
        cue.trig_delay.on,
        cue.trig_delay.val as f64 / TPB,
        0.25,
        0.0..=32.0,
        " b",
        2,
    ) {
        let val = (v * TPB).round().max(0.0) as u32;
        let _ = tx.send(Command::SetCueParam(cue.id, CueParam::TrigDelay(Toggle { on, val })));
    }

    ui.add_space(SP_LG);
    speed_section(ui, cue, tx);
}

/// Dwell: an "inherit" toggle plus a beats `DragValue` when overridden.
fn dwell_row(ui: &mut Ui, m: &UiMirror, cue: &CueView, tx: &Sender<Command>) {
    ui.horizontal(|ui| {
        let mut inherit = cue.dwell.is_none();
        if ui.checkbox(&mut inherit, "inherit").changed() {
            let cmd = (!inherit).then(|| m.phrase_len.max(1) * LOOP_TICKS_PER_BEAT);
            let _ = tx.send(Command::SetCueParam(cue.id, CueParam::Dwell(cmd)));
        }
        match cue.dwell {
            Some(ticks) => {
                let mut beats = ticks as f64 / TPB;
                if ui
                    .add(
                        egui::DragValue::new(&mut beats)
                            .speed(0.25)
                            .range(0.25..=256.0)
                            .suffix(" b")
                            .fixed_decimals(2),
                    )
                    .changed()
                {
                    let val = (beats * TPB).round().max(1.0) as u32;
                    let _ = tx.send(Command::SetCueParam(cue.id, CueParam::Dwell(Some(val))));
                }
            }
            None => {
                ui.label(
                    egui::RichText::new(format!("{} b (global)", m.phrase_len))
                        .color(PALETTE.fg_muted),
                );
            }
        }
    });
}

/// Loop rate: inherit / off / one of the shared cadences, via a combo box.
fn loop_row(ui: &mut Ui, cue: &CueView, tx: &Sender<Command>) {
    let selected = match cue.loop_len {
        None => "inherit".to_string(),
        Some(0) => "off".to_string(),
        Some(t) => LOOP_CADENCE
            .iter()
            .find(|(_, tk)| *tk == t)
            .map(|(l, _)| (*l).to_string())
            .unwrap_or_else(|| format!("{t} tk")),
    };
    egui::ComboBox::from_id_salt("cue_loop")
        .selected_text(selected)
        .show_ui(ui, |ui| {
            if ui.selectable_label(cue.loop_len.is_none(), "inherit").clicked() {
                let _ = tx.send(Command::SetCueParam(cue.id, CueParam::Loop(None)));
            }
            if ui.selectable_label(cue.loop_len == Some(0), "off").clicked() {
                let _ = tx.send(Command::SetCueParam(cue.id, CueParam::Loop(Some(0))));
            }
            for (label, ticks) in LOOP_CADENCE {
                if ui.selectable_label(cue.loop_len == Some(ticks), label).clicked() {
                    let _ = tx.send(Command::SetCueParam(cue.id, CueParam::Loop(Some(ticks))));
                }
            }
        });
}

/// A toggle + `DragValue` row for an offset param. Returns `(on, value)` when the
/// user changed either. Value is retained (shown greyed) while switched off.
#[allow(clippy::too_many_arguments)]
fn param_row(
    ui: &mut Ui,
    label: &str,
    hover: &str,
    on: bool,
    val: f64,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
    suffix: &str,
    decimals: usize,
) -> Option<(bool, f64)> {
    let mut out = None;
    ui.horizontal(|ui| {
        let mut o = on;
        if ui.checkbox(&mut o, "").on_hover_text(hover).changed() {
            out = Some((o, val));
        }
        let color = if o { PALETTE.fg_secondary } else { PALETTE.fg_muted };
        ui.label(egui::RichText::new(label).color(color));
        ui.add_enabled_ui(o, |ui| {
            let mut v = val;
            if ui
                .add(
                    egui::DragValue::new(&mut v)
                        .speed(speed)
                        .range(range)
                        .suffix(suffix)
                        .fixed_decimals(decimals),
                )
                .changed()
            {
                out = Some((o, v));
            }
        });
    });
    out
}

/// Speed: clip/cue BPM metadata, the BPM-sync toggle, the user multiplier, and
/// the resolved effective-speed readout. The two toggles stack.
fn speed_section(ui: &mut Ui, cue: &CueView, tx: &Sender<Command>) {
    widgets::section_label(ui, "speed").on_hover_text(
        "Playback speed = BPM-sync factor × user multiplier. Both stack; either can be off.",
    );
    // Clip-level BPM metadata (shared by every cue on this clip).
    ui.horizontal(|ui| {
        let mut has = cue.clip_bpm.is_some();
        if ui
            .checkbox(&mut has, "clip bpm")
            .on_hover_text("Source tempo of the underlying clip, shared by every cue on it.")
            .changed()
        {
            let _ = tx.send(Command::SetClipBpm(cue.clip, has.then(|| cue.clip_bpm.unwrap_or(120.0))));
        }
        if let Some(bpm) = cue.clip_bpm {
            let mut v = bpm;
            if ui.add(bpm_drag(&mut v)).changed() {
                let _ = tx.send(Command::SetClipBpm(cue.clip, Some(v)));
            }
        }
    });
    // Per-cue BPM override.
    ui.horizontal(|ui| {
        let mut has = cue.bpm.is_some();
        if ui
            .checkbox(&mut has, "cue bpm")
            .on_hover_text("Override the clip's tempo for just this cue.")
            .changed()
        {
            let seed = cue.bpm.or(cue.clip_bpm).unwrap_or(120.0);
            let _ = tx.send(Command::SetCueParam(cue.id, CueParam::Bpm(has.then_some(seed))));
        }
        if let Some(bpm) = cue.bpm {
            let mut v = bpm;
            if ui.add(bpm_drag(&mut v)).changed() {
                let _ = tx.send(Command::SetCueParam(cue.id, CueParam::Bpm(Some(v))));
            }
        }
    });
    // BPM-sync toggle (needs a known source tempo).
    let source_known = cue.bpm.or(cue.clip_bpm).is_some();
    ui.add_enabled_ui(source_known, |ui| {
        let mut sync = cue.bpm_sync_on;
        if ui
            .checkbox(&mut sync, "sync to tempo")
            .on_hover_text("Retime playback so the clip runs at the session tempo (needs a source BPM).")
            .changed()
        {
            let _ = tx.send(Command::SetCueParam(cue.id, CueParam::BpmSync(sync)));
        }
    });
    // User multiplier, stacked on top.
    ui.horizontal(|ui| {
        let mut on = cue.speed_mul.on;
        if ui.checkbox(&mut on, "× mult").changed() {
            let _ = tx.send(Command::SetCueParam(
                cue.id,
                CueParam::SpeedMul(Toggle { on, val: cue.speed_mul.val }),
            ));
        }
        ui.add_enabled_ui(on, |ui| {
            let mut v = cue.speed_mul.val;
            if ui
                .add(
                    egui::DragValue::new(&mut v)
                        .speed(0.01)
                        .range(0.05..=20.0)
                        .suffix("×")
                        .fixed_decimals(2),
                )
                .changed()
            {
                let _ = tx.send(Command::SetCueParam(cue.id, CueParam::SpeedMul(Toggle { on, val: v })));
            }
        });
    });
    ui.label(
        egui::RichText::new(format!("→ {:.2}× effective", cue.speed))
            .monospace()
            .color(PALETTE.fg_secondary),
    );
}

/// A BPM `DragValue`, shared by the clip and cue tempo fields.
fn bpm_drag(v: &mut f64) -> egui::DragValue<'_> {
    egui::DragValue::new(v).speed(0.5).range(20.0..=400.0).suffix(" bpm").fixed_decimals(1)
}
