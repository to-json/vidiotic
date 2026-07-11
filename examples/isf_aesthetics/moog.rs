//! Direction 2 — **Moog** (Minimoog/Matriarch): a warm instrument panel.
//! Fluted rotary knobs with a printed skirt scale, rocker switches, a
//! multi-position selector, jack sockets for streams, and a cream-faced VU
//! meter. Deviations from hardware: no wood texture (warmth lives in the
//! palette), and values print under each legend — a panel can stay mute, a
//! screen shouldn't. Absolute knobs mean MIDI needs soft pickup: bound knobs
//! ghost the incoming CC position as a translucent pointer.

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, vec2};

use crate::schema::{DemoState, MidiState, MockKind, MockRole, Value, mock_clips};
use crate::util;

const PANEL: Color32 = Color32::from_rgb(24, 20, 17);
const KNOB_BODY: Color32 = Color32::from_rgb(15, 13, 11);
const KNOB_HAT: Color32 = Color32::from_rgb(42, 37, 30);
const CREAM: Color32 = Color32::from_rgb(232, 220, 192);
const CREAM_DIM: Color32 = Color32::from_rgb(150, 140, 118);
const EDGE: Color32 = Color32::from_rgb(64, 56, 45);
const RED: Color32 = Color32::from_rgb(224, 74, 58);
const VU_FACE: Color32 = Color32::from_rgb(228, 214, 180);

const BLOCK_H: f32 = 108.0;

fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Render the whole direction panel.
pub fn show(ui: &mut Ui, st: &mut DemoState) {
    let panel = ui.available_rect_before_wrap();
    ui.painter().rect_filled(panel.expand(8.0), CornerRadius::same(6), PANEL);

    // Panel legend with a double rule, engraved-plate style.
    let (hrect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let painter = ui.painter();
    painter.text(
        hrect.left_center(),
        Align2::LEFT_CENTER,
        "KALEIDO BLOOM",
        FontId::proportional(13.0),
        CREAM,
    );
    for dy in [-4.0, -1.0] {
        painter.line_segment(
            [
                Pos2::new(hrect.min.x + 130.0, hrect.center().y + dy),
                Pos2::new(hrect.max.x, hrect.center().y + dy),
            ],
            Stroke::new(1.0, EDGE),
        );
    }
    ui.add_space(8.0);

    let time = st.time;
    let armed = st.midi.armed;
    let DemoState { inputs, values, midi, .. } = st;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(14.0, 14.0);
        for (i, (inp, val)) in inputs.iter().zip(values.iter_mut()).enumerate() {
            let frozen = *val;
            let incoming = midi.binding(i).and_then(|_| midi.incoming_pos(i, time));
            let rects = control(ui, i, inp.label, &inp.kind, val, time, incoming);
            if armed {
                *val = frozen;
            }
            if let Some(first) = rects.first() {
                bind_legend(ui, midi, i, &inp.kind, *first, time);
            }
            for rect in rects {
                learn_ring(ui, midi, i, &inp.kind, rect, time);
            }
        }
    });
    ui.add_space(10.0);
    util::footer(
        ui,
        CREAM_DIM,
        Color32::from_rgb(190, 178, 152),
        &[
            ("DENSITY", "Lowest: ~80×108 px per scalar. Two or three chained effects fill a window — this is a focused-slot view, not a chain view."),
            ("EPAINT COST", "Mid — flutes, skirt ticks, and the VU needle are all line fans; nothing exotic, but each knob is ~40 shapes."),
            ("MIDI", "Printed legend under the control; absolute knobs demand the soft-pickup ghost pointer, which this direction makes legible for free."),
            ("FIT", "Furthest from the current chrome; would pull theme.rs warm. Strongest at communicating 'instrument', weakest at density."),
        ],
    );
}

