//! Direction 5 — **Phosphor**: the midpoint between Terminal and Hybrid.
//! Terminal's glyph discipline and density come back — every scalar control
//! is a single 18-px buffer row — but the control *semantics* stay
//! hardware: floats are faders (a cap sliding a tick-marked track), longs
//! are detented slide switches, and point2D keeps the oscilloscope with its
//! beam and persistence rasterized to shading blocks (`█▓▒░`), so even the
//! phosphor decay lives on the character grid. Audio and FFT render as
//! eighth-block traces. The Everforest HSL theme — dark/light anchors, hue
//! rotation, theme bar — is shared with Hybrid via [`crate::everforest`].

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

use crate::everforest::{Grid, Theme, mono, put, theme, theme_bar};
use crate::nf;
use crate::schema::{DemoState, MidiState, MockKind, MockRole, Value, mock_clips};
use crate::util;

fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Eighth-block ramp for glyph traces.
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Grid rows a control spends — Terminal's budget: only the scope and the
/// color triple buy extra rows.
fn rows_of(kind: &MockKind) -> usize {
    match kind {
        MockKind::Point2D { .. } => 4,
        MockKind::Color { .. } => 2,
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
    // Winbar-style title with the border broken behind it (light mode needs
    // the patch as much as dark).
    let title = "─ kaleido-bloom.fs · crt ─";
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
            ("DENSITY", "Terminal's 18 px scalar rows are back: the fader and detent switch keep hardware semantics without buying height. Only the scope (4 rows) and color (2) spend more."),
            ("EPAINT COST", "Glyphs almost everywhere — the scope adds two rects and ~25 text sprites for the beam. No polylines at all."),
            ("MIDI", "Faders stay absolute, so soft pickup survives the diet: a hollow shaded cap marks the incoming-CC cell. Learn mode and the <cc74> tag are Terminal's, unchanged."),
            ("THEME", "Same Everforest HSL anchors and hue bar as Hybrid (everforest.rs) — Terminal, Phosphor, and Hybrid are one palette at three hardware dosages."),
            ("FIT", "The middle detent: pick this if Hybrid's knobs read as furniture but plain Terminal drops the rotary/detent semantics the hands expect."),
        ],
    );
}

/// The app's widget vocabulary in the phosphor idiom: Terminal's buffer
/// rows under the Everforest theme, with the level and spectrum as
/// eighth-block glyph traces.
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

    let rows = 12.0;
    let height = rows * g.rh + 30.0;
    let (frame, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(frame, CornerRadius::same(2), th.bg);
    painter.rect_stroke(frame, CornerRadius::same(2), Stroke::new(1.0, th.frame), StrokeKind::Inside);
    let title = "─ vidiotic · widgets · crt ─";
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

    // Row 0: taps + beat glyphs + phrase strip + bpm.
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
    let beats: String = (0..4).map(|k| if k == beat { '●' } else { '○' }).collect();
    put(ui, &g, body, col + 1.0, 0, &beats, alpha(th.phosphor, 120 + (pulse * 135.0) as u8));
    let pos = util::phrase(time);
    let strip: String = (0..16).map(|k| if k <= pos { '▰' } else { '▱' }).collect();
    put(ui, &g, body, col + 7.0, 0, &strip, th.blue);
    put(ui, &g, body, col + 24.0, 0, "120.0", th.fg);

    // Row 1: sync + cadence as bracket lists.
    put(ui, &g, body, 1.0, 1, "sync", th.dim);
    put(ui, &g, body, 12.0, 1, "[internal] link", th.yellow);
    put(ui, &g, body, 30.0, 1, "next  1 2 [4] 8", th.fg);

    // Row 2: chips as parenthesized tags.
    put(ui, &g, body, 1.0, 2, "tags", th.dim);
    put(ui, &g, body, 12.0, 2, "(cue: intro)", th.dim);
    put(ui, &g, body, 26.0, 2, "(2 peers)", th.green);
    put(ui, &g, body, 37.0, 2, "(audio! ✕)", th.red);

    // Rows 3–8: clip tiles as bordered cells, block raster following the hue.
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

    // Rows 9–10: level + spectrum as eighth-block traces.
    let meter_row = Rect::from_min_size(Pos2::new(body.min.x, body.min.y + 8.6 * g.rh), vec2(body.width(), g.rh));
    put(ui, &g, meter_row, 1.0, 0, "level", th.dim);
    glyph_wave(ui, &g, &th, meter_row, 12.0, time);
    let fft_row = Rect::from_min_size(Pos2::new(body.min.x, body.min.y + 9.8 * g.rh), vec2(body.width(), g.rh));
    put(ui, &g, fft_row, 1.0, 0, "fft", th.dim);
    glyph_fft(ui, &g, &th, fft_row, 12.0, time);

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
        &[("WIDGETS", "Terminal's buffer rows under the Everforest theme; hardware survives only where it fits a cell — a fader cap, shading-block phosphor decay, eighth-block level and spectrum traces.")],
    );
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
            let bipolar = *min < 0.0 && *max > 0.0;
            let ghost = midi.binding(i).and_then(|_| midi.incoming_pos(i, time));
            fader(ui, g, th, row, CTL, i, *min, *max, v, bipolar, ghost);
            put(ui, g, row, CTL + 24.0, 0, &format!("{v:>7.2}"), th.fg);
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
            detent_switch(ui, g, th, row, CTL, i, labels.len(), sel);
            let mut col = CTL + (labels.len() - 1) as f32 * 2.0 + 3.0;
            for (k, lab) in labels.iter().enumerate() {
                let selected = k == *sel;
                let text = if selected { format!("[{lab}]") } else { format!(" {lab} ") };
                let color = if selected { th.yellow } else { th.dim };
                let r = put(ui, g, row, col, 0, &text, color);
                let resp = ui.interact(r, ui.id().with(("long", i, k)), Sense::click());
                if resp.clicked() {
                    *sel = k;
                }
                col += text.chars().count() as f32 + 1.0;
            }
            put(ui, g, row, col + 1.0, 0, &format!("= {}", values[*sel]), th.dim);
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
            glyph_scope(ui, g, th, row, i, CTL, min, max, p);
            put(ui, g, row, CTL + 29.0, 1, &format!("x {:>5.2}", p[0]), th.fg);
            put(ui, g, row, CTL + 29.0, 2, &format!("y {:>5.2}", p[1]), th.fg);
        }
        (MockKind::Color { .. }, Value::Color(c)) => {
            let color = Color32::from(*c);
            let [r8, g8, b8, _] = color.to_array();
            put(ui, g, row, CTL, 0, "███", color);
            put(ui, g, row, CTL + 4.0, 0, &format!("#{r8:02x}{g8:02x}{b8:02x}"), th.fg);
            let mut col = CTL;
            for (k, ch_label) in ["h", "s", "v"].iter().enumerate() {
                let ch = match k {
                    0 => &mut c.h,
                    1 => &mut c.s,
                    _ => &mut c.v,
                };
                put(ui, g, row, col, 1, ch_label, th.dim);
                minibar(ui, g, th, row, col + 1.0, 1, i, k, ch);
                col += 9.0;
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
            glyph_wave(ui, g, th, row, CTL, time);
        }
        (MockKind::AudioFft, _) => {
            if nerd {
                put(ui, g, row, CTL - 2.5, 0, nf::check(nf::FFT), th.dim);
            }
            glyph_fft(ui, g, th, row, CTL, time);
        }
        _ => {}
    }
}

