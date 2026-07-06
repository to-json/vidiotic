//! Transport panel: BPM readout/entry, tap controls, the beat/bar indicator,
//! sync source, and the auto-advance/re-loop cadence controls.

use crossbeam_channel::Sender;
use egui::Ui;

use super::theme::{PALETTE, SP_MD, SP_SM};
use super::widgets;
use crate::commands::{Command, SyncKind, UiMirror};

/// Uniform square size for the downbeat/reset/tempo tap buttons.
const TAP_BUTTON_SIZE: egui::Vec2 = egui::Vec2::splat(46.0);

/// Bar-based cadence choices for the "Next every" (sequencer) grid.
const CADENCE_BARS: [u32; 5] = [1, 2, 4, 8, 16];

/// "Loop every" cadence: (label, ticks) at 32 ticks/beat (LOOP_TICKS_PER_BEAT).
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

fn bars_label(bars: u32) -> String {
    if bars == 1 {
        "1 bar".to_string()
    } else {
        format!("{bars} bars")
    }
}

/// The top panel: BPM hero, tap controls, beat indicator, sync, and cadence rows.
pub(super) fn show(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    egui::Panel::top(egui::Id::new("transport")).show(ui, |ui| {
        ui.add_space(SP_SM);
        ui.horizontal(|ui| {
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
            if ui
                .add(egui::Button::new(egui::RichText::new("1").size(24.0)).min_size(TAP_BUTTON_SIZE))
                .on_hover_text("Downbeat: snap to now (phase only, nearest bar). Key: t / Space")
                .clicked()
                || (!ui.ctx().egui_wants_keyboard_input()
                    && ui.input(|i| i.key_pressed(egui::Key::Space)))
            {
                let _ = tx.send(Command::TapDownbeat);
            }
            if ui
                .add(egui::Button::new(egui::RichText::new("↺").size(24.0)).min_size(TAP_BUTTON_SIZE))
                .on_hover_text("Soft reset: clock to bar 1, beat 1. Playlist position and playhead unchanged.")
                .clicked()
            {
                let _ = tx.send(Command::SoftReset);
            }
            if ui
                .add(
                    egui::Button::new(egui::RichText::new("⏮").size(24.0).color(PALETTE.error))
                        .min_size(TAP_BUTTON_SIZE),
                )
                .on_hover_text(
                    "Hard reset: soft reset, AND jump the playlist back to its first cue \
                     and restart its playhead from the in-point.",
                )
                .clicked()
            {
                let _ = tx.send(Command::HardReset);
            }
            if ui
                .add(egui::Button::new(egui::RichText::new("♩").size(24.0)).min_size(TAP_BUTTON_SIZE))
                .on_hover_text("Tap tempo: tap 2+ times to set BPM from the interval. Key: b")
                .clicked()
            {
                let _ = tx.send(Command::TapTempo);
            }
            ui.add_space(SP_MD);
            ui.vertical(|ui| {
                ui.label(format!(
                    "bar {}/{}",
                    m.bar_in_phrase + 1,
                    m.bars_per_phrase.max(1)
                ));
                // beat pulse dot
                let pulse = 1.0 - (m.phase / m.quantum.max(1.0)) as f32;
                let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                let g = (0.25 + 0.75 * pulse).clamp(0.0, 1.0);
                ui.painter().circle_filled(
                    r.center(),
                    7.0,
                    PALETTE.bg_inset.lerp_to_gamma(PALETTE.playing, g),
                );
            });
            ui.add_space(SP_MD);
            egui::ComboBox::from_id_salt("sync")
                .selected_text(match m.sync.unwrap_or(SyncKind::Internal) {
                    SyncKind::Internal => "Internal",
                    SyncKind::Link => "Link",
                })
                .show_ui(ui, |ui| {
                    let cur = m.sync.unwrap_or(SyncKind::Internal);
                    if ui.selectable_label(cur == SyncKind::Internal, "Internal").clicked() {
                        let _ = tx.send(Command::SetSyncSource(SyncKind::Internal));
                    }
                    if ui.selectable_label(cur == SyncKind::Link, "Link").clicked() {
                        let _ = tx.send(Command::SetSyncSource(SyncKind::Link));
                    }
                });
        });
        ui.add_space(SP_SM);
        // Cadence controls, in bars. `Next` = how often the sequencer advances to
        // the next active clip; `Loop` = how often the current clip restarts.
        ui.horizontal(|ui| {
            ui.label("Next every:")
                .on_hover_text("Beats between auto-transitions to the next active clip");
            for bars in CADENCE_BARS {
                let beats = bars * 4;
                if ui
                    .selectable_label(m.phrase_len == beats, bars_label(bars))
                    .clicked()
                {
                    let _ = tx.send(Command::SetPhraseLen(beats));
                }
            }
            ui.add_space(SP_MD);
            ui.label("Loop every:")
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
    });
}
