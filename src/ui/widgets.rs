//! Shared custom-painted widgets used across the control window's panels.
//! All colors come from [`super::theme::PALETTE`], so the look stays coherent
//! as new panels adopt these.

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, StrokeKind, TextStyle, Ui, Vec2};

use super::theme::{self, PALETTE, SP_MD, SP_SM};
use crate::commands::ClipRole;

/// One-of-N segmented control (a pill-group replacing rows of
/// `selectable_label`). Returns the clicked index, if any.
pub fn segmented(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    labels: &[&str],
    selected: Option<usize>,
) -> Option<usize> {
    let p = &PALETTE;
    let font = TextStyle::Button.resolve(ui.style());
    let base_id = ui.make_persistent_id(id_salt);
    let mut clicked = None;
    let last = labels.len().saturating_sub(1);

    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.horizontal(|ui| {
            for (i, label) in labels.iter().enumerate() {
                let galley = ui.painter().layout_no_wrap(label.to_string(), font.clone(), p.fg_primary);
                let size = egui::vec2(galley.size().x + SP_MD * 2.0, 22.0);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let resp = ui.interact(rect, base_id.with(i), Sense::click());

                let is_selected = selected == Some(i);
                let fill = if is_selected {
                    p.accent_dim
                } else if resp.hovered() {
                    p.bg_elevated
                } else {
                    p.bg_inset
                };
                let text_color = if is_selected { p.fg_primary } else { p.fg_secondary };
                let radius = CornerRadius {
                    nw: if i == 0 { 4 } else { 0 },
                    sw: if i == 0 { 4 } else { 0 },
                    ne: if i == last { 4 } else { 0 },
                    se: if i == last { 4 } else { 0 },
                };
                ui.painter().rect_filled(rect, radius, fill);
                ui.painter()
                    .text(rect.center(), egui::Align2::CENTER_CENTER, *label, font.clone(), text_color);

                if resp.clicked() {
                    clicked = Some(i);
                }
            }
        });
    });

    clicked
}

/// Small uppercase muted label for grouping controls, e.g. "NEXT EVERY".
pub fn section_label(ui: &mut Ui, text: &str) -> Response {
    ui.label(egui::RichText::new(text.to_uppercase()).small().color(PALETTE.fg_muted))
}

/// Small rounded pill badge — cue metadata, the peers marker, error tags.
pub fn chip(ui: &mut Ui, text: &str, tint: Option<Color32>) {
    let p = &PALETTE;
    let font = TextStyle::Small.resolve(ui.style());
    let (fill, text_color) = match tint {
        Some(t) => (t.linear_multiply(0.15), t),
        None => (p.bg_elevated, p.fg_secondary),
    };

    let height = 18.0;
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), text_color);
    let size = egui::vec2(galley.size().x + SP_SM * 2.0, height);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());

    let radius = CornerRadius::same((height / 2.0) as u8);
    ui.painter().rect_filled(rect, radius, fill);
    ui.painter().text(
        egui::pos2(rect.min.x + SP_SM, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        text_color,
    );
}

/// What a [`media_tile`] needs painted: a clip pool tile or a cue chip.
pub struct TileSpec<'a> {
    pub name: &'a str,
    pub tex: Option<&'a egui::TextureHandle>,
    pub role: ClipRole,
    /// Accent selection ring (cue list: this cue is selected for editing).
    pub selected: bool,
    /// In-pool "referenced by a cue" marker (clip pool only).
    pub active: bool,
    /// 0..1, decays from 1.0 on the beat; drives the playing-tile glow.
    pub beat_pulse: f32,
    pub size: Vec2,
}

/// What happened to a [`media_tile`] this frame.
pub struct TileResponse {
    pub clicked: bool,
    pub double_clicked: bool,
    pub hovered: bool,
    pub rect: Rect,
}

