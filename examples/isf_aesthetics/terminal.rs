//! Direction 4 — **Terminal**: the nvim / Claude Code TUI idiom, rendered in
//! egui. One monospace grid; every control is text-first — bracket sliders,
//! `[x]` toggles, a cell-block spectrum, a statusline that owns the
//! MIDI-learn mode (nvim visual-mode style: arm, click a row, done).
//! Performance is the best of the five directions: glyphs come from the
//! existing font atlas, no polylines at all.
//!
//! Font note for production: OFL fonts are accepted, so this direction loads
//! an installed Iosevka Nerd Font Mono when present (see `install_nerd_font`
//! in main.rs) and uses icon glyphs from license-clean sets only — the
//! CC-BY/unlicensed ranges are banned in [`crate::nf`], with license texts
//! collected in `licenses/` at the repo root.

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

use crate::nf;
use crate::schema::{DemoState, MidiState, MockKind, MockRole, Value, mock_clips};
use crate::util;

const BG: Color32 = Color32::from_rgb(14, 16, 20);
const FG: Color32 = Color32::from_rgb(214, 214, 214);
const DIM: Color32 = Color32::from_rgb(107, 114, 128);
const FRAME: Color32 = Color32::from_rgb(58, 63, 75);
const GREEN: Color32 = Color32::from_rgb(152, 195, 121);
const YELLOW: Color32 = Color32::from_rgb(229, 192, 123);
const BLUE: Color32 = Color32::from_rgb(97, 175, 239);
const MAGENTA: Color32 = Color32::from_rgb(198, 120, 221);
const RED: Color32 = Color32::from_rgb(224, 108, 117);
const SELECT: Color32 = Color32::from_rgb(38, 44, 56);

fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

fn mono() -> FontId {
    FontId::monospace(12.0)
}

/// Character-grid metrics for the frame.
struct Grid {
    cw: f32,
    rh: f32,
}

