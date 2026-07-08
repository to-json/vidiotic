//! Transport panel: BPM readout/entry, tap controls, the beat/bar indicator,
//! sync source, and the auto-advance/re-loop cadence controls.

use crossbeam_channel::Sender;
use egui::Ui;

use super::theme::{PALETTE, SP_MD, SP_SM, SP_XS};
use super::widgets;
use crate::commands::{Command, SyncKind, UiMirror};

/// Uniform square size for the downbeat/reset/tempo tap buttons.
const TAP_BUTTON_SIZE: egui::Vec2 = egui::Vec2::splat(46.0);

/// Bar-based cadence choices for the "Next every" (sequencer) grid: (label, bars).
const CADENCE_BARS: [(&str, u32); 5] = [
    ("1 bar", 1),
    ("2 bars", 2),
    ("4 bars", 4),
    ("8 bars", 8),
    ("16 bars", 16),
];

/// "Loop every" cadence: (label, ticks) at 32 ticks/beat (`LOOP_TICKS_PER_BEAT`).
/// A beat is a quarter note (32), so an eighth note is 16 and a 4/4 bar is 128.
/// Goes sub-bar for beat-roll/stutter effects.
const LOOP_CADENCE: [(&str, u32); 8] = [
    ("1/8", 16),
    ("1/4", 32),
    ("1/2", 64),
    ("1 bar", 128),
    ("2 bars", 256),
    ("4 bars", 512),
    ("8 bars", 1024),
    ("16 bars", 2048),
];

/// Run one tap button through the flash-decay temp-memory pattern: paints at
/// the previous frame's decayed flash, then bumps it back to 1.0 if triggered
/// this frame (by click or `extra_trigger`, an egui-side key shortcut).
/// Winit-side keys (handled outside the egui frame) aren't worth plumbing a
/// trigger for, per the elegance plan.
fn tap_button(
    ui: &mut Ui,
    id_salt: &str,
    label: &str,
    text_color: egui::Color32,
    hover: &str,
    extra_trigger: bool,
) -> bool {
    let id = ui.id().with(id_salt);
    let mut flash: f32 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(0.0) * 0.85;
    let resp = widgets::transport_button(ui, label, TAP_BUTTON_SIZE, 24.0, text_color, flash)
        .on_hover_text(hover);
    let triggered = resp.clicked() || extra_trigger;
    if triggered {
        flash = 1.0;
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, flash));
    triggered
}

/// One dot per beat in the bar: the current beat is brightest and fades
/// toward the next; the downbeat (index 0) is tinted `playing` instead of
/// `accent` so bar starts read at a glance.
fn beat_dots(ui: &mut Ui, m: &UiMirror) {
    let n = m.quantum.round().max(1.0) as usize;
    let current = m.phase.floor() as usize % n;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = SP_XS;
        for i in 0..n {
            let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(8.0), egui::Sense::hover());
            let color = if i == current {
                let bright = (1.0 - m.phase.fract() as f32).powi(2).clamp(0.0, 1.0);
                let base = if i == 0 { PALETTE.playing } else { PALETTE.accent };
                PALETTE.bg_elevated.lerp_to_gamma(base, bright)
            } else {
                PALETTE.bg_elevated
            };
            ui.painter().circle_filled(rect.center(), 4.0, color);
        }
    });
}

/// Time-to-next-cut, as a 3px full-width strip: `accent_dim` fill up to the
/// phrase fraction with an `accent` head, tick marks at each bar boundary.
fn phrase_strip(ui: &mut Ui, m: &UiMirror) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 3.0), egui::Sense::hover());
    let phrase_len = m.phrase_len.max(1) as f64;
    let quantum = m.quantum.max(1.0);
    let progressed = m.bar_in_phrase as f64 * quantum + m.phase;
    let frac = (progressed / phrase_len).clamp(0.0, 1.0) as f32;

    let painter = ui.painter();
    painter.rect_filled(rect, egui::CornerRadius::ZERO, PALETTE.bg_inset);
    let fill_w = rect.width() * frac;
    if fill_w > 0.0 {
        let fill = egui::Rect::from_min_size(rect.min, egui::vec2(fill_w, rect.height()));
        painter.rect_filled(fill, egui::CornerRadius::ZERO, PALETTE.accent_dim);
        let head_x = (rect.min.x + fill_w - 1.5).min(rect.max.x - 1.5).max(rect.min.x);
        let head = egui::Rect::from_min_size(egui::pos2(head_x, rect.min.y), egui::vec2(1.5, rect.height()));
        painter.rect_filled(head, egui::CornerRadius::ZERO, PALETTE.accent);
    }
    let bars = (phrase_len / quantum).round().max(1.0) as usize;
    for i in 1..bars {
        let x = rect.min.x + rect.width() * (i as f32 / bars as f32);
        painter.line_segment(
            [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
            egui::Stroke::new(1.0, PALETTE.border),
        );
    }
}

