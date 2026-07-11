//! Direction 5 — **Hybrid**: the Terminal buffer grows hardware organs.
//! Same character grid and statusline discipline as the Terminal direction,
//! but floats and longs become Moog-style chickenhead knobs (drawn flat,
//! 1-px strokes, mono skirt legends — as a terminal would draw them) and
//! point2D becomes an oscilloscope XY: dot-glyph graticule, phosphor trace
//! with persistence, glowing beam dot. Text keeps every control where text
//! wins (bool, event, image, statusline, bind tags).
//!
//! The palette is Everforest, implemented in HSL: every role is an
//! (h, s, l) anchor run through [`util::hsl`] with a global hue rotation,
//! so the header's hue slider re-tints the whole scheme coherently. Dark
//! and light are separate anchor sets; the scope screen and phosphor stay
//! fixed-dark in both — a CRT is glass, not paper.

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, vec2};

use crate::nf;
use crate::schema::{DemoState, MidiState, MockKind, MockRole, Value, mock_clips};
use crate::util;

fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

fn mono() -> FontId {
    FontId::monospace(12.0)
}

/// The direction's semantic palette, filled from Everforest HSL anchors.
struct Theme {
    bg: Color32,
    /// Scope glass — always dark.
    screen: Color32,
    knob: Color32,
    frame: Color32,
    select: Color32,
    fg: Color32,
    dim: Color32,
    green: Color32,
    yellow: Color32,
    blue: Color32,
    magenta: Color32,
    red: Color32,
    /// Scope beam — always the dark-mode green, regardless of mode.
    phosphor: Color32,
}

/// Everforest (medium) as HSL anchors, rotated by `hue` degrees.
fn theme(dark: bool, hue: f32) -> Theme {
    let h = |base: f32| base + hue;
    let phosphor = util::hsl(h(83.0), 0.34, 0.63);
    let screen = util::hsl(h(202.0), 0.14, 0.14);
    if dark {
        Theme {
            bg: util::hsl(h(206.0), 0.13, 0.20),
            screen,
            knob: util::hsl(h(199.0), 0.13, 0.24),
            frame: util::hsl(h(201.0), 0.11, 0.31),
            select: util::hsl(h(199.0), 0.12, 0.27),
            fg: util::hsl(h(41.0), 0.32, 0.75),
            dim: util::hsl(h(139.0), 0.06, 0.55),
            green: phosphor,
            yellow: util::hsl(h(40.0), 0.56, 0.68),
            blue: util::hsl(h(172.0), 0.31, 0.62),
            magenta: util::hsl(h(332.0), 0.43, 0.72),
            red: util::hsl(h(359.0), 0.68, 0.70),
            phosphor,
        }
    } else {
        Theme {
            bg: util::hsl(h(44.0), 0.87, 0.94),
            screen,
            knob: util::hsl(h(43.0), 0.67, 0.92),
            frame: util::hsl(h(55.0), 0.26, 0.78),
            select: util::hsl(h(43.0), 0.57, 0.89),
            fg: util::hsl(h(202.0), 0.11, 0.40),
            dim: util::hsl(h(111.0), 0.07, 0.60),
            green: util::hsl(h(68.0), 0.99, 0.32),
            yellow: util::hsl(h(43.0), 1.0, 0.44),
            blue: util::hsl(h(201.0), 0.55, 0.50),
            magenta: util::hsl(h(319.0), 0.65, 0.64),
            red: util::hsl(h(1.0), 0.92, 0.65),
            phosphor,
        }
    }
}

struct Grid {
    cw: f32,
    rh: f32,
}

/// Grid rows a control spends (knobs and the scope buy taller rows).
fn rows_of(kind: &MockKind) -> usize {
    match kind {
        MockKind::Float { .. } | MockKind::Audio => 2,
        MockKind::Long { .. } | MockKind::Color { .. } => 3,
        MockKind::Point2D { .. } => 5,
        _ => 1,
    }
}