/// Render the whole direction panel.
pub fn show(ui: &mut Ui, st: &mut DemoState) {
    let cw = ui.painter().layout_no_wrap("─".into(), mono(), FG).size().x;
    let g = Grid { cw, rh: 18.0 };
    // The title rides the top border; give its upper half room to paint.
    ui.add_space(8.0);

    let time = st.time;
    let armed = st.midi.armed;
    let nerd = st.nerd;

    // The buffer: rows for every input (point2D pads and color spend 2).
    let rows: usize = st
        .inputs
        .iter()
        .map(|inp| match inp.kind {
            MockKind::Point2D { .. } => 4,
            MockKind::Color { .. } => 2,
            _ => 1,
        })
        .sum();
    let height = rows as f32 * g.rh + 30.0;
    let (frame, _) = ui.allocate_exact_size(vec2(ui.available_width().min(560.0), height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(frame, CornerRadius::same(2), BG);
    painter.rect_stroke(frame, CornerRadius::same(2), Stroke::new(1.0, FRAME), StrokeKind::Inside);
    // Title embedded in the top border, nvim winbar style.
    painter.text(
        Pos2::new(frame.min.x + g.cw * 2.0, frame.min.y),
        Align2::LEFT_CENTER,
        "─ kaleido-bloom.fs ─",
        mono(),
        DIM,
    );

    let mut cursor = frame.min.y + 8.0;
    let DemoState { inputs, values, midi, .. } = st;
    for (i, (inp, val)) in inputs.iter().zip(values.iter_mut()).enumerate() {
        let row_count = match inp.kind {
            MockKind::Point2D { .. } => 4,
            MockKind::Color { .. } => 2,
            _ => 1,
        };
        let row = Rect::from_min_size(
            Pos2::new(frame.min.x + 4.0, cursor),
            vec2(frame.width() - 8.0, row_count as f32 * g.rh),
        );
        cursor += row_count as f32 * g.rh;

        if armed && MidiState::bindable(&inp.kind) {
            // Visual-mode row highlight; a click anywhere in the row binds.
            let pulse = 0.5 + 0.5 * ((time * 6.0).sin() as f32);
            ui.painter().rect_filled(row, CornerRadius::ZERO, alpha(SELECT, 160 + (pulse * 80.0) as u8));
            let resp = ui.interact(row, ui.id().with(("learn", i)), Sense::click());
            if resp.clicked() {
                midi.learn_click(i, &inp.kind);
            }
        }

        let frozen = *val;
        control(ui, &g, row, i, inp.label, inp.name, &inp.kind, val, time, nerd);
        if armed {
            *val = frozen;
        }
        bind_tag(ui, &g, midi, i, &inp.kind, row, time, nerd);
    }

    // Statusline, reverse video.
    let status = Rect::from_min_size(
        Pos2::new(frame.min.x + 1.0, frame.max.y - 19.0),
        vec2(frame.width() - 2.0, 18.0),
    );
    let painter = ui.painter();
    if armed {
        painter.rect_filled(status, CornerRadius::ZERO, MAGENTA);
        painter.text(
            Pos2::new(status.min.x + g.cw, status.center().y),
            Align2::LEFT_CENTER,
            "-- LEARN --  click a param to bind · click bound to unbind",
            mono(),
            BG,
        );
    } else {
        painter.rect_filled(status, CornerRadius::ZERO, SELECT);
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
            DIM,
        );
    }

    ui.add_space(10.0);
    util::footer(
        ui,
        DIM,
        Color32::from_rgb(170, 174, 182),
        &[
            ("DENSITY", "~18 px per scalar row — tied with Console, and column alignment makes long chains scannable like a buffer."),
            ("EPAINT COST", "Cheapest of the five: pure atlas glyphs plus a handful of rects. No polylines anywhere."),
            ("MIDI", "Inline <cc74> tag costs 7 chars; learn as a statusline mode is the cleanest arming story of the five."),
            ("FONT", "OFL accepted: loads your installed Iosevka Nerd Font Mono at runtime (falls back to Hack). Per-set constraint remains: Font Awesome + Codicons are CC-BY-4.0, Font Logos unlicensed — nf.rs bans those ranges, licenses/ holds the retained sets' texts, and a production embed must strip the banned ranges from the font file itself."),
            ("FIT", "Closest cousin to the app's flat chrome; adopting it is mostly a font embed plus discipline, not a repaint."),
        ],
    );
}

/// The app's widget vocabulary as buffer rows: bracket taps, glyph beat
/// dots, a block-character phrase strip, `(chip)` text, and clip tiles as
/// bordered cells with ANSI-block art.
pub fn show_widgets(ui: &mut Ui, st: &mut DemoState) {
    let cw = ui.painter().layout_no_wrap("─".into(), mono(), FG).size().x;
    let g = Grid { cw, rh: 18.0 };
    ui.add_space(8.0);
    let time = st.time;
    let nerd = st.nerd;
    let (beat, pulse) = util::beat(time);

    let rows = 12.0;
    let height = rows * g.rh + 30.0;
    let (frame, _) = ui.allocate_exact_size(vec2(ui.available_width().min(560.0), height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(frame, CornerRadius::same(2), BG);
    painter.rect_stroke(frame, CornerRadius::same(2), Stroke::new(1.0, FRAME), StrokeKind::Inside);
    painter.text(
        Pos2::new(frame.min.x + g.cw * 2.0, frame.min.y),
        Align2::LEFT_CENTER,
        "─ vidiotic · widgets ─",
        mono(),
        DIM,
    );
    let body = Rect::from_min_size(Pos2::new(frame.min.x + 4.0, frame.min.y + 8.0), vec2(frame.width() - 8.0, rows * g.rh));

    // Row 0: transport taps + beat glyphs + phrase strip + bpm.
    put(ui, &g, body, 1.0, 0, "transport", DIM);
    let mut col = 12.0;
    for label in ["▼", "⟲", "tap"] {
        let text = format!("[ {label} ]");
        let r = put(ui, &g, body, col, 0, &text, FG);
        let resp = ui.interact(r, ui.id().with(("wtap", label)), Sense::click());
        let flash = util::tap_flash(ui, ui.id().with(("wtapf", label)), resp.clicked(), time);
        if flash > 0.0 {
            ui.painter().rect_filled(r, CornerRadius::ZERO, alpha(YELLOW, 60 + (flash * 195.0) as u8));
            put(ui, &g, body, col, 0, &text, BG);
        }
        col += text.chars().count() as f32 + 1.0;
    }
    let beats: String = (0..4).map(|k| if k == beat { '●' } else { '○' }).collect();
    put(ui, &g, body, col + 1.0, 0, &beats, GREEN);
    let pos = util::phrase(time);
    let strip: String = (0..16).map(|k| if k <= pos { '▰' } else { '▱' }).collect();
    put(ui, &g, body, col + 7.0, 0, &strip, alpha(BLUE, 220));
    put(ui, &g, body, col + 24.0, 0, "120.0", FG);

    // Row 1: sync + cadence as bracket lists.
    put(ui, &g, body, 1.0, 1, "sync", DIM);
    put(ui, &g, body, 12.0, 1, "[internal] link", YELLOW);
    put(ui, &g, body, 30.0, 1, "next  1 2 [4] 8", FG);

    // Row 2: chips as parenthesized tags.
    put(ui, &g, body, 1.0, 2, "tags", DIM);
    put(ui, &g, body, 12.0, 2, "(cue: intro)", DIM);
    put(ui, &g, body, 26.0, 2, "(2 peers)", GREEN);
    put(ui, &g, body, 37.0, 2, "(audio! ✕)", RED);

    // Rows 3–8: clip tiles as bordered cells with block art.
    put(ui, &g, body, 1.0, 3, "pool", DIM);
    for (k, clip) in mock_clips().iter().enumerate() {
        let tile = Rect::from_min_size(
            Pos2::new(body.min.x + (12.0 + k as f32 * 13.0) * g.cw, body.min.y + 3.2 * g.rh),
            vec2(g.cw * 12.0, g.rh * 4.2),
        );
        let painter = ui.painter();
        let border = if clip.selected {
            YELLOW
        } else if clip.role == MockRole::Playing {
            alpha(GREEN, 120 + (pulse * 135.0) as u8)
        } else {
            FRAME
        };
        painter.rect_stroke(tile, CornerRadius::ZERO, Stroke::new(1.0, border), StrokeKind::Inside);
        // ANSI-block art: a coarse cell raster, hue seeded per clip.
        let art = tile.shrink2(vec2(3.0, 3.0));
        for gy in 0..3 {
            for gx in 0..8 {
                let h = util::hash01(clip.seed, gy * 8 + gx);
                let cell = Rect::from_min_size(
                    Pos2::new(art.min.x + art.width() * gx as f32 / 8.0, art.min.y + (art.height() - g.rh) * gy as f32 / 3.0),
                    vec2(art.width() / 8.0 - 1.0, (art.height() - g.rh) / 3.0 - 1.0),
                );
                painter.rect_filled(cell, CornerRadius::ZERO, util::hsl(180.0 + h * 140.0, 0.30, 0.14 + util::hash01(clip.seed, gy * 8 + gx + 31) * 0.18));
            }
        }
        let short = clip.name.split('.').next().unwrap_or(clip.name);
        let glyph = match clip.role {
            MockRole::Playing => "▶",
            MockRole::Armed => "○",
            MockRole::None => " ",
        };
        let name_color = if clip.selected { YELLOW } else { FG };
        painter.text(
            Pos2::new(tile.min.x + 3.0, tile.max.y - 9.0),
            Align2::LEFT_CENTER,
            format!("{glyph}{}", &short[..short.len().min(10)]),
            FontId::monospace(10.0),
            name_color,
        );
    }

    // Rows 9–10: level + spectrum, cell style.
    let meter_row = Rect::from_min_size(Pos2::new(body.min.x, body.min.y + 8.6 * g.rh), vec2(body.width(), g.rh));
    put(ui, &g, meter_row, 1.0, 0, "level", DIM);
    cells_meter(ui, &g, meter_row, 12.0, time);
    let fft_row = Rect::from_min_size(Pos2::new(body.min.x, body.min.y + 9.8 * g.rh), vec2(body.width(), g.rh));
    put(ui, &g, fft_row, 1.0, 0, "fft", DIM);
    cells_fft(ui, &g, fft_row, 12.0, time);

    // Statusline.
    let status = Rect::from_min_size(
        Pos2::new(frame.min.x + 1.0, frame.max.y - 19.0),
        vec2(frame.width() - 2.0, 18.0),
    );
    ui.painter().rect_filled(status, CornerRadius::ZERO, SELECT);
    let tag = if nerd { nf::check(nf::MIDI) } else { "midi" };
    ui.painter().text(
        Pos2::new(status.min.x + g.cw, status.center().y),
        Align2::LEFT_CENTER,
        format!("NORMAL   widgets   4 clips · 1 playing · {tag} learn ready"),
        mono(),
        DIM,
    );

    ui.add_space(10.0);
    util::footer(
        ui,
        DIM,
        Color32::from_rgb(170, 174, 182),
        &[("WIDGETS", "Everything stays on the grid: taps are bracket buttons, beat is filled/hollow dot glyphs, the phrase strip is block glyphs, chips are (parenthesized), and clip tiles are bordered cells with a coarse block raster for art.")],
    );
}

/// Paint text at a character column within a row.
fn put(ui: &Ui, g: &Grid, row: Rect, col: f32, line: usize, text: &str, color: Color32) -> Rect {
    let pos = Pos2::new(row.min.x + col * g.cw, row.min.y + (line as f32 + 0.5) * g.rh);
    let galley = ui.painter().layout_no_wrap(text.to_string(), mono(), color);
    let size = galley.size();
    ui.painter().galley(pos - vec2(0.0, size.y * 0.5), galley, color);
    Rect::from_min_size(pos - vec2(0.0, g.rh * 0.5), vec2(size.x, g.rh))
}

/// One control, laid out on the character grid of `row`.
#[allow(clippy::too_many_arguments)]
fn control(
    ui: &mut Ui,
    g: &Grid,
    row: Rect,
    i: usize,
    label: &str,
    name: &str,
    kind: &MockKind,
    val: &mut Value,
    time: f64,
    nerd: bool,
) {
    put(ui, g, row, 1.0, 0, &label.to_lowercase(), DIM);
    const CTL: f32 = 12.0;
    match (kind, val) {
        (MockKind::Float { min, max, .. }, Value::Float(v)) => {
            let bipolar = *min < 0.0 && *max > 0.0;
            bar(ui, g, row, CTL, i, *min, *max, v, bipolar);
            put(ui, g, row, CTL + 24.0, 0, &format!("{v:>7.2}"), FG);
        }
        (MockKind::Bool { .. }, Value::Bool(b)) => {
            let r = put(ui, g, row, CTL, 0, if *b { "[x]" } else { "[ ]" }, if *b { GREEN } else { DIM });
            put(ui, g, row, CTL + 4.0, 0, if *b { "on" } else { "off" }, FG);
            let resp = ui.interact(r.expand2(vec2(g.cw * 2.0, 0.0)), ui.id().with(("bool", i)), Sense::click());
            if resp.clicked() {
                *b = !*b;
            }
        }
        (MockKind::Long { values, labels, .. }, Value::LongIdx(sel)) => {
            let mut col = CTL;
            for (k, name) in labels.iter().enumerate() {
                let selected = k == *sel;
                let text = if selected { format!("[{name}]") } else { format!(" {name} ") };
                let color = if selected { YELLOW } else { DIM };
                let r = put(ui, g, row, col, 0, &text, color);
                let resp = ui.interact(r, ui.id().with(("long", i, k)), Sense::click());
                if resp.clicked() {
                    *sel = k;
                }
                col += text.chars().count() as f32 + 1.0;
            }
            put(ui, g, row, col + 1.0, 0, &format!("= {}", values[*sel]), DIM);
        }
        (MockKind::Event, Value::EventAt(at)) => {
            let flash = util::event_flash(time, *at);
            let fire = if nerd {
                format!("[ {} fire ]", nf::check(nf::FLASH))
            } else {
                "[ fire ]".to_string()
            };
            let r = put(ui, g, row, CTL, 0, &fire, if flash > 0.0 { BG } else { FG });
            if flash > 0.0 {
                ui.painter().rect_filled(r, CornerRadius::ZERO, alpha(YELLOW, 60 + (flash * 195.0) as u8));
                put(ui, g, row, CTL, 0, &fire, BG);
            }
            let resp = ui.interact(r, ui.id().with(("event", i)), Sense::click());
            if resp.clicked() {
                *at = time;
            }
        }
        (MockKind::Point2D { min, max, .. }, Value::Point(p)) => {
            put(ui, g, row, CTL + 20.0, 1, &format!("x {:>5.2}", p[0]), FG);
            put(ui, g, row, CTL + 20.0, 2, &format!("y {:>5.2}", p[1]), FG);
            // A dot-grid pad, plotter-on-a-teletype style.
            let pad = Rect::from_min_size(
                Pos2::new(row.min.x + CTL * g.cw, row.min.y + 4.0),
                vec2(g.cw * 17.0, g.rh * 4.0 - 8.0),
            );
            let resp = ui.interact(pad, ui.id().with(("pad", i)), Sense::click_and_drag());
            if resp.dragged() || resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let tx = ((pos.x - pad.min.x) / pad.width()).clamp(0.0, 1.0);
                    let ty = 1.0 - ((pos.y - pad.min.y) / pad.height()).clamp(0.0, 1.0);
                    p[0] = min[0] + tx * (max[0] - min[0]);
                    p[1] = min[1] + ty * (max[1] - min[1]);
                }
            }
            let painter = ui.painter();
            for gy in 0..4 {
                for gx in 0..8 {
                    let pos = Pos2::new(
                        pad.min.x + (gx as f32 + 0.5) / 8.0 * pad.width(),
                        pad.min.y + (gy as f32 + 0.5) / 4.0 * pad.height(),
                    );
                    painter.text(pos, Align2::CENTER_CENTER, "·", mono(), alpha(DIM, 120));
                }
            }
            let tx = (p[0] - min[0]) / (max[0] - min[0]);
            let ty = 1.0 - (p[1] - min[1]) / (max[1] - min[1]);
            let dot = Pos2::new(pad.min.x + pad.width() * tx, pad.min.y + pad.height() * ty);
            painter.text(dot, Align2::CENTER_CENTER, "+", mono(), BLUE);
        }
        (MockKind::Color { .. }, Value::Color(c)) => {
            let color = Color32::from(*c);
            let [r8, g8, b8, _] = color.to_array();
            put(ui, g, row, CTL, 0, "███", color);
            put(ui, g, row, CTL + 4.0, 0, &format!("#{r8:02x}{g8:02x}{b8:02x}"), FG);
            let mut col = CTL;
            for (k, ch_label) in ["h", "s", "v"].iter().enumerate() {
                let ch = match k {
                    0 => &mut c.h,
                    1 => &mut c.s,
                    _ => &mut c.v,
                };
                put(ui, g, row, col, 1, ch_label, DIM);
                minibar(ui, g, row, col + 1.0, 1, i, k, ch);
                col += 9.0;
            }
        }
        (MockKind::Image, _) => {
            if nerd {
                put(ui, g, row, CTL - 3.0, 0, nf::check(nf::IMAGE), BLUE);
            }
            put(ui, g, row, CTL, 0, &format!("src ▸ {}", name.to_lowercase()), BLUE);
            put(ui, g, row, CTL + 24.0, 0, ":route", DIM);
        }
        (MockKind::Audio, _) => {
            if nerd {
                put(ui, g, row, CTL - 3.0, 0, nf::check(nf::AUDIO), DIM);
            }
            cells_meter(ui, g, row, CTL, time);
        }
        (MockKind::AudioFft, _) => {
            if nerd {
                put(ui, g, row, CTL - 2.5, 0, nf::check(nf::FFT), DIM);
            }
            cells_fft(ui, g, row, CTL, time);
        }
        _ => {}
    }
}

/// Bracket slider: `[██████░░░░░░]`, drag-to-set across the cell span.
/// Bipolar fills from the center cell outward.
#[allow(clippy::too_many_arguments)]
fn bar(ui: &mut Ui, g: &Grid, row: Rect, col: f32, i: usize, min: f32, max: f32, v: &mut f32, bipolar: bool) {
    const N: usize = 20;
    let span = Rect::from_min_size(
        Pos2::new(row.min.x + col * g.cw, row.min.y),
        vec2(g.cw * (N as f32 + 2.0), g.rh),
    );
    let resp = ui.interact(span, ui.id().with(("bar", i)), Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - span.min.x - g.cw) / (g.cw * N as f32)).clamp(0.0, 1.0);
            *v = min + t * (max - min);
        }
    }
    let t = (*v - min) / (max - min);
    let mut s = String::with_capacity(N + 2);
    s.push('[');
    let filled = |k: usize| {
        if bipolar {
            let mid = N / 2;
            let vc = (t * N as f32).round() as usize;
            (k >= mid.min(vc)) && (k < mid.max(vc)) || (vc == mid && k == mid)
        } else {
            k < (t * N as f32).round() as usize
        }
    };
    for k in 0..N {
        s.push(if filled(k) { '█' } else { '░' });
    }
    s.push(']');
    put(ui, g, row, col, 0, &s, if bipolar { MAGENTA } else { GREEN });
}

