//! The clip pool, bank tabs, and the edit bank's cue list.

use std::collections::HashMap;

use crossbeam_channel::Sender;
use egui::{Color32, CornerRadius, FontId, Rect, Sense, TextStyle, Ui};

use super::theme::{self, PALETTE, SP_MD, SP_SM};
use super::widgets::{self, TileSpec};
use super::{pick_file, PickKind};
use crate::commands::{BankView, ClipBankView, ClipEntry, ClipId, Command, CueView, UiMirror};

/// Central panel: the source clip pool, bank tabs, and the edit bank's cue list.
pub(super) fn show(
    ui: &mut Ui,
    m: &UiMirror,
    tx: &Sender<Command>,
    thumbs: &HashMap<ClipId, egui::TextureHandle>,
) {
    // Drives the playing tile's beat-synced pulse stroke; brightest right on
    // the beat, decaying toward the next.
    let beat_pulse = 1.0 - m.phase.fract() as f32;

    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal(|ui| {
            widgets::section_label(ui, "clips");
            if ui
                .button("Folder…")
                .on_hover_text("Pick a folder of video clips to fill the pool")
                .clicked()
            {
                pick_file(tx.clone(), PickKind::ClipDir);
            }
            if let Some(d) = &m.clip_dir {
                ui.add(egui::Label::new(egui::RichText::new(d).small().color(PALETTE.fg_muted)).truncate());
            }
        });
        clip_bank_bar(ui, m, tx);
        ui.weak("double-click a clip to add it as a cue to the edit bank");
        egui::ScrollArea::vertical()
            .id_salt("clip_pool")
            .max_height(190.0)
            .show(ui, |ui| {
                if m.clips.is_empty() {
                    empty_state(
                        ui,
                        "No clips loaded",
                        "Pick a folder to fill the pool",
                        Some((tx, PickKind::ClipDir)),
                    );
                } else {
                    ui.horizontal_wrapped(|ui| {
                        for clip in &m.clips {
                            clip_tile(ui, clip, thumbs, tx, beat_pulse);
                        }
                    });
                }
            });

        ui.add_space(SP_MD);
        bank_bar(ui, m, tx);
        egui::ScrollArea::vertical()
            .id_salt("cue_list")
            .show(ui, |ui| {
                if m.cues.is_empty() {
                    empty_state(ui, "Empty bank", "double-click a clip above to add a cue", None);
                } else {
                    ui.horizontal_wrapped(|ui| {
                        for (i, cue) in m.cues.iter().enumerate() {
                            cue_chip(ui, m, i, cue, thumbs, tx, beat_pulse);
                        }
                    });
                }
            });
    });
}

/// Centered muted two-line prompt for an empty pool or bank; when
/// `folder_pick` is given, a real "Folder…" button follows.
fn empty_state(ui: &mut Ui, headline: &str, sub: &str, folder_pick: Option<(&Sender<Command>, PickKind)>) {
    ui.vertical_centered(|ui| {
        ui.add_space(SP_MD * 3.0);
        ui.label(egui::RichText::new(headline).color(PALETTE.fg_muted));
        ui.label(egui::RichText::new(sub).small().color(PALETTE.fg_muted));
        if let Some((tx, kind)) = folder_pick {
            ui.add_space(SP_SM);
            if ui
                .button("Folder…")
                .on_hover_text("Pick a folder of video clips to fill the pool")
                .clicked()
            {
                pick_file(tx.clone(), kind);
            }
        }
        ui.add_space(SP_MD * 3.0);
    });
}

/// A source-clip tile in the pool. Double-click adds a cue to the edit bank.
fn clip_tile(
    ui: &mut Ui,
    clip: &ClipEntry,
    thumbs: &HashMap<ClipId, egui::TextureHandle>,
    tx: &Sender<Command>,
    beat_pulse: f32,
) {
    let spec = TileSpec {
        name: &clip.name,
        tex: thumbs.get(&clip.id),
        role: clip.role,
        selected: false,
        active: clip.active,
        beat_pulse,
        size: egui::vec2(128.0, 86.0),
    };
    let resp = widgets::media_tile(ui, &spec);
    if resp.hovered {
        let hover_id = ui.id().with(("clip_hover", clip.id));
        ui.interact(resp.rect, hover_id, egui::Sense::hover())
            .on_hover_text("double-click: add a cue to the edit bank");
    }
    if resp.double_clicked {
        let _ = tx.send(Command::AddCue(clip.id));
    }
}

