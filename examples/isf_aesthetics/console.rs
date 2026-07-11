//! Direction 0 — **Console**: the app's current language, refined. Flat dark
//! panels, cyan accent, tick-marked sliders with a mono readout, pill
//! segments, crosshair minimap. The cheap-adoption baseline: everything here
//! is a short step from `src/ui/widgets.rs`.

use egui::ecolor::Hsva;
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, vec2,
};

use crate::schema::{DemoState, MidiState, MockKind, MockRole, Value, mock_clips};
use crate::util;

const BG_INSET: Color32 = Color32::from_rgb(9, 9, 12);
const BG_ELEVATED: Color32 = Color32::from_rgb(31, 31, 39);
const FG: Color32 = Color32::from_rgb(232, 232, 237);
const FG_DIM: Color32 = Color32::from_rgb(158, 158, 170);
const FG_MUTED: Color32 = Color32::from_rgb(102, 102, 116);
const ACCENT: Color32 = Color32::from_rgb(82, 191, 255);
const ACCENT_DIM: Color32 = Color32::from_rgb(36, 63, 83);
const BORDER: Color32 = Color32::from_rgb(45, 45, 56);

const LABEL_W: f32 = 92.0;
const CTL_W: f32 = 230.0;

fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Render the whole direction panel.
pub fn show(ui: &mut Ui, st: &mut DemoState) {
    header(ui);
    let time = st.time;
    let DemoState { inputs, values, midi, .. } = st;
    for (i, (inp, val)) in inputs.iter().zip(values.iter_mut()).enumerate() {
        ui.horizontal(|ui| {
            label_cell(ui, inp.label);
            let rect = control(ui, i, inp.name, &inp.kind, val, time);
            midi_affordance(ui, midi, i, &inp.kind, rect, time);
        });
        ui.add_space(6.0);
    }
    footer(ui);
}

fn header(ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("KALEIDO BLOOM").strong().color(FG));
        ui.label(egui::RichText::new("fs · 11 inputs").small().color(FG_MUTED));
    });
    ui.add_space(8.0);
}

fn label_cell(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(LABEL_W, 22.0), Sense::hover());
    ui.painter().text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        text.to_uppercase(),
        FontId::proportional(10.0),
        FG_MUTED,
    );
}

/// One control; returns its rect for the MIDI overlay.
fn control(ui: &mut Ui, i: usize, name: &str, kind: &MockKind, val: &mut Value, time: f64) -> Rect {
    match (kind, val) {
        (MockKind::Float { min, max, .. }, Value::Float(v)) => slider(ui, i, *min, *max, v),
        (MockKind::Bool { .. }, Value::Bool(b)) => toggle(ui, b),
        (MockKind::Long { labels, .. }, Value::LongIdx(sel)) => pills(ui, i, labels, sel),
        (MockKind::Event, Value::EventAt(at)) => event_button(ui, at, time),
        (MockKind::Point2D { min, max, .. }, Value::Point(p)) => pad(ui, *min, *max, p),
        (MockKind::Color { .. }, Value::Color(c)) => color_row(ui, i, c),
        (MockKind::Image, _) => source_tile(ui, name),
        (MockKind::Audio, _) => audio_meter(ui, time),
        (MockKind::AudioFft, _) => fft_bars(ui, time),
        _ => ui.allocate_exact_size(vec2(0.0, 0.0), Sense::hover()).0,
    }
}

fn slider(ui: &mut Ui, i: usize, min: f32, max: f32, v: &mut f32) -> Rect {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(CTL_W - 52.0, 22.0), Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
            *v = min + t * (max - min);
        }
    }
    let t = (*v - min) / (max - min);
    let track_y = rect.center().y;
    let painter = ui.painter();
    // Ticks under the track.
    for k in 0..=8 {
        let x = rect.min.x + rect.width() * k as f32 / 8.0;
        painter.line_segment(
            [Pos2::new(x, track_y + 5.0), Pos2::new(x, track_y + 8.0)],
            Stroke::new(1.0, BORDER),
        );
    }
    painter.line_segment(
        [Pos2::new(rect.min.x, track_y), Pos2::new(rect.max.x, track_y)],
        Stroke::new(2.0, BG_ELEVATED),
    );
    let fill_x = rect.min.x + rect.width() * t;
    painter.line_segment(
        [Pos2::new(rect.min.x, track_y), Pos2::new(fill_x, track_y)],
        Stroke::new(2.0, ACCENT),
    );
    let handle = Pos2::new(fill_x, track_y);
    painter.circle_filled(handle, 5.0, if resp.hovered() { FG } else { FG_DIM });
    painter.circle_stroke(handle, 5.0, Stroke::new(1.0, BG_INSET));

    // Mono readout to the right.
    let (vrect, _) = ui.allocate_exact_size(vec2(48.0, 22.0), Sense::hover());
    ui.painter().text(
        vrect.right_center(),
        Align2::RIGHT_CENTER,
        format!("{v:.2}"),
        FontId::monospace(11.0),
        FG,
    );
    let _ = i;
    rect.union(vrect)
}