/// 6-cell mini bar for HSV channels.
#[allow(clippy::too_many_arguments)]
fn minibar(ui: &mut Ui, g: &Grid, row: Rect, col: f32, line: usize, i: usize, k: usize, ch: &mut f32) {
    const N: usize = 6;
    let span = Rect::from_min_size(
        Pos2::new(row.min.x + col * g.cw, row.min.y + line as f32 * g.rh),
        vec2(g.cw * (N as f32 + 2.0), g.rh),
    );
    let resp = ui.interact(span, ui.id().with(("mini", i, k)), Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            *ch = ((pos.x - span.min.x - g.cw) / (g.cw * N as f32)).clamp(0.0, 1.0);
        }
    }
    let mut s = String::new();
    s.push('[');
    for j in 0..N {
        s.push(if (j as f32) < *ch * N as f32 - 0.5 { '█' } else { '░' });
    }
    s.push(']');
    put(ui, g, row, col, line, &s, GREEN);
}

/// Stereo meter as terminal cells (guaranteed-render rects, not glyphs).
fn cells_meter(ui: &Ui, g: &Grid, row: Rect, col: f32, time: f64) {
    let painter = ui.painter();
    for ch in 0..2 {
        let lvl = util::level(time, ch as f32);
        let y = row.min.y + 4.0 + ch as f32 * 6.0;
        for k in 0..24 {
            let t = k as f32 / 24.0;
            let on = t < lvl;
            let color = if t > 0.85 { RED } else { GREEN };
            let cell = Rect::from_min_size(
                Pos2::new(row.min.x + (col + k as f32) * g.cw, y),
                vec2(g.cw - 2.0, 4.0),
            );
            painter.rect_filled(cell, CornerRadius::ZERO, if on { color } else { alpha(color, 30) });
        }
    }
}