/// Render the whole direction panel.
pub fn show(ui: &mut Ui, st: &mut DemoState) {
    let cw = ui.painter().layout_no_wrap("─".into(), mono(), Color32::WHITE).size().x;
    let g = Grid { cw, rh: 18.0 };
    ui.add_space(8.0);

    let th = theme(st.dark, st.hue);
    let width = ui.available_width().min(560.0);
    theme_bar(ui, &g, width, st, &th);
    // The buffer title rides the frame's top border; keep its upper half
    // clear of the theme bar.
    ui.add_space(10.0);

    let time = st.time;
    let armed = st.midi.armed;
    let nerd = st.nerd;

    let rows: usize = st.inputs.iter().map(|inp| rows_of(&inp.kind)).sum();
    let height = rows as f32 * g.rh + 30.0;
    let (frame, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(frame, CornerRadius::same(2), th.bg);
    painter.rect_stroke(frame, CornerRadius::same(2), Stroke::new(1.0, th.frame), StrokeKind::Inside);
    // Winbar-style title: break the border behind it so it reads as part
    // of the frame in both modes.
    let title = "─ kaleido-bloom.fs · hw ─";
    let galley = painter.layout_no_wrap(title.into(), mono(), th.dim);
    let title_pos = Pos2::new(frame.min.x + g.cw * 2.0, frame.min.y);
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(title_pos.x - g.cw * 0.5, title_pos.y - galley.size().y * 0.5),
            galley.size() + vec2(g.cw, 0.0),
        ),
        CornerRadius::ZERO,
        th.bg,
    );
    painter.text(title_pos, Align2::LEFT_CENTER, title, mono(), th.dim);

    let mut cursor = frame.min.y + 8.0;
    let DemoState { inputs, values, midi, .. } = st;
    for (i, (inp, val)) in inputs.iter().zip(values.iter_mut()).enumerate() {
        let row_count = rows_of(&inp.kind);
        let row = Rect::from_min_size(
            Pos2::new(frame.min.x + 4.0, cursor),
            vec2(frame.width() - 8.0, row_count as f32 * g.rh),
        );
        cursor += row_count as f32 * g.rh;

        if armed && MidiState::bindable(&inp.kind) {
            let pulse = 0.5 + 0.5 * ((time * 6.0).sin() as f32);
            ui.painter().rect_filled(row, CornerRadius::ZERO, alpha(th.select, 160 + (pulse * 80.0) as u8));
            let resp = ui.interact(row, ui.id().with(("learn", i)), Sense::click());
            if resp.clicked() {
                midi.learn_click(i, &inp.kind);
            }
        }

        let frozen = *val;
        control(ui, &g, &th, row, i, inp.label, inp.name, &inp.kind, val, time, nerd, midi);
        if armed {
            *val = frozen;
        }
        bind_tag(ui, &g, &th, midi, i, &inp.kind, row, time, nerd);
    }

    let status = Rect::from_min_size(
        Pos2::new(frame.min.x + 1.0, frame.max.y - 19.0),
        vec2(frame.width() - 2.0, 18.0),
    );
    let painter = ui.painter();
    if armed {
        painter.rect_filled(status, CornerRadius::ZERO, th.magenta);
        painter.text(
            Pos2::new(status.min.x + g.cw, status.center().y),
            Align2::LEFT_CENTER,
            "-- LEARN --  click a param to bind · click bound to unbind",
            mono(),
            th.bg,
        );
    } else {
        painter.rect_filled(status, CornerRadius::ZERO, th.select);
        let bound = inputs
            .iter()
            .enumerate()
            .filter(|(i, _)| midi.binding(*i).is_some())
            .count();
        let tag = if nerd { nf::check(nf::MIDI) } else { "midi" };
        painter.text(
            Pos2::new(status.min.x + g.cw, status.center().y),
            Align2::LEFT_CENTER,
            format!("NORMAL   kaleido-bloom.fs   11 params · {tag} {bound} bound"),
            mono(),
            th.fg,
        );
    }

    ui.add_space(10.0);
    util::footer(
        ui,
        th.dim,
        ui.visuals().text_color(),
        &[
            ("DENSITY", "Knobs spend 2 grid rows (~36 px) per float and longs 3 — half Terminal's density, spent exactly where rotary/detent semantics pay. Text rows stay 18 px."),
            ("EPAINT COST", "Still light: a chickenhead is one convex polygon + a dozen strokes, the scope ~40 segments of trace. No textures, no gradients."),
            ("MIDI", "Chickenheads are absolute, so soft pickup returns: bound knobs show a ghost tick at the incoming-CC angle. Everything else keeps Terminal's <cc74> tag + statusline learn mode."),
            ("THEME", "Everforest in HSL: 13 role anchors as (h,s,l) tuples, so the hue slider rotates the whole scheme coherently. Dark/light are separate anchor sets; the scope glass + phosphor stay fixed-dark in both."),
            ("FIT", "Reads as one idiom because the rendering rules never change — mono glyphs, 1-px strokes, one palette; only the *forms* are hardware."),
        ],
    );
}