/// The app's widget vocabulary as panel hardware: momentary buttons + jewel
/// lamps for transport, a rocker for sync, a rotary selector for cadence,
/// engraved plates for chips, bezeled windows for clip tiles, the VU for
/// levels. Interactive state parks in egui temp memory.
pub fn show_widgets(ui: &mut Ui, st: &mut DemoState) {
    let panel = ui.available_rect_before_wrap();
    ui.painter().rect_filled(panel.expand(8.0), CornerRadius::same(6), PANEL);
    let time = st.time;
    let (beat, pulse) = util::beat(time);

    let (hrect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    ui.painter().text(hrect.left_center(), Align2::LEFT_CENTER, "VIDIOTIC · WIDGETS", FontId::proportional(13.0), CREAM);
    for dy in [-4.0, -1.0] {
        ui.painter().line_segment(
            [Pos2::new(hrect.min.x + 150.0, hrect.center().y + dy), Pos2::new(hrect.max.x, hrect.center().y + dy)],
            Stroke::new(1.0, EDGE),
        );
    }
    ui.add_space(8.0);

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(14.0, 14.0);
        for label in ["Downbeat", "Reset", "Tap"] {
            let id = ui.id().with(("wmom", label));
            let mut at: f64 = ui.ctx().data_mut(|d| d.get_temp(id).unwrap_or(-1.0));
            momentary_block(ui, label, &mut at, time);
            ui.ctx().data_mut(|d| d.insert_temp(id, at));
        }
        // Clock block: four jewel lamps + an engraved phrase scale.
        let (rect, _) = ui.allocate_exact_size(vec2(130.0, BLOCK_H), Sense::hover());
        let painter = ui.painter();
        for k in 0..4 {
            let lamp = Pos2::new(rect.min.x + 22.0 + k as f32 * 28.0, rect.min.y + 22.0);
            painter.circle_stroke(lamp, 6.5, Stroke::new(1.5, CREAM_DIM));
            if k == beat {
                painter.circle_filled(lamp, 5.0, alpha(RED, 90 + (pulse * 160.0) as u8));
            } else {
                painter.circle_filled(lamp, 5.0, Color32::from_rgb(50, 26, 22));
            }
        }
        let y = rect.min.y + 52.0;
        let pos = util::phrase(time);
        painter.line_segment([Pos2::new(rect.min.x + 10.0, y), Pos2::new(rect.max.x - 10.0, y)], Stroke::new(1.0, CREAM_DIM));
        for k in 0..16 {
            let x = rect.min.x + 10.0 + (rect.width() - 20.0) * k as f32 / 15.0;
            let major = k % 4 == 0;
            painter.line_segment([Pos2::new(x, y), Pos2::new(x, y - if major { 6.0 } else { 4.0 })], Stroke::new(1.0, CREAM_DIM));
            if k == pos {
                painter.line_segment([Pos2::new(x, y + 2.0), Pos2::new(x, y + 9.0)], Stroke::new(2.0, CREAM));
            }
        }
        legend(ui, rect, "Clock", "120 BPM");

        let link_id = ui.id().with("wlink");
        let mut link: bool = ui.ctx().data_mut(|d| d.get_temp(link_id).unwrap_or(false));
        rocker_block(ui, "Link", &mut link);
        ui.ctx().data_mut(|d| d.insert_temp(link_id, link));

        let next_id = ui.id().with("wnext");
        let mut next: usize = ui.ctx().data_mut(|d| d.get_temp(next_id).unwrap_or(2));
        selector_block(ui, 90, "Next Every", &["1", "2", "4", "8"], &mut next);
        ui.ctx().data_mut(|d| d.insert_temp(next_id, next));

        // Chips as engraved plates.
        let (rect, _) = ui.allocate_exact_size(vec2(86.0, BLOCK_H), Sense::hover());
        let painter = ui.painter();
        for (k, (text, color)) in [("2 PEERS", CREAM), ("AUDIO ⚠", RED)].iter().enumerate() {
            let plate = Rect::from_center_size(
                Pos2::new(rect.center().x, rect.min.y + 20.0 + k as f32 * 26.0),
                vec2(74.0, 18.0),
            );
            painter.rect_stroke(plate, CornerRadius::same(2), Stroke::new(1.0, CREAM_DIM), StrokeKind::Inside);
            painter.text(plate.center(), Align2::CENTER_CENTER, *text, FontId::proportional(9.0), *color);
        }
        legend(ui, rect, "Status", "");

        // Clip pool: bezeled windows with band art, a role lamp, selection ring.
        for clip in mock_clips() {
            let short = clip.name.split('.').next().unwrap_or(clip.name);
            let seed = clip.seed;
            let rect = window_block(ui, short, |painter, win| {
                for k in 0..5 {
                    let h0 = util::hash01(seed, k);
                    let band = Rect::from_min_size(
                        Pos2::new(win.min.x, win.min.y + win.height() * k as f32 / 5.0),
                        vec2(win.width(), win.height() / 5.0 + 1.0),
                    );
                    painter.rect_filled(band, CornerRadius::ZERO, util::hsl(28.0 + h0 * 30.0, 0.35, 0.10 + util::hash01(seed, k + 9) * 0.16));
                }
            });
            let painter = ui.painter();
            let lamp = Pos2::new(rect.max.x - 12.0, rect.min.y + 8.0);
            match clip.role {
                MockRole::Playing => {
                    painter.circle_filled(lamp, 4.5, alpha(RED, 70));
                    painter.circle_filled(lamp, 2.5, alpha(RED, 130 + (pulse * 125.0) as u8));
                }
                MockRole::Armed => {
                    painter.circle_stroke(lamp, 3.0, Stroke::new(1.5, CREAM_DIM));
                }
                MockRole::None => {}
            }
            if clip.selected {
                painter.rect_stroke(rect.shrink(1.0), CornerRadius::same(4), Stroke::new(1.5, CREAM), StrokeKind::Inside);
            }
        }
        vu_block(ui, time);
        spectrum_block(ui, time);
    });
    ui.add_space(10.0);
    util::footer(
        ui,
        CREAM_DIM,
        Color32::from_rgb(190, 178, 152),
        &[("WIDGETS", "Taps are momentary buttons with jewel lamps, the phrase is an engraved scale with a cream pointer, chips are engraved plates, clip tiles are bezeled windows with a role lamp — and levels get the VU it always wanted.")],
    );
}

