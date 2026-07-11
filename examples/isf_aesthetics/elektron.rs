//! Direction 1 — **Elektron** (Digitakt/Octatrack): the data-dense
//! screen-and-encoder paradigm. Params are a tight grid of encoder cells —
//! backlit arc, uppercase short name, crisp mono readout — on a near-black
//! LCD panel with amber/red/green LED accents. Deviation from hardware: no
//! page banks, one wrapped grid; encoders are relative, so MIDI binds show as
//! a plain corner tag with no pickup problem.

use egui::ecolor::Hsva;
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

use crate::schema::{DemoState, MidiState, MockKind, MockRole, Value, mock_clips};
use crate::util;

const BG: Color32 = Color32::from_rgb(13, 13, 13);
const CELL: Color32 = Color32::from_rgb(22, 22, 22);
const EDGE: Color32 = Color32::from_rgb(43, 43, 43);
const TEXT: Color32 = Color32::from_rgb(236, 236, 236);
const DIM: Color32 = Color32::from_rgb(122, 122, 122);
const AMBER: Color32 = Color32::from_rgb(255, 171, 64);
const RED: Color32 = Color32::from_rgb(255, 77, 58);
const GREEN: Color32 = Color32::from_rgb(98, 217, 107);
const KEY: Color32 = Color32::from_rgb(201, 201, 201);

const CELL_W: f32 = 96.0;
const CELL_H: f32 = 64.0;
const GAP: f32 = 5.0;

fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Render the whole direction panel.
pub fn show(ui: &mut Ui, st: &mut DemoState) {
    let panel = ui.available_rect_before_wrap();
    ui.painter().rect_filled(panel.expand(8.0), CornerRadius::same(6), BG);

    // Page strip, like the machine's screen header.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("SYN·1").monospace().size(10.0).color(AMBER));
        ui.label(egui::RichText::new("KALEIDO BLOOM").monospace().size(10.0).color(TEXT));
        ui.label(egui::RichText::new("11 PARAMS · NO PAGES").monospace().size(9.0).color(DIM));
    });
    ui.add_space(6.0);

    let time = st.time;
    let armed = st.midi.armed;
    let DemoState { inputs, values, midi, .. } = st;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        for (i, (inp, val)) in inputs.iter().zip(values.iter_mut()).enumerate() {
            let frozen = *val;
            let rects = control(ui, i, inp.label, inp.name, &inp.kind, val, time);
            if armed {
                *val = frozen;
            }
            for rect in rects {
                bind_tag(ui, midi, i, &inp.kind, rect, time);
            }
        }
    });
    ui.add_space(10.0);
    util::footer(
        ui,
        DIM,
        Color32::from_rgb(170, 170, 170),
        &[
            ("DENSITY", "Highest: ~34 px² per param in a wrapped grid; 11 inputs in three rows. Scales sideways, not down."),
            ("EPAINT COST", "Low-mid — arcs are short polylines; everything else is rects and mono text."),
            ("MIDI", "Corner tag inside the cell costs zero extra space; encoders read as relative, so no pickup UI is ever needed."),
            ("FIT", "Strong for chain slots: a slot could be one encoder row. Wants a mono/LCD font and an amber secondary accent added to the theme."),
        ],
    );
}