fn toggle(ui: &mut Ui, b: &mut bool) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(34.0, 18.0), Sense::click());
    if resp.clicked() {
        *b = !*b;
    }
    let t = ui.ctx().animate_bool(resp.id, *b);
    let painter = ui.painter();
    let fill = if *b { ACCENT_DIM } else { BG_INSET };
    painter.rect_filled(rect, CornerRadius::same(9), fill);
    painter.rect_stroke(rect, CornerRadius::same(9), Stroke::new(1.0, BORDER), StrokeKind::Inside);
    let cx = egui::lerp(rect.min.x + 9.0..=rect.max.x - 9.0, t);
    painter.circle_filled(Pos2::new(cx, rect.center().y), 6.0, if *b { ACCENT } else { FG_MUTED });
    rect
}

fn pills(ui: &mut Ui, i: usize, labels: &[&str], sel: &mut usize) -> Rect {
    let mut union: Option<Rect> = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (k, label) in labels.iter().enumerate() {
            let (rect, _) = ui.allocate_exact_size(vec2(34.0, 20.0), Sense::hover());
            let resp = ui.interact(rect, ui.id().with(("pill", i, k)), Sense::click());
            if resp.clicked() {
                *sel = k;
            }
            let last = labels.len() - 1;
            let radius = CornerRadius {
                nw: u8::from(k == 0) * 4,
                sw: u8::from(k == 0) * 4,
                ne: u8::from(k == last) * 4,
                se: u8::from(k == last) * 4,
            };
            let fill = if *sel == k {
                ACCENT_DIM
            } else if resp.hovered() {
                BG_ELEVATED
            } else {
                BG_INSET
            };
            let color = if *sel == k { FG } else { FG_DIM };
            ui.painter().rect_filled(rect, radius, fill);
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                *label,
                FontId::proportional(11.0),
                color,
            );
            union = Some(union.map_or(rect, |u| u.union(rect)));
        }
    });
    union.unwrap()
}

fn event_button(ui: &mut Ui, at: &mut f64, time: f64) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(64.0, 22.0), Sense::click());
    if resp.clicked() {
        *at = time;
    }
    let flash = util::event_flash(time, *at);
    let painter = ui.painter();
    let fill = if resp.is_pointer_button_down_on() {
        ACCENT_DIM
    } else if resp.hovered() {
        BG_ELEVATED.gamma_multiply(1.3)
    } else {
        BG_ELEVATED
    };
    painter.rect_filled(rect, CornerRadius::same(4), fill);
    if flash > 0.0 {
        painter.rect_filled(rect, CornerRadius::same(4), alpha(ACCENT, (flash * 130.0) as u8));
    }
    painter.text(rect.center(), Align2::CENTER_CENTER, "FIRE", FontId::proportional(10.0), FG);
    rect
}

