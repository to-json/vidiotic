//! The cue editor: the right-hand panel showing the selected cue's in/out
//! trim, preserve-playhead override, and shader override.

use crossbeam_channel::Sender;
use egui::Ui;

use super::theme::{PALETTE, SP_LG, SP_MD, SP_SM};
use super::widgets;
use super::{fmt_time, LOOP_CADENCE};
use crate::bank::Toggle;
use crate::commands::{
    ChainSlot, ClipRole, Command, CueParam, CueView, SlotRef, UiMirror, LOOP_TICKS_PER_BEAT,
};
use crate::isf::{IsfInput, IsfInputKind, IsfValue};

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
    chain_section(ui, m, cue, tx);

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

/// Human-readable label for a chain slot.
fn slot_label(slot: &SlotRef, m: &UiMirror) -> String {
    match slot {
        SlotRef::Live => "Live shader".to_string(),
        SlotRef::Builtin(name) => name.to_string(),
        SlotRef::Pinned(id) => m
            .shader_pool
            .iter()
            .find(|s| s.id == *id)
            .map(|s| s.name.to_string())
            .unwrap_or_else(|| format!("pin #{id}")),
        SlotRef::Isf(path) => {
            let stem = std::path::Path::new(path.as_ref())
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(path.as_ref());
            format!("ISF: {stem}")
        }
    }
}

/// The per-cue effect chain: an ordered stack of shaders. Each stage reads the
/// previous stage's output via `prev()`; empty = the live shader. Rows can be
/// reordered and removed; the combo appends a slot.
fn chain_section(ui: &mut Ui, m: &UiMirror, cue: &CueView, tx: &Sender<Command>) {
    widgets::section_label(ui, "effect chain").on_hover_text(
        "Stack shaders applied while this cue plays. Each stage feeds the next via prev(); \
         empty runs the live shader. The live shader can sit anywhere in the stack.",
    );

    if cue.chain.is_empty() {
        ui.label(
            egui::RichText::new("No effects — runs the live shader.")
                .small()
                .color(PALETTE.fg_muted),
        );
    }

    let n = cue.chain.len();
    for (i, slot) in cue.chain.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("{}.", i + 1));
            ui.label(slot_label(&slot.shader, m));
            // Right-aligned reorder / remove controls.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(i + 1 < n, egui::Button::new("▼").small())
                    .on_hover_text("Move down")
                    .clicked()
                {
                    let mut c = cue.chain.clone();
                    c.swap(i, i + 1);
                    let _ = tx.send(Command::SetCueChain(cue.id, c));
                }
                if ui.add_enabled(i > 0, egui::Button::new("▲").small())
                    .on_hover_text("Move up")
                    .clicked()
                {
                    let mut c = cue.chain.clone();
                    c.swap(i, i - 1);
                    let _ = tx.send(Command::SetCueChain(cue.id, c));
                }
                if ui
                    .add(egui::Button::new(egui::RichText::new("✕").color(PALETTE.error)).small())
                    .on_hover_text("Remove from chain")
                    .clicked()
                {
                    let mut c = cue.chain.clone();
                    c.remove(i);
                    let _ = tx.send(Command::SetCueChain(cue.id, c));
                }
            });
        });

        // ISF slots expose their declared inputs as inline controls.
        if let SlotRef::Isf(path) = &slot.shader {
            if let Some(entry) = m.shader_pool.iter().find(|s| s.name.as_ref() == path.as_ref()) {
                isf_params_ui(ui, cue, i, slot, &entry.inputs, tx);
            }
        }
    }

    // Append control: Live shader, each pool shader (built-ins + pins), and a
    // file picker for loading an ISF `.fs`.
    let append = |slot: SlotRef| {
        let mut c = cue.chain.clone();
        c.push(ChainSlot::new(slot));
        Command::SetCueChain(cue.id, c)
    };
    egui::ComboBox::from_id_salt("cue_chain_add")
        .selected_text("＋ add effect")
        .show_ui(ui, |ui| {
            if ui.selectable_label(false, "Live shader").clicked() {
                let _ = tx.send(append(SlotRef::Live));
            }
            for s in &m.shader_pool {
                let label = if s.inputs.is_empty() {
                    s.name.to_string()
                } else {
                    // ISF pool entries carry a schema; show a friendly stem.
                    let stem = std::path::Path::new(s.name.as_ref())
                        .file_stem()
                        .and_then(|n| n.to_str())
                        .unwrap_or(s.name.as_ref());
                    format!("ISF: {stem}")
                };
                if ui.selectable_label(false, label).clicked() {
                    let slot = if !s.inputs.is_empty() {
                        SlotRef::Isf(s.name.clone())
                    } else if s.builtin {
                        SlotRef::Builtin(s.name.clone())
                    } else {
                        SlotRef::Pinned(s.id)
                    };
                    let _ = tx.send(append(slot));
                }
            }
            ui.separator();
            if ui.selectable_label(false, "＋ Load ISF file…").clicked() {
                crate::ui::pick_file(tx.clone(), crate::ui::PickKind::Isf);
            }
        });
}

