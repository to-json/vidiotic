//! Shared custom-painted widgets in the phosphor idiom: every control is
//! buffer text on the character grid — bracket buttons and selectors, paren
//! tags, glyph checkboxes and faders, eighth-block meters — with square
//! bordered media tiles as the one bitmap concession. All colors come from
//! [`crate::theme::palette`], so the look stays coherent as panels adopt
//! these and survives hue rotation.

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Color32, CornerRadius, FontId, Popup, Rect, RectAlign, Response, Sense, Stroke,
    StrokeKind, Ui, Vec2,
};

use crate::theme::{self, mono, palette, ROW};

/// How a [`media_tile`] participates in playback, carried on [`TileSpec`]:
/// picks the name-row glyph and the playing pulse border.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TileRole {
    Playing,
    Armed,
    None,
}

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

/// How far a drag must travel (in points) to step one detent.
const DETENT_DRAG_STEP: f32 = 24.0;
/// How far the wheel-scroll accumulator must travel (in points) to step one detent.
const DETENT_SCROLL_STEP: f32 = 40.0;

/// Scroll-with-detents selector: shows the current choice as `[ label ]`.
/// Left-click steps forward one detent, right-click steps back one; a
/// horizontal click-and-drag scrubs continuously (same "grab it" feel as
/// [`fader`]); hovering and scrolling the wheel also steps through the
/// detents. Values wrap around at either end. Renders `--` when `selected`
/// is `None`. Returns the newly selected index when the user steps to a
/// different one.
pub fn detent_scroll(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    labels: &[&str],
    selected: Option<usize>,
    width_chars: usize,
) -> Option<usize> {
    let text = match selected {
        Some(i) => format!("[{:^width$}]", labels[i], width = width_chars),
        None => format!("[{:^width$}]", "--", width = width_chars),
    };
    let id = ui.make_persistent_id(id_salt);
    let (rect, resp) = detent_frame(ui, &text);
    let stepped = detent_step(ui, id, &resp, labels.len());
    let new_selected = stepped.map(|delta| {
        let cur = selected.unwrap_or(0) as i32;
        (cur + delta).rem_euclid(labels.len() as i32) as usize
    });
    detent_paint(ui, rect, &text, resp.hovered());
    new_selected.filter(|&i| Some(i) != selected)
}

/// `detent_scroll` for a bare integer within `range` (wraps at either end).
/// Returns the new value when it changes.
pub fn detent_scroll_uint(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    v: u32,
    range: std::ops::RangeInclusive<u32>,
    width_chars: usize,
) -> Option<u32> {
    let text = format!("[{:^width$}]", v, width = width_chars);
    let id = ui.make_persistent_id(id_salt);
    let (rect, resp) = detent_frame(ui, &text);
    let span = *range.end() as i64 - *range.start() as i64 + 1;
    let stepped = detent_step(ui, id, &resp, span as usize);
    let new_v = stepped.map(|delta| {
        let cur = v as i64 - *range.start() as i64;
        (*range.start() as i64 + (cur + delta as i64).rem_euclid(span)) as u32
    });
    detent_paint(ui, rect, &text, resp.hovered());
    new_v.filter(|&x| x != v)
}

/// Allocate the fixed-width cell for a detent scroller, sensing click and
/// drag (painting happens after step handling so the updated value shows
/// immediately).
fn detent_frame(ui: &mut Ui, text: &str) -> (Rect, Response) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), mono(), Color32::WHITE);
    ui.allocate_exact_size(egui::vec2(galley.size().x, ROW), Sense::click_and_drag())
}

/// Resolve this frame's interaction into a whole-detent step: a left-click
/// is `+1` and a right-click is `-1`, both immediate; otherwise a horizontal
/// drag or (while hovered) the wheel accumulates into `id`'s stored offset
/// and steps whenever it crosses a threshold. `count` gates how many detents
/// exist so a lone detent never "steps".
fn detent_step(ui: &mut Ui, id: egui::Id, resp: &Response, count: usize) -> Option<i32> {
    if count <= 1 {
        return None;
    }
    if resp.clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(id, 0.0_f32));
        return Some(1);
    }
    if resp.secondary_clicked() {
        ui.ctx().data_mut(|d| d.insert_temp(id, 0.0_f32));
        return Some(-1);
    }
    let mut accum: f32 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or(0.0);
    let step_size = if resp.dragged() {
        accum += resp.drag_delta().x;
        DETENT_DRAG_STEP
    } else if resp.hovered() {
        accum += ui.input(|i| i.smooth_scroll_delta.y);
        DETENT_SCROLL_STEP
    } else {
        accum = 0.0;
        DETENT_SCROLL_STEP
    };
    let steps = (accum / step_size).trunc() as i32;
    accum -= steps as f32 * step_size;
    ui.ctx().data_mut(|d| d.insert_temp(id, accum));
    (steps != 0).then_some(steps)
}