/// The app's widget vocabulary in the machine idiom: trig keys for taps,
/// LED chains for beat/phrase, sample slots for clip tiles, position-dot
/// cells for segmented controls, corner tags for chips.
pub fn show_widgets(ui: &mut Ui, st: &mut DemoState) {
    let panel = ui.available_rect_before_wrap();
    ui.painter().rect_filled(panel.expand(8.0), CornerRadius::same(6), BG);
    let time = st.time;
    let (beat, pulse) = util::beat(time);

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("TRK·1").monospace().size(10.0).color(AMBER));
        ui.label(egui::RichText::new("VIDIOTIC WIDGETS").monospace().size(10.0).color(TEXT));
    });
    ui.add_space(6.0);

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
        // Transport: three trig keys with flash.
        for (k, label) in ["DOWN", "RESET", "TAP"].iter().enumerate() {
            let (rect, resp) = chassis(ui, CELL_W, Sense::click());
            cell_name(ui, rect, label);
            let flash = util::tap_flash(ui, ui.id().with(("wtap", k)), resp.clicked(), time);
            let painter = ui.painter();
            let key = Rect::from_min_max(rect.min + vec2(8.0, 26.0), rect.max - vec2(8.0, 8.0));
            let fill = if resp.is_pointer_button_down_on() { KEY.gamma_multiply(0.7) } else { KEY };
            painter.rect_filled(key, CornerRadius::same(2), fill);
            if flash > 0.0 {
                let tint = if k == 1 { RED } else { AMBER };
                painter.rect_filled(key, CornerRadius::same(2), alpha(tint, (flash * 150.0) as u8));
            }
            painter.text(key.center(), Align2::CENTER_CENTER, *label, FontId::monospace(9.0), BG);
        }
        // Beat LEDs + 16-step page dots in one cell.
        let (rect, _) = chassis(ui, CELL_W * 2.0 + GAP, Sense::hover());
        cell_name(ui, rect, "CLOCK · 120.0");
        let painter = ui.painter();
        for k in 0..4 {
            let led = Pos2::new(rect.min.x + 16.0 + k as f32 * 18.0, rect.min.y + 30.0);
            if k == beat {
                painter.circle_filled(led, 5.0, alpha(RED, 70));
                painter.circle_filled(led, 3.0 + pulse, RED);
            } else {
                painter.circle_filled(led, 2.5, Color32::from_rgb(58, 34, 32));
            }
        }
        let pos = util::phrase(time);
        for k in 0..16 {
            let x = rect.min.x + 12.0 + (rect.width() - 24.0) * k as f32 / 15.0;
            let dot = Pos2::new(x, rect.max.y - 12.0);
            if k == pos {
                painter.circle_filled(dot, 2.4, AMBER);
            } else {
                painter.circle_filled(dot, 1.4, if k < pos { alpha(AMBER, 120) } else { DIM });
            }
        }
        // Segmented controls as position-dot cells.
        for (name, labels, sel) in [("SYNC", &["INT", "LINK"][..], 0usize), ("NEXT", &["1", "2", "4", "8"][..], 2)] {
            let (rect, _) = chassis(ui, CELL_W, Sense::click_and_drag());
            cell_name(ui, rect, name);
            let painter = ui.painter();
            painter.text(
                Pos2::new(rect.min.x + 8.0, rect.max.y - 22.0),
                Align2::LEFT_CENTER,
                labels[sel],
                FontId::monospace(15.0),
                AMBER,
            );
            for k in 0..labels.len() {
                let x = rect.min.x + 10.0 + (rect.width() - 20.0) * k as f32 / (labels.len() - 1) as f32;
                let dot = Pos2::new(x, rect.max.y - 6.0);
                if k == sel {
                    painter.circle_filled(dot, 2.0, AMBER);
                } else {
                    painter.circle_filled(dot, 1.2, DIM);
                }
            }
        }
        // Status cell: chips as LCD tags.
        let (rect, _) = chassis(ui, CELL_W, Sense::hover());
        cell_name(ui, rect, "STATUS");
        let painter = ui.painter();
        painter.text(rect.min + vec2(8.0, 26.0), Align2::LEFT_TOP, "2 PEERS", FontId::monospace(10.0), GREEN);
        painter.text(rect.min + vec2(8.0, 42.0), Align2::LEFT_TOP, "AUDIO!", FontId::monospace(10.0), RED);
        // Clip pool: sample slots with a mini art strip.
        for (k, clip) in mock_clips().iter().enumerate() {
            let (rect, resp) = chassis(ui, CELL_W * 2.0 + GAP, Sense::click());
            let painter = ui.painter();
            let short = clip.name.split('.').next().unwrap_or(clip.name).to_uppercase();
            painter.text(
                Pos2::new(rect.min.x + 8.0, rect.min.y + 14.0),
                Align2::LEFT_CENTER,
                format!("{:02}", k + 1),
                FontId::monospace(12.0),
                DIM,
            );
            painter.text(
                Pos2::new(rect.min.x + 32.0, rect.min.y + 14.0),
                Align2::LEFT_CENTER,
                short,
                FontId::monospace(11.0),
                if clip.selected { AMBER } else { TEXT },
            );
            // Art strip: seeded amber waveform blocks on the LCD.
            let well = Rect::from_min_max(rect.min + vec2(8.0, 26.0), rect.max - vec2(8.0, 8.0));
            painter.rect_filled(well, CornerRadius::same(2), BG);
            for b in 0..24 {
                let h = util::hash01(clip.seed, b) * (well.height() - 6.0);
                let x = well.min.x + 3.0 + b as f32 / 24.0 * (well.width() - 6.0);
                let bar = Rect::from_min_size(Pos2::new(x, well.max.y - 3.0 - h), vec2(4.0, h));
                painter.rect_filled(bar, CornerRadius::ZERO, alpha(AMBER, 60 + (util::hash01(clip.seed, b + 40) * 120.0) as u8));
            }
            match clip.role {
                MockRole::Playing => {
                    painter.text(Pos2::new(rect.max.x - 8.0, rect.min.y + 14.0), Align2::RIGHT_CENTER, "PLAY", FontId::monospace(9.0), GREEN);
                    painter.rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.0, alpha(GREEN, (pulse * 200.0) as u8)), StrokeKind::Inside);
                }
                MockRole::Armed => {
                    painter.text(Pos2::new(rect.max.x - 8.0, rect.min.y + 14.0), Align2::RIGHT_CENTER, "ARM", FontId::monospace(9.0), AMBER);
                }
                MockRole::None => {}
            }
            if clip.selected {
                painter.rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.5, AMBER), StrokeKind::Inside);
            } else if resp.hovered() {
                painter.rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.0, KEY), StrokeKind::Inside);
            }
        }
        meter_cell(ui, "LEVELS", time);
        fft_cell(ui, "SPECTRUM", time);
    });
    ui.add_space(10.0);
    util::footer(
        ui,
        DIM,
        Color32::from_rgb(170, 170, 170),
        &[("WIDGETS", "Everything is a cell: taps are trig keys, beat/phrase are LED chains, segmented is a value + position dots, chips are LCD tags, clip tiles are Digitakt sample slots with a waveform strip.")],
    );
}