fn pad(ui: &mut Ui, min: [f32; 2], max: [f32; 2], p: &mut [f32; 2]) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(120.0, 72.0), Sense::click_and_drag());
    if (resp.dragged() || resp.clicked()) && resp.interact_pointer_pos().is_some() {
        let pos = resp.interact_pointer_pos().unwrap();
        let tx = ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
        let ty = 1.0 - ((pos.y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
        p[0] = min[0] + tx * (max[0] - min[0]);
        p[1] = min[1] + ty * (max[1] - min[1]);
    }
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(4), BG_INSET);
    painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, BORDER), StrokeKind::Inside);
    // Quarter grid.
    for k in 1..4 {
        let x = rect.min.x + rect.width() * k as f32 / 4.0;
        let y = rect.min.y + rect.height() * k as f32 / 4.0;
        painter.line_segment([Pos2::new(x, rect.min.y), Pos2::new(x, rect.max.y)], Stroke::new(1.0, alpha(BORDER, 90)));
        painter.line_segment([Pos2::new(rect.min.x, y), Pos2::new(rect.max.x, y)], Stroke::new(1.0, alpha(BORDER, 90)));
    }
    let tx = (p[0] - min[0]) / (max[0] - min[0]);
    let ty = 1.0 - (p[1] - min[1]) / (max[1] - min[1]);
    let dot = Pos2::new(rect.min.x + rect.width() * tx, rect.min.y + rect.height() * ty);
    painter.line_segment([Pos2::new(dot.x, rect.min.y), Pos2::new(dot.x, rect.max.y)], Stroke::new(1.0, ACCENT_DIM));
    painter.line_segment([Pos2::new(rect.min.x, dot.y), Pos2::new(rect.max.x, dot.y)], Stroke::new(1.0, ACCENT_DIM));
    painter.circle_filled(dot, 4.0, ACCENT);

    let (vrect, _) = ui.allocate_exact_size(vec2(66.0, 72.0), Sense::hover());
    ui.painter().text(
        Pos2::new(vrect.min.x + 4.0, vrect.center().y - 7.0),
        Align2::LEFT_CENTER,
        format!("x {:.2}", p[0]),
        FontId::monospace(11.0),
        FG_DIM,
    );
    ui.painter().text(
        Pos2::new(vrect.min.x + 4.0, vrect.center().y + 7.0),
        Align2::LEFT_CENTER,
        format!("y {:.2}", p[1]),
        FontId::monospace(11.0),
        FG_DIM,
    );
    rect.union(vrect)
}

fn color_row(ui: &mut Ui, i: usize, c: &mut Hsva) -> Rect {
    let (swatch, _) = ui.allocate_exact_size(vec2(34.0, 18.0), Sense::hover());
    ui.painter().rect_filled(swatch, CornerRadius::same(4), Color32::from(*c));
    ui.painter().rect_stroke(swatch, CornerRadius::same(4), Stroke::new(1.0, BORDER), StrokeKind::Inside);
    let mut union = swatch;
    for (k, label) in ["H", "S", "V"].iter().enumerate() {
        let ch = match k {
            0 => &mut c.h,
            1 => &mut c.s,
            _ => &mut c.v,
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(58.0, 18.0), Sense::click_and_drag());
        let _ = ui.id().with(("hsv", i, k));
        if resp.dragged() || resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                *ch = ((p.x - rect.min.x - 12.0) / (rect.width() - 12.0)).clamp(0.0, 1.0);
            }
        }
        let painter = ui.painter();
        painter.text(
            Pos2::new(rect.min.x, rect.center().y),
            Align2::LEFT_CENTER,
            *label,
            FontId::proportional(9.0),
            FG_MUTED,
        );
        let track = Rect::from_min_max(Pos2::new(rect.min.x + 12.0, rect.center().y - 1.0), Pos2::new(rect.max.x, rect.center().y + 1.0));
        painter.rect_filled(track, CornerRadius::same(1), BG_ELEVATED);
        let x = egui::lerp(track.min.x..=track.max.x, *ch);
        painter.circle_filled(Pos2::new(x, rect.center().y), 4.0, FG_DIM);
        union = union.union(rect);
    }
    union
}

fn source_tile(ui: &mut Ui, name: &str) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(CTL_W, 24.0), Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(4), BG_INSET);
    painter.rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, BORDER), StrokeKind::Inside);
    let thumb = Rect::from_min_size(rect.min + vec2(3.0, 3.0), vec2(30.0, 18.0));
    painter.rect_filled(thumb, CornerRadius::same(2), BG_ELEVATED);
    painter.text(thumb.center(), Align2::CENTER_CENTER, "▶", FontId::proportional(9.0), FG_MUTED);
    painter.text(
        Pos2::new(thumb.max.x + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        FontId::monospace(11.0),
        FG_DIM,
    );
    let hint = if resp.hovered() { FG } else { FG_MUTED };
    painter.text(
        Pos2::new(rect.max.x - 6.0, rect.center().y),
        Align2::RIGHT_CENTER,
        "ROUTE ▾",
        FontId::proportional(9.0),
        hint,
    );
    rect
}

