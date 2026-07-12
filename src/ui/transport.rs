//! Transport panel: BPM readout/entry, tap controls, the beat/bar indicator,
//! sync source, and the auto-advance/re-loop cadence controls — all in the
//! phosphor buffer idiom: bracket-button taps, `●○○○` beat glyphs, and a
//! `▰▱` phrase strip. The tempo / taps+beat / sync clusters share one line
//! on a wide window and stack as rows on a narrow one.

use crossbeam_channel::Sender;
use egui::Ui;

use super::theme::{self, palette, SP_MD, SP_SM};
use super::widgets;
use crate::commands::{Command, SyncKind, UiMirror};

/// Below this available width the transport clusters stack as rows.
const STACK_BELOW: f32 = 800.0;

/// Bar-based cadence choices for the "Next every" (sequencer) grid:
/// (label, bars). Labels are bare bar counts — the "next every" section
/// label carries the unit.
const CADENCE_BARS: [(&str, u32); 5] = [
    ("1", 1),
    ("2", 2),
    ("4", 4),
    ("8", 8),
    ("16", 16),
];

use super::LOOP_CADENCE;

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
    let resp = widgets::bracket_button(ui, label, Some(text_color), flash).on_hover_text(hover);
    let triggered = resp.clicked() || extra_trigger;
    if triggered {
        flash = 1.0;
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, flash));
    triggered
}

/// One glyph per beat in the bar (`●` current, `○` rest): the current beat is
/// brightest and fades toward the next; the downbeat (index 0) is tinted
/// `phosphor` instead of `accent` so bar starts read at a glance.
fn beat_glyphs(ui: &mut Ui, m: &UiMirror) {
    let p = palette();
    let n = m.quantum.round().max(1.0) as usize;
    let current = m.phase.floor() as usize % n;
    let cw = widgets::cell_width(ui);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(cw * n as f32, theme::ROW), egui::Sense::hover());
    let painter = ui.painter();
    for i in 0..n {
        let (ch, color) = if i == current {
            let bright = (1.0 - m.phase.fract() as f32).powi(2).clamp(0.0, 1.0);
            let base = if i == 0 { p.phosphor } else { p.accent };
            ("●", theme::with_alpha(base, 120 + (bright * 135.0) as u8))
        } else {
            ("○", p.fg_muted)
        };
        painter.text(
            egui::pos2(rect.min.x + i as f32 * cw, rect.center().y),
            egui::Align2::LEFT_CENTER,
            ch,
            theme::mono(),
            color,
        );
    }
}

/// Time-to-next-cut as a `▰▱` glyph strip: one cell per beat (pooled down
/// past 32 beats), filled up to the phrase fraction.
fn phrase_strip(ui: &mut Ui, m: &UiMirror) {
    let p = palette();
    let phrase_len = m.phrase_len.max(1) as f64;
    let quantum = m.quantum.max(1.0);
    let progressed = m.bar_in_phrase as f64 * quantum + m.phase;
    let frac = (progressed / phrase_len).clamp(0.0, 1.0) as f32;

    let cells = (m.phrase_len.max(1) as usize).min(32);
    let filled = (frac * cells as f32).floor() as usize;
    let strip: String = (0..cells).map(|k| if k < filled { '▰' } else { '▱' }).collect();
    ui.label(egui::RichText::new(strip).monospace().color(p.blue));
}

/// BPM hero (or the keyboard-entry readout) plus the drag/nudge stack.
fn bpm_cluster(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    let p = palette();
    if let Some(entry) = &m.bpm_entry {
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(format!("{entry}▏"))
                    .monospace()
                    .size(32.0)
                    .color(p.accent),
            );
            ui.label(egui::RichText::new("enter to set").small().color(p.fg_muted));
        });
    } else {
        ui.label(
            egui::RichText::new(format!("{:6.1}", m.bpm))
                .monospace()
                .size(32.0)
                .color(p.fg_primary),
        );
        // Tempo edits are disabled for listen-only sources (Link): the
        // readout above still tracks the followed tempo, but nothing here
        // can write back to the session.
        ui.add_enabled_ui(m.can_set_tempo, |ui| {
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
                    if widgets::bracket_button(ui, "−.1%", None, 0.0)
                        .on_hover_text("Nudge tempo down for beat-matching drift. Key: [")
                        .clicked()
                    {
                        let _ = tx.send(Command::NudgeBpm(-0.001));
                    }
                    if widgets::bracket_button(ui, "+.1%", None, 0.0)
                        .on_hover_text("Nudge tempo up for beat-matching drift. Key: ]")
                        .clicked()
                    {
                        let _ = tx.send(Command::NudgeBpm(0.001));
                    }
                });
            });
        });
    }
}

/// The four tap buttons: downbeat, soft/hard reset, tap tempo.
fn tap_cluster(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    let p = palette();
    // Downbeat is a phase edit: greyed out (and the Space shortcut inert)
    // when the source is listen-only, e.g. Link.
    let downbeat_key = m.can_set_phase
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(egui::Key::Space));
    if ui
        .add_enabled_ui(m.can_set_phase, |ui| {
            tap_button(
                ui,
                "downbeat",
                "▼1",
                p.fg_primary,
                "Downbeat: snap to now (phase only, nearest bar). Key: t / Space",
                downbeat_key,
            )
        })
        .inner
    {
        let _ = tx.send(Command::TapDownbeat);
    }
    if tap_button(
        ui,
        "soft_reset",
        "⟲",
        p.fg_primary,
        "Soft reset: clock to bar 1, beat 1. Playlist position and playhead unchanged. Key: r",
        false,
    ) {
        let _ = tx.send(Command::SoftReset);
    }
    if tap_button(
        ui,
        "hard_reset",
        "⏮",
        p.error,
        "Hard reset: soft reset, AND jump the playlist back to its first cue \
         and restart its playhead from the in-point. Key: Shift+r",
        false,
    ) {
        let _ = tx.send(Command::HardReset);
    }
    // Tap tempo sets BPM: disabled for listen-only sources (Link).
    if ui
        .add_enabled_ui(m.can_set_tempo, |ui| {
            tap_button(
                ui,
                "tempo",
                "tap",
                p.fg_primary,
                "Tap tempo: tap 2+ times to set BPM from the interval. Key: b",
                false,
            )
        })
        .inner
    {
        let _ = tx.send(Command::TapTempo);
    }
}