/// One control as one-or-more grid cells; returns each cell rect so the bind
/// tag can attach per cell.
fn control(
    ui: &mut Ui,
    i: usize,
    label: &str,
    name: &str,
    kind: &MockKind,
    val: &mut Value,
    time: f64,
) -> Vec<Rect> {
    match (kind, val) {
        (MockKind::Float { min, max, .. }, Value::Float(v)) => {
            let bipolar = *min < 0.0 && *max > 0.0;
            let mut t = (*v - min) / (max - min);
            let rect = enc_cell(ui, i, 0, label, &format!("{v:.2}"), Some((&mut t, bipolar)));
            *v = min + t * (max - min);
            vec![rect]
        }
        (MockKind::Bool { .. }, Value::Bool(b)) => vec![led_cell(ui, label, b)],
        (MockKind::Long { labels, .. }, Value::LongIdx(sel)) => {
            vec![long_cell(ui, i, label, labels, sel)]
        }
        (MockKind::Event, Value::EventAt(at)) => vec![trig_cell(ui, label, at, time)],
        (MockKind::Point2D { min, max, .. }, Value::Point(p)) => {
            let mut out = Vec::new();
            for (axis, (lo, hi)) in min.iter().zip(max.iter()).enumerate() {
                let mut t = (p[axis] - lo) / (hi - lo);
                let axis_label = if axis == 0 { "X" } else { "Y" };
                let rect = enc_cell(
                    ui,
                    i,
                    axis + 1,
                    &format!("{label} {axis_label}"),
                    &format!("{:.2}", p[axis]),
                    Some((&mut t, false)),
                );
                p[axis] = lo + t * (hi - lo);
                out.push(rect);
            }
            out.push(xy_cell(ui, min, max, p));
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
                out.push(enc_cell(
                    ui,
                    i,
                    k + 1,
                    ch_label,
                    &format!("{:>3}", (shown * 127.0) as u8),
                    Some((ch, false)),
                ));
            }
            out.push(swatch_cell(ui, label, *c));
            out
        }
        (MockKind::Image, _) => vec![slot_cell(ui, label, name)],
        (MockKind::Audio, _) => vec![meter_cell(ui, label, time)],
        (MockKind::AudioFft, _) => vec![fft_cell(ui, label, time)],
        _ => vec![],
    }
}