/// Paint a detent scroller's bracketed text: accent on hover, primary
/// otherwise (mirrors [`segmented`]'s selected-item coloring).
fn detent_paint(ui: &mut Ui, rect: Rect, text: &str, hovered: bool) {
    let p = palette();
    let color = if hovered { p.accent } else { p.fg_primary };
    ui.painter().text(egui::pos2(rect.min.x, rect.center().y), Align2::LEFT_CENTER, text, mono(), color);
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

/// Lay out a cluster of row widgets as one wrapping unit inside a
/// `horizontal_wrapped` row: when the whole cluster no longer fits the
/// remaining row it breaks to the next line intact, rather than splitting
/// between its children. Only when the intact cluster is wider than a full
/// row — i.e. it would overflow even from the leftmost position — do the
/// children flow into the wrapped row individually so they can break.
///
/// The intact width comes from the previous frame's measurement (nested
/// groups report their size only after layout), so a resize corrects on the
/// next frame.
pub fn wrap_unit(
    ui: &mut Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    add: impl FnOnce(&mut Ui),
) {
    let id = ui.make_persistent_id(id_salt);
    let known_width: Option<f32> = ui.data(|d| d.get_temp(id));
    let row_width = ui.max_rect().width();
    if known_width.is_some_and(|w| w > row_width) {
        let at_row_start = ui.available_size_before_wrap().x >= row_width - 0.5;
        if !at_row_start {
            ui.end_row();
        }
        add(ui);
    } else {
        if known_width.is_some_and(|w| w > ui.available_size_before_wrap().x) {
            ui.end_row();
        }
        let measured = ui.horizontal(|ui| add(ui)).response.rect.width();
        ui.data_mut(|d| d.insert_temp(id, measured));
    }
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
    pub role: TileRole,
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
        TileRole::Playing => "▶",
        TileRole::Armed => "○",
        TileRole::None => " ",
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
    if spec.role == TileRole::Playing {
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

/// Buffer cells [`theme_controls`] occupies: `[dark] light` (13) + the hue
/// strip (14) + margins. [`theme_toggle`]'s popup reserves this much width
/// (× [`cell_width`]) for the expanded picker.
pub const THEME_CELLS: f32 = 31.0;

/// Buffer cells [`theme_toggle`]'s collapsed button occupies at the edge of
/// its rect — callers reserve this much width (× [`cell_width`]) instead of
/// the full [`THEME_CELLS`], since the picker itself now lives in a popup.
pub const THEME_TOGGLE_CELLS: f32 = 5.0;

/// Right-aligned theme switchboard painted inside `rect`: `[dark] light` and
/// the hue-rotation strip, in the buffer's own idiom. Mutations land through
/// [`theme::set_state`], so the restyle shows up when the app next calls
/// [`theme::sync`] — typically the following frame.
pub fn theme_controls(ui: &mut Ui, rect: Rect) {
    let p = palette();
    let cw = cell_width(ui);
    let mut st = theme::state(ui.ctx());
    let painter = ui.painter();

    // Hue strip at the right edge.
    const STRIP_CELLS: f32 = 14.0;
    let strip = Rect::from_min_size(
        egui::pos2(rect.max.x - cw * (STRIP_CELLS + 1.0), rect.min.y + 4.0),
        egui::vec2(cw * STRIP_CELLS, rect.height() - 8.0),
    );
    let resp = ui.interact(strip, ui.id().with("theme_hue"), Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            // Hue is circular: the strip's right edge wraps back to 0.
            st.hue = (((pos.x - strip.min.x) / strip.width()).clamp(0.0, 1.0) * 360.0).rem_euclid(360.0);
        }
    }
    const N: usize = 28;
    for k in 0..N {
        let cell = Rect::from_min_size(
            egui::pos2(strip.min.x + strip.width() * k as f32 / N as f32, strip.min.y),
            egui::vec2(strip.width() / N as f32 + 0.5, strip.height()),
        );
        painter.rect_filled(cell, CornerRadius::ZERO, theme::hsl(k as f32 / N as f32 * 360.0, 0.5, 0.5));
    }
    let x = strip.min.x + strip.width() * st.hue / 360.0;
    painter.line_segment(
        [egui::pos2(x, strip.min.y - 2.0), egui::pos2(x, strip.max.y + 2.0)],
        Stroke::new(2.0, p.fg_primary),
    );

    // `[dark] light` selector to the strip's left.
    let mut col = strip.min.x / cw - 15.0;
    for (lab, dark) in [("dark", true), ("light", false)] {
        let selected = st.dark == dark;
        let text = if selected { format!("[{lab}]") } else { format!(" {lab} ") };
        let w = text.chars().count() as f32;
        let r = Rect::from_min_size(
            egui::pos2(col * cw, rect.min.y),
            egui::vec2(w * cw, rect.height()),
        );
        let resp = ui.interact(r, ui.id().with(("theme_mode", lab)), Sense::click());
        let color = if selected {
            p.accent
        } else if resp.hovered() {
            p.fg_primary
        } else {
            p.fg_secondary
        };
        painter.text(egui::pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, text, mono(), color);
        if resp.clicked() {
            st.dark = dark;
        }
        col += w + 1.0;
    }

    theme::set_state(ui.ctx(), st);
}

/// Collapsed stand-in for [`theme_controls`]: a small `[◐]`/`[◑]` toggle
/// painted at `rect` (dark/light glyph tracks the current mode) that opens
/// the full switchboard in a floating popup on click, instead of the picker
/// always eating a full row's width.
pub fn theme_toggle(ui: &mut Ui, rect: Rect) {
    let p = palette();
    let resp = ui
        .interact(rect, ui.id().with("theme_toggle"), Sense::click())
        .on_hover_text("theme: dark/light + hue");
    let glyph = if theme::state(ui.ctx()).dark { "[◐]" } else { "[◑]" };
    let color = if resp.hovered() { p.accent } else { p.fg_secondary };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, mono(), color);

    Popup::from_toggle_button_response(&resp).align(RectAlign::TOP).show(|ui| {
        let cw = cell_width(ui);
        let (prect, _) = ui.allocate_exact_size(
            egui::vec2(cw * (THEME_CELLS + 2.0), ROW + theme::SP_MD),
            Sense::hover(),
        );
        theme_controls(ui, prect);
    });
}

/// The shared statusline strip: a full-width `select`-filled bar with a mode
/// segment (`mode.0`, tinted with `mode.1` when something is happening, else
/// neutral), a `summary` readout, and the collapsed [`theme_toggle`] at the
/// right edge. Used as the last row of each app's bottom panel. Returns
/// whether the mode segment was clicked, so a tinted mode (e.g. "ERROR") can
/// double as a click target without the widget knowing what that means.
pub fn statusline(ui: &mut Ui, mode: (&str, Option<Color32>), summary: &str) -> bool {
    let p = palette();
    let cw = cell_width(ui);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::ZERO, p.accent_dim);

    // Mode segment: its own fill when something is happening.
    let (mode_label, mode_bg) = mode;
    let mode_cells = mode_label.chars().count() as f32 + 2.0;
    let mode_rect = Rect::from_min_size(rect.min, egui::vec2(cw * mode_cells, rect.height()));
    if let Some(bg) = mode_bg {
        painter.rect_filled(mode_rect, CornerRadius::ZERO, bg);
    }
    let mode_resp = ui.interact(mode_rect, ui.id().with("statusline_mode"), Sense::click());
    let mode_fg = if mode_bg.is_some() { p.bg_inset } else { p.fg_primary };
    ui.painter().text(
        egui::pos2(rect.min.x + cw, rect.center().y),
        Align2::LEFT_CENTER,
        mode_label,
        mono(),
        mode_fg,
    );

    // Clip the summary short of the theme toggle so a narrow window
    // truncates it instead of running the two together.
    let summary_clip = Rect::from_min_max(
        rect.min,
        egui::pos2(rect.max.x - cw * THEME_TOGGLE_CELLS, rect.max.y),
    );
    painter.with_clip_rect(summary_clip).text(
        egui::pos2(rect.min.x + cw * (mode_cells + 2.0), rect.center().y),
        Align2::LEFT_CENTER,
        summary,
        mono(),
        p.fg_secondary,
    );

    let toggle_rect = Rect::from_min_size(
        egui::pos2(rect.max.x - cw * THEME_TOGGLE_CELLS, rect.min.y),
        egui::vec2(cw * THEME_TOGGLE_CELLS, rect.height()),
    );
    theme_toggle(ui, toggle_rect);
    mode_resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive `wrap_unit` for a few frames at the given panel width and report
    /// the rects of the unit's two fixed-size children (30×10 and 90×10),
    /// preceded by a 100×10 filler in the same wrapped row.
    fn run_wrap_unit(panel_width: f32) -> (Rect, Rect, Rect) {
        let ctx = egui::Context::default();
        let mut out = None;
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(panel_width, 400.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                ui.horizontal_wrapped(|ui| {
                    let (filler, _) =
                        ui.allocate_exact_size(egui::vec2(100.0, 10.0), Sense::hover());
                    let mut a = Rect::NOTHING;
                    let mut b = Rect::NOTHING;
                    wrap_unit(ui, "unit", |ui| {
                        a = ui.allocate_exact_size(egui::vec2(30.0, 10.0), Sense::hover()).0;
                        b = ui.allocate_exact_size(egui::vec2(90.0, 10.0), Sense::hover()).0;
                    });
                    out = Some((filler, a, b));
                });
            });
        }
        out.unwrap()
    }

    #[test]
    fn wrap_unit_stays_inline_when_it_fits() {
        let (filler, a, b) = run_wrap_unit(300.0);
        assert_eq!(a.min.y, filler.min.y, "unit should share the filler's row");
        assert_eq!(a.min.x, filler.max.x);
        assert_eq!(b.min.x, a.max.x, "children stay adjacent");
    }

    #[test]
    fn wrap_unit_breaks_to_next_row_intact() {
        // 100 filler + 120 unit > 180 row: the unit moves down whole.
        let (filler, a, b) = run_wrap_unit(180.0);
        assert!(a.min.y > filler.max.y, "unit should start a new row, got {a:?}");
        assert_eq!(a.min.x, 0.0, "unit should start at the row edge");
        assert_eq!(b.min.y, a.min.y, "children stay on one row");
        assert_eq!(b.min.x, a.max.x, "children stay adjacent");
    }

    #[test]
    fn wrap_unit_wider_than_row_lets_children_wrap() {
        // Unit alone is 120 wide > 100 row: children wrap individually.
        let (filler, a, b) = run_wrap_unit(100.0);
        assert!(a.min.y > filler.max.y, "unit should leave the filler's row");
        assert_eq!(a.min.x, 0.0, "first child starts at the row edge");
        assert!(b.min.y >= a.max.y, "second child wraps below the first, got a={a:?} b={b:?}");
    }

