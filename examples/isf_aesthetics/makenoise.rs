//! Direction 3 — **Make Noise** (Maths/Morphagene): black-ink illustration on
//! a paper panel. Small knobs with printed arc scales, signal-flow lines
//! from patch points into the param cluster, iconographic bool/event
//! controls, and the color swatch as the panel's only pigment. Deviations:
//! an INK/FILM inversion toggle (the white panel can fight the app's dark
//! chrome), and MIDI binds render as the most literal affordance here — an
//! empty patch point that fills when patched.

use egui::ecolor::Hsva;
use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, vec2};

use crate::schema::{DemoState, MidiState, MockKind, MockRole, Value, mock_clips};
use crate::util;

/// Paper/ink pair, swappable by the FILM toggle.
#[derive(Clone, Copy)]
struct Pal {
    paper: Color32,
    ink: Color32,
    dim: Color32,
}

fn palette(film: bool) -> Pal {
    if film {
        Pal {
            paper: Color32::from_rgb(20, 18, 24),
            ink: Color32::from_rgb(235, 230, 218),
            dim: Color32::from_rgb(128, 124, 116),
        }
    } else {
        Pal {
            paper: Color32::from_rgb(236, 231, 219),
            ink: Color32::from_rgb(25, 22, 33),
            dim: Color32::from_rgb(130, 126, 118),
        }
    }
}

fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

const BLOCK_H: f32 = 96.0;

/// Render the whole direction panel.
pub fn show(ui: &mut Ui, st: &mut DemoState) {
    let pal = palette(st.ink_film);
    let panel = ui.available_rect_before_wrap();
    ui.painter().rect_filled(panel.expand(8.0), CornerRadius::same(6), pal.paper);
    ui.painter().rect_stroke(
        panel.expand(4.0),
        CornerRadius::same(4),
        Stroke::new(1.5, pal.ink),
        StrokeKind::Inside,
    );

    // Title row + the INK/FILM inversion toggle.
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("KALEIDO BLOOM").monospace().size(12.0).color(pal.ink));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(72.0, 16.0), Sense::click());
            if resp.clicked() {
                st.ink_film = !st.ink_film;
            }
            let painter = ui.painter();
            let label = if st.ink_film { "▣ FILM" } else { "▢ INK" };
            painter.text(rect.center(), Align2::CENTER_CENTER, label, FontId::monospace(9.0), pal.dim);
        });
    });
    ui.add_space(8.0);

    let time = st.time;
    let armed = st.midi.armed;
    let DemoState { inputs, values, midi, .. } = st;

    // Reserve a shape slot so the flow lines paint *under* everything drawn
    // after this point.
    let flow_slot = ui.painter().add(Shape::Noop);
    let mut jack_taps: Vec<Pos2> = Vec::new();
    let mut cluster: Option<Rect> = None;

    ui.horizontal(|ui| {
        ui.add_space(6.0);
        // Left rail: the stream inputs as patch points with printed scopes.
        ui.vertical(|ui| {
            ui.set_width(104.0);
            for (inp, _) in inputs.iter().zip(values.iter()).filter(|(inp, _)| !MidiState::bindable(&inp.kind)) {
                let tap = match inp.kind {
                    MockKind::Image => jack(ui, pal, inp.label, None),
                    MockKind::Audio => jack(ui, pal, inp.label, Some(ScopeKind::Wave { time })),
                    MockKind::AudioFft => jack(ui, pal, inp.label, Some(ScopeKind::Fft { time })),
                    _ => continue,
                };
                jack_taps.push(tap);
                ui.add_space(10.0);
            }
        });
        ui.add_space(16.0);
        // Param cluster: everything bindable, as wrapped ink blocks.
        ui.vertical(|ui| {
            let top_left = ui.next_widget_position();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
                for (i, (inp, val)) in inputs.iter().zip(values.iter_mut()).enumerate() {
                    if !MidiState::bindable(&inp.kind) {
                        continue;
                    }
                    let frozen = *val;
                    let rects = control(ui, pal, i, inp.label, &inp.kind, val, time);
                    if armed {
                        *val = frozen;
                    }
                    for (k, rect) in rects.iter().enumerate() {
                        cluster = Some(cluster.map_or(*rect, |c| c.union(*rect)));
                        bind_point(ui, pal, midi, i, &inp.kind, *rect, time, k == rects.len() - 1);
                    }
                }
            });
            let _ = top_left;
        });
    });

    // Flow graphic: each jack tap runs to a bus line, the bus arrows into
    // the cluster — the panel says "these streams feed those params".
    if let (Some(cluster), false) = (cluster, jack_taps.is_empty()) {
        let bus_x = jack_taps.iter().map(|p| p.x).fold(f32::MIN, f32::max) + 22.0;
        let mut shapes = Vec::new();
        let stroke = Stroke::new(1.2, pal.dim);
        let top = jack_taps.iter().map(|p| p.y).fold(f32::MAX, f32::min);
        let bottom = jack_taps.iter().map(|p| p.y).fold(f32::MIN, f32::max);
        for tap in &jack_taps {
            shapes.push(Shape::line_segment([*tap, Pos2::new(bus_x, tap.y)], stroke));
        }
        shapes.push(Shape::line_segment([Pos2::new(bus_x, top), Pos2::new(bus_x, bottom)], stroke));
        let entry = Pos2::new(cluster.min.x - 6.0, cluster.min.y + 18.0);
        shapes.push(Shape::line_segment([Pos2::new(bus_x, top), Pos2::new(bus_x, entry.y)], stroke));
        shapes.push(Shape::line_segment([Pos2::new(bus_x, entry.y), entry], stroke));
        for da in [-0.5, 0.5_f32] {
            let dir = egui::Vec2::angled(std::f32::consts::PI + da) * 6.0;
            shapes.push(Shape::line_segment([entry, entry + dir], stroke));
        }
        ui.painter().set(flow_slot, Shape::Vec(shapes));
    }

    ui.add_space(10.0);
    util::footer(
        ui,
        pal.dim,
        pal.ink,
        &[
            ("DENSITY", "Mid: ~72×96 px per scalar, but the flow rail spends a column on representation, not control."),
            ("EPAINT COST", "Mid — arcs, radial glyphs, and flow lines are all polylines; the look depends on stroke discipline, not assets."),
            ("MIDI", "The patch-point circle is the most literal bind affordance of the four; it reads instantly but costs a corner of every block."),
            ("FIT", "INK fights the dark chrome (hence FILM). Best at making a shader feel like a designed module; most opinionated of the four."),
        ],
    );
}