/// The app's widget vocabulary in the hybrid idiom: Terminal's grid and
/// text controls, hardware forms where they pay — jewel-lamp beat dots, a
/// detented selector feel for cadence, scope-glass meters — all under the
/// Everforest HSL theme.
pub fn show_widgets(ui: &mut Ui, st: &mut DemoState) {
    let cw = ui.painter().layout_no_wrap("─".into(), mono(), Color32::WHITE).size().x;
    let g = Grid { cw, rh: 18.0 };
    ui.add_space(8.0);
    let th = theme(st.dark, st.hue);
    let width = ui.available_width().min(560.0);
    theme_bar(ui, &g, width, st, &th);
    ui.add_space(10.0);
    let time = st.time;
    let nerd = st.nerd;
    let (beat, pulse) = util::beat(time);

    let rows = 13.0;
    let height = rows * g.rh + 30.0;
    let (frame, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(frame, CornerRadius::same(2), th.bg);
    painter.rect_stroke(frame, CornerRadius::same(2), Stroke::new(1.0, th.frame), StrokeKind::Inside);
    let title = "─ vidiotic · widgets · hw ─";
    let galley = painter.layout_no_wrap(title.into(), mono(), th.dim);
    let title_pos = Pos2::new(frame.min.x + g.cw * 2.0, frame.min.y);
    painter.rect_filled(
        Rect::from_min_size(
            Pos2::new(title_pos.x - g.cw * 0.5, title_pos.y - galley.size().y * 0.5),
            galley.size() + vec2(g.cw, 0.0),
        ),
        CornerRadius::ZERO,
        th.bg,
    );
    painter.text(title_pos, Align2::LEFT_CENTER, title, mono(), th.dim);
    let body = Rect::from_min_size(Pos2::new(frame.min.x + 4.0, frame.min.y + 8.0), vec2(frame.width() - 8.0, rows * g.rh));

    // Row 0: taps + jewel-lamp beat dots + phrase strip + bpm.
    put(ui, &g, body, 1.0, 0, "transport", th.dim);
    let mut col = 12.0;
    for label in ["▼", "⟲", "tap"] {
        let text = format!("[ {label} ]");
        let r = put(ui, &g, body, col, 0, &text, th.fg);
        let resp = ui.interact(r, ui.id().with(("wtap", label)), Sense::click());
        let flash = util::tap_flash(ui, ui.id().with(("wtapf", label)), resp.clicked(), time);
        if flash > 0.0 {
            ui.painter().rect_filled(r, CornerRadius::ZERO, alpha(th.yellow, 60 + (flash * 195.0) as u8));
            put(ui, &g, body, col, 0, &text, th.bg);
        }
        col += text.chars().count() as f32 + 1.0;
    }
    for k in 0..4 {
        let c = Pos2::new(body.min.x + (col + 1.5 + k as f32 * 2.0) * g.cw, body.min.y + 0.5 * g.rh);
        if k == beat {
            ui.painter().circle_filled(c, 5.0, alpha(th.phosphor, 60));
            ui.painter().circle_filled(c, 3.0 + pulse, th.phosphor);
        } else {
            ui.painter().circle_filled(c, 2.5, th.select);
            ui.painter().circle_stroke(c, 2.5, Stroke::new(1.0, th.frame));
        }
    }
    let pos = util::phrase(time);
    let strip: String = (0..16).map(|k| if k <= pos { '▰' } else { '▱' }).collect();
    put(ui, &g, body, col + 10.0, 0, &strip, th.blue);
    put(ui, &g, body, col + 27.0, 0, "120.0", th.fg);

    // Row 1: sync + cadence.
    put(ui, &g, body, 1.0, 1, "sync", th.dim);
    put(ui, &g, body, 12.0, 1, "[internal] link", th.yellow);
    put(ui, &g, body, 30.0, 1, "next  1 2 [4] 8", th.fg);

    // Row 2: chips.
    put(ui, &g, body, 1.0, 2, "tags", th.dim);
    put(ui, &g, body, 12.0, 2, "(cue: intro)", th.dim);
    put(ui, &g, body, 26.0, 2, "(2 peers)", th.green);
    put(ui, &g, body, 37.0, 2, "(audio! ✕)", th.red);

    // Rows 3–8: clip tiles, bordered cells with block art.
    put(ui, &g, body, 1.0, 3, "pool", th.dim);
    for (k, clip) in mock_clips().iter().enumerate() {
        let tile = Rect::from_min_size(
            Pos2::new(body.min.x + (12.0 + k as f32 * 13.0) * g.cw, body.min.y + 3.2 * g.rh),
            vec2(g.cw * 12.0, g.rh * 4.2),
        );
        let painter = ui.painter();
        let border = if clip.selected {
            th.yellow
        } else if clip.role == MockRole::Playing {
            alpha(th.phosphor, 120 + (pulse * 135.0) as u8)
        } else {
            th.frame
        };
        painter.rect_stroke(tile, CornerRadius::ZERO, Stroke::new(1.0, border), StrokeKind::Inside);
        let art = tile.shrink2(vec2(3.0, 3.0));
        for gy in 0..3 {
            for gx in 0..8 {
                let h = util::hash01(clip.seed, gy * 8 + gx);
                let cell = Rect::from_min_size(
                    Pos2::new(art.min.x + art.width() * gx as f32 / 8.0, art.min.y + (art.height() - g.rh) * gy as f32 / 3.0),
                    vec2(art.width() / 8.0 - 1.0, (art.height() - g.rh) / 3.0 - 1.0),
                );
                painter.rect_filled(
                    cell,
                    CornerRadius::ZERO,
                    util::hsl(st.hue + 60.0 + h * 120.0, 0.25, if st.dark { 0.14 + util::hash01(clip.seed, gy * 8 + gx + 31) * 0.18 } else { 0.55 + util::hash01(clip.seed, gy * 8 + gx + 31) * 0.25 }),
                );
            }
        }
        let short = clip.name.split('.').next().unwrap_or(clip.name);
        let glyph = match clip.role {
            MockRole::Playing => "▶",
            MockRole::Armed => "○",
            MockRole::None => " ",
        };
        painter.text(
            Pos2::new(tile.min.x + 3.0, tile.max.y - 9.0),
            Align2::LEFT_CENTER,
            format!("{glyph}{}", &short[..short.len().min(10)]),
            FontId::monospace(10.0),
            if clip.selected { th.yellow } else { th.fg },
        );
    }

    // Rows 9–11: level as scope glass, spectrum in theme green.
    let meter_row = Rect::from_min_size(Pos2::new(body.min.x, body.min.y + 8.6 * g.rh), vec2(body.width(), g.rh * 2.0));
    put(ui, &g, meter_row, 1.0, 0, "level", th.dim);
    scope_wave(ui, &g, &th, meter_row, 12.0, time);
    let fft_row = Rect::from_min_size(Pos2::new(body.min.x, body.min.y + 11.0 * g.rh), vec2(body.width(), g.rh));
    put(ui, &g, fft_row, 1.0, 0, "fft", th.dim);
    phosphor_fft(ui, &g, &th, fft_row, 12.0, time);

    // Statusline.
    let status = Rect::from_min_size(
        Pos2::new(frame.min.x + 1.0, frame.max.y - 19.0),
        vec2(frame.width() - 2.0, 18.0),
    );
    ui.painter().rect_filled(status, CornerRadius::ZERO, th.select);
    let tag = if nerd { nf::check(nf::MIDI) } else { "midi" };
    ui.painter().text(
        Pos2::new(status.min.x + g.cw, status.center().y),
        Align2::LEFT_CENTER,
        format!("NORMAL   widgets   4 clips · 1 playing · {tag} learn ready"),
        mono(),
        th.fg,
    );

    ui.add_space(10.0);
    util::footer(
        ui,
        th.dim,
        ui.visuals().text_color(),
        &[("WIDGETS", "Terminal's grid everywhere text wins; hardware where it pays — jewel-lamp beat dots, scope glass for the level trace — all re-tinted live by the Everforest hue slider.")],
    );
}

/// Header row: `[dark]/[light]` selector and the hue-rotation slider, in the
/// buffer's own idiom. Mutations land next frame (the demo repaints
/// continuously).
fn theme_bar(ui: &mut Ui, g: &Grid, width: f32, st: &mut DemoState, th: &Theme) {
    let (bar, _) = ui.allocate_exact_size(vec2(width, g.rh + 4.0), Sense::hover());
    // The bar carries its own themed background so its text has the
    // palette's contrast regardless of the app chrome behind it.
    ui.painter().rect_filled(bar, CornerRadius::same(2), th.bg);
    ui.painter().rect_stroke(bar, CornerRadius::same(2), Stroke::new(1.0, th.frame), StrokeKind::Inside);
    let row = bar.translate(vec2(0.0, 2.0));
    put(ui, g, row, 1.0, 0, "theme", th.dim);
    let mut col = 8.0;
    for (lab, dark) in [("dark", true), ("light", false)] {
        let selected = st.dark == dark;
        let text = if selected { format!("[{lab}]") } else { format!(" {lab} ") };
        let color = if selected { th.yellow } else { th.dim };
        let r = put(ui, g, row, col, 0, &text, color);
        let resp = ui.interact(r, ui.id().with(("mode", lab)), Sense::click());
        if resp.clicked() {
            st.dark = dark;
        }
        col += text.chars().count() as f32 + 1.0;
    }

    put(ui, g, row, col + 2.0, 0, "hue", th.dim);
    let strip = Rect::from_min_size(
        Pos2::new(bar.min.x + (col + 6.0) * g.cw, bar.min.y + 5.0),
        vec2(g.cw * 20.0, g.rh - 6.0),
    );
    let resp = ui.interact(strip, ui.id().with("hue"), Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            // Hue is circular: the strip's right edge wraps back to 0.
            st.hue = (((pos.x - strip.min.x) / strip.width()).clamp(0.0, 1.0) * 360.0).rem_euclid(360.0);
        }
    }
    let painter = ui.painter();
    const N: usize = 40;
    for k in 0..N {
        let cell = Rect::from_min_size(
            Pos2::new(strip.min.x + strip.width() * k as f32 / N as f32, strip.min.y),
            vec2(strip.width() / N as f32 + 0.5, strip.height()),
        );
        painter.rect_filled(cell, CornerRadius::ZERO, util::hsl(k as f32 / N as f32 * 360.0, 0.5, 0.5));
    }
    painter.rect_stroke(strip, CornerRadius::ZERO, Stroke::new(1.0, th.frame), StrokeKind::Outside);
    let x = strip.min.x + strip.width() * st.hue / 360.0;
    painter.line_segment([Pos2::new(x, strip.min.y - 2.0), Pos2::new(x, strip.max.y + 2.0)], Stroke::new(2.0, th.fg));
    put(ui, g, row, col + 6.0 + 21.0, 0, &format!("{:>4.0}°", st.hue), th.fg);
}