// Scratch reproduction of the transport cadence row, appended to widgets.rs
// tests temporarily. Prints where each cluster lands at several widths.

#[test]
fn repro_cadence_row() {
    for width in [900.0_f32, 700.0, 560.0, 420.0, 320.0, 240.0] {
        let ctx = egui::Context::default();
        let mut rows = Vec::new();
        for _ in 0..3 {
            rows.clear();
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 400.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8.0, 7.0);
                ui.horizontal_wrapped(|ui| {
                    let nl = section_label(ui, "next every").rect;
                    segmented(ui, "next_cadence", &["1", "2", "4", "8", "16"], Some(2));
                    ui.add_space(8.0);
                    let mut ll = Rect::NOTHING;
                    let mut last = Rect::NOTHING;
                    wrap_unit(ui, "loop_every_unit", |ui| {
                        ll = section_label(ui, "loop every").rect;
                        segmented(
                            ui,
                            "loop_cadence",
                            &["off", "1/8", "1/4", "1/2", "1", "2", "4", "8", "16"],
                            Some(0),
                        );
                        last = ui.min_rect();
                    });
                    ui.add_space(8.0);
                    let mut pp = true;
                    let ppr = glyph_checkbox(ui, &mut pp, "preserve playhead").rect;
                    rows.push(("next_label", nl));
                    rows.push(("loop_label", ll));
                    rows.push(("loop_unit", last));
                    rows.push(("preserve", ppr));
                });
            });
        }
        println!("--- width {width} ---");
        for (name, r) in &rows {
            println!("{name:12} x {:7.1}..{:7.1}  y {:5.1}..{:5.1}", r.min.x, r.max.x, r.min.y, r.max.y);
        }
    }
}
}
