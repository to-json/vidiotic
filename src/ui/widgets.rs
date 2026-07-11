//! Shared custom-painted widgets in the phosphor idiom: every control is
//! buffer text on the character grid — bracket buttons and selectors, paren
//! tags, glyph checkboxes and faders, eighth-block meters — with square
//! bordered media tiles as the one bitmap concession. All colors come from
//! [`super::theme::palette`], so the look stays coherent as panels adopt
//! these and survives hue rotation.

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::theme::{self, mono, palette, ROW};
use crate::commands::ClipRole;

/// Eighth-block ramp for glyph meters.
pub const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Advance width of one buffer cell in the current mono font.
pub fn cell_width(ui: &Ui) -> f32 {
    ui.painter().layout_no_wrap("─".into(), mono(), Color32::WHITE).size().x
}

/// Lay out `text` in the buffer font and paint it centered on a fresh
/// one-row allocation. Returns the rect for interaction.
fn alloc_text(ui: &mut Ui, text: &str, color: Color32) -> (Rect, egui::Response) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), mono(), color);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(galley.size().x, ROW), Sense::click());
    ui.painter()
        .galley(egui::pos2(rect.min.x, rect.center().y - galley.size().y * 0.5), galley, color);
    (rect, resp)
}

/// One-of-N selector as a bracket list: `[a] b c`. The selected label wears
/// the brackets and the accent; the rest sit dim until hovered. Items flow
/// into the parent layout one by one, so inside a wrapped row a long list
/// breaks across lines instead of running off the edge. Returns the clicked
/// index, if any.
pub fn segmented(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    labels: &[&str],
    selected: Option<usize>,
) -> Option<usize> {
    let p = palette();
    let base_id = ui.make_persistent_id(id_salt);
    let mut clicked = None;

    for (i, label) in labels.iter().enumerate() {
        let is_selected = selected == Some(i);
        let text = if is_selected { format!("[{label}]") } else { format!(" {label} ") };
        let galley = ui.painter().layout_no_wrap(text.clone(), mono(), p.fg_muted);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(galley.size().x, ROW), Sense::hover());
        let resp = ui.interact(rect, base_id.with(i), Sense::click());
        let color = if is_selected {
            p.accent
        } else if resp.hovered() {
            p.fg_primary
        } else {
            p.fg_secondary
        };
        ui.painter().text(
            egui::pos2(rect.min.x, rect.center().y),
            Align2::LEFT_CENTER,
            text,
            mono(),
            color,
        );
        if resp.clicked() {
            clicked = Some(i);
        }
    }

    clicked
}

/// Small lowercase muted label for grouping controls, e.g. "next every".
/// Never splits: in a wrapped row it moves to the next line as a unit.
pub fn section_label(ui: &mut Ui, text: &str) -> Response {
    unit_label(
        ui,
        egui::RichText::new(text.to_lowercase()).monospace().size(10.0).color(palette().fg_muted),
    )
}

/// A label that moves to the next wrapped line as a unit instead of
/// splitting its text at the row edge.
pub fn unit_label(ui: &mut Ui, text: impl Into<egui::WidgetText>) -> Response {
    ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend))
}

/// What happened to a [`chip`] this frame.
pub struct ChipResponse {
    pub clicked: bool,
    pub removed: bool,
    pub rect: Rect,
}

/// Parenthesized tag — cue metadata, the peers marker, error tags, pinned
/// shaders: `(2 peers)`. When `removable`, a trailing `✕` sits inside the
/// parens; its click reports as `removed`, separate from the tag's own
/// `clicked`.
pub fn chip(ui: &mut Ui, text: &str, tint: Option<Color32>, removable: bool) -> ChipResponse {
    let p = palette();
    let color = tint.unwrap_or(p.fg_secondary);
    let display = if removable { format!("({text} ×)") } else { format!("({text})") };
    let (rect, resp) = alloc_text(ui, &display, color);

    let mut removed = false;
    if removable {
        let cw = cell_width(ui);
        let close_rect = Rect::from_min_max(egui::pos2(rect.max.x - cw * 2.5, rect.min.y), rect.max);
        let close_resp = ui.interact(close_rect, resp.id.with("close"), Sense::click());
        if close_resp.hovered() {
            ui.painter().text(
                egui::pos2(close_rect.max.x - cw, close_rect.center().y),
                Align2::RIGHT_CENTER,
                "×)",
                mono(),
                p.error,
            );
        }
        removed = close_resp.clicked();
    }

    ChipResponse { clicked: resp.clicked() && !removed, removed, rect }
}