/// Allocate and paint an empty cell chassis.
fn chassis(ui: &mut Ui, w: f32, sense: Sense) -> (Rect, egui::Response) {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, CELL_H), sense);
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(3), CELL);
    painter.rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.0, EDGE), StrokeKind::Inside);
    (rect, resp)
}

fn cell_name(ui: &Ui, rect: Rect, name: &str) {
    ui.painter().text(
        rect.min + vec2(6.0, 6.0),
        Align2::LEFT_TOP,
        name.to_uppercase(),
        FontId::monospace(8.0),
        DIM,
    );
}

/// The core Elektron widget: an encoder cell. `t` is the normalized value;
/// `bipolar` sweeps the lit arc from center instead of from the left stop.
fn enc_cell(
    ui: &mut Ui,
    i: usize,
    sub: usize,
    name: &str,
    readout: &str,
    t: Option<(&mut f32, bool)>,
) -> Rect {
    let (rect, resp) = chassis(ui, CELL_W, Sense::click_and_drag());
    cell_name(ui, rect, name);
    let center = Pos2::new(rect.min.x + 22.0, rect.max.y - 22.0);
    if let Some((t, bipolar)) = t {
        util::vdrag(&resp, t, 0.0, 1.0, 140.0);
        if resp.hovered() {
            ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeVertical);
        }
        let painter = ui.painter();
        painter.add(egui::Shape::line(util::knob_arc(center, 13.0, 0.0, 1.0), Stroke::new(2.0, EDGE)));
        let (a0, a1) = if bipolar { (0.5, *t) } else { (0.0, *t) };
        painter.add(egui::Shape::line(util::knob_arc(center, 13.0, a0, a1), Stroke::new(2.5, AMBER)));
        if bipolar {
            let top = center + util::knob_dir(0.5) * 16.0;
            painter.line_segment([center + util::knob_dir(0.5) * 11.0, top], Stroke::new(1.0, DIM));
        }
        painter.circle_filled(center, 6.5, Color32::from_rgb(32, 32, 32));
        painter.circle_stroke(center, 6.5, Stroke::new(1.0, EDGE));
        let tip = center + util::knob_dir(*t) * 5.0;
        painter.line_segment([center, tip], Stroke::new(1.5, TEXT));
        painter.text(
            Pos2::new(rect.max.x - 7.0, rect.max.y - 22.0),
            Align2::RIGHT_CENTER,
            readout,
            FontId::monospace(13.0),
            TEXT,
        );
    }
    let _ = (i, sub);
    rect
}

fn led_cell(ui: &mut Ui, name: &str, b: &mut bool) -> Rect {
    let (rect, resp) = chassis(ui, CELL_W, Sense::click());
    if resp.clicked() {
        *b = !*b;
    }
    cell_name(ui, rect, name);
    let painter = ui.painter();
    let led = Pos2::new(rect.min.x + 22.0, rect.max.y - 22.0);
    if *b {
        painter.circle_filled(led, 6.0, alpha(GREEN, 60));
        painter.circle_filled(led, 3.5, GREEN);
    } else {
        painter.circle_filled(led, 3.5, Color32::from_rgb(40, 48, 40));
        painter.circle_stroke(led, 3.5, Stroke::new(1.0, EDGE));
    }
    painter.text(
        Pos2::new(rect.max.x - 7.0, rect.max.y - 22.0),
        Align2::RIGHT_CENTER,
        if *b { "ON" } else { "OFF" },
        FontId::monospace(13.0),
        if *b { TEXT } else { DIM },
    );
    rect
}

fn long_cell(ui: &mut Ui, i: usize, name: &str, labels: &[&str], sel: &mut usize) -> Rect {
    let (rect, resp) = chassis(ui, CELL_W, Sense::click_and_drag());
    let steps = util::vdrag_steps(ui, ui.id().with(("long", i)), &resp, 14.0);
    let n = labels.len() as i32;
    let mut s = *sel as i32 + steps;
    if resp.clicked() {
        s += 1;
    }
    *sel = s.rem_euclid(n) as usize;
    cell_name(ui, rect, name);
    let painter = ui.painter();
    painter.text(
        Pos2::new(rect.min.x + 8.0, rect.max.y - 22.0),
        Align2::LEFT_CENTER,
        labels[*sel],
        FontId::monospace(15.0),
        AMBER,
    );
    // Position dots along the bottom, one per VALUES entry.
    let n = labels.len();
    for k in 0..n {
        let x = rect.min.x + 10.0 + (rect.width() - 20.0) * k as f32 / (n - 1) as f32;
        let dot = Pos2::new(x, rect.max.y - 6.0);
        if k == *sel {
            painter.circle_filled(dot, 2.0, AMBER);
        } else {
            painter.circle_filled(dot, 1.2, DIM);
        }
    }
    painter.text(
        Pos2::new(rect.max.x - 6.0, rect.center().y),
        Align2::RIGHT_CENTER,
        "↕",
        FontId::proportional(10.0),
        DIM,
    );
    rect
}