/// Paint a clip/cue tile: thumbnail, bottom-scrim name, role badge, and a
/// selection/hover/playing outline.
pub fn media_tile(ui: &mut Ui, spec: &TileSpec) -> TileResponse {
    let p = &PALETTE;
    let (rect, resp) = ui.allocate_exact_size(spec.size, Sense::click());
    let radius = CornerRadius::same(4);

    if let Some(tex) = spec.tex {
        egui::Image::new((tex.id(), spec.size)).corner_radius(radius).paint_at(ui, rect);
        if resp.hovered() {
            ui.painter().rect_filled(rect, radius, theme::with_alpha(Color32::WHITE, 24));
        }
    } else {
        let painter = ui.painter();
        painter.rect_filled(rect, radius, p.bg_inset);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "decoding…",
            FontId::proportional(11.0),
            p.fg_muted,
        );
    }

    // Bottom scrim: two stacked translucent bands stand in for a gradient.
    let scrim_h = 22.0_f32.min(spec.size.y);
    let upper = Rect::from_min_max(
        egui::pos2(rect.min.x, rect.max.y - scrim_h),
        egui::pos2(rect.max.x, rect.max.y - scrim_h * 0.5),
    );
    let lower = Rect::from_min_max(upper.left_bottom(), rect.max);
    let bottom_radius = CornerRadius { nw: 0, ne: 0, sw: radius.sw, se: radius.se };
    {
        let painter = ui.painter();
        painter.rect_filled(upper, CornerRadius::ZERO, theme::with_alpha(Color32::BLACK, 60));
        painter.rect_filled(lower, bottom_radius, theme::with_alpha(Color32::BLACK, 140));
    }

    // Name, truncated by width rather than char count, sitting in the scrim.
    let name_font = TextStyle::Small.resolve(ui.style());
    let max_w = (spec.size.x - SP_SM * 2.0).max(0.0);
    let mut job = LayoutJob::single_section(spec.name.to_string(), TextFormat::simple(name_font, p.fg_primary));
    job.wrap = TextWrapping::truncate_at_width(max_w);
    {
        let painter = ui.painter();
        let galley = painter.layout_job(job);
        let name_pos =
            egui::pos2(rect.min.x + SP_SM, rect.max.y - scrim_h * 0.5 - galley.size().y * 0.5);
        painter.galley(name_pos, galley, p.fg_primary);
    }

    // Role badge, top-left: dark halo behind a glyph in the role's color.
    if spec.role != ClipRole::None {
        let (glyph, color) = match spec.role {
            ClipRole::Playing => ("▶", p.playing),
            ClipRole::Armed => ("○", p.armed),
            ClipRole::None => unreachable!(),
        };
        let center = rect.min + Vec2::splat(10.0);
        let painter = ui.painter();
        painter.circle_filled(center, 7.0, theme::with_alpha(Color32::BLACK, 170));
        painter.text(center, egui::Align2::CENTER_CENTER, glyph, FontId::proportional(10.0), color);
    }

    // Outline: selection ring beats the in-pool "active" ring, which beats
    // hover border; a playing tile also gets a beat-synced pulse stroke on
    // top of whichever of those it has.
    {
        let painter = ui.painter();
        if spec.selected {
            painter.rect_stroke(rect, radius, Stroke::new(2.0, p.accent), StrokeKind::Inside);
        } else if spec.active {
            painter.rect_stroke(rect, radius, Stroke::new(1.0, p.armed), StrokeKind::Inside);
        } else if resp.hovered() {
            painter.rect_stroke(rect, radius, Stroke::new(1.0, p.border), StrokeKind::Inside);
        }
        if spec.role == ClipRole::Playing {
            let alpha = (spec.beat_pulse.powi(2) * 160.0) as u8;
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(2.0, theme::with_alpha(p.playing, alpha)),
                StrokeKind::Inside,
            );
        }
    }

    TileResponse {
        clicked: resp.clicked(),
        double_clicked: resp.double_clicked(),
        hovered: resp.hovered(),
        rect,
    }
}

/// Uniform big button for the transport row (downbeat/reset/tempo taps).
/// `text_size`/`text_color` let a glyph run larger than body text and, for a
/// destructive action like hard reset, tint independent of hover/press state.
/// `flash` (0..1, decaying) overlays an accent tint so taps read as hits.
pub fn transport_button(
    ui: &mut Ui,
    label: &str,
    size: Vec2,
    text_size: f32,
    text_color: Color32,
    flash: f32,
) -> Response {
    let p = &PALETTE;
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());

    let fill = if resp.is_pointer_button_down_on() {
        p.accent_dim
    } else if resp.hovered() {
        p.bg_elevated.gamma_multiply(1.3)
    } else {
        p.bg_elevated
    };
    let font = FontId::proportional(text_size);
    let radius = CornerRadius::same(4);
    let painter = ui.painter();
    painter.rect_filled(rect, radius, fill);
    if flash > 0.0 {
        let alpha = (flash * 120.0) as u8;
        painter.rect_filled(rect, radius, theme::with_alpha(p.accent, alpha));
    }
    painter.text(rect.center(), egui::Align2::CENTER_CENTER, label, font, text_color);

    resp
}