/// Spectrum as one cell-column histogram per character cell.
fn cells_fft(ui: &Ui, g: &Grid, row: Rect, col: f32, time: f64) {
    let painter = ui.painter();
    let n = 24;
    for k in 0..n {
        let mag = util::fft(time, k, n);
        let h = (g.rh - 5.0) * mag;
        let cell = Rect::from_min_max(
            Pos2::new(row.min.x + (col + k as f32) * g.cw, row.min.y + g.rh - 2.0 - h),
            Pos2::new(row.min.x + (col + k as f32) * g.cw + g.cw - 2.0, row.min.y + g.rh - 2.0),
        );
        painter.rect_filled(cell, CornerRadius::ZERO, alpha(BLUE, 90 + (mag * 165.0) as u8));
    }
}

/// Terminal's bind affordance: an inline `<cc74>` tag at the right edge of
/// the row, brightening on mock activity.
#[allow(clippy::too_many_arguments)]
fn bind_tag(ui: &Ui, g: &Grid, midi: &MidiState, idx: usize, kind: &MockKind, row: Rect, time: f64, nerd: bool) {
    if !MidiState::bindable(kind) {
        return;
    }
    if let Some(b) = midi.binding(idx) {
        let act = midi.activity(idx, time);
        let color = if act > 0.0 { YELLOW } else { alpha(YELLOW, 140) };
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