/// The top panel: BPM hero, tap controls, beat indicator, sync, and cadence rows.
pub(super) fn show(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    egui::Panel::top(egui::Id::new("transport")).show(ui, |ui| {
        ui.add_space(SP_SM);
        ui.horizontal(|ui| {
            if let Some(entry) = &m.bpm_entry {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{entry}▏"))
                            .monospace()
                            .size(40.0)
                            .strong()
                            .color(PALETTE.accent),
                    );
                    ui.label(egui::RichText::new("Enter to set").small().color(PALETTE.fg_muted));
                });
            } else {
                ui.label(
                    egui::RichText::new(format!("{:6.1}", m.bpm))
                        .monospace()
                        .size(40.0)
                        .strong(),
                );
                ui.vertical(|ui| {
                    let mut bpm = m.bpm;
                    if ui
                        .add(
                            egui::DragValue::new(&mut bpm)
                                .speed(0.1)
                                .range(20.0..=300.0)
                                .fixed_decimals(1),
                        )
                        .changed()
                    {
                        let _ = tx.send(Command::SetBpm(bpm));
                    }
                    ui.horizontal(|ui| {
                        if ui.button("−0.1%").clicked() {
                            let _ = tx.send(Command::NudgeBpm(-0.001));
                        }
                        if ui.button("+0.1%").clicked() {
                            let _ = tx.send(Command::NudgeBpm(0.001));
                        }
                    });
                });
            }

            let downbeat_key = !ui.ctx().egui_wants_keyboard_input()
                && ui.input(|i| i.key_pressed(egui::Key::Space));
            if tap_button(
                ui,
                "downbeat",
                "1",
                PALETTE.fg_primary,
                "Downbeat: snap to now (phase only, nearest bar). Key: t / Space",
                downbeat_key,
            ) {
                let _ = tx.send(Command::TapDownbeat);
            }
            if tap_button(
                ui,
                "soft_reset",
                "↺",
                PALETTE.fg_primary,
                "Soft reset: clock to bar 1, beat 1. Playlist position and playhead unchanged.",
                false,
            ) {
                let _ = tx.send(Command::SoftReset);
            }
            if tap_button(
                ui,
                "hard_reset",
                "⏮",
                PALETTE.error,
                "Hard reset: soft reset, AND jump the playlist back to its first cue \
                 and restart its playhead from the in-point.",
                false,
            ) {
                let _ = tx.send(Command::HardReset);
            }
            if tap_button(
                ui,
                "tempo",
                "♩",
                PALETTE.fg_primary,
                "Tap tempo: tap 2+ times to set BPM from the interval. Key: b",
                false,
            ) {
                let _ = tx.send(Command::TapTempo);
            }

            ui.add_space(SP_MD);
            ui.vertical(|ui| {
                ui.label(format!(
                    "bar {}/{}",
                    m.bar_in_phrase + 1,
                    m.bars_per_phrase.max(1)
                ));
                beat_dots(ui, m);
            });

            ui.add_space(SP_MD);
            let sync_idx = match m.sync.unwrap_or(SyncKind::Internal) {
                SyncKind::Internal => 0,
                SyncKind::Link => 1,
            };
            if let Some(i) = widgets::segmented(ui, "sync", &["Internal", "Link"], Some(sync_idx)) {
                let kind = if i == 0 { SyncKind::Internal } else { SyncKind::Link };
                let _ = tx.send(Command::SetSyncSource(kind));
            }
            if m.sync == Some(SyncKind::Link) && m.peers > 0 {
                ui.add_space(SP_SM);
                widgets::chip(ui, &format!("{} peers", m.peers), Some(PALETTE.playing), false);
            }
        });
        ui.add_space(SP_SM);
        // Cadence controls, in bars. `Next` = how often the sequencer advances to
        // the next active clip; `Loop` = how often the current clip restarts.
        ui.horizontal(|ui| {
            widgets::section_label(ui, "next every")
                .on_hover_text("Beats between auto-transitions to the next active clip");
            let next_labels: Vec<&str> = CADENCE_BARS.iter().map(|(l, _)| *l).collect();
            let next_selected = CADENCE_BARS.iter().position(|(_, bars)| bars * 4 == m.phrase_len);
            if let Some(i) = widgets::segmented(ui, "next_cadence", &next_labels, next_selected) {
                let _ = tx.send(Command::SetPhraseLen(CADENCE_BARS[i].1 * 4));
            }

            ui.add_space(SP_MD);
            widgets::section_label(ui, "loop every")
                .on_hover_text("Force the current clip back to its start on this beat grid");
            let loop_labels: Vec<&str> =
                std::iter::once("off").chain(LOOP_CADENCE.iter().map(|(l, _)| *l)).collect();
            let loop_selected = m.loop_len.map_or(0, |beats| {
                LOOP_CADENCE.iter().position(|(_, b)| *b == beats).map_or(0, |i| i + 1)
            });
            if let Some(i) = widgets::segmented(ui, "loop_cadence", &loop_labels, Some(loop_selected)) {
                let beats = if i == 0 { None } else { Some(LOOP_CADENCE[i - 1].1) };
                let _ = tx.send(Command::SetLoopLen(beats));
            }

            ui.add_space(SP_MD);
            let mut preserve = m.preserve_playhead;
            if ui
                .checkbox(&mut preserve, "preserve playhead")
                .on_hover_text(
                    "On a cut, carry the playhead into the next clip (it comes in \
                     already running). Off: the next clip restarts from its start.",
                )
                .changed()
            {
                let _ = tx.send(Command::SetPreservePlayhead(preserve));
            }
        });
        ui.add_space(SP_SM);
        phrase_strip(ui, m);
    });
}