/// The bar-in-phrase readout over the per-beat glyphs.
fn bar_beat(ui: &mut Ui, m: &UiMirror) {
    let p = palette();
    ui.vertical(|ui| {
        widgets::unit_label(
            ui,
            egui::RichText::new(format!(
                "bar {}/{}",
                m.bar_in_phrase + 1,
                m.bars_per_phrase.max(1)
            ))
            .color(p.fg_secondary),
        );
        beat_glyphs(ui, m);
    });
}

/// Sync source selector plus the Link peers tag.
fn sync_cluster(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    let p = palette();
    widgets::section_label(ui, "sync")
        .on_hover_text("Sync source: free-running Internal clock, or follow Ableton Link");
    let sync_idx = match m.sync.unwrap_or(SyncKind::Internal) {
        SyncKind::Internal => 0,
        SyncKind::Link => 1,
    };
    if let Some(i) = widgets::segmented(ui, "sync", &["internal", "link"], Some(sync_idx)) {
        let kind = if i == 0 { SyncKind::Internal } else { SyncKind::Link };
        let _ = tx.send(Command::SetSyncSource(kind));
    }
    if m.sync == Some(SyncKind::Link) && m.peers > 0 {
        ui.add_space(SP_SM);
        widgets::chip(ui, &format!("{} peers", m.peers), Some(p.playing), false);
    }
}

/// The top panel: BPM hero, tap controls, beat indicator, sync, and cadence rows.
pub(super) fn show(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    egui::Panel::top(egui::Id::new("transport")).show(ui, |ui| {
        ui.add_space(SP_SM);
        // The clusters hold `ui.vertical` groups, which a wrapped row would
        // place past the right edge rather than wrap — so stacking is an
        // explicit width check, not `horizontal_wrapped`.
        if ui.available_width() >= STACK_BELOW {
            ui.horizontal(|ui| {
                bpm_cluster(ui, m, tx);
                tap_cluster(ui, m, tx);
                ui.add_space(SP_MD);
                bar_beat(ui, m);
                ui.add_space(SP_MD);
                sync_cluster(ui, m, tx);
            });
        } else {
            ui.horizontal(|ui| bpm_cluster(ui, m, tx));
            ui.horizontal(|ui| {
                tap_cluster(ui, m, tx);
                ui.add_space(SP_MD);
                bar_beat(ui, m);
            });
            ui.horizontal_wrapped(|ui| sync_cluster(ui, m, tx));
        }
        ui.add_space(SP_SM);
        // Cadence controls, in bars. `Next` = how often the sequencer advances to
        // the next active clip; `Loop` = how often the current clip restarts.
        // Wrapped: the bracket lists break between items on a narrow window.
        ui.horizontal_wrapped(|ui| {
            widgets::section_label(ui, "next every")
                .on_hover_text("Beats between auto-transitions to the next active clip");
            let next_labels: Vec<&str> = CADENCE_BARS.iter().map(|(l, _)| *l).collect();
            let next_selected = CADENCE_BARS.iter().position(|(_, bars)| bars * 4 == m.phrase_len);
            if let Some(i) = widgets::segmented(ui, "next_cadence", &next_labels, next_selected) {
                let _ = tx.send(Command::SetPhraseLen(CADENCE_BARS[i].1 * 4));
            }

            ui.add_space(SP_MD);
            widgets::wrap_unit(ui, "loop_every_unit", |ui| {
                widgets::section_label(ui, "loop every")
                    .on_hover_text("Force the current clip back to its start on this beat grid");
                let loop_labels: Vec<&str> =
                    std::iter::once("off").chain(LOOP_CADENCE.iter().map(|(l, _)| *l)).collect();
                let loop_selected = m.loop_len.map_or(0, |beats| {
                    LOOP_CADENCE.iter().position(|(_, b)| *b == beats).map_or(0, |i| i + 1)
                });
                if let Some(i) =
                    widgets::segmented(ui, "loop_cadence", &loop_labels, Some(loop_selected))
                {
                    let beats = if i == 0 { None } else { Some(LOOP_CADENCE[i - 1].1) };
                    let _ = tx.send(Command::SetLoopLen(beats));
                }
            });

            ui.add_space(SP_MD);
            let mut preserve = m.preserve_playhead;
            if widgets::glyph_checkbox(ui, &mut preserve, "preserve playhead")
                .on_hover_text(
                    "On a cut, carry the playhead into the next clip (it comes in \
                     already running). Off: the next clip restarts from its start.",
                )
                .changed()
            {
                let _ = tx.send(Command::SetPreservePlayhead(preserve));
            }

            ui.add_space(SP_MD);
            let mut advanced = m.advanced;
            if widgets::glyph_checkbox(ui, &mut advanced, "advanced")
                .on_hover_text(
                    "Advanced sequencer: each cue gets its own dwell length, loop \
                     rate, timing offsets, and playback speed (edited per cue on the \
                     right). Off: the simple global cadence above drives every cue.",
                )
                .changed()
            {
                let _ = tx.send(Command::SetAdvancedMode(advanced));
            }
        });
        phrase_strip(ui, m);
        ui.add_space(SP_SM);
    });
}