/// One control as one-or-more panel blocks; returns each block rect.
fn control(
    ui: &mut Ui,
    i: usize,
    label: &str,
    kind: &MockKind,
    val: &mut Value,
    time: f64,
    incoming: Option<f32>,
) -> Vec<Rect> {
    match (kind, val) {
        (MockKind::Float { min, max, .. }, Value::Float(v)) => {
            let mut t = (*v - min) / (max - min);
            let rect = knob_block(ui, i, 0, label, &format!("{v:.2}"), &mut t, 46.0, incoming);
            *v = min + t * (max - min);
            vec![rect]
        }
        (MockKind::Bool { .. }, Value::Bool(b)) => vec![rocker_block(ui, label, b)],
        (MockKind::Long { labels, .. }, Value::LongIdx(sel)) => {
            vec![selector_block(ui, i, label, labels, sel)]
        }
        (MockKind::Event, Value::EventAt(at)) => vec![momentary_block(ui, label, at, time)],
        (MockKind::Point2D { min, max, .. }, Value::Point(p)) => {
            let mut out = Vec::new();
            for axis in 0..2 {
                let mut t = (p[axis] - min[axis]) / (max[axis] - min[axis]);
                let axis_label = if axis == 0 { "X" } else { "Y" };
                out.push(knob_block(
                    ui,
                    i,
                    axis + 1,
                    &format!("{label} {axis_label}"),
                    &format!("{:.2}", p[axis]),
                    &mut t,
                    40.0,
                    incoming,
                ));
                p[axis] = min[axis] + t * (max[axis] - min[axis]);
            }
            out.push(window_block(ui, label, |painter, win| {
                let tx = (p[0] - min[0]) / (max[0] - min[0]);
                let ty = 1.0 - (p[1] - min[1]) / (max[1] - min[1]);
                let dot = Pos2::new(win.min.x + win.width() * tx, win.min.y + win.height() * ty);
                painter.circle_filled(dot, 3.0, CREAM);
            }));
            out
        }
        (MockKind::Color { .. }, Value::Color(c)) => {
            let mut out = Vec::new();
            for (k, ch_label) in ["HUE", "SAT", "VAL"].iter().enumerate() {
                let ch = match k {
                    0 => &mut c.h,
                    1 => &mut c.s,
                    _ => &mut c.v,
                };
                let shown = *ch;
                out.push(knob_block(ui, i, k + 1, ch_label, &format!("{shown:.2}"), ch, 34.0, None));
            }
            let color = Color32::from(*c);
            out.push(window_block(ui, label, |painter, win| {
                painter.rect_filled(win, CornerRadius::same(2), color);
            }));
            out
        }
        (MockKind::Image, _) => vec![jack_block(ui, label, "VIDEO")],
        (MockKind::Audio, _) => vec![jack_block(ui, label, "AUDIO"), vu_block(ui, time)],
        (MockKind::AudioFft, _) => vec![jack_block(ui, label, "FFT"), spectrum_block(ui, time)],
        _ => vec![],
    }
}