fn trig_cell(ui: &mut Ui, name: &str, at: &mut f64, time: f64) -> Rect {
    let (rect, resp) = chassis(ui, CELL_W, Sense::click());
    if resp.clicked() {
        *at = time;
    }
    cell_name(ui, rect, name);
    let flash = util::event_flash(time, *at);
    let painter = ui.painter();
    // A gray trig key with a red LED above it.
    let key = Rect::from_min_max(rect.min + vec2(8.0, 26.0), rect.max - vec2(8.0, 8.0));
    let fill = if resp.is_pointer_button_down_on() { KEY.gamma_multiply(0.7) } else { KEY };
    painter.rect_filled(key, CornerRadius::same(2), fill);
    if flash > 0.0 {
        painter.rect_filled(key, CornerRadius::same(2), alpha(RED, (flash * 150.0) as u8));
    }
    painter.text(key.center(), Align2::CENTER_CENTER, "TRIG", FontId::monospace(9.0), BG);
    let led = Pos2::new(rect.max.x - 12.0, rect.min.y + 10.0);
    if flash > 0.0 {
        painter.circle_filled(led, 4.0, alpha(RED, 70));
        painter.circle_filled(led, 2.2, RED);
    } else {
        painter.circle_filled(led, 2.2, Color32::from_rgb(58, 34, 32));
    }
    rect
}

fn xy_cell(ui: &mut Ui, min: &[f32; 2], max: &[f32; 2], p: &mut [f32; 2]) -> Rect {
    let (rect, resp) = chassis(ui, CELL_W, Sense::click_and_drag());
    cell_name(ui, rect, "XY");
    let inset = Rect::from_min_max(rect.min + vec2(30.0, 14.0), rect.max - vec2(8.0, 8.0));
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let tx = ((pos.x - inset.min.x) / inset.width()).clamp(0.0, 1.0);
            let ty = 1.0 - ((pos.y - inset.min.y) / inset.height()).clamp(0.0, 1.0);
            p[0] = min[0] + tx * (max[0] - min[0]);
            p[1] = min[1] + ty * (max[1] - min[1]);
        }
    }
    let painter = ui.painter();
    painter.rect_filled(inset, CornerRadius::same(2), BG);
    painter.rect_stroke(inset, CornerRadius::same(2), Stroke::new(1.0, EDGE), StrokeKind::Inside);
    let tx = (p[0] - min[0]) / (max[0] - min[0]);
    let ty = 1.0 - (p[1] - min[1]) / (max[1] - min[1]);
    let dot = Pos2::new(
        inset.min.x + inset.width() * tx,
        inset.min.y + inset.height() * ty,
    );
    painter.circle_filled(dot, 2.5, AMBER);
    rect
}

fn swatch_cell(ui: &mut Ui, name: &str, c: Hsva) -> Rect {
    let (rect, _) = chassis(ui, CELL_W, Sense::hover());
    cell_name(ui, rect, name);
    let window = Rect::from_min_max(rect.min + vec2(8.0, 18.0), rect.max - vec2(8.0, 8.0));
    let painter = ui.painter();
    painter.rect_filled(window, CornerRadius::same(2), Color32::from(c));
    painter.rect_stroke(window, CornerRadius::same(2), Stroke::new(1.0, EDGE), StrokeKind::Inside);
    rect
}