/// What a [`media_tile`] needs painted: a clip pool tile or a cue chip.
pub struct TileSpec<'a> {
    pub name: &'a str,
    pub tex: Option<&'a egui::TextureHandle>,
    pub role: ClipRole,
    /// Accent selection border (cue list: this cue is selected for editing).
    pub selected: bool,
    /// In-pool "referenced by a cue" marker (clip pool only).
    pub active: bool,
    /// 0..1, decays from 1.0 on the beat; drives the playing-tile border pulse.
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

/// Paint a clip/cue tile as a bordered buffer cell: thumbnail inset above a
/// one-row glyph name (`▶name`), border color carrying selection/role, and a
/// phosphor pulse border while playing.
pub fn media_tile(ui: &mut Ui, spec: &TileSpec) -> TileResponse {
    let p = palette();
    let (rect, resp) = ui.allocate_exact_size(spec.size, Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, p.bg_inset);

    // Art sits inset with a reserved name row along the bottom.
    let name_h = 14.0;
    let art = Rect::from_min_max(
        rect.min + Vec2::splat(3.0),
        egui::pos2(rect.max.x - 3.0, rect.max.y - name_h),
    );
    if let Some(tex) = spec.tex {
        egui::Image::new((tex.id(), art.size())).paint_at(ui, art);
        if resp.hovered() {
            painter.rect_filled(art, CornerRadius::ZERO, theme::with_alpha(Color32::WHITE, 20));
        }
    } else {
        painter.text(
            art.center(),
            Align2::CENTER_CENTER,
            "decoding…",
            FontId::monospace(10.0),
            p.fg_muted,
        );
    }

    // Glyph-prefixed name, truncated by width.
    let glyph = match spec.role {
        ClipRole::Playing => "▶",
        ClipRole::Armed => "○",
        ClipRole::None => " ",
    };
    let name_color = if spec.selected { p.accent } else { p.fg_primary };
    let mut job = LayoutJob::single_section(
        format!("{glyph}{}", spec.name),
        TextFormat::simple(FontId::monospace(10.0), name_color),
    );
    job.wrap = TextWrapping::truncate_at_width((spec.size.x - 6.0).max(0.0));
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(rect.min.x + 3.0, rect.max.y - name_h * 0.5 - galley.size().y * 0.5),
        galley,
        name_color,
    );

    // Border: selection beats the in-pool "active" marker, which beats hover;
    // a playing tile also gets a beat-synced phosphor pulse on top.
    let border = if spec.selected {
        p.accent
    } else if spec.active {
        p.armed
    } else if resp.hovered() {
        p.fg_secondary
    } else {
        p.border
    };
    painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, border), StrokeKind::Inside);
    if spec.role == ClipRole::Playing {
        let alpha = 120 + (spec.beat_pulse.powi(2) * 135.0) as u8;
        painter.rect_stroke(
            rect,
            CornerRadius::ZERO,
            Stroke::new(1.0, theme::with_alpha(p.phosphor, alpha)),
            StrokeKind::Inside,
        );
    }

    TileResponse {
        clicked: resp.clicked(),
        double_clicked: resp.double_clicked(),
        hovered: resp.hovered(),
        rect,
    }
}

/// Bracket button: `[ label ]` in buffer text. `color` tints the label (e.g.
/// error red for hard reset); hover swaps it to the accent. `flash` (0..1,
/// decaying) inverts the button onto an accent fill so taps read as hits.
pub fn bracket_button(ui: &mut Ui, label: &str, color: Option<Color32>, flash: f32) -> Response {
    let p = palette();
    let text = format!("[ {label} ]");
    let galley = ui.painter().layout_no_wrap(text.clone(), mono(), p.fg_primary);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(galley.size().x, ROW), Sense::click());
    let mut fg = color.unwrap_or(p.fg_primary);
    if resp.hovered() {
        fg = p.accent;
    }
    let painter = ui.painter();
    if flash > 0.0 {
        painter.rect_filled(rect, CornerRadius::ZERO, theme::with_alpha(p.accent, 60 + (flash * 195.0) as u8));
        fg = p.bg_inset;
    } else if resp.is_pointer_button_down_on() {
        painter.rect_filled(rect, CornerRadius::ZERO, p.accent_dim);
    }
    painter.text(egui::pos2(rect.min.x, rect.center().y), Align2::LEFT_CENTER, text, mono(), fg);
    resp
}

