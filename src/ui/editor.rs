//! The cue editor: the right-hand panel showing the selected cue's in/out
//! trim, preserve-playhead override, and effect chain. ISF parameters render
//! in the phosphor idiom — floats are glyph faders, longs are bracket lists,
//! bools are `[x]` checkboxes.

use crossbeam_channel::Sender;
use egui::Ui;

use phosphor::icon;
use phosphor::theme::{palette, SP_LG, SP_MD, SP_SM};
use phosphor::widgets;

use super::{fmt_time, LOOP_CADENCE};
use crate::bank::{CamDelay, Toggle};
use crate::commands::{
    ChainSlot, ClipRole, Command, CueParam, CueView, SlotRef, UiMirror, LOOP_TICKS_PER_BEAT,
};
use crate::video::capture::DELAY_CAP;
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
    let p = palette();
    ui.vertical_centered(|ui| {
        ui.add_space(SP_MD * 3.0);
        ui.label(egui::RichText::new("No cue selected").color(p.fg_secondary));
        ui.label(
            egui::RichText::new("Double-click a clip to add a cue, then click it here to edit")
                .small()
                .color(p.fg_muted),
        );
        ui.add_space(SP_MD * 3.0);
    });
}

/// The fields for one cue: header, in/out trim, per-cue preserve, effect
/// chain, and a bottom-anchored remove button.
fn cue_editor(ui: &mut Ui, m: &UiMirror, cue: &CueView, tx: &Sender<Command>) {
    let p = palette();
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(cue.name.as_ref()).color(p.fg_primary));
        let (role_text, role_tint) = match cue.role {
            ClipRole::Playing => ("playing", Some(p.playing)),
            ClipRole::Armed => ("armed", Some(p.armed)),
            ClipRole::None => ("idle", None),
        };
        widgets::chip(ui, role_text, role_tint, false);
        widgets::chip(ui, &format!("#{}", cue.id), None, false);
    });
    ui.label(
        egui::RichText::new(format!("⏱ {}", fmt_time(m.playhead_sec)))
            .monospace()
            .color(p.fg_secondary),
    );
    ui.add_space(SP_MD);

    ui.style_mut().drag_value_text_style = egui::TextStyle::Monospace;
    // The fields scroll: advanced mode adds enough rows to overflow a short window.
    egui::ScrollArea::vertical()
        .id_salt("cue_fields")
        .auto_shrink([false, false])
        .show(ui, |ui| cue_fields(ui, m, cue, tx));
}