/// The app's widget vocabulary in ink: burst circles for taps, printed
/// beat/phrase marks, a lever and detent for sync/cadence, circled text for
/// chips, crosshatch-art frames for clip tiles. Interactive state parks in
/// egui temp memory.
pub fn show_widgets(ui: &mut Ui, st: &mut DemoState) {
    let pal = palette(st.ink_film);
    let panel = ui.available_rect_before_wrap();
    ui.painter().rect_filled(panel.expand(8.0), CornerRadius::same(6), pal.paper);
    ui.painter().rect_stroke(panel.expand(4.0), CornerRadius::same(4), Stroke::new(1.5, pal.ink), StrokeKind::Inside);
    let time = st.time;
    let (beat, pulse) = util::beat(time);

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("VIDIOTIC · WIDGETS").monospace().size(12.0).color(pal.ink));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(72.0, 16.0), Sense::click());
            if resp.clicked() {
                st.ink_film = !st.ink_film;
            }
            let label = if st.ink_film { "▣ FILM" } else { "▢ INK" };
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, FontId::monospace(9.0), pal.dim);
        });
    });
    ui.add_space(8.0);

    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
        for label in ["downbeat", "reset", "tap"] {
            let id = ui.id().with(("wburst", label));
            let mut at: f64 = ui.ctx().data_mut(|d| d.get_temp(id).unwrap_or(-1.0));
            burst(ui, pal, label, &mut at, time);
            ui.ctx().data_mut(|d| d.insert_temp(id, at));
        }
        // Clock block: four printed circles + a tick timeline with an arrow.
        let (rect, _) = ui.allocate_exact_size(vec2(150.0, BLOCK_H), Sense::hover());
        let painter = ui.painter();
        for k in 0..4 {
            let c = Pos2::new(rect.min.x + 20.0 + k as f32 * 24.0, rect.min.y + 24.0);
            painter.circle_stroke(c, 6.0, Stroke::new(1.5, pal.ink));
            if k == beat {
                painter.circle_filled(c, 3.5 + pulse * 1.5, pal.ink);
            }
        }
        let y = rect.min.y + 50.0;
        painter.line_segment([Pos2::new(rect.min.x + 8.0, y), Pos2::new(rect.max.x - 8.0, y)], Stroke::new(1.5, pal.ink));
        let pos = util::phrase(time);
        for k in 0..16 {
            let x = rect.min.x + 8.0 + (rect.width() - 16.0) * k as f32 / 15.0;
            let major = k % 4 == 0;
            painter.line_segment([Pos2::new(x, y), Pos2::new(x, y - if major { 7.0 } else { 4.0 })], Stroke::new(1.2, pal.ink));
            if k == pos {
                // Printed arrow head under the current step.
                painter.add(Shape::convex_polygon(
                    vec![Pos2::new(x - 4.0, y + 10.0), Pos2::new(x + 4.0, y + 10.0), Pos2::new(x, y + 3.0)],
                    pal.ink,
                    Stroke::NONE,
                ));
            }
        }
        label_under(ui, pal, rect, "clock", "120");

        let link_id = ui.id().with("wlink");
        let mut link: bool = ui.ctx().data_mut(|d| d.get_temp(link_id).unwrap_or(false));
        toggle(ui, pal, "link", &mut link);
        ui.ctx().data_mut(|d| d.insert_temp(link_id, link));

        let next_id = ui.id().with("wnext");
        let mut next: usize = ui.ctx().data_mut(|d| d.get_temp(next_id).unwrap_or(2));
        detent(ui, pal, 91, "next every", &["1", "2", "4", "8"], &mut next);
        ui.ctx().data_mut(|d| d.insert_temp(next_id, next));

        // Chips: circled ink text.
        let (rect, _) = ui.allocate_exact_size(vec2(96.0, BLOCK_H), Sense::hover());
        let painter = ui.painter();
        for (k, text) in ["( 2 peers )", "( audio ! )"].iter().enumerate() {
            let plate = Rect::from_center_size(
                Pos2::new(rect.center().x, rect.min.y + 22.0 + k as f32 * 26.0),
                vec2(84.0, 18.0),
            );
            painter.rect_stroke(plate, CornerRadius::same(9), Stroke::new(1.2, pal.ink), StrokeKind::Inside);
            painter.text(plate.center(), Align2::CENTER_CENTER, *text, FontId::monospace(9.0), pal.ink);
        }
        label_under(ui, pal, rect, "status", "");

        // Clip pool: ink frames with crosshatch art.
        for clip in mock_clips() {
            let (rect, resp) = ui.allocate_exact_size(vec2(104.0, BLOCK_H), Sense::click());
            let painter = ui.painter();
            let frame = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, rect.min.y + 4.0), vec2(96.0, 56.0));
            painter.rect_stroke(frame, CornerRadius::ZERO, Stroke::new(1.5, pal.ink), StrokeKind::Inside);
            if clip.selected {
                painter.rect_stroke(frame.expand(3.0), CornerRadius::ZERO, Stroke::new(1.0, pal.ink), StrokeKind::Inside);
            }
            // Crosshatch density seeded per clip: ink's grayscale.
            let inner = frame.shrink(3.0);
            let n = 8 + (util::hash01(clip.seed, 0) * 8.0) as usize;
            for k in 0..n {
                let t = k as f32 / n as f32;
                let x0 = inner.min.x + inner.width() * t;
                painter.line_segment(
                    [Pos2::new(x0, inner.max.y), Pos2::new((x0 + inner.height() * 0.6).min(inner.max.x), inner.min.y)],
                    Stroke::new(1.0, alpha(pal.ink, 90 + (util::hash01(clip.seed, k + 3) * 100.0) as u8)),
                );
            }
            match clip.role {
                MockRole::Playing => {
                    let c = Pos2::new(frame.min.x + 10.0, frame.min.y + 10.0);
                    painter.circle_filled(c, 4.0, pal.ink);
                    painter.circle_stroke(c, 6.0 + pulse * 2.0, Stroke::new(1.0, alpha(pal.ink, (pulse * 200.0) as u8)));
                }
                MockRole::Armed => {
                    painter.circle_stroke(Pos2::new(frame.min.x + 10.0, frame.min.y + 10.0), 4.0, Stroke::new(1.5, pal.ink));
                }
                MockRole::None => {}
            }
            if resp.hovered() {
                painter.rect_stroke(frame.expand(1.0), CornerRadius::ZERO, Stroke::new(1.0, pal.dim), StrokeKind::Inside);
            }
            let short = clip.name.split('.').next().unwrap_or(clip.name);
            label_under(ui, pal, rect, short, "");
        }

        // Level + spectrum as woodcut bars.
        for (name, is_fft) in [("level", false), ("spectrum", true)] {
            let (rect, _) = ui.allocate_exact_size(vec2(150.0, BLOCK_H), Sense::hover());
            let painter = ui.painter();
            let well = Rect::from_min_size(Pos2::new(rect.min.x + 6.0, rect.min.y + 8.0), vec2(138.0, 52.0));
            painter.rect_stroke(well, CornerRadius::ZERO, Stroke::new(1.5, pal.ink), StrokeKind::Inside);
            let inner = well.shrink(4.0);
            if is_fft {
                let n = 20;
                for k in 0..n {
                    let mag = util::fft(time, k, n);
                    let h = inner.height() * mag;
                    let x = inner.min.x + inner.width() * k as f32 / n as f32;
                    let bar = Rect::from_min_max(Pos2::new(x, inner.max.y - h), Pos2::new(x + inner.width() / n as f32 * 0.65, inner.max.y));
                    painter.rect_filled(bar, CornerRadius::ZERO, pal.ink);
                }
            } else {
                for ch in 0..2 {
                    let lvl = util::level(time, ch as f32);
                    let y = inner.min.y + 12.0 + ch as f32 * 20.0;
                    painter.line_segment([Pos2::new(inner.min.x, y), Pos2::new(inner.max.x, y)], Stroke::new(1.0, pal.dim));
                    painter.line_segment([Pos2::new(inner.min.x, y), Pos2::new(inner.min.x + inner.width() * lvl, y)], Stroke::new(5.0, pal.ink));
                }
            }
            label_under(ui, pal, rect, name, "");
        }
    });
    ui.add_space(10.0);
    util::footer(
        ui,
        pal.dim,
        pal.ink,
        &[("WIDGETS", "Taps are burst circles, the phrase is a printed timeline with an arrow cursor, chips are circled text, clip tiles are ink frames whose crosshatch density stands in for a thumbnail — grayscale as line frequency.")],
    );
}