fn slot_cell(ui: &mut Ui, name: &str, uniform: &str) -> Rect {
    let (rect, _) = chassis(ui, CELL_W * 2.0 + GAP, Sense::click());
    cell_name(ui, rect, name);
    let painter = ui.painter();
    // Sample-slot readout, Digitakt style: slot number + name on the LCD.
    painter.text(
        Pos2::new(rect.min.x + 8.0, rect.max.y - 22.0),
        Align2::LEFT_CENTER,
        "01",
        FontId::monospace(15.0),
        DIM,
    );
    painter.text(
        Pos2::new(rect.min.x + 32.0, rect.max.y - 22.0),
        Align2::LEFT_CENTER,
        uniform.to_uppercase(),
        FontId::monospace(13.0),
        TEXT,
    );
    painter.text(
        Pos2::new(rect.max.x - 8.0, rect.max.y - 22.0),
        Align2::RIGHT_CENTER,
        "SRC▸",
        FontId::monospace(9.0),
        AMBER,
    );
    rect
}

fn meter_cell(ui: &mut Ui, name: &str, time: f64) -> Rect {
    let (rect, _) = chassis(ui, CELL_W * 2.0 + GAP, Sense::hover());
    cell_name(ui, rect, name);
    let painter = ui.painter();
    let well = Rect::from_min_max(rect.min + vec2(8.0, 20.0), rect.max - vec2(8.0, 8.0));
    painter.rect_filled(well, CornerRadius::same(2), BG);
    // Segmented LED ladder per channel, green into red.
    for ch in 0..2 {
        let lvl = util::level(time, ch as f32);
        let y = well.min.y + 6.0 + ch as f32 * 16.0;
        let n = 24;
        for k in 0..n {
            let t = k as f32 / n as f32;
            let on = t < lvl;
            let color = if t > 0.85 { RED } else { GREEN };
            let x = well.min.x + 4.0 + t * (well.width() - 8.0);
            let seg = Rect::from_min_size(Pos2::new(x, y), vec2(4.0, 8.0));
            painter.rect_filled(seg, CornerRadius::ZERO, if on { color } else { alpha(color, 26) });
        }
    }
    rect
}

fn fft_cell(ui: &mut Ui, name: &str, time: f64) -> Rect {
    let (rect, _) = chassis(ui, CELL_W * 2.0 + GAP, Sense::hover());
    cell_name(ui, rect, name);
    let painter = ui.painter();
    let well = Rect::from_min_max(rect.min + vec2(8.0, 18.0), rect.max - vec2(8.0, 8.0));
    painter.rect_filled(well, CornerRadius::same(2), BG);
    let n = 32;
    let bw = (well.width() - 4.0) / n as f32;
    for k in 0..n {
        let mag = util::fft(time, k, n);
        let h = (well.height() - 4.0) * mag;
        let bar = Rect::from_min_max(
            Pos2::new(well.min.x + 2.0 + k as f32 * bw, well.max.y - 2.0 - h),
            Pos2::new(well.min.x + 2.0 + (k as f32 + 0.75) * bw, well.max.y - 2.0),
        );
        painter.rect_filled(bar, CornerRadius::ZERO, alpha(AMBER, 70 + (mag * 185.0) as u8));
    }
    rect
}

/// Elektron's bind affordance: a tiny amber controller number in the cell's
/// top-right corner; learn mode pulses the cell border.
fn bind_tag(ui: &mut Ui, midi: &mut MidiState, idx: usize, kind: &MockKind, rect: Rect, time: f64) {
    if !MidiState::bindable(kind) {
        return;
    }
    if let Some(b) = midi.binding(idx) {
        let act = midi.activity(idx, time);
        let color = if act > 0.0 { Color32::from_rgb(255, 220, 170) } else { AMBER };
        ui.painter().text(
            Pos2::new(rect.max.x - 5.0, rect.min.y + 5.0),
            Align2::RIGHT_TOP,
            b.label(),
            FontId::monospace(8.0),
            color,
        );
        if act > 0.0 {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(3),
                Stroke::new(1.0, alpha(AMBER, (act * 200.0) as u8)),
                StrokeKind::Inside,
            );
        }
    }
    if midi.armed {
        let resp = ui.interact(rect, ui.id().with(("learn", idx, rect.min.x as i32)), Sense::click());
        let pulse = 0.5 + 0.5 * ((time * 6.0).sin() as f32);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(3),
            Stroke::new(1.5, alpha(AMBER, 90 + (pulse * 150.0) as u8)),
            StrokeKind::Inside,
        );
        if resp.clicked() {
            midi.learn_click(idx, kind);
        }
    }
}