/// The scrollable field stack: trim, preserve, chain, the advanced sections
/// (when enabled), and the remove button.
fn cue_fields(ui: &mut Ui, m: &UiMirror, cue: &CueView, tx: &Sender<Command>) {
    let p = palette();
    // Camera cues have no timeline: the trim grid gives way to the live-delay
    // controls, and the seek-dependent knobs below render greyed and inert.
    if cue.camera {
        ui.label(
            egui::RichText::new("live camera — no timeline to trim")
                .small()
                .color(p.fg_muted),
        );
        ui.add_space(SP_MD);
        delay_section(ui, m, cue, tx);
    }
    if !cue.camera {
    egui::Grid::new("cue_trim").num_columns(2).spacing(egui::vec2(SP_SM, SP_SM)).show(ui, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new("in").color(p.fg_muted));
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
            if widgets::bracket_button(ui, "⏺", Some(p.accent), 0.0)
                .on_hover_text("set in-point to the current playhead")
                .clicked()
            {
                let _ = tx.send(Command::SetCueInToPlayhead(cue.id));
            }
        });
        ui.end_row();

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new("out").color(p.fg_muted));
        });
        ui.horizontal(|ui| {
            let mut trimmed = cue.out_sec.is_some();
            if widgets::glyph_checkbox(ui, &mut trimmed, "")
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
                    if widgets::bracket_button(ui, "⏺", Some(p.accent), 0.0)
                        .on_hover_text("set out-point to the current playhead")
                        .clicked()
                    {
                        let _ = tx.send(Command::SetCueOutToPlayhead(cue.id));
                    }
                }
                None => {
                    ui.label(egui::RichText::new("clip end").color(p.fg_muted));
                }
            }
        });
        ui.end_row();
    });
    }

    ui.add_space(SP_LG);
    // Preserve is meaningless once restart is a no-op (camera): greyed, inert.
    ui.add_enabled_ui(!cue.camera, |ui| {
        widgets::section_label(ui, "preserve playhead").on_hover_text(
            "On a cut, carry the playhead into this cue. Inherit follows the global toggle.",
        );
        let preserve_idx = match cue.preserve {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        };
        if let Some(i) =
            widgets::segmented(ui, "cue_preserve", &["inherit", "on", "off"], Some(preserve_idx))
        {
            let val = match i {
                0 => None,
                1 => Some(true),
                _ => Some(false),
            };
            if !cue.camera {
                let _ = tx.send(Command::SetCuePreserve(cue.id, val));
            }
        }
    });

    ui.add_space(SP_LG);
    chain_section(ui, m, cue, tx);

    if m.advanced {
        advanced_sections(ui, m, cue, tx);
    }

    ui.add_space(SP_LG);
    if widgets::bracket_button(ui, "remove cue", Some(p.error), 0.0)
        .on_hover_text("Remove this cue from the bank")
        .clicked()
    {
        let _ = tx.send(Command::RemoveCue(cue.id));
    }
    ui.add_space(SP_SM);
    ui.label(
        egui::RichText::new("Trim, timing & speed apply the next time this cue is triggered.")
            .small()
            .color(p.fg_muted),
    );
    ui.add_space(SP_SM);
}

/// The camera cue's voluntary live delay: a fader dialed in seconds or beats
/// (unit toggle re-expresses the value so the resolved delay stays put), plus
/// the quantize-to-loop-grid toggle and the current effective readout. Beats
/// mode shows its clamp whenever `beats × 60/bpm` exceeds the ring window.
fn delay_section(ui: &mut Ui, m: &UiMirror, cue: &CueView, tx: &Sender<Command>) {
    let p = palette();
    let d = cue.delay;
    let send = |d: CamDelay| {
        let _ = tx.send(Command::SetCueParam(cue.id, CueParam::CamDelay(d)));
    };
    widgets::section_label(ui, "live delay").on_hover_text(
        "Play this camera behind the live feed. Changes glide toward the new value \
         (or snap at loop-grid boundaries with quantize on).",
    );
    let bpm = m.bpm.max(1.0);
    ui.horizontal(|ui| {
        if let Some(i) =
            widgets::segmented(ui, "cam_delay_unit", &["seconds", "beats"], Some(usize::from(d.beats)))
        {
            let beats = i == 1;
            if beats != d.beats {
                let value = if beats { d.value * bpm / 60.0 } else { d.value * 60.0 / bpm };
                send(CamDelay { value, beats, ..d });
            }
        }
    });
    ui.horizontal(|ui| {
        let max = if d.beats { (DELAY_CAP * bpm / 60.0).max(1.0) } else { DELAY_CAP };
        let mut v = d.value as f32;
        if widgets::fader(ui, "cam_delay_fader", 0.0, max as f32, &mut v, 12).changed() {
            send(CamDelay { value: f64::from(v), ..d });
        }
        let unit = if d.beats { "b" } else { "s" };
        ui.label(
            egui::RichText::new(format!("{:>5.2} {unit}", d.value))
                .monospace()
                .color(p.fg_primary),
        );
    });
    if d.beats && d.seconds(bpm) > DELAY_CAP {
        ui.label(
            egui::RichText::new(format!("clamped to {DELAY_CAP:.1} s at {bpm:.0} bpm"))
                .small()
                .color(p.armed),
        );
    }
    let mut q = d.quantize;
    if widgets::glyph_checkbox(ui, &mut q, "quantize to loop grid")
        .on_hover_text("Apply delay changes exactly on loop-grid boundaries instead of gliding.")
        .changed()
    {
        send(CamDelay { quantize: q, ..d });
    }
    ui.label(
        egui::RichText::new(format!("→ {:.2} s effective", cue.delay_eff))
            .monospace()
            .color(p.fg_secondary),
    );
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
    let p = palette();
    widgets::section_label(ui, "effect chain").on_hover_text(
        "Stack shaders applied while this cue plays. Each stage feeds the next via prev(); \
         empty runs the live shader. The live shader can sit anywhere in the stack.",
    );

    if cue.chain.is_empty() {
        ui.label(
            egui::RichText::new("No effects — runs the live shader.")
                .small()
                .color(p.fg_muted),
        );
    }

    let n = cue.chain.len();
    for (i, slot) in cue.chain.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{}.", i + 1)).color(p.fg_muted));
            // Right-aligned reorder / remove controls; the name truncates
            // into whatever width they leave on a narrow panel.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(i + 1 < n, |ui| {
                    if widgets::bracket_button(ui, icon::MOVE_DOWN, None, 0.0)
                        .on_hover_text("Move down")
                        .clicked()
                    {
                        let mut c = cue.chain.clone();
                        c.swap(i, i + 1);
                        let _ = tx.send(Command::SetCueChain(cue.id, c));
                    }
                });
                ui.add_enabled_ui(i > 0, |ui| {
                    if widgets::bracket_button(ui, icon::MOVE_UP, None, 0.0)
                        .on_hover_text("Move up")
                        .clicked()
                    {
                        let mut c = cue.chain.clone();
                        c.swap(i, i - 1);
                        let _ = tx.send(Command::SetCueChain(cue.id, c));
                    }
                });
                if widgets::bracket_button(ui, icon::DELETE, Some(p.error), 0.0)
                    .on_hover_text("Remove from chain")
                    .clicked()
                {
                    let mut c = cue.chain.clone();
                    c.remove(i);
                    let _ = tx.send(Command::SetCueChain(cue.id, c));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(slot_label(&slot.shader, m)).color(p.fg_primary),
                        )
                        .truncate(),
                    );
                });
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
        .selected_text("+ add effect")
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
            if ui.selectable_label(false, "+ Load ISF file…").clicked() {
                crate::ui::pick_file(tx.clone(), crate::ui::PickKind::Isf);
            }
        });
}