enum ScopeKind {
    Wave { time: f64 },
    Fft { time: f64 },
}

/// A patch point in the rail: double-circle jack, label, optional printed
/// scope beneath. Returns the tap position flow lines leave from.
fn jack(ui: &mut Ui, pal: Pal, label: &str, scope: Option<ScopeKind>) -> Pos2 {
    let h = if scope.is_some() { 74.0 } else { 44.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(100.0, h), Sense::hover());
    let center = Pos2::new(rect.min.x + 16.0, rect.min.y + 16.0);
    let painter = ui.painter();
    painter.circle_stroke(center, 9.0, Stroke::new(1.6, pal.ink));
    painter.circle_filled(center, 4.5, pal.ink);
    painter.text(
        Pos2::new(center.x + 16.0, center.y),
        Align2::LEFT_CENTER,
        label.to_uppercase(),
        FontId::monospace(9.0),
        pal.ink,
    );
    if let Some(kind) = scope {
        let frame = Rect::from_min_size(Pos2::new(rect.min.x + 6.0, rect.min.y + 34.0), vec2(88.0, 32.0));
        painter.rect_stroke(frame, CornerRadius::ZERO, Stroke::new(1.2, pal.ink), StrokeKind::Inside);
        match kind {
            ScopeKind::Wave { time } => {
                let n = 44;
                let pts: Vec<Pos2> = (0..n)
                    .map(|k| {
                        let x = frame.min.x + 2.0 + (frame.width() - 4.0) * k as f32 / (n - 1) as f32;
                        let y = frame.center().y - util::wave(time, k, n) * (frame.height() * 0.4);
                        Pos2::new(x, y)
                    })
                    .collect();
                painter.add(Shape::line(pts, Stroke::new(1.2, pal.ink)));
            }
            ScopeKind::Fft { time } => {
                let n = 22;
                let bw = (frame.width() - 4.0) / n as f32;
                for k in 0..n {
                    let mag = util::fft(time, k, n);
                    let h = (frame.height() - 4.0) * mag;
                    let bar = Rect::from_min_max(
                        Pos2::new(frame.min.x + 2.0 + k as f32 * bw, frame.max.y - 2.0 - h),
                        Pos2::new(frame.min.x + 2.0 + (k as f32 + 0.65) * bw, frame.max.y - 2.0),
                    );
                    painter.rect_filled(bar, CornerRadius::ZERO, pal.ink);
                }
            }
        }
    }
    Pos2::new(center.x + 9.0, center.y)
}