/// Glyph checkbox: `[x] label` / `[ ] label`. Click anywhere toggles; the
/// returned response reports `changed`.
pub fn glyph_checkbox(ui: &mut Ui, checked: &mut bool, label: &str) -> Response {
    let p = palette();
    let box_text = if *checked { "[x]" } else { "[ ]" };
    let text = if label.is_empty() { box_text.to_string() } else { format!("{box_text} {label}") };
    let galley = ui.painter().layout_no_wrap(text, mono(), p.fg_primary);
    let (rect, mut resp) = ui.allocate_exact_size(egui::vec2(galley.size().x, ROW), Sense::click());
    if resp.clicked() {
        *checked = !*checked;
        resp.mark_changed();
    }
    let box_color = if *checked { p.playing } else { p.fg_muted };
    let label_color = if resp.hovered() { p.accent } else { p.fg_primary };
    let painter = ui.painter();
    let box_text = if *checked { "[x]" } else { "[ ]" };
    painter.text(egui::pos2(rect.min.x, rect.center().y), Align2::LEFT_CENTER, box_text, mono(), box_color);
    if !label.is_empty() {
        let cw = cell_width(ui);
        painter.text(
            egui::pos2(rect.min.x + cw * 4.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            mono(),
            label_color,
        );
    }
    resp
}

/// Fader: a solid cap sliding a tick-marked glyph track, one row tall:
/// `├────┼────┤` with a `█` cap. Click or drag along the track. A bipolar
/// range gets a bright center detent. The returned response reports `changed`.
pub fn fader(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    min: f32,
    max: f32,
    v: &mut f32,
    cells: usize,
) -> Response {
    let p = palette();
    let cw = cell_width(ui);
    let n = cells.max(4);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(cw * (n as f32 + 2.0), ROW), Sense::hover());
    let mut resp = ui.interact(rect, ui.make_persistent_id(id_salt), Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - rect.min.x - cw) / (cw * n as f32)).clamp(0.0, 1.0);
            let next = min + t * (max - min);
            if next != *v {
                *v = next;
                resp.mark_changed();
            }
        }
    }
    let bipolar = min < 0.0 && max > 0.0;
    let t = ((*v - min) / (max - min)).clamp(0.0, 1.0);
    let mut track = String::with_capacity(n + 2);
    track.push('├');
    for k in 0..n {
        track.push(if k > 0 && k % (n / 4).max(1) == 0 { '┼' } else { '─' });
    }
    track.push('┤');
    let painter = ui.painter();
    let put = |col: f32, text: &str, color: Color32| {
        painter.text(
            egui::pos2(rect.min.x + col * cw, rect.center().y),
            Align2::LEFT_CENTER,
            text,
            mono(),
            color,
        );
    };
    put(0.0, &track, theme::with_alpha(p.fg_muted, 200));
    if bipolar {
        put(1.0 + (n - 1) as f32 * 0.5, "┼", p.fg_primary);
    }
    let k = (t * (n - 1) as f32).round();
    put(1.0 + k, "█", if bipolar { p.magenta } else { p.playing });
    resp
}

/// Mono level as a short eighth-block bar: filled cells up to the magnitude
/// (already 0..1), phosphor under half scale, armed past it, error near
/// clipping.
pub fn glyph_level(ui: &mut Ui, mag: f32, cells: usize) {
    let p = palette();
    let mag = mag.clamp(0.0, 1.0);
    let color = if mag > 0.85 {
        p.error
    } else if mag > 0.5 {
        p.armed
    } else {
        p.phosphor
    };
    let filled = mag * cells as f32;
    let mut s = String::with_capacity(cells);
    for k in 0..cells {
        let f = (filled - k as f32).clamp(0.0, 1.0);
        s.push(if f <= 0.0 { '▁' } else { BLOCKS[((f * 7.99) as usize).min(7)] });
    }
    let galley = ui.painter().layout_no_wrap(s, mono(), color);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(galley.size().x, ROW), Sense::hover());
    ui.painter()
        .galley(egui::pos2(rect.min.x, rect.center().y - galley.size().y * 0.5), galley, color);
}

/// Spectrum as per-column eighth blocks (magnitudes already 0..1): green with
/// brightness following the bin, red on clipping bins.
pub fn glyph_fft(ui: &mut Ui, mags: &[f32]) {
    let p = palette();
    let cw = cell_width(ui);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(cw * mags.len() as f32, ROW), Sense::hover());
    let painter = ui.painter();
    for (k, &mag) in mags.iter().enumerate() {
        let mag = mag.clamp(0.0, 1.0);
        let ch = BLOCKS[((mag * 7.99) as usize).min(7)];
        let color = if mag > 0.85 { p.error } else { p.phosphor };
        painter.text(
            egui::pos2(rect.min.x + k as f32 * cw, rect.center().y),
            Align2::LEFT_CENTER,
            ch.to_string(),
            mono(),
            theme::with_alpha(color, 120 + (mag * 135.0) as u8),
        );
    }
}
