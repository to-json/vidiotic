//! The clip pool, bank tabs, and the edit bank's cue list.

use std::collections::HashMap;

use crossbeam_channel::Sender;
use egui::{Color32, CornerRadius, FontId, Rect, Sense, TextStyle, Ui};

use super::theme::{self, PALETTE, SP_MD, SP_SM};
use super::widgets::{self, TileSpec};
use super::{pick_file, PickKind};
use crate::commands::{BankView, ClipEntry, ClipId, Command, CueView, UiMirror};

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
            if ui.button("Folder…").clicked() {
                pick_file(tx.clone(), PickKind::ClipDir);
            }
            if let Some(d) = &m.clip_dir {
                ui.add(egui::Label::new(egui::RichText::new(d).small().color(PALETTE.fg_muted)).truncate());
            }
        });
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
                        for cue in &m.cues {
                            cue_chip(ui, m, cue, thumbs, tx, beat_pulse);
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
            if ui.button("Folder…").clicked() {
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
            if resp.clicked {
                let _ = tx.send(Command::SelectCue(Some(cue.id)));
            }
            if resp.hovered {
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
                widgets::chip(ui, &trim_label(cue), None);
                if let Some(p) = cue.preserve {
                    let (text, tint) =
                        if p { ("keep", PALETTE.playing) } else { ("cut", PALETTE.fg_muted) };
                    widgets::chip(ui, text, Some(tint));
                }
                if cue.shader.is_some() {
                    widgets::chip(ui, "fx", Some(PALETTE.accent));
                }
            });
        });
    });
}

fn trim_label(cue: &CueView) -> String {
    let out = cue.out_sec.map(super::fmt_time).unwrap_or_else(|| "end".to_string());
    format!("{}–{}", super::fmt_time(cue.in_sec), out)
}