/// One bindable control as one-or-more ink blocks.
fn control(
    ui: &mut Ui,
    pal: Pal,
    i: usize,
    label: &str,
    kind: &MockKind,
    val: &mut Value,
    time: f64,
) -> Vec<Rect> {
    match (kind, val) {
        (MockKind::Float { min, max, .. }, Value::Float(v)) => {
            let bipolar = *min < 0.0 && *max > 0.0;
            let mut t = (*v - min) / (max - min);
            let rect = knob(ui, pal, i, 0, label, &format!("{v:.2}"), &mut t, bipolar);
            *v = min + t * (max - min);
            vec![rect]
        }
        (MockKind::Bool { .. }, Value::Bool(b)) => vec![toggle(ui, pal, label, b)],
        (MockKind::Long { labels, .. }, Value::LongIdx(sel)) => {
            vec![detent(ui, pal, i, label, labels, sel)]
        }
        (MockKind::Event, Value::EventAt(at)) => vec![burst(ui, pal, label, at, time)],
        (MockKind::Point2D { min, max, .. }, Value::Point(p)) => vec![pad(ui, pal, label, min, max, p)],
        (MockKind::Color { .. }, Value::Color(c)) => color_blocks(ui, pal, i, label, c),
        _ => vec![],
    }
}

fn label_under(ui: &Ui, pal: Pal, rect: Rect, label: &str, value: &str) {
    let painter = ui.painter();
    painter.text(
        Pos2::new(rect.center().x, rect.max.y - 18.0),
        Align2::CENTER_CENTER,
        label.to_uppercase(),
        FontId::monospace(9.0),
        pal.ink,
    );
    if !value.is_empty() {
        painter.text(
            Pos2::new(rect.center().x, rect.max.y - 7.0),
            Align2::CENTER_CENTER,
            value,
            FontId::monospace(8.0),
            pal.dim,
        );
    }
}