/// Paint text at a character column within a row.
fn put(ui: &Ui, g: &Grid, row: Rect, col: f32, line: usize, text: &str, color: Color32) -> Rect {
    let pos = Pos2::new(row.min.x + col * g.cw, row.min.y + (line as f32 + 0.5) * g.rh);
    let galley = ui.painter().layout_no_wrap(text.to_string(), mono(), color);
    let size = galley.size();
    ui.painter().galley(pos - vec2(0.0, size.y * 0.5), galley, color);
    Rect::from_min_size(pos - vec2(0.0, g.rh * 0.5), vec2(size.x, g.rh))
}

/// One control on the character grid.
#[allow(clippy::too_many_arguments)]
fn control(
    ui: &mut Ui,
    g: &Grid,
    th: &Theme,
    row: Rect,
    i: usize,
    label: &str,
    name: &str,
    kind: &MockKind,
    val: &mut Value,
    time: f64,
    nerd: bool,
    midi: &MidiState,
) {
    put(ui, g, row, 1.0, 0, &label.to_lowercase(), th.dim);
    const CTL: f32 = 12.0;
    match (kind, val) {
        (MockKind::Float { min, max, .. }, Value::Float(v)) => {
            let center = Pos2::new(row.min.x + (CTL + 3.0) * g.cw, row.center().y);
            let bipolar = *min < 0.0 && *max > 0.0;
            let ghost = midi.binding(i).and_then(|_| midi.incoming_pos(i, time));
            chickenhead(ui, th, i, center, 13.0, (*v - min) / (max - min), bipolar, ghost, |t| {
                *v = min + t * (max - min);
            });
            put(ui, g, row, CTL + 8.0, 0, &format!("{v:>7.2}"), th.fg);
            put(ui, g, row, CTL + 8.0, 1, if bipolar { "-1 ·· +1" } else { "" }, alpha(th.dim, 140));
        }
        (MockKind::Bool { .. }, Value::Bool(b)) => {
            let r = put(ui, g, row, CTL, 0, if *b { "[x]" } else { "[ ]" }, if *b { th.green } else { th.dim });
            put(ui, g, row, CTL + 4.0, 0, if *b { "on" } else { "off" }, th.fg);
            let resp = ui.interact(r.expand2(vec2(g.cw * 2.0, 0.0)), ui.id().with(("bool", i)), Sense::click());
            if resp.clicked() {
                *b = !*b;
            }
        }
        (MockKind::Long { values, labels, .. }, Value::LongIdx(sel)) => {
            // The canonical chickenhead: a detented selector with radial
            // mono legends. Center sits low in the block so the top legend
            // stays inside this row.
            let center = Pos2::new(row.min.x + (CTL + 3.0) * g.cw, row.min.y + 1.9 * g.rh);
            let n = labels.len();
            let mut t = *sel as f32 / (n - 1) as f32;
            chickenhead(ui, th, i, center, 13.0, t, false, None, |nt| t = nt);
            *sel = (t * (n - 1) as f32).round() as usize;
            for (k, lab) in labels.iter().enumerate() {
                let dir = util::knob_dir(k as f32 / (n - 1) as f32);
                let pos = center + dir * 26.0;
                let color = if k == *sel { th.yellow } else { th.dim };
                ui.painter().text(pos, Align2::CENTER_CENTER, *lab, FontId::monospace(10.0), color);
            }
            put(ui, g, row, CTL + 10.0, 1, &format!("= {}", values[*sel]), th.dim);
        }
        (MockKind::Event, Value::EventAt(at)) => {
            let flash = util::event_flash(time, *at);
            let fire = if nerd {
                format!("[ {} fire ]", nf::check(nf::FLASH))
            } else {
                "[ fire ]".to_string()
            };
            let r = put(ui, g, row, CTL, 0, &fire, if flash > 0.0 { th.bg } else { th.fg });
            if flash > 0.0 {
                ui.painter().rect_filled(r, CornerRadius::ZERO, alpha(th.yellow, 60 + (flash * 195.0) as u8));
                put(ui, g, row, CTL, 0, &fire, th.bg);
            }
            let resp = ui.interact(r, ui.id().with(("event", i)), Sense::click());
            if resp.clicked() {
                *at = time;
            }
        }
        (MockKind::Point2D { min, max, .. }, Value::Point(p)) => {
            scope_xy(ui, g, th, row, i, CTL, min, max, p);
            put(ui, g, row, CTL + 29.0, 1, &format!("x {:>5.2}", p[0]), th.fg);
            put(ui, g, row, CTL + 29.0, 2, &format!("y {:>5.2}", p[1]), th.fg);
        }
        (MockKind::Color { .. }, Value::Color(c)) => {
            let color = Color32::from(*c);
            let [r8, g8, b8, _] = color.to_array();
            put(ui, g, row, CTL, 0, "███", color);
            put(ui, g, row, CTL + 4.0, 0, &format!("#{r8:02x}{g8:02x}{b8:02x}"), th.fg);
            // Mini chickenheads for the HSV triple, on their own line with
            // labels clear of the skirt ticks.
            for (k, ch_label) in ["h", "s", "v"].iter().enumerate() {
                let ch = match k {
                    0 => &mut c.h,
                    1 => &mut c.s,
                    _ => &mut c.v,
                };
                let x = row.min.x + (CTL + 1.5 + k as f32 * 8.0) * g.cw;
                let center = Pos2::new(x, row.min.y + 2.0 * g.rh);
                put(ui, g, row, CTL - 2.0 + k as f32 * 8.0, 2, ch_label, th.dim);
                chickenhead(ui, th, i * 8 + k, center, 6.5, *ch, false, None, |t| *ch = t);
            }
        }
        (MockKind::Image, _) => {
            if nerd {
                put(ui, g, row, CTL - 3.0, 0, nf::check(nf::IMAGE), th.blue);
            }
            put(ui, g, row, CTL, 0, &format!("src ▸ {}", name.to_lowercase()), th.blue);
            put(ui, g, row, CTL + 24.0, 0, ":route", th.dim);
        }
        (MockKind::Audio, _) => {
            if nerd {
                put(ui, g, row, CTL - 3.0, 0, nf::check(nf::AUDIO), th.dim);
            }
            scope_wave(ui, g, th, row, CTL, time);
        }
        (MockKind::AudioFft, _) => {
            if nerd {
                put(ui, g, row, CTL - 2.5, 0, nf::check(nf::FFT), th.dim);
            }
            phosphor_fft(ui, g, th, row, CTL, time);
        }
        _ => {}
    }
}