/// The clip-bank bar: pick which clip bank (source folder) the pool grid shows,
/// plus `＋` to add another folder as a new bank. Hidden until at least one bank
/// exists. The clip-dir "Folder…" button replaces the pool; `＋` here appends.
fn clip_bank_bar(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    if m.clip_banks.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        for (i, b) in m.clip_banks.iter().enumerate() {
            clip_bank_tab(ui, m, i, b, tx);
        }
        if ui
            .button("＋")
            .on_hover_text("add a folder as another clip bank")
            .clicked()
        {
            pick_file(tx.clone(), PickKind::ClipBankDir);
        }
    });
}

/// One clip-bank tab: name + muted clip count, accent underline when active.
fn clip_bank_tab(ui: &mut Ui, m: &UiMirror, i: usize, bank: &ClipBankView, tx: &Sender<Command>) {
    let p = &PALETTE;
    let selected = i == m.active_clip_bank;
    let base_id = ui.id().with(("clip_bank_tab", i));

    let name_font = TextStyle::Body.resolve(ui.style());
    let count_font = TextStyle::Small.resolve(ui.style());
    let name_color = if selected { p.fg_primary } else { p.fg_secondary };
    let name_galley = ui.painter().layout_no_wrap(bank.name.to_string(), name_font, name_color);
    let count_galley =
        ui.painter().layout_no_wrap(format!("({})", bank.clip_count), count_font, p.fg_muted);

    let content_w = name_galley.size().x + SP_SM + count_galley.size().x;
    let size = egui::vec2(content_w + SP_MD * 2.0, 24.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let resp = ui
        .interact(rect, base_id, Sense::click())
        .on_hover_text("show this clip bank in the pool");

    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), p.bg_elevated);
    }

    let mut x = rect.min.x + SP_MD;
    let name_y = rect.center().y - name_galley.size().y * 0.5;
    ui.painter().galley(egui::pos2(x, name_y), name_galley.clone(), name_color);
    x += name_galley.size().x + SP_SM;
    let count_y = rect.center().y - count_galley.size().y * 0.5;
    ui.painter().galley(egui::pos2(x, count_y), count_galley, p.fg_muted);

    if selected {
        let underline = Rect::from_min_max(
            egui::pos2(rect.min.x, rect.max.y - 2.0),
            egui::pos2(rect.max.x, rect.max.y),
        );
        ui.painter().rect_filled(underline, CornerRadius::ZERO, p.accent);
    }

    if resp.clicked() {
        let _ = tx.send(Command::SetActiveClipBank(i));
    }
}

/// The bank bar: custom-painted underline tabs to pick the edit bank, plus
/// `＋` to add one. The live bank shows a small dot before its name; the
/// edit bank (shown in the list below) gets an accent underline.
fn bank_bar(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    ui.horizontal(|ui| {
        widgets::section_label(ui, "banks");
        for (i, b) in m.banks.iter().enumerate() {
            bank_tab(ui, m, i, b, tx);
        }
        if ui.button("＋").on_hover_text("add a bank").clicked() {
            let _ = tx.send(Command::AddBank);
        }
    });
}

