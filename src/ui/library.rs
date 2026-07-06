//! The clip pool, bank tabs, and the edit bank's cue list.

use std::collections::HashMap;

use crossbeam_channel::Sender;
use egui::Ui;

use super::theme::{PALETTE, SP_MD};
use super::{pick_file, PickKind};
use crate::commands::{ClipEntry, ClipId, ClipRole, Command, CueView, UiMirror};

/// Central panel: the source clip pool, bank tabs, and the edit bank's cue list.
pub(super) fn show(
    ui: &mut Ui,
    m: &UiMirror,
    tx: &Sender<Command>,
    thumbs: &HashMap<ClipId, egui::TextureHandle>,
) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading("Clips");
            if ui.button("Folder…").clicked() {
                pick_file(tx.clone(), PickKind::ClipDir);
            }
            if let Some(d) = &m.clip_dir {
                ui.weak(d);
            }
        });
        ui.weak("double-click a clip to add it as a cue to the edit bank");
        egui::ScrollArea::vertical()
            .id_salt("clip_pool")
            .max_height(190.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for clip in &m.clips {
                        clip_tile(ui, clip, thumbs, tx);
                    }
                });
            });

        ui.add_space(SP_MD);
        bank_bar(ui, m, tx);
        egui::ScrollArea::vertical()
            .id_salt("cue_list")
            .show(ui, |ui| {
                if m.cues.is_empty() {
                    ui.add_space(SP_MD);
                    ui.weak("Empty bank — double-click a clip above to add a cue.");
                }
                ui.horizontal_wrapped(|ui| {
                    for cue in &m.cues {
                        cue_chip(ui, m, cue, thumbs, tx);
                    }
                });
            });
    });
}

/// A source-clip tile in the pool. Double-click adds a cue to the edit bank.
fn clip_tile(
    ui: &mut Ui,
    clip: &ClipEntry,
    thumbs: &HashMap<ClipId, egui::TextureHandle>,
    tx: &Sender<Command>,
) {
    ui.allocate_ui(egui::vec2(128.0, 104.0), |ui| {
        ui.vertical(|ui| {
            let resp = if let Some(tex) = thumbs.get(&clip.id) {
                let img = egui::Image::new((tex.id(), egui::vec2(120.0, 68.0)));
                ui.add(egui::Button::image(img).selected(clip.active))
            } else {
                ui.add_sized(
                    egui::vec2(120.0, 68.0),
                    egui::Button::new("…").selected(clip.active),
                )
            };
            let resp = resp.on_hover_text("double-click: add a cue to the edit bank");
            let marker = match clip.role {
                ClipRole::Playing => "▶ ",
                ClipRole::Armed => "○ ",
                ClipRole::None => "",
            };
            let color = match clip.role {
                ClipRole::Playing => PALETTE.playing,
                ClipRole::Armed => PALETTE.armed,
                ClipRole::None if clip.active => PALETTE.armed.gamma_multiply(0.7),
                ClipRole::None => PALETTE.fg_muted,
            };
            ui.colored_label(color, format!("{marker}{}", ellipsize(&clip.name, 16)));
            if resp.double_clicked() {
                let _ = tx.send(Command::AddCue(clip.id));
            }
        });
    });
}

/// The bank bar: pick which bank to edit, send one live, add a bank. `●` marks
/// the live bank; the selected tab is the edit bank shown in the list below.
fn bank_bar(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    ui.horizontal(|ui| {
        ui.strong("Banks");
        for (i, b) in m.banks.iter().enumerate() {
            let live = i == m.live_bank;
            let label = format!("{}{} ({})", if live { "● " } else { "" }, b.name, b.cue_count);
            if ui
                .selectable_label(i == m.edit_bank, label)
                .on_hover_text("edit this bank (shown below)")
                .clicked()
            {
                let _ = tx.send(Command::SetEditBank(i));
            }
            if !live
                && ui
                    .small_button("▶")
                    .on_hover_text("play this bank (it takes over at the next phrase)")
                    .clicked()
            {
                let _ = tx.send(Command::SetLiveBank(i));
            }
        }
        if ui.button("＋").on_hover_text("add a bank").clicked() {
            let _ = tx.send(Command::AddBank);
        }
    });
}

/// A cue tile in the edit bank's list. Click selects it for the editor.
fn cue_chip(
    ui: &mut Ui,
    m: &UiMirror,
    cue: &CueView,
    thumbs: &HashMap<ClipId, egui::TextureHandle>,
    tx: &Sender<Command>,
) {
    let selected = m.selected_cue == Some(cue.id);
    ui.allocate_ui(egui::vec2(146.0, 116.0), |ui| {
        let stroke = if selected {
            egui::Stroke::new(2.0, PALETTE.accent)
        } else {
            egui::Stroke::new(1.0, PALETTE.border)
        };
        egui::Frame::group(ui.style()).stroke(stroke).show(ui, |ui| {
            ui.vertical(|ui| {
                let resp = if let Some(tex) = thumbs.get(&cue.clip) {
                    let img = egui::Image::new((tex.id(), egui::vec2(120.0, 56.0)));
                    ui.add(egui::Button::image(img).selected(selected))
                } else {
                    ui.add_sized(
                        egui::vec2(120.0, 56.0),
                        egui::Button::new(ellipsize(&cue.name, 10)).selected(selected),
                    )
                };
                if resp.clicked() {
                    let _ = tx.send(Command::SelectCue(Some(cue.id)));
                }
                ui.horizontal(|ui| {
                    let (marker, color) = match cue.role {
                        ClipRole::Playing => ("▶", PALETTE.playing),
                        ClipRole::Armed => ("○", PALETTE.armed),
                        ClipRole::None => (" ", PALETTE.fg_muted),
                    };
                    ui.colored_label(color, marker);
                    ui.label(ellipsize(&cue.name, 12));
                });
                ui.horizontal(|ui| {
                    ui.small(trim_label(cue));
                    if cue.preserve.is_some() {
                        ui.small(if cue.preserve == Some(true) { "·keep" } else { "·cut" });
                    }
                    if cue.shader.is_some() {
                        ui.small("·fx").on_hover_text("has a shader override");
                    }
                    if ui.small_button("✕").on_hover_text("remove cue").clicked() {
                        let _ = tx.send(Command::RemoveCue(cue.id));
                    }
                });
            });
        });
    });
}

fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        s.to_string()
    }
}

fn trim_label(cue: &CueView) -> String {
    let out = cue.out_sec.map(super::fmt_time).unwrap_or_else(|| "end".to_string());
    format!("{}–{}", super::fmt_time(cue.in_sec), out)
}