/// A chickenhead knob, drawn the way a terminal would: one flat convex
/// teardrop, 1-px strokes, skirt ticks. `t` is the value in `0..=1`; drag
/// vertically to edit. `ghost` paints a soft-pickup tick at the incoming-CC
/// position.
#[allow(clippy::too_many_arguments)]
fn chickenhead(
    ui: &mut Ui,
    th: &Theme,
    seed: usize,
    center: Pos2,
    r: f32,
    t: f32,
    bipolar: bool,
    ghost: Option<f32>,
    mut set: impl FnMut(f32),
) {
    let hit = Rect::from_center_size(center, vec2(r * 3.2, r * 3.2));
    let resp = ui.interact(hit, ui.id().with(("chicken", seed)), Sense::drag());
    let mut nt = t;
    if util::vdrag(&resp, &mut nt, 0.0, 1.0, 120.0) {
        set(nt);
    }
    let t = nt;
    let painter = ui.painter();

    // Skirt ticks: ends, middle (bright when it's the bipolar rest), quarters.
    for (k, tick) in [0.0_f32, 0.25, 0.5, 0.75, 1.0].iter().enumerate() {
        let dir = util::knob_dir(*tick);
        let major = k % 2 == 0;
        let color = if bipolar && k == 2 { th.fg } else { alpha(th.dim, if major { 200 } else { 110 }) };
        painter.line_segment([center + dir * (r + 3.0), center + dir * (r + if major { 7.0 } else { 5.0 })], Stroke::new(1.0, color));
    }
    if let Some(gt) = ghost {
        let dir = util::knob_dir(gt);
        painter.line_segment([center + dir * (r + 3.0), center + dir * (r + 8.0)], Stroke::new(2.0, th.yellow));
    }

    // Body: circle arc with a notch replaced by the pointer apex — one
    // closed teardrop.
    let dir = util::knob_dir(t);
    let apex_angle = dir.angle();
    let mut pts = Vec::with_capacity(26);
    pts.push(center + dir * (r * 1.45));
    let half_notch = 0.45_f32; // radians each side of the apex
    for k in 0..=22 {
        let a = apex_angle + half_notch + (std::f32::consts::TAU - 2.0 * half_notch) * (k as f32 / 22.0);
        pts.push(center + egui::Vec2::angled(a) * r);
    }
    let stroke_color = if resp.dragged() || resp.hovered() { th.fg } else { th.frame };
    painter.add(Shape::convex_polygon(pts, th.knob, Stroke::new(1.0, stroke_color)));
    // Pointer line down the tip, in the value color.
    painter.line_segment([center + dir * (r * 0.25), center + dir * (r * 1.35)], Stroke::new(1.5, th.green));
    painter.circle_filled(center, 1.5, alpha(th.fg, 90));
}