/// Inline controls for one ISF slot's declared inputs, in the phosphor
/// vocabulary: floats are faders, longs are bracket lists, bools are `[x]`
/// checkboxes, events are bracket buttons. Each edit sends a
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
    let p = palette();
    if inputs.iter().all(|i| i.kind.is_texture()) {
        return; // nothing tweakable — every input is a texture (image/audio)
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
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(label.to_lowercase()).color(p.fg_muted));
                        if widgets::fader(
                            ui,
                            ("isf_fader", slot_index, input.name.as_str()),
                            *min,
                            *max,
                            &mut v,
                            12,
                        )
                        .changed()
                        {
                            send(&input.name, IsfValue::Float(v));
                        }
                        ui.label(egui::RichText::new(format!("{v:>7.2}")).monospace().color(p.fg_primary));
                    });
                }
                IsfInputKind::Bool { default } => {
                    let mut v = match slot.param(&input.name) {
                        Some(IsfValue::Bool(b)) => *b,
                        _ => *default,
                    };
                    if widgets::glyph_checkbox(ui, &mut v, &label.to_lowercase()).changed() {
                        send(&input.name, IsfValue::Bool(v));
                    }
                }
                IsfInputKind::Long { values, labels, default } => {
                    let cur = match slot.param(&input.name) {
                        Some(IsfValue::Long(i)) => *i,
                        _ => *default,
                    };
                    let selected = values.iter().position(|v| *v == cur);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(label.to_lowercase()).color(p.fg_muted));
                        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                        if let Some(idx) = widgets::segmented(
                            ui,
                            ("isf_long", slot_index, input.name.as_str()),
                            &labels,
                            selected,
                        ) {
                            if let Some(val) = values.get(idx) {
                                send(&input.name, IsfValue::Long(*val));
                            }
                        }
                    });
                }
                IsfInputKind::Color { default } => {
                    let cur = match slot.param(&input.name) {
                        Some(IsfValue::Color(c)) => *c,
                        _ => *default,
                    };
                    let mut rgba = egui::Rgba::from_rgba_unmultiplied(cur[0], cur[1], cur[2], cur[3]);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(label.to_lowercase()).color(p.fg_muted));
                        let swatch = egui::Color32::from(rgba);
                        ui.label(egui::RichText::new("███").monospace().color(swatch));
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
                        Some(IsfValue::Point2D(pt)) => *pt,
                        _ => *default,
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new(label.to_lowercase()).color(p.fg_muted));
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
                    if widgets::bracket_button(ui, &label.to_lowercase(), None, 0.0).clicked() {
                        send(&input.name, IsfValue::Bool(true));
                    }
                }
                // Texture inputs have no scalar widget.
                IsfInputKind::Image | IsfInputKind::Audio | IsfInputKind::AudioFft => {}
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
    // Nudge shifts the in-point — meaningless without a timeline.
    ui.add_enabled_ui(!cue.camera, |ui| {
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
            if !cue.camera {
                let _ = tx
                    .send(Command::SetCueParam(cue.id, CueParam::StartNudge(Toggle { on, val: v })));
            }
        }
    });
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
    // Speed (BPM-sync × multiplier) is pacing math on a seekable timeline; a
    // live feed is already real-time. Greyed and inert for camera cues.
    ui.add_enabled_ui(!cue.camera, |ui| {
        speed_section(ui, cue, tx);
    });
}