fn audio_meter(ui: &mut Ui, time: f64) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(CTL_W, 14.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), BG_INSET);
    for ch in 0..2 {
        let lvl = util::level(time, ch as f32);
        let y0 = rect.min.y + 2.0 + ch as f32 * 6.0;
        let bar = Rect::from_min_size(
            Pos2::new(rect.min.x + 2.0, y0),
            vec2((rect.width() - 4.0) * lvl, 4.0),
        );
        painter.rect_filled(bar, CornerRadius::same(1), if lvl > 0.9 { Color32::from_rgb(255, 99, 99) } else { ACCENT });
    }
    rect
}

fn fft_bars(ui: &mut Ui, time: f64) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(CTL_W, 26.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(2), BG_INSET);
    let n = 48;
    let bw = (rect.width() - 4.0) / n as f32;
    for k in 0..n {
        let mag = util::fft(time, k, n);
        let h = (rect.height() - 4.0) * mag;
        let bar = Rect::from_min_max(
            Pos2::new(rect.min.x + 2.0 + k as f32 * bw, rect.max.y - 2.0 - h),
            Pos2::new(rect.min.x + 2.0 + (k as f32 + 0.8) * bw, rect.max.y - 2.0),
        );
        painter.rect_filled(bar, CornerRadius::ZERO, alpha(ACCENT, 90 + (mag * 165.0) as u8));
    }
    rect
}

/// Console's bind affordance: a small chip after the control; in learn mode a
/// dashed accent outline over the control, clickable to (un)bind.
fn midi_affordance(ui: &mut Ui, midi: &mut MidiState, idx: usize, kind: &MockKind, rect: Rect, time: f64) {
    if !MidiState::bindable(kind) {
        return;
    }
    if let Some(b) = midi.binding(idx) {
        let text = b.label();
        let font = FontId::monospace(9.0);
        let galley = ui.painter().layout_no_wrap(text.clone(), font.clone(), ACCENT);
        let (crect, _) = ui.allocate_exact_size(galley.size() + vec2(10.0, 6.0), Sense::hover());
        let act = midi.activity(idx, time);
        let fill = if act > 0.0 { alpha(ACCENT, 40 + (act * 120.0) as u8) } else { ACCENT_DIM };
        ui.painter().rect_filled(crect, CornerRadius::same(8), fill);
        ui.painter().text(crect.center(), Align2::CENTER_CENTER, text, font, ACCENT);
    }
    if midi.armed {
        let resp = ui.interact(rect, ui.id().with(("learn", idx)), Sense::click());
        let pulse = 0.5 + 0.5 * ((time * 6.0).sin() as f32);
        let color = alpha(ACCENT, 120 + (pulse * 120.0) as u8);
        for [a, b] in edges(rect.expand(2.0)) {
            ui.painter().add(Shape::dashed_line(&[a, b], Stroke::new(1.0, color), 4.0, 3.0));
        }
        if resp.clicked() {
            midi.learn_click(idx, kind);
        }
    }
}

fn edges(r: Rect) -> [[Pos2; 2]; 4] {
    [
        [r.left_top(), r.right_top()],
        [r.right_top(), r.right_bottom()],
        [r.right_bottom(), r.left_bottom()],
        [r.left_bottom(), r.left_top()],
    ]
}