/// Fader: a solid cap sliding a tick-marked glyph track, one row tall.
/// Click or drag along the track. A bipolar fader gets a bright center
/// detent; `ghost` paints a hollow cap at the incoming-CC cell for soft
/// pickup.
#[allow(clippy::too_many_arguments)]
fn fader(
    ui: &mut Ui,
    g: &Grid,
    th: &Theme,
    row: Rect,
    col: f32,
    i: usize,
    min: f32,
    max: f32,
    v: &mut f32,
    bipolar: bool,
    ghost: Option<f32>,
) {
    const N: usize = 20;
    let span = Rect::from_min_size(
        Pos2::new(row.min.x + col * g.cw, row.min.y),
        vec2(g.cw * (N as f32 + 2.0), g.rh),
    );
    let resp = ui.interact(span, ui.id().with(("fader", i)), Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - span.min.x - g.cw) / (g.cw * N as f32)).clamp(0.0, 1.0);
            *v = min + t * (max - min);
        }
    }
    let t = (*v - min) / (max - min);
    let mut s = String::with_capacity(N + 2);
    s.push('├');
    for k in 0..N {
        s.push(if k == 5 || k == 10 || k == 15 { '┼' } else { '─' });
    }
    s.push('┤');
    put(ui, g, row, col, 0, &s, alpha(th.dim, 170));
    if bipolar {
        // The rest position between the two center cells.
        put(ui, g, row, col + 1.0 + (N - 1) as f32 * 0.5, 0, "┼", th.fg);
    }
    if let Some(gt) = ghost {
        let k = (gt * (N - 1) as f32).round();
        put(ui, g, row, col + 1.0 + k, 0, "▒", alpha(th.yellow, 180));
    }
    let k = (t * (N - 1) as f32).round();
    put(ui, g, row, col + 1.0 + k, 0, "█", if bipolar { th.magenta } else { th.green });
}