fn legend(ui: &Ui, rect: Rect, label: &str, value: &str) {
    let painter = ui.painter();
    painter.text(
        Pos2::new(rect.center().x, rect.max.y - 22.0),
        Align2::CENTER_CENTER,
        label.to_uppercase(),
        FontId::proportional(9.0),
        CREAM,
    );
    if !value.is_empty() {
        painter.text(
            Pos2::new(rect.center().x, rect.max.y - 10.0),
            Align2::CENTER_CENTER,
            value,
            FontId::monospace(9.0),
            CREAM_DIM,
        );
    }
}

/// A fluted Moog knob with skirt scale. `t` is normalized; `incoming` paints
/// the soft-pickup ghost pointer at the mock incoming-CC position.
#[allow(clippy::too_many_arguments)]
fn knob_block(
    ui: &mut Ui,
    i: usize,
    sub: usize,
    label: &str,
    value: &str,
    t: &mut f32,
    diameter: f32,
    incoming: Option<f32>,
) -> Rect {
    let w = diameter + 34.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, BLOCK_H), Sense::click_and_drag());
    util::vdrag(&resp, t, 0.0, 1.0, 160.0);
    if resp.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeVertical);
    }
    let center = Pos2::new(rect.center().x, rect.min.y + diameter / 2.0 + 12.0);
    let r = diameter / 2.0;
    let painter = ui.painter();

    // Skirt scale: ticks around the sweep, printed on the panel.
    for k in 0..=10 {
        let tt = k as f32 / 10.0;
        let dir = util::knob_dir(tt);
        painter.line_segment([center + dir * (r + 3.0), center + dir * (r + 7.0)], Stroke::new(1.0, CREAM_DIM));
    }
    painter.text(center + util::knob_dir(0.0) * (r + 13.0), Align2::CENTER_CENTER, "0", FontId::proportional(8.0), CREAM_DIM);
    painter.text(center + util::knob_dir(1.0) * (r + 13.0), Align2::CENTER_CENTER, "10", FontId::proportional(8.0), CREAM_DIM);

    // Body: skirt with flutes, then the hat.
    painter.circle_filled(center, r, KNOB_BODY);
    painter.circle_stroke(center, r, Stroke::new(1.0, EDGE));
    for k in 0..18 {
        let a = k as f32 / 18.0 * std::f32::consts::TAU;
        let dir = egui::Vec2::angled(a);
        painter.line_segment([center + dir * (r - 4.0), center + dir * r], Stroke::new(1.2, KNOB_HAT));
    }
    let hat_r = r * 0.62;
    painter.circle_filled(center, hat_r, KNOB_HAT);
    painter.circle_stroke(center, hat_r, Stroke::new(1.0, EDGE));

    // Ghost pointer first, so the real pointer wins where they overlap.
    if let Some(inc) = incoming {
        let dir = util::knob_dir(inc);
        painter.line_segment([center + dir * (hat_r * 0.2), center + dir * (r - 1.0)], Stroke::new(2.0, alpha(CREAM, 70)));
    }
    let dir = util::knob_dir(*t);
    painter.line_segment([center + dir * (hat_r * 0.15), center + dir * (r - 1.0)], Stroke::new(2.0, CREAM));

    legend(ui, rect, label, value);
    let _ = (i, sub);
    rect
}