/// Ink knob with a printed arc scale; bipolar knobs get a center notch and
/// half-shaded arc (the attenuverter idiom).
#[allow(clippy::too_many_arguments)]
fn knob(ui: &mut Ui, pal: Pal, i: usize, sub: usize, label: &str, value: &str, t: &mut f32, bipolar: bool) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(72.0, BLOCK_H), Sense::click_and_drag());
    util::vdrag(&resp, t, 0.0, 1.0, 150.0);
    if resp.hovered() {
        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::ResizeVertical);
    }
    let center = Pos2::new(rect.center().x, rect.min.y + 30.0);
    let r = 15.0;
    let painter = ui.painter();
    painter.add(Shape::line(util::knob_arc(center, r + 6.0, 0.0, 1.0), Stroke::new(1.2, pal.ink)));
    for tt in [0.0, 0.5, 1.0] {
        let dir = util::knob_dir(tt);
        painter.line_segment([center + dir * (r + 4.0), center + dir * (r + 9.0)], Stroke::new(1.2, pal.ink));
    }
    if bipolar {
        // Shade the negative half of the printed arc.
        painter.add(Shape::line(util::knob_arc(center, r + 6.0, 0.0, 0.5), Stroke::new(3.0, alpha(pal.ink, 70))));
        painter.text(center + util::knob_dir(0.0) * (r + 15.0), Align2::CENTER_CENTER, "−", FontId::monospace(9.0), pal.ink);
        painter.text(center + util::knob_dir(1.0) * (r + 15.0), Align2::CENTER_CENTER, "+", FontId::monospace(9.0), pal.ink);
    }
    painter.circle_filled(center, r, pal.ink);
    let dir = util::knob_dir(*t);
    painter.line_segment([center + dir * 3.0, center + dir * (r - 1.5)], Stroke::new(2.0, pal.paper));
    label_under(ui, pal, rect, label, value);
    let _ = (i, sub);
    rect
}