/// Dwell: an "inherit" toggle plus a beats `DragValue` when overridden.
fn dwell_row(ui: &mut Ui, m: &UiMirror, cue: &CueView, tx: &Sender<Command>) {
    let p = palette();
    ui.horizontal(|ui| {
        let mut inherit = cue.dwell.is_none();
        if widgets::glyph_checkbox(ui, &mut inherit, "inherit").changed() {
            let cmd = (!inherit)
                .then(|| (m.phrase_beats.max(1.0) * LOOP_TICKS_PER_BEAT as f64).round() as u32);
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
                    egui::RichText::new(format!("{:.2} b (global)", m.phrase_beats))
                        .color(p.fg_muted),
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
    let p = palette();
    let mut out = None;
    ui.horizontal(|ui| {
        let mut o = on;
        if widgets::glyph_checkbox(ui, &mut o, "").on_hover_text(hover).changed() {
            out = Some((o, val));
        }
        let color = if o { p.fg_secondary } else { p.fg_muted };
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
    let p = palette();
    widgets::section_label(ui, "speed").on_hover_text(
        "Playback speed = BPM-sync factor × user multiplier. Both stack; either can be off.",
    );
    // Clip-level BPM metadata (shared by every cue on this clip).
    ui.horizontal(|ui| {
        let mut has = cue.clip_bpm.is_some();
        if widgets::glyph_checkbox(ui, &mut has, "clip bpm")
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
        if widgets::glyph_checkbox(ui, &mut has, "cue bpm")
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
        if widgets::glyph_checkbox(ui, &mut sync, "sync to tempo")
            .on_hover_text("Retime playback so the clip runs at the session tempo (needs a source BPM).")
            .changed()
        {
            let _ = tx.send(Command::SetCueParam(cue.id, CueParam::BpmSync(sync)));
        }
    });
    // User multiplier, stacked on top.
    ui.horizontal(|ui| {
        let mut on = cue.speed_mul.on;
        if widgets::glyph_checkbox(ui, &mut on, "× mult").changed() {
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
            .color(p.fg_secondary),
    );
}

/// A BPM `DragValue`, shared by the clip and cue tempo fields.
fn bpm_drag(v: &mut f64) -> egui::DragValue<'_> {
    egui::DragValue::new(v).speed(0.5).range(20.0..=400.0).suffix(" bpm").fixed_decimals(1)
}