/// One bank tab: live dot, name, muted cue count, accent underline when this
/// is the edit bank, and a hover-only `▶` (on non-live tabs) to send it live.
fn bank_tab(ui: &mut Ui, m: &UiMirror, i: usize, bank: &BankView, tx: &Sender<Command>) {
    let p = &PALETTE;
    let live = i == m.live_bank;
    let selected = i == m.edit_bank;
    let base_id = ui.id().with(("bank_tab", i));

    let name_font = TextStyle::Body.resolve(ui.style());
    let count_font = TextStyle::Small.resolve(ui.style());
    let name_color = if selected { p.fg_primary } else { p.fg_secondary };
    let name_galley = ui.painter().layout_no_wrap(bank.name.to_string(), name_font, name_color);
    let count_galley =
        ui.painter().layout_no_wrap(format!("({})", bank.cue_count), count_font, p.fg_muted);

    let dot_w = if live { 10.0 } else { 0.0 };
    let play_w = if live { 0.0 } else { 20.0 };
    let content_w = dot_w + name_galley.size().x + SP_SM + count_galley.size().x + play_w;
    let size = egui::vec2(content_w + SP_MD * 2.0, 26.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let resp =
        ui.interact(rect, base_id, Sense::click()).on_hover_text("edit this bank (shown below)");

    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), p.bg_elevated);
    }

    let mut x = rect.min.x + SP_MD;
    if live {
        ui.painter().circle_filled(egui::pos2(x + 3.0, rect.center().y), 2.5, p.playing);
        x += dot_w;
    }
    let name_y = rect.center().y - name_galley.size().y * 0.5;
    ui.painter().galley(egui::pos2(x, name_y), name_galley.clone(), name_color);
    x += name_galley.size().x + SP_SM;
    let count_y = rect.center().y - count_galley.size().y * 0.5;
    ui.painter().galley(egui::pos2(x, count_y), count_galley, p.fg_muted);

    if selected {
        let underline = Rect::from_min_max(
            egui::pos2(rect.min.x, rect.max.y - 2.0),
            egui::pos2(rect.max.x, rect.max.y),
        );
        ui.painter().rect_filled(underline, CornerRadius::ZERO, p.accent);
    }

    if !live && resp.hovered() {
        let play_rect = Rect::from_min_size(
            egui::pos2(rect.max.x - play_w - SP_SM, rect.min.y + 3.0),
            egui::vec2(play_w, 20.0),
        );
        let play_resp = ui
            .interact(play_rect, base_id.with("play"), Sense::click())
            .on_hover_text("play this bank (it takes over at the next phrase)");
        let color = if play_resp.hovered() { p.fg_primary } else { p.fg_secondary };
        ui.painter().text(
            play_rect.center(),
            egui::Align2::CENTER_CENTER,
            "▶",
            FontId::proportional(11.0),
            color,
        );
        if play_resp.clicked() {
            let _ = tx.send(Command::SetLiveBank(i));
        }
    }

    if resp.clicked() {
        let _ = tx.send(Command::SetEditBank(i));
    }
}