fn toggle(ui: &mut Ui, pal: Pal, label: &str, b: &mut bool) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(58.0, BLOCK_H), Sense::click());
    if resp.clicked() {
        *b = !*b;
    }
    let center = Pos2::new(rect.center().x, rect.min.y + 30.0);
    let painter = ui.painter();
    // Printed state icons at either end of a lever.
    let on_pos = center + vec2(0.0, -16.0);
    let off_pos = center + vec2(0.0, 16.0);
    painter.circle_filled(on_pos, 4.0, if *b { pal.ink } else { alpha(pal.ink, 60) });
    painter.circle_stroke(off_pos, 4.0, Stroke::new(1.4, if *b { alpha(pal.ink, 60) } else { pal.ink }));
    let tip = if *b { on_pos + vec2(0.0, 6.0) } else { off_pos - vec2(0.0, 6.0) };
    painter.line_segment([center, tip], Stroke::new(3.0, pal.ink));
    painter.circle_filled(center, 4.5, pal.ink);
    label_under(ui, pal, rect, label, if *b { "on" } else { "off" });
    rect
}

fn detent(ui: &mut Ui, pal: Pal, i: usize, label: &str, labels: &[&str], sel: &mut usize) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(84.0, BLOCK_H), Sense::click_and_drag());
    let steps = util::vdrag_steps(ui, ui.id().with(("detent", i)), &resp, 16.0);
    let n = labels.len();
    let mut s = *sel as i32 + steps;
    if resp.clicked() {
        s += 1;
    }
    *sel = s.rem_euclid(n as i32) as usize;
    let center = Pos2::new(rect.center().x, rect.min.y + 30.0);
    let r = 13.0;
    let painter = ui.painter();
    for (k, name) in labels.iter().enumerate() {
        let tt = k as f32 / (n - 1) as f32;
        let dir = util::knob_dir(tt);
        painter.line_segment([center + dir * (r + 3.0), center + dir * (r + 7.0)], Stroke::new(1.2, pal.ink));
        let color = if k == *sel { pal.ink } else { pal.dim };
        painter.text(center + dir * (r + 14.0), Align2::CENTER_CENTER, *name, FontId::monospace(8.0), color);
    }
    painter.circle_filled(center, r, pal.ink);
    let t = *sel as f32 / (n - 1) as f32;
    let dir = util::knob_dir(t);
    painter.line_segment([center, center + dir * (r - 1.5)], Stroke::new(2.0, pal.paper));
    label_under(ui, pal, rect, label, "");
    rect
}

fn burst(ui: &mut Ui, pal: Pal, label: &str, at: &mut f64, time: f64) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(58.0, BLOCK_H), Sense::click());
    if resp.clicked() {
        *at = time;
    }
    let flash = util::event_flash(time, *at);
    let center = Pos2::new(rect.center().x, rect.min.y + 30.0);
    let painter = ui.painter();
    // Burst glyph: a ring of radiating strokes.
    for k in 0..8 {
        let a = k as f32 / 8.0 * std::f32::consts::TAU;
        let dir = egui::Vec2::angled(a);
        painter.line_segment([center + dir * 12.0, center + dir * 17.0], Stroke::new(1.6, pal.ink));
    }
    let fill = if resp.is_pointer_button_down_on() { alpha(pal.ink, 200) } else { pal.ink };
    painter.circle_filled(center, 8.0, fill);
    if flash > 0.0 {
        // Ink-splash: an expanding, fading ring.
        let rr = 10.0 + (1.0 - flash) * 16.0;
        painter.circle_stroke(center, rr, Stroke::new(2.0, alpha(pal.ink, (flash * 220.0) as u8)));
    }
    label_under(ui, pal, rect, label, "");
    rect
}