fn rocker_block(ui: &mut Ui, label: &str, b: &mut bool) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(58.0, BLOCK_H), Sense::click());
    if resp.clicked() {
        *b = !*b;
    }
    let painter = ui.painter();
    let sw = Rect::from_center_size(Pos2::new(rect.center().x, rect.min.y + 34.0), vec2(24.0, 44.0));
    painter.rect_filled(sw, CornerRadius::same(4), KNOB_BODY);
    painter.rect_stroke(sw, CornerRadius::same(4), Stroke::new(1.0, EDGE), StrokeKind::Inside);
    // The pressed half sits dark; the raised half catches light.
    let (raised, pressed) = if *b {
        (Rect::from_min_max(sw.min, Pos2::new(sw.max.x, sw.center().y)), Rect::from_min_max(Pos2::new(sw.min.x, sw.center().y), sw.max))
    } else {
        (Rect::from_min_max(Pos2::new(sw.min.x, sw.center().y), sw.max), Rect::from_min_max(sw.min, Pos2::new(sw.max.x, sw.center().y)))
    };
    painter.rect_filled(raised.shrink(2.0), CornerRadius::same(3), KNOB_HAT);
    painter.rect_filled(pressed.shrink(2.0), CornerRadius::same(3), Color32::from_rgb(10, 9, 8));
    painter.text(Pos2::new(sw.center().x, sw.min.y - 7.0), Align2::CENTER_CENTER, "ON", FontId::proportional(8.0), if *b { CREAM } else { CREAM_DIM });
    painter.text(Pos2::new(sw.center().x, sw.max.y + 7.0), Align2::CENTER_CENTER, "OFF", FontId::proportional(8.0), if *b { CREAM_DIM } else { CREAM });
    legend(ui, rect, label, "");
    rect
}

fn selector_block(ui: &mut Ui, i: usize, label: &str, labels: &[&str], sel: &mut usize) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(86.0, BLOCK_H), Sense::click_and_drag());
    let steps = util::vdrag_steps(ui, ui.id().with(("sel", i)), &resp, 16.0);
    let n = labels.len();
    let mut s = *sel as i32 + steps;
    if resp.clicked() {
        s += 1;
    }
    *sel = s.rem_euclid(n as i32) as usize;

    let center = Pos2::new(rect.center().x, rect.min.y + 34.0);
    let r = 20.0;
    let painter = ui.painter();
    // Radial legends at each detent.
    for (k, name) in labels.iter().enumerate() {
        let tt = k as f32 / (n - 1) as f32;
        let pos = center + util::knob_dir(tt) * (r + 11.0);
        let color = if k == *sel { CREAM } else { CREAM_DIM };
        painter.text(pos, Align2::CENTER_CENTER, *name, FontId::proportional(8.0), color);
        let dir = util::knob_dir(tt);
        painter.line_segment([center + dir * (r + 2.0), center + dir * (r + 5.0)], Stroke::new(1.0, CREAM_DIM));
    }
    painter.circle_filled(center, r, KNOB_BODY);
    painter.circle_stroke(center, r, Stroke::new(1.0, EDGE));
    painter.circle_filled(center, r * 0.55, KNOB_HAT);
    let t = *sel as f32 / (n - 1) as f32;
    let dir = util::knob_dir(t);
    painter.line_segment([center, center + dir * (r - 2.0)], Stroke::new(2.5, CREAM));
    legend(ui, rect, label, labels[*sel]);
    rect
}