/// Oscilloscope XY for point2D: dot-glyph graticule, center axes with tick
/// marks, phosphor beam with a persistence trail (drag history kept in egui
/// temp memory).
#[allow(clippy::too_many_arguments)]
fn scope_xy(ui: &mut Ui, g: &Grid, th: &Theme, row: Rect, i: usize, col: f32, min: &[f32; 2], max: &[f32; 2], p: &mut [f32; 2]) {
    let screen = Rect::from_min_size(
        Pos2::new(row.min.x + col * g.cw, row.min.y + 4.0),
        vec2(g.cw * 26.0, row.height() - 8.0),
    );
    let resp = ui.interact(screen, ui.id().with(("scope", i)), Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let tx = ((pos.x - screen.min.x) / screen.width()).clamp(0.0, 1.0);
            let ty = 1.0 - ((pos.y - screen.min.y) / screen.height()).clamp(0.0, 1.0);
            p[0] = min[0] + tx * (max[0] - min[0]);
            p[1] = min[1] + ty * (max[1] - min[1]);
        }
    }
    let painter = ui.painter();
    painter.rect_filled(screen, CornerRadius::same(2), th.screen);
    painter.rect_stroke(screen, CornerRadius::same(2), Stroke::new(1.0, th.frame), StrokeKind::Inside);

    // Graticule: dot glyphs off-axis, dim solid center axes with ticks.
    let grat = alpha(util::hsl(150.0, 0.1, 0.6), 110);
    const DX: usize = 8;
    const DY: usize = 4;
    for gy in 0..=DY {
        for gx in 0..=DX {
            if gx == DX / 2 || gy == DY / 2 {
                continue;
            }
            let pos = Pos2::new(
                screen.min.x + gx as f32 / DX as f32 * screen.width(),
                screen.min.y + gy as f32 / DY as f32 * screen.height(),
            );
            painter.text(pos, Align2::CENTER_CENTER, "·", mono(), grat);
        }
    }
    let mid = screen.center();
    painter.line_segment([Pos2::new(screen.min.x, mid.y), Pos2::new(screen.max.x, mid.y)], Stroke::new(1.0, alpha(grat, 70)));
    painter.line_segment([Pos2::new(mid.x, screen.min.y), Pos2::new(mid.x, screen.max.y)], Stroke::new(1.0, alpha(grat, 70)));
    for gx in 0..=DX * 2 {
        let x = screen.min.x + gx as f32 / (DX * 2) as f32 * screen.width();
        painter.line_segment([Pos2::new(x, mid.y - 2.0), Pos2::new(x, mid.y + 2.0)], Stroke::new(1.0, grat));
    }
    for gy in 0..=DY * 2 {
        let y = screen.min.y + gy as f32 / (DY * 2) as f32 * screen.height();
        painter.line_segment([Pos2::new(mid.x - 2.0, y), Pos2::new(mid.x + 2.0, y)], Stroke::new(1.0, grat));
    }

    // Beam: persistence trail through recent positions, then the glow dot.
    let tx = (p[0] - min[0]) / (max[0] - min[0]);
    let ty = 1.0 - (p[1] - min[1]) / (max[1] - min[1]);
    let dot = Pos2::new(
        screen.min.x + screen.width() * tx,
        screen.min.y + screen.height() * ty,
    );
    let hist_id = ui.id().with(("scope-hist", i));
    let mut hist: Vec<(f32, f32)> = ui.ctx().data_mut(|d| d.get_temp(hist_id)).unwrap_or_default();
    hist.push((dot.x, dot.y));
    if hist.len() > 32 {
        let excess = hist.len() - 32;
        hist.drain(..excess);
    }
    ui.ctx().data_mut(|d| d.insert_temp(hist_id, hist.clone()));
    for (k, seg) in hist.windows(2).enumerate() {
        let a = (k as f32 / hist.len() as f32 * 120.0) as u8;
        painter.line_segment(
            [Pos2::new(seg[0].0, seg[0].1), Pos2::new(seg[1].0, seg[1].1)],
            Stroke::new(1.2, alpha(th.phosphor, a)),
        );
    }
    painter.circle_filled(dot, 8.0, alpha(th.phosphor, 22));
    painter.circle_filled(dot, 4.5, alpha(th.phosphor, 60));
    painter.circle_filled(dot, 2.2, th.phosphor);
}

