//! The clip pool, bank tabs, and the edit bank's cue list, in the phosphor
//! idiom: bracket-text tabs, paren metadata tags, and square bordered tiles.

use std::collections::HashMap;

use crossbeam_channel::Sender;
use egui::{Align2, Rect, Sense, Ui};

use phosphor::theme::{self, mono, palette, ROW, SP_MD, SP_SM};
use phosphor::widgets::{self, TileSpec};

use super::{pick_file, tile_role, PickKind};
use crate::commands::{
    BankView, CameraEntry, ClipBankView, ClipEntry, ClipId, ClipRole, Command, CueView, UiMirror,
};

/// Central panel: the source clip pool, bank tabs, and the edit bank's cue list.
pub(super) fn show(
    ui: &mut Ui,
    m: &UiMirror,
    tx: &Sender<Command>,
    thumbs: &HashMap<ClipId, egui::TextureHandle>,
) {
    let p = palette();
    // Drives the playing tile's beat-synced pulse border; brightest right on
    // the beat, decaying toward the next.
    let beat_pulse = 1.0 - m.phase.fract() as f32;

    egui::CentralPanel::default().show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            widgets::section_label(ui, "clips");
            if widgets::bracket_button(ui, "folder…", None, 0.0)
                .on_hover_text("Pick a folder of video clips to fill the pool")
                .clicked()
            {
                pick_file(tx.clone(), PickKind::ClipDir);
            }
            if let Some(d) = &m.clip_dir {
                ui.add(egui::Label::new(egui::RichText::new(d).small().color(p.fg_muted)).truncate());
            }
        });
        clip_bank_bar(ui, m, tx);
        ui.label(
            egui::RichText::new("double-click a clip to add it as a cue to the edit bank")
                .small()
                .color(p.fg_muted),
        );
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
        cameras_section(ui, m, tx);

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
/// `folder_pick` is given, a real "folder…" button follows.
fn empty_state(ui: &mut Ui, headline: &str, sub: &str, folder_pick: Option<(&Sender<Command>, PickKind)>) {
    let p = palette();
    ui.vertical_centered(|ui| {
        ui.add_space(SP_MD * 3.0);
        ui.label(egui::RichText::new(headline).color(p.fg_secondary));
        ui.label(egui::RichText::new(sub).small().color(p.fg_muted));
        if let Some((tx, kind)) = folder_pick {
            ui.add_space(SP_SM);
            if widgets::bracket_button(ui, "folder…", None, 0.0)
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
        role: tile_role(clip.role),
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

/// The cameras section: one row per enumerated capture device with an on-air
/// toggle and a status tag. Double-click a device's name to add a cue, like a
/// clip tile.
fn cameras_section(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    let p = palette();
    ui.horizontal_wrapped(|ui| {
        widgets::section_label(ui, "cameras");
        if widgets::bracket_button(ui, "refresh", None, 0.0)
            .on_hover_text("re-enumerate capture devices (after plugging/unplugging)")
            .clicked()
        {
            let _ = tx.send(Command::RefreshCameras);
        }
    });
    if m.cameras.is_empty() {
        ui.label(egui::RichText::new("no capture devices found").small().color(p.fg_muted));
        return;
    }
    for cam in &m.cameras {
        camera_row(ui, m, cam, tx);
    }
}

/// One capture device: on-air toggle, camera-glyph name (double-click adds a
/// cue to the edit bank), status tag, and the playing/armed role tag. Rows for
/// a saved-but-absent device offer a pick-a-device relink instead.
fn camera_row(ui: &mut Ui, m: &UiMirror, cam: &CameraEntry, tx: &Sender<Command>) {
    let p = palette();
    ui.horizontal(|ui| {
        if cam.missing {
            missing_camera_row(ui, m, cam, tx);
            return;
        }
        let mut on = cam.on_air;
        if widgets::glyph_checkbox(ui, &mut on, "")
            .on_hover_text(
                "on air: keep this camera capturing (privacy light on) regardless of cue rotation",
            )
            .changed()
        {
            let _ = tx.send(Command::SetCameraOnAir(cam.uid.clone(), on));
        }
        let name_color = if cam.active { p.fg_primary } else { p.fg_secondary };
        let resp = ui
            .add(
                egui::Label::new(egui::RichText::new(format!("◉ {}", cam.name)).color(name_color))
                    .sense(Sense::click()),
            )
            .on_hover_text("double-click: add a cue to the edit bank");
        if resp.double_clicked() {
            let _ = tx.send(Command::AddCameraCue(cam.uid.clone()));
        }
        widgets::chip(ui, &cam.status, cam.on_air.then_some(p.phosphor), false);
        match cam.role {
            ClipRole::Playing => {
                widgets::chip(ui, "playing", Some(p.playing), false);
            }
            ClipRole::Armed => {
                widgets::chip(ui, "armed", Some(p.armed), false);
            }
            ClipRole::None => {}
        }
    });
}

/// A saved camera whose device isn't connected: name, "missing device" tag,
/// and a combo listing the connected devices to relink onto.
fn missing_camera_row(ui: &mut Ui, m: &UiMirror, cam: &CameraEntry, tx: &Sender<Command>) {
    let p = palette();
    ui.label(egui::RichText::new(format!("◉ {}", cam.name)).color(p.fg_muted));
    widgets::chip(ui, &cam.status, Some(p.error), false);
    egui::ComboBox::from_id_salt(("cam_relink", &cam.uid))
        .selected_text("relink…")
        .show_ui(ui, |ui| {
            for other in m.cameras.iter().filter(|c| !c.missing) {
                if ui.selectable_label(false, other.name.as_ref()).clicked() {
                    let _ = tx.send(Command::RelinkCamera {
                        from: cam.uid.clone(),
                        to: other.uid.clone(),
                    });
                }
            }
        });
}

/// One bracket-text tab: `[name (3)]` accent when selected, dim otherwise,
/// with an optional `●` live dot before the name. Returns the click response.
fn glyph_tab(ui: &mut Ui, id: egui::Id, name: &str, count: usize, selected: bool, live: bool) -> egui::Response {
    let p = palette();
    let body = format!("{name} ({count})");
    let text = if selected { format!("[{body}]") } else { format!(" {body} ") };
    let dot_w = if live { 2.0 } else { 0.0 };
    let cw = widgets::cell_width(ui);
    let galley = ui.painter().layout_no_wrap(text.clone(), mono(), p.fg_muted);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(galley.size().x + dot_w * cw, ROW), Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    let painter = ui.painter();
    let mut x = rect.min.x;
    if live {
        painter.text(egui::pos2(x + cw * 0.5, rect.center().y), Align2::LEFT_CENTER, "●", mono(), p.phosphor);
        x += dot_w * cw;
    }
    let color = if selected {
        p.accent
    } else if resp.hovered() {
        p.fg_primary
    } else {
        p.fg_secondary
    };
    painter.text(egui::pos2(x, rect.center().y), Align2::LEFT_CENTER, text, mono(), color);
    resp
}

/// The clip-bank bar: pick which clip bank (source folder) the pool grid shows,
/// plus `+` to add another folder as a new bank. Hidden until at least one bank
/// exists. The clip-dir "folder…" button replaces the pool; `+` here appends.
fn clip_bank_bar(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    if m.clip_banks.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (i, b) in m.clip_banks.iter().enumerate() {
            clip_bank_tab(ui, m, i, b, tx);
        }
        if widgets::bracket_button(ui, "+", None, 0.0)
            .on_hover_text("add a folder as another clip bank")
            .clicked()
        {
            pick_file(tx.clone(), PickKind::ClipBankDir);
        }
    });
}

/// One clip-bank tab: name + clip count, bracketed when active.
fn clip_bank_tab(ui: &mut Ui, m: &UiMirror, i: usize, bank: &ClipBankView, tx: &Sender<Command>) {
    let selected = i == m.active_clip_bank;
    let id = ui.id().with(("clip_bank_tab", i));
    let resp = glyph_tab(ui, id, &bank.name, bank.clip_count, selected, false)
        .on_hover_text("show this clip bank in the pool");
    if resp.clicked() {
        let _ = tx.send(Command::SetActiveClipBank(i));
    }
}

/// The bank bar: bracket-text tabs to pick the edit bank, plus `+` to add
/// one. The live bank shows a `●` dot before its name; the edit bank (shown
/// in the list below) wears the brackets.
fn bank_bar(ui: &mut Ui, m: &UiMirror, tx: &Sender<Command>) {
    ui.horizontal_wrapped(|ui| {
        widgets::section_label(ui, "banks");
        for (i, b) in m.banks.iter().enumerate() {
            bank_tab(ui, m, i, b, tx);
        }
        if widgets::bracket_button(ui, "+", None, 0.0).on_hover_text("add a bank").clicked() {
            let _ = tx.send(Command::AddBank);
        }
    });
}

/// One bank tab: live dot, bracketed name + cue count, and a hover-only `▶`
/// (on non-live tabs) to send it live.
fn bank_tab(ui: &mut Ui, m: &UiMirror, i: usize, bank: &BankView, tx: &Sender<Command>) {
    let p = palette();
    let live = i == m.live_bank;
    let selected = i == m.edit_bank;
    let base_id = ui.id().with(("bank_tab", i));

    let resp = glyph_tab(ui, base_id, &bank.name, bank.cue_count, selected, live)
        .on_hover_text("edit this bank (shown below)");

    if !live && resp.hovered() {
        let cw = widgets::cell_width(ui);
        let play_rect = Rect::from_min_size(
            egui::pos2(resp.rect.max.x, resp.rect.min.y),
            egui::vec2(cw * 2.0, resp.rect.height()),
        );
        let play_resp = ui
            .interact(play_rect, base_id.with("play"), Sense::click())
            .on_hover_text("play this bank (it takes over at the next phrase). Keys: , / . cycle live bank");
        let color = if play_resp.hovered() { p.accent } else { p.fg_secondary };
        ui.painter().text(play_rect.center(), Align2::CENTER_CENTER, "▶", mono(), color);
        if play_resp.clicked() {
            let _ = tx.send(Command::SetLiveBank(i));
        }
    }

    if resp.clicked() {
        let _ = tx.send(Command::SetEditBank(i));
    }
}

/// A cue tile in the edit bank's list, plus a metadata tag row (trim,
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
    let p = palette();
    let selected = m.selected_cue == Some(cue.id);
    ui.allocate_ui(egui::vec2(146.0, 122.0), |ui| {
        ui.vertical(|ui| {
            let spec = TileSpec {
                name: &cue.name,
                tex: thumbs.get(&cue.clip),
                role: tile_role(cue.role),
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
                        egui::CornerRadius::ZERO,
                        theme::with_alpha(egui::Color32::BLACK, 90),
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
                let color = if cross_resp.hovered() { p.error } else { p.fg_primary };
                let painter = ui.painter();
                painter.rect_filled(
                    cross_rect,
                    egui::CornerRadius::ZERO,
                    theme::with_alpha(p.bg_inset, 200),
                );
                painter.text(cross_rect.center(), Align2::CENTER_CENTER, "×", mono(), color);
                if cross_resp.clicked() {
                    let _ = tx.send(Command::RemoveCue(cue.id));
                }
            }

            ui.horizontal(|ui| {
                if m.advanced {
                    ui.spacing_mut().item_spacing.x = SP_SM;
                    if widgets::bracket_button(ui, "◀", None, 0.0).on_hover_text("move earlier").clicked()
                        && index > 0
                    {
                        let _ = tx.send(Command::MoveCue(cue.id, index - 1));
                    }
                    if widgets::bracket_button(ui, "▶", None, 0.0).on_hover_text("move later").clicked() {
                        let _ = tx.send(Command::MoveCue(cue.id, index + 1));
                    }
                }
                widgets::chip(ui, &trim_label(cue), None, false);
                if let Some(pv) = cue.preserve {
                    let (text, tint) = if pv { ("keep", p.playing) } else { ("cut", p.fg_muted) };
                    widgets::chip(ui, text, Some(tint), false);
                }
                if !cue.chain.is_empty() {
                    widgets::chip(ui, "fx", Some(p.blue), false);
                }
                if m.advanced {
                    advanced_badges(ui, cue);
                }
            });
        });
    });
}

/// Compact advanced-mode metadata tags: dwell, loop rate, active offsets, and
/// a non-unity playback speed. Only the set/non-default knobs show, to keep the
/// tile's tag row from overflowing.
fn advanced_badges(ui: &mut Ui, cue: &CueView) {
    let p = palette();
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
        widgets::chip(ui, "offset", Some(p.armed), false);
    }
    if (cue.speed - 1.0).abs() > 1e-3 {
        widgets::chip(ui, &format!("{:.2}×", cue.speed), Some(p.blue), false);
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
    if cue.camera {
        return "live".to_string();
    }
    let out = cue.out_sec.map(super::fmt_time).unwrap_or_else(|| "end".to_string());
    format!("{}–{}", super::fmt_time(cue.in_sec), out)
}