/// Inline controls for one ISF slot's declared inputs. Each edit sends a
/// [`Command::SetChainParam`] targeting `(cue, slot_index)` so a drag doesn't
/// replace the whole chain. Current values come from the slot's overrides,
/// falling back to each input's schema default.
fn isf_params_ui(
    ui: &mut Ui,
    cue: &CueView,
    slot_index: usize,
    slot: &ChainSlot,
    inputs: &[IsfInput],
    tx: &Sender<Command>,
) {
    if inputs.iter().all(|i| matches!(i.kind, IsfInputKind::Image)) {
        return; // nothing tweakable
    }
    let send = |name: &str, value: IsfValue| {
        let _ = tx.send(Command::SetChainParam {
            cue: cue.id,
            slot: slot_index,
            name: name.into(),
            value,
        });
    };
    // Rendered directly on `ui` (no ui.indent/push_id, which can break
    // ScrollArea wheel input — see memory `egui-push-id-breaks-scroll`).
    {
        for input in inputs {
            let label = input.label.as_deref().unwrap_or(&input.name);
            match &input.kind {
                IsfInputKind::Float { min, max, default } => {
                    let mut v = match slot.param(&input.name) {
                        Some(IsfValue::Float(f)) => *f,
                        _ => *default,
                    };
                    if ui.add(egui::Slider::new(&mut v, *min..=*max).text(label)).changed() {
                        send(&input.name, IsfValue::Float(v));
                    }
                }
                IsfInputKind::Bool { default } => {
                    let mut v = match slot.param(&input.name) {
                        Some(IsfValue::Bool(b)) => *b,
                        _ => *default,
                    };
                    if ui.checkbox(&mut v, label).changed() {
                        send(&input.name, IsfValue::Bool(v));
                    }
                }
                IsfInputKind::Long { values, labels, default } => {
                    let cur = match slot.param(&input.name) {
                        Some(IsfValue::Long(i)) => *i,
                        _ => *default,
                    };
                    let cur_label = values
                        .iter()
                        .position(|v| *v == cur)
                        .and_then(|idx| labels.get(idx))
                        .map(String::as_str)
                        .unwrap_or("—");
                    ui.horizontal(|ui| {
                        ui.label(label);
                        egui::ComboBox::from_id_salt(("isf_long", slot_index, input.name.as_str()))
                            .selected_text(cur_label)
                            .show_ui(ui, |ui| {
                                for (idx, val) in values.iter().enumerate() {
                                    let lbl = labels.get(idx).map(String::as_str).unwrap_or("?");
                                    if ui.selectable_label(cur == *val, lbl).clicked() {
                                        send(&input.name, IsfValue::Long(*val));
                                    }
                                }
                            });
                    });
                }
                IsfInputKind::Color { default } => {
                    let cur = match slot.param(&input.name) {
                        Some(IsfValue::Color(c)) => *c,
                        _ => *default,
                    };
                    let mut rgba = egui::Rgba::from_rgba_unmultiplied(cur[0], cur[1], cur[2], cur[3]);
                    ui.horizontal(|ui| {
                        ui.label(label);
                        let resp = egui::color_picker::color_edit_button_rgba(
                            ui,
                            &mut rgba,
                            egui::color_picker::Alpha::OnlyBlend,
                        );
                        if resp.changed() {
                            let c = rgba.to_rgba_unmultiplied();
                            send(&input.name, IsfValue::Color([c[0], c[1], c[2], c[3]]));
                        }
                    });
                }
                IsfInputKind::Point2D { min, max, default } => {
                    let mut cur = match slot.param(&input.name) {
                        Some(IsfValue::Point2D(p)) => *p,
                        _ => *default,
                    };
                    ui.horizontal(|ui| {
                        ui.label(label);
                        let dx = ui.add(
                            egui::DragValue::new(&mut cur[0])
                                .speed(0.005)
                                .range(min[0]..=max[0]),
                        );
                        let dy = ui.add(
                            egui::DragValue::new(&mut cur[1])
                                .speed(0.005)
                                .range(min[1]..=max[1]),
                        );
                        if dx.changed() || dy.changed() {
                            send(&input.name, IsfValue::Point2D(cur));
                        }
                    });
                }
                IsfInputKind::Event => {
                    // A momentary trigger: send true on click (it latches for one frame).
                    if ui.button(label).clicked() {
                        send(&input.name, IsfValue::Bool(true));
                    }
                }
                IsfInputKind::Image => {}
            }
        }
    }
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