/// Audio as a single-trace scope: dim centerline, phosphor waveform.
fn scope_wave(ui: &Ui, g: &Grid, th: &Theme, row: Rect, col: f32, time: f64) {
    let screen = Rect::from_min_size(
        Pos2::new(row.min.x + col * g.cw, row.min.y + 3.0),
        vec2(g.cw * 26.0, row.height() - 6.0),
    );
    let painter = ui.painter();
    painter.rect_filled(screen, CornerRadius::same(2), th.screen);
    painter.rect_stroke(screen, CornerRadius::same(2), Stroke::new(1.0, th.frame), StrokeKind::Inside);
    painter.line_segment(
        [Pos2::new(screen.min.x, screen.center().y), Pos2::new(screen.max.x, screen.center().y)],
        Stroke::new(1.0, alpha(util::hsl(150.0, 0.1, 0.6), 70)),
    );
    const N: usize = 56;
    let pts: Vec<Pos2> = (0..N)
        .map(|k| {
            let s = util::wave(time, k, N);
            Pos2::new(
                screen.min.x + 2.0 + (screen.width() - 4.0) * k as f32 / (N - 1) as f32,
                screen.center().y - s * (screen.height() * 0.42),
            )
        })
        .collect();
    painter.add(Shape::line(pts, Stroke::new(1.2, th.phosphor)));
}