/// The app's shared widget vocabulary (src/ui/widgets.rs + transport/status
/// specials), near-verbatim: the comparison baseline.
pub fn show_widgets(ui: &mut Ui, st: &mut DemoState) {
    let time = st.time;
    let (beat, pulse) = util::beat(time);

    wsection(ui, "TRANSPORT");
    ui.horizontal(|ui| {
        for (k, (glyph, color)) in [("▼", FG), ("⟲", Color32::from_rgb(255, 99, 99)), ("TAP", FG)].iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(52.0, 40.0), Sense::click());
            let flash = util::tap_flash(ui, ui.id().with(("tap", k)), resp.clicked(), time);
            let fill = if resp.is_pointer_button_down_on() {
                ACCENT_DIM
            } else if resp.hovered() {
                BG_ELEVATED.gamma_multiply(1.3)
            } else {
                BG_ELEVATED
            };
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(4), fill);
            if flash > 0.0 {
                painter.rect_filled(rect, CornerRadius::same(4), alpha(ACCENT, (flash * 120.0) as u8));
            }
            painter.text(rect.center(), Align2::CENTER_CENTER, *glyph, FontId::monospace(16.0), *color);
        }
        ui.add_space(10.0);
        // Beat dots + phrase strip + bpm readout.
        let (rect, _) = ui.allocate_exact_size(vec2(64.0, 40.0), Sense::hover());
        for k in 0..4 {
            let center = Pos2::new(rect.min.x + 8.0 + k as f32 * 15.0, rect.center().y);
            if k == beat {
                ui.painter().circle_filled(center, 4.0 + pulse * 1.5, ACCENT);
            } else {
                ui.painter().circle_filled(center, 3.0, BG_ELEVATED);
            }
        }
        let (strip, _) = ui.allocate_exact_size(vec2(128.0, 40.0), Sense::hover());
        let pos = util::phrase(time);
        for k in 0..16 {
            let r = Rect::from_min_size(
                Pos2::new(strip.min.x + k as f32 * 8.0, strip.center().y - 5.0),
                vec2(6.0, 10.0),
            );
            let fill = if k == pos { ACCENT } else if k < pos { ACCENT_DIM } else { BG_ELEVATED };
            ui.painter().rect_filled(r, CornerRadius::same(1), fill);
        }
        ui.label(egui::RichText::new("120.0").monospace().size(15.0).color(FG));
        ui.label(egui::RichText::new("bpm").small().color(FG_MUTED));
    });

    ui.add_space(10.0);
    wsection(ui, "SYNC · NEXT EVERY");
    ui.horizontal(|ui| {
        wsegmented(ui, "sync", &["INTERNAL", "LINK"], 0);
        ui.add_space(12.0);
        wsegmented(ui, "next", &["1", "2", "4", "8"], 2);
        ui.add_space(12.0);
        wchip(ui, "cue: intro", None, false);
        wchip(ui, "2 peers", Some(Color32::from_rgb(98, 217, 107)), false);
        wchip(ui, "audio ⚠", Some(Color32::from_rgb(255, 99, 99)), true);
    });

    ui.add_space(10.0);
    wsection(ui, "CLIP POOL");
    ui.horizontal(|ui| {
        for clip in mock_clips() {
            wtile(ui, &clip, pulse, time);
        }
    });

    ui.add_space(10.0);
    wsection(ui, "LEVELS");
    audio_meter(ui, time);
    ui.add_space(4.0);
    fft_bars(ui, time);

    ui.add_space(10.0);
    util::footer(
        ui,
        FG_MUTED,
        FG_DIM,
        &[("WIDGETS", "segmented, section_label, chip (tint/removable), media_tile (scrim name, role badge, selection/active/pulse rings), transport_button (flash), beat dots, phrase strip, level bar, spectrum — as shipped in src/ui.")],
    );
}

fn wsection(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(FG_MUTED));
    ui.add_space(2.0);
}

fn wsegmented(ui: &mut Ui, salt: &str, labels: &[&str], selected: usize) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, label) in labels.iter().enumerate() {
            let galley = ui.painter().layout_no_wrap((*label).into(), FontId::proportional(11.0), FG);
            let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(16.0, 10.0), Sense::hover());
            let resp = ui.interact(rect, ui.id().with((salt, i)), Sense::click());
            let last = labels.len() - 1;
            let radius = CornerRadius {
                nw: u8::from(i == 0) * 4,
                sw: u8::from(i == 0) * 4,
                ne: u8::from(i == last) * 4,
                se: u8::from(i == last) * 4,
            };
            let fill = if selected == i {
                ACCENT_DIM
            } else if resp.hovered() {
                BG_ELEVATED
            } else {
                BG_INSET
            };
            ui.painter().rect_filled(rect, radius, fill);
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                *label,
                FontId::proportional(11.0),
                if selected == i { FG } else { FG_DIM },
            );
        }
    });
}