/// Detented slide switch for `long`: a cap snapping between `n` track
/// detents two cells apart. Click or drag along the track (the labels
/// beside it are clickable too).
#[allow(clippy::too_many_arguments)]
fn detent_switch(ui: &mut Ui, g: &Grid, th: &Theme, row: Rect, col: f32, i: usize, n: usize, sel: &mut usize) {
    let track_cols = (n - 1) as f32 * 2.0;
    let span = Rect::from_min_size(
        Pos2::new(row.min.x + col * g.cw, row.min.y),
        vec2(g.cw * (track_cols + 1.0), g.rh),
    );
    let resp = ui.interact(span, ui.id().with(("detent", i)), Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - span.min.x - g.cw * 0.5) / (g.cw * track_cols)).clamp(0.0, 1.0);
            *sel = (t * (n - 1) as f32).round() as usize;
        }
    }
    let mut s = String::new();
    for k in 0..n {
        if k > 0 {
            s.push('─');
        }
        s.push('┼');
    }
    put(ui, g, row, col, 0, &s, alpha(th.dim, 170));
    put(ui, g, row, col + *sel as f32 * 2.0, 0, "█", th.green);
}

/// Oscilloscope XY rasterized to the grid: a dot-glyph graticule with `┼`
/// at the origin, and a beam whose persistence trail is shading blocks
/// quantized to half-cell steps — phosphor decay as a character ramp
/// (`█ → ▓ → ▒ → ░`).
#[allow(clippy::too_many_arguments)]
fn glyph_scope(ui: &mut Ui, g: &Grid, th: &Theme, row: Rect, i: usize, col: f32, min: &[f32; 2], max: &[f32; 2], p: &mut [f32; 2]) {
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

    // Interior-only graticule so nothing straddles the glass edge.
    let grat = alpha(util::hsl(150.0, 0.1, 0.6), 110);
    for gy in 1..4 {
        for gx in 1..8 {
            let pos = Pos2::new(
                screen.min.x + gx as f32 / 8.0 * screen.width(),
                screen.min.y + gy as f32 / 4.0 * screen.height(),
            );
            let ch = if gx == 4 && gy == 2 { "┼" } else { "·" };
            painter.text(pos, Align2::CENTER_CENTER, ch, mono(), grat);
        }
    }

    // Beam position, quantized to half-cell raster steps.
    let tx = (p[0] - min[0]) / (max[0] - min[0]);
    let ty = 1.0 - (p[1] - min[1]) / (max[1] - min[1]);
    let q = |v: f32, origin: f32, step: f32| origin + ((v - origin) / step).round() * step;
    let dot = Pos2::new(
        q(screen.min.x + screen.width() * tx, screen.min.x, g.cw * 0.5),
        q(screen.min.y + screen.height() * ty, screen.min.y, g.rh * 0.5),
    );
    let hist_id = ui.id().with(("crt-hist", i));
    let mut hist: Vec<(f32, f32)> = ui.ctx().data_mut(|d| d.get_temp(hist_id)).unwrap_or_default();
    if hist.last() != Some(&(dot.x, dot.y)) {
        hist.push((dot.x, dot.y));
    }
    if hist.len() > 24 {
        let excess = hist.len() - 24;
        hist.drain(..excess);
    }
    ui.ctx().data_mut(|d| d.insert_temp(hist_id, hist.clone()));
    let sprite = FontId::monospace(10.0);
    let painter = painter.with_clip_rect(screen);
    for (k, &(x, y)) in hist.iter().enumerate() {
        let a = (k + 1) as f32 / hist.len() as f32;
        let ch = if a > 0.66 { "▓" } else if a > 0.33 { "▒" } else { "░" };
        painter.text(Pos2::new(x, y), Align2::CENTER_CENTER, ch, sprite.clone(), alpha(th.phosphor, 25 + (a * 110.0) as u8));
    }
    painter.text(dot, Align2::CENTER_CENTER, "█", sprite, th.phosphor);
}

/// 6-cell mini bar for HSV channels.
#[allow(clippy::too_many_arguments)]
fn minibar(ui: &mut Ui, g: &Grid, th: &Theme, row: Rect, col: f32, line: usize, i: usize, k: usize, ch: &mut f32) {
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
    put(ui, g, row, col, line, &s, th.green);
}

/// Audio as an eighth-block glyph trace in phosphor green, one row tall.
fn glyph_wave(ui: &Ui, g: &Grid, th: &Theme, row: Rect, col: f32, time: f64) {
    const N: usize = 24;
    let mut s = String::with_capacity(N);
    for k in 0..N {
        let a = util::wave(time, k, N) * 0.5 + 0.5;
        s.push(BLOCKS[((a * 7.99) as usize).min(7)]);
    }
    put(ui, g, row, col, 0, &s, th.phosphor);
}

/// Spectrum as per-column eighth blocks, red on clipping bins.
fn glyph_fft(ui: &Ui, g: &Grid, th: &Theme, row: Rect, col: f32, time: f64) {
    const N: usize = 24;
    for k in 0..N {
        let mag = util::fft(time, k, N);
        let ch = BLOCKS[((mag * 7.99) as usize).min(7)];
        let color = if mag > 0.85 { th.red } else { th.green };
        put(ui, g, row, col + k as f32, 0, &ch.to_string(), alpha(color, 120 + (mag * 135.0) as u8));
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