/// Spectrum in the theme's green, cell style carried over from Terminal.
fn phosphor_fft(ui: &Ui, g: &Grid, th: &Theme, row: Rect, col: f32, time: f64) {
    let painter = ui.painter();
    let n = 24;
    for k in 0..n {
        let mag = util::fft(time, k, n);
        let h = (g.rh - 5.0) * mag;
        let cell = Rect::from_min_max(
            Pos2::new(row.min.x + (col + k as f32) * g.cw, row.min.y + g.rh - 2.0 - h),
            Pos2::new(row.min.x + (col + k as f32) * g.cw + g.cw - 2.0, row.min.y + g.rh - 2.0),
        );
        let color = if mag > 0.85 { th.red } else { th.green };
        painter.rect_filled(cell, CornerRadius::ZERO, alpha(color, 90 + (mag * 165.0) as u8));
    }
}

/// Same right-edge `<cc74>` tag as Terminal.
#[allow(clippy::too_many_arguments)]
fn bind_tag(ui: &Ui, g: &Grid, th: &Theme, midi: &MidiState, idx: usize, kind: &MockKind, row: Rect, time: f64, nerd: bool) {
    if !MidiState::bindable(kind) {
        return;
    }
    if let Some(b) = midi.binding(idx) {
        let act = midi.activity(idx, time);
        let color = if act > 0.0 { th.yellow } else { alpha(th.yellow, 140) };
        let text = if nerd {
            format!("{} <{}>", nf::check(nf::MIDI), b.label().to_lowercase())
        } else {
            format!("<{}>", b.label().to_lowercase())
        };
        let cols = text.chars().count() as f32 + 1.0;
        let col = (row.width() / g.cw - cols).floor();
        put(ui, g, row, col, 0, &text, color);
    }
}