fn wchip(ui: &mut Ui, text: &str, tint: Option<Color32>, removable: bool) {
    let (fill, color) = match tint {
        Some(t) => (t.linear_multiply(0.15), t),
        None => (BG_ELEVATED, FG_DIM),
    };
    let font = FontId::proportional(10.0);
    let galley = ui.painter().layout_no_wrap(text.into(), font.clone(), color);
    let close_w = if removable { 12.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 12.0 + close_w, 18.0), Sense::click());
    let fill = if resp.hovered() { fill.gamma_multiply(1.25) } else { fill };
    ui.painter().rect_filled(rect, CornerRadius::same(9), fill);
    ui.painter().text(
        Pos2::new(rect.min.x + 6.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        font,
        color,
    );
    if removable && resp.hovered() {
        ui.painter().text(
            Pos2::new(rect.max.x - 8.0, rect.center().y),
            Align2::CENTER_CENTER,
            "✕",
            FontId::proportional(9.0),
            FG_MUTED,
        );
    }
}

/// `media_tile` stand-in: procedural band art, bottom scrim, role badge, and
/// the selection/active/hover/pulse outline stack.
fn wtile(ui: &mut Ui, clip: &crate::schema::MockClip, pulse: f32, _time: f64) {
    let size = vec2(116.0, 66.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let radius = CornerRadius::same(4);
    let painter = ui.painter();
    // Seeded band art in place of a decoded thumbnail.
    painter.rect_filled(rect, radius, BG_INSET);
    for k in 0..6 {
        let h0 = util::hash01(clip.seed, k);
        let h1 = util::hash01(clip.seed, k + 17);
        let band = Rect::from_min_size(
            Pos2::new(rect.min.x, rect.min.y + rect.height() * k as f32 / 6.0),
            vec2(rect.width(), rect.height() / 6.0 + 1.0),
        );
        painter.rect_filled(band, CornerRadius::ZERO, util::hsl(190.0 + h0 * 130.0, 0.35, 0.12 + h1 * 0.22));
    }
    if resp.hovered() {
        painter.rect_filled(rect, radius, alpha(Color32::WHITE, 24));
    }
    // Scrim + name.
    let scrim = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 20.0), rect.max);
    painter.rect_filled(scrim, CornerRadius { nw: 0, ne: 0, sw: 4, se: 4 }, alpha(Color32::BLACK, 140));
    painter.text(
        Pos2::new(rect.min.x + 4.0, rect.max.y - 10.0),
        Align2::LEFT_CENTER,
        clip.name,
        FontId::proportional(9.0),
        FG,
    );
    // Role badge.
    let (glyph, color) = match clip.role {
        MockRole::Playing => ("▶", Color32::from_rgb(98, 217, 107)),
        MockRole::Armed => ("○", Color32::from_rgb(255, 171, 64)),
        MockRole::None => ("", FG),
    };
    if !glyph.is_empty() {
        let center = rect.min + vec2(10.0, 10.0);
        painter.circle_filled(center, 7.0, alpha(Color32::BLACK, 170));
        painter.text(center, Align2::CENTER_CENTER, glyph, FontId::proportional(10.0), color);
    }
    // Outline stack.
    if clip.selected {
        painter.rect_stroke(rect, radius, Stroke::new(2.0, ACCENT), StrokeKind::Inside);
    } else if clip.role == MockRole::Armed {
        painter.rect_stroke(rect, radius, Stroke::new(1.0, Color32::from_rgb(255, 171, 64)), StrokeKind::Inside);
    } else if resp.hovered() {
        painter.rect_stroke(rect, radius, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    }
    if clip.role == MockRole::Playing {
        let a = (pulse.powi(2) * 160.0) as u8;
        painter.rect_stroke(rect, radius, Stroke::new(2.0, alpha(Color32::from_rgb(98, 217, 107), a)), StrokeKind::Inside);
    }
}

fn footer(ui: &mut Ui) {
    ui.add_space(10.0);
    util::footer(
        ui,
        FG_MUTED,
        FG_DIM,
        &[
            ("DENSITY", "~28 px per scalar row; the whole shader fits a chain slot without paging."),
            ("EPAINT COST", "Lowest of the four — every widget is a short step from widgets.rs; no arcs, no fonts, no textures."),
            ("MIDI", "Chip after the control; learn = dashed outline. Reads fine at density but the chip column widens rows."),
            ("FIT", "Zero theme risk: this is PALETTE verbatim. The safe default if the chain editor stays terminal-flat."),
        ],
    );
}