/// A cue tile in the edit bank's list, plus a metadata chip row (trim,
/// keep/cut, fx) and a hover-only remove button. Click selects it for the editor.
fn cue_chip(
    ui: &mut Ui,
    m: &UiMirror,
    index: usize,
    cue: &CueView,
    thumbs: &HashMap<ClipId, egui::TextureHandle>,
    tx: &Sender<Command>,
    beat_pulse: f32,
) {
    let selected = m.selected_cue == Some(cue.id);
    ui.allocate_ui(egui::vec2(146.0, 122.0), |ui| {
        ui.vertical(|ui| {
            let spec = TileSpec {
                name: &cue.name,
                tex: thumbs.get(&cue.clip),
                role: cue.role,
                selected,
                active: false,
                beat_pulse,
                size: egui::vec2(146.0, 98.0),
            };
            let resp = widgets::media_tile(ui, &spec);
            // Advanced mode: the tile doubles as a drag-to-reorder handle. Click
            // still selects; a drag carries this cue's id and reorders on drop.
            // Uses an explicit-id `interact` (never `push_id`, which would break
            // the surrounding ScrollArea's wheel input).
            let mut clicked = resp.clicked;
            // The drag overlay, when present, sits above the tile and takes the
            // pointer — so route click/hover through it, not the tile beneath.
            let mut hovered = resp.hovered;
            if m.advanced {
                let drag_id = ui.id().with(("cue_drag", cue.id));
                let dr = ui.interact(resp.rect, drag_id, egui::Sense::click_and_drag());
                clicked = dr.clicked();
                hovered = dr.hovered();
                if dr.drag_started() {
                    egui::DragAndDrop::set_payload(ui.ctx(), cue.id);
                }
                if dr.dragged() {
                    ui.painter().rect_filled(
                        resp.rect,
                        egui::CornerRadius::same(4),
                        theme::with_alpha(Color32::BLACK, 90),
                    );
                }
                if let Some(dragged) = dr.dnd_release_payload::<crate::bank::CueId>() {
                    if *dragged != cue.id {
                        let _ = tx.send(Command::MoveCue(*dragged, index));
                    }
                }
            }
            if clicked {
                let _ = tx.send(Command::SelectCue(Some(cue.id)));
            }
            if hovered {
                let hover_id = ui.id().with(("cue_hover", cue.id));
                ui.interact(resp.rect, hover_id, egui::Sense::hover())
                    .on_hover_text("click to edit this cue");
                let cross_rect = egui::Rect::from_min_size(
                    egui::pos2(resp.rect.max.x - 20.0, resp.rect.min.y + 2.0),
                    egui::vec2(18.0, 18.0),
                );
                let cross_id = ui.id().with(("cue_remove", cue.id));
                let cross_resp = ui
                    .interact(cross_rect, cross_id, egui::Sense::click())
                    .on_hover_text("remove cue");
                let color = if cross_resp.hovered() { PALETTE.error } else { PALETTE.fg_primary };
                let painter = ui.painter();
                painter.circle_filled(cross_rect.center(), 9.0, theme::with_alpha(Color32::BLACK, 170));
                painter.text(
                    cross_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "✕",
                    egui::FontId::proportional(11.0),
                    color,
                );
                if cross_resp.clicked() {
                    let _ = tx.send(Command::RemoveCue(cue.id));
                }
            }

            ui.horizontal(|ui| {
                if m.advanced {
                    ui.spacing_mut().item_spacing.x = SP_SM;
                    ui.spacing_mut().button_padding = egui::vec2(4.0, 0.0);
                    if ui.small_button("◀").on_hover_text("move earlier").clicked() && index > 0 {
                        let _ = tx.send(Command::MoveCue(cue.id, index - 1));
                    }
                    if ui.small_button("▶").on_hover_text("move later").clicked() {
                        let _ = tx.send(Command::MoveCue(cue.id, index + 1));
                    }
                }
                widgets::chip(ui, &trim_label(cue), None, false);
                if let Some(p) = cue.preserve {
                    let (text, tint) =
                        if p { ("keep", PALETTE.playing) } else { ("cut", PALETTE.fg_muted) };
                    widgets::chip(ui, text, Some(tint), false);
                }
                if cue.shader.is_some() {
                    widgets::chip(ui, "fx", Some(PALETTE.accent), false);
                }
                if m.advanced {
                    advanced_badges(ui, cue);
                }
            });
        });
    });
}

/// Compact advanced-mode metadata chips: dwell, loop rate, active offsets, and
/// a non-unity playback speed. Only the set/non-default knobs show, to keep the
/// tile's chip row from overflowing.
fn advanced_badges(ui: &mut Ui, cue: &CueView) {
    if let Some(ticks) = cue.dwell {
        widgets::chip(ui, &format!("⌛{}b", fmt_beats(ticks)), None, false);
    }
    match cue.loop_len {
        Some(0) => {
            widgets::chip(ui, "loop off", None, false);
        }
        Some(t) => {
            widgets::chip(ui, &format!("↻{}b", fmt_beats(t)), None, false);
        }
        None => {}
    }
    if cue.loop_phase.on || cue.start_nudge.on || cue.trig_delay.on {
        widgets::chip(ui, "offset", Some(PALETTE.armed), false);
    }
    if (cue.speed - 1.0).abs() > 1e-3 {
        widgets::chip(ui, &format!("{:.2}×", cue.speed), Some(PALETTE.accent), false);
    }
}

/// Format a tick count as a compact beat count: `512` → `16`, `48` → `1.5`.
fn fmt_beats(ticks: u32) -> String {
    let b = ticks as f64 / crate::commands::LOOP_TICKS_PER_BEAT as f64;
    if b.fract() == 0.0 {
        format!("{}", b as i64)
    } else {
        format!("{b:.2}").trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn trim_label(cue: &CueView) -> String {
    let out = cue.out_sec.map(super::fmt_time).unwrap_or_else(|| "end".to_string());
    format!("{}–{}", super::fmt_time(cue.in_sec), out)
}