fn pad(ui: &mut Ui, pal: Pal, label: &str, min: &[f32; 2], max: &[f32; 2], p: &mut [f32; 2]) -> Rect {
    let (rect, resp) = ui.allocate_exact_size(vec2(86.0, BLOCK_H), Sense::click_and_drag());
    let square = Rect::from_center_size(Pos2::new(rect.center().x, rect.min.y + 32.0), vec2(58.0, 58.0));
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let tx = ((pos.x - square.min.x) / square.width()).clamp(0.0, 1.0);
            let ty = 1.0 - ((pos.y - square.min.y) / square.height()).clamp(0.0, 1.0);
            p[0] = min[0] + tx * (max[0] - min[0]);
            p[1] = min[1] + ty * (max[1] - min[1]);
        }
    }
    let painter = ui.painter();
    painter.rect_stroke(square, CornerRadius::ZERO, Stroke::new(1.4, pal.ink), StrokeKind::Inside);
    // Printed corner ticks, plotter style.
    for k in 1..4 {
        let x = square.min.x + square.width() * k as f32 / 4.0;
        let y = square.min.y + square.height() * k as f32 / 4.0;
        painter.line_segment([Pos2::new(x, square.max.y - 3.0), Pos2::new(x, square.max.y)], Stroke::new(1.0, pal.ink));
        painter.line_segment([Pos2::new(square.min.x, y), Pos2::new(square.min.x + 3.0, y)], Stroke::new(1.0, pal.ink));
    }
    let tx = (p[0] - min[0]) / (max[0] - min[0]);
    let ty = 1.0 - (p[1] - min[1]) / (max[1] - min[1]);
    let dot = Pos2::new(square.min.x + square.width() * tx, square.min.y + square.height() * ty);
    painter.line_segment([Pos2::new(dot.x - 6.0, dot.y), Pos2::new(dot.x + 6.0, dot.y)], Stroke::new(1.0, pal.dim));
    painter.line_segment([Pos2::new(dot.x, dot.y - 6.0), Pos2::new(dot.x, dot.y + 6.0)], Stroke::new(1.0, pal.dim));
    painter.circle_filled(dot, 3.5, pal.ink);
    label_under(ui, pal, rect, label, "");
    rect
}

fn color_blocks(ui: &mut Ui, pal: Pal, i: usize, label: &str, c: &mut Hsva) -> Vec<Rect> {
    let mut out = Vec::new();
    for (k, ch_label) in ["HUE", "SAT", "VAL"].iter().enumerate() {
        let ch = match k {
            0 => &mut c.h,
            1 => &mut c.s,
            _ => &mut c.v,
        };
        let shown = *ch;
        out.push(knob(ui, pal, i, k + 1, ch_label, &format!("{shown:.2}"), ch, false));
    }
    // The swatch: the only pigment on the panel.
    let (rect, _) = ui.allocate_exact_size(vec2(64.0, BLOCK_H), Sense::hover());
    let center = Pos2::new(rect.center().x, rect.min.y + 30.0);
    let painter = ui.painter();
    painter.circle_filled(center, 16.0, Color32::from(*c));
    painter.circle_stroke(center, 16.0, Stroke::new(1.6, pal.ink));
    label_under(ui, pal, rect, label, "");
    out.push(rect);
    out
}

/// Make Noise's bind affordance: an empty patch-point circle in the block's
/// corner that fills when bound — MIDI as literal patching.
#[allow(clippy::too_many_arguments)]
fn bind_point(
    ui: &mut Ui,
    pal: Pal,
    midi: &mut MidiState,
    idx: usize,
    kind: &MockKind,
    rect: Rect,
    time: f64,
    show_point: bool,
) {
    if !MidiState::bindable(kind) {
        return;
    }
    if show_point {
        let center = Pos2::new(rect.max.x - 7.0, rect.min.y + 7.0);
        let painter = ui.painter();
        if let Some(b) = midi.binding(idx) {
            let act = midi.activity(idx, time);
            painter.circle_stroke(center, 4.5, Stroke::new(1.4, pal.ink));
            painter.circle_filled(center, 2.5 + act * 1.5, pal.ink);
            painter.text(
                center + vec2(-8.0, 0.0),
                Align2::RIGHT_CENTER,
                b.label(),
                FontId::monospace(8.0),
                pal.dim,
            );
        } else {
            let a = if midi.armed {
                (128.0 + 120.0 * (time * 6.0).sin() as f32) as u8
            } else {
                70
            };
            painter.circle_stroke(center, 4.5, Stroke::new(1.4, alpha(pal.ink, a)));
        }
    }
    if midi.armed {
        let resp = ui.interact(rect, ui.id().with(("learn", idx, rect.min.x as i32)), Sense::click());
        if resp.clicked() {
            midi.learn_click(idx, kind);
        }
    }
}