fn momentary_block(ui: &mut Ui, label: &str, at: &mut f64, time: f64) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(58.0, BLOCK_H), Sense::click());
    if resp.clicked() {
        *at = time;
    }
    let flash = util::event_flash(time, *at);
    let center = Pos2::new(rect.center().x, rect.min.y + 34.0);
    let painter = ui.painter();
    painter.circle_stroke(center, 14.0, Stroke::new(2.0, CREAM_DIM));
    let fill = if resp.is_pointer_button_down_on() { Color32::from_rgb(8, 7, 6) } else { KNOB_BODY };
    painter.circle_filled(center, 11.0, fill);
    if flash > 0.0 {
        painter.circle_filled(center, 11.0, alpha(RED, (flash * 150.0) as u8));
    }
    // The little panel LED beside the button.
    let led = center + vec2(22.0, -10.0);
    if flash > 0.0 {
        painter.circle_filled(led, 4.5, alpha(RED, 80));
        painter.circle_filled(led, 2.5, RED);
    } else {
        painter.circle_filled(led, 2.5, Color32::from_rgb(70, 30, 26));
    }
    legend(ui, rect, label, "");
    rect
}

/// A cream-bezeled panel window with custom contents (position dot, swatch).
fn window_block(ui: &mut Ui, label: &str, paint: impl FnOnce(&egui::Painter, Rect)) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(74.0, BLOCK_H), Sense::hover());
    let win = Rect::from_center_size(Pos2::new(rect.center().x, rect.min.y + 34.0), vec2(62.0, 48.0));
    let painter = ui.painter();
    painter.rect_stroke(win.expand(2.0), CornerRadius::same(3), Stroke::new(2.0, CREAM_DIM), StrokeKind::Outside);
    painter.rect_filled(win, CornerRadius::same(2), Color32::from_rgb(10, 9, 8));
    paint(painter, win.shrink(4.0));
    legend(ui, rect, label, "");
    rect
}

fn jack_block(ui: &mut Ui, label: &str, kind: &str) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(58.0, BLOCK_H), Sense::click());
    let center = Pos2::new(rect.center().x, rect.min.y + 34.0);
    let painter = ui.painter();
    // Hex nut, then the socket.
    let pts: Vec<Pos2> = (0..6)
        .map(|k| center + egui::Vec2::angled(k as f32 / 6.0 * std::f32::consts::TAU + 0.26) * 15.0)
        .collect();
    painter.add(Shape::convex_polygon(pts, KNOB_HAT, Stroke::new(1.0, EDGE)));
    painter.circle_filled(center, 9.0, KNOB_BODY);
    painter.circle_filled(center, 5.0, Color32::BLACK);
    painter.circle_stroke(center, 9.0, Stroke::new(1.0, CREAM_DIM));
    legend(ui, rect, label, kind);
    rect
}

fn vu_block(ui: &mut Ui, time: f64) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(110.0, BLOCK_H), Sense::hover());
    let face = Rect::from_center_size(Pos2::new(rect.center().x, rect.min.y + 36.0), vec2(100.0, 58.0));
    let painter = ui.painter();
    painter.rect_stroke(face.expand(2.0), CornerRadius::same(4), Stroke::new(2.0, EDGE), StrokeKind::Outside);
    painter.rect_filled(face, CornerRadius::same(3), VU_FACE);
    let pivot = Pos2::new(face.center().x, face.max.y - 6.0);
    // Scale arc with a red overload zone, needle from the pivot.
    let arc = |t0: f32, t1: f32, color: Color32, width: f32| {
        let n = 24;
        let pts: Vec<Pos2> = (0..=n)
            .map(|k| {
                let t = t0 + (t1 - t0) * k as f32 / n as f32;
                pivot + egui::Vec2::angled((250.0 + t * 40.0).to_radians()) * 44.0
            })
            .collect();
        Shape::line(pts, Stroke::new(width, color))
    };
    painter.add(arc(0.0, 0.78, Color32::from_rgb(40, 34, 26), 1.5));
    painter.add(arc(0.78, 1.0, RED, 2.0));
    let lvl = util::level(time, 0.5);
    let needle_dir = egui::Vec2::angled((250.0 + lvl * 40.0).to_radians());
    painter.line_segment([pivot, pivot + needle_dir * 48.0], Stroke::new(1.5, Color32::from_rgb(30, 25, 20)));
    painter.circle_filled(pivot, 3.0, Color32::from_rgb(30, 25, 20));
    painter.text(face.min + vec2(6.0, 6.0), Align2::LEFT_TOP, "VU", FontId::proportional(8.0), Color32::from_rgb(90, 78, 60));
    legend(ui, rect, "Level", "");
    rect
}

fn spectrum_block(ui: &mut Ui, time: f64) -> Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(110.0, BLOCK_H), Sense::hover());
    let face = Rect::from_center_size(Pos2::new(rect.center().x, rect.min.y + 36.0), vec2(100.0, 58.0));
    let painter = ui.painter();
    painter.rect_stroke(face.expand(2.0), CornerRadius::same(4), Stroke::new(2.0, EDGE), StrokeKind::Outside);
    painter.rect_filled(face, CornerRadius::same(3), VU_FACE);
    let n = 24;
    let inner = face.shrink(6.0);
    let bw = inner.width() / n as f32;
    for k in 0..n {
        let mag = util::fft(time, k, n);
        let h = inner.height() * mag;
        let bar = Rect::from_min_max(
            Pos2::new(inner.min.x + k as f32 * bw, inner.max.y - h),
            Pos2::new(inner.min.x + (k as f32 + 0.7) * bw, inner.max.y),
        );
        painter.rect_filled(bar, CornerRadius::ZERO, Color32::from_rgb(40, 34, 26));
    }
    legend(ui, rect, "Spectrum", "");
    rect
}

/// Moog's bind affordance: a printed `MIDI 1:74` legend under the first
/// block of the control.
fn bind_legend(ui: &Ui, midi: &MidiState, idx: usize, kind: &MockKind, rect: Rect, time: f64) {
    if !MidiState::bindable(kind) {
        return;
    }
    if let Some(b) = midi.binding(idx) {
        let act = midi.activity(idx, time);
        let color = if act > 0.0 { CREAM } else { CREAM_DIM };
        ui.painter().text(
            Pos2::new(rect.center().x, rect.max.y + 1.0),
            Align2::CENTER_CENTER,
            format!("MIDI {}:{}", b.ch, b.label()),
            FontId::monospace(8.0),
            color,
        );
    }
}

/// Learn mode: a pulsing cream ring around each block; click (un)binds.
fn learn_ring(ui: &mut Ui, midi: &mut MidiState, idx: usize, kind: &MockKind, rect: Rect, time: f64) {
    if !midi.armed || !MidiState::bindable(kind) {
        return;
    }
    let resp = ui.interact(rect, ui.id().with(("learn", idx, rect.min.x as i32)), Sense::click());
    let pulse = 0.5 + 0.5 * ((time * 6.0).sin() as f32);
    ui.painter().rect_stroke(
        rect.shrink(1.0),
        CornerRadius::same(6),
        Stroke::new(1.5, alpha(CREAM, 70 + (pulse * 150.0) as u8)),
        StrokeKind::Inside,
    );
    if resp.clicked() {
        midi.learn_click(idx, kind);
    }
}
