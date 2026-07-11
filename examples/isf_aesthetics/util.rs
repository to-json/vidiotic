//! Geometry, drag, and signal-simulation helpers shared by every direction.

use egui::{Id, Pos2, Response, Ui, Vec2};

/// Knob sweep in degrees: from lower-left, over the top, to lower-right.
pub const KNOB_SWEEP: f32 = 270.0;
/// Screen angle (y-down) where the sweep starts, in degrees.
pub const KNOB_START: f32 = 135.0;

/// Unit direction of a knob pointer at `t` in `0..=1`.
pub fn knob_dir(t: f32) -> Vec2 {
    Vec2::angled((KNOB_START + t.clamp(0.0, 1.0) * KNOB_SWEEP).to_radians())
}

/// Polyline approximating the knob arc between `t0` and `t1`.
pub fn knob_arc(center: Pos2, radius: f32, t0: f32, t1: f32) -> Vec<Pos2> {
    let n = (((t1 - t0).abs() * 48.0) as usize).max(2);
    (0..=n)
        .map(|i| {
            let t = t0 + (t1 - t0) * (i as f32 / n as f32);
            center + knob_dir(t) * radius
        })
        .collect()
}

/// Vertical-drag editing for a continuous value: a full-range sweep takes
/// `px` pixels of drag. Returns true when the value changed.
pub fn vdrag(resp: &Response, value: &mut f32, min: f32, max: f32, px: f32) -> bool {
    if !resp.dragged() {
        return false;
    }
    let dy = resp.drag_delta().y;
    if dy == 0.0 {
        return false;
    }
    *value = (*value - dy / px * (max - min)).clamp(min, max);
    true
}

/// Quantized vertical drag for detented controls: emits whole steps every
/// `px_per_step` pixels, carrying the fractional remainder across frames in
/// egui temp memory under `id`.
pub fn vdrag_steps(ui: &Ui, id: Id, resp: &Response, px_per_step: f32) -> i32 {
    if !resp.dragged() {
        ui.ctx().data_mut(|d| d.remove::<f32>(id));
        return 0;
    }
    let carry: f32 = ui.ctx().data_mut(|d| d.get_temp(id).unwrap_or(0.0));
    let acc = carry - resp.drag_delta().y / px_per_step;
    let steps = acc.trunc();
    ui.ctx().data_mut(|d| d.insert_temp(id, acc - steps));
    steps as i32
}

/// Synthetic audio sample in `-1..=1` for waveform mocks: a slow chord of
/// sines scanned across `n` display columns.
pub fn wave(time: f64, i: usize, n: usize) -> f32 {
    let x = i as f32 / n as f32;
    let t = time as f32;
    let s = (x * 12.6 + t * 3.1).sin() * 0.55
        + (x * 31.4 + t * 5.7).sin() * 0.3
        + (x * 77.0 + t * 11.3).sin() * 0.15;
    s * (0.55 + 0.45 * (t * 0.7).sin())
}

/// Synthetic FFT magnitude in `0..=1` for bin `i` of `n`: a pink-ish slope
/// with a couple of wandering peaks.
pub fn fft(time: f64, i: usize, n: usize) -> f32 {
    let x = i as f32 / n as f32;
    let t = time as f32;
    let slope = (1.0 - x).powf(1.6) * 0.45;
    let peak = |c: f32, w: f32, a: f32| a * (-((x - c) / w).powi(2)).exp();
    let p1 = peak(0.12 + 0.06 * (t * 0.9).sin(), 0.05, 0.55);
    let p2 = peak(0.45 + 0.2 * (t * 0.37).sin(), 0.08, 0.4);
    let jitter = 0.05 * (x * 173.0 + t * 17.0).sin();
    (slope + p1 + p2 + jitter).clamp(0.0, 1.0)
}

/// Synthetic peak level in `0..=1` for mono meters.
pub fn level(time: f64, seed: f32) -> f32 {
    let t = time as f32 + seed * 10.0;
    (0.55 + 0.3 * (t * 2.3).sin() + 0.15 * (t * 7.1).sin()).clamp(0.0, 1.0)
}

/// Decaying flash for an event fired at `at`: 1 at the instant, gone ~350ms
/// later. `at < 0` means never fired.
pub fn event_flash(time: f64, at: f64) -> f32 {
    if at < 0.0 {
        return 0.0;
    }
    (1.0 - ((time - at) * 3.0) as f32).max(0.0)
}

/// 120-BPM beat clock for the widgets view: (beat index in a 4/4 bar,
/// decaying beat pulse `0..=1`).
pub fn beat(time: f64) -> (usize, f32) {
    let b = time * 2.0;
    ((b as usize) % 4, 1.0 - b.fract() as f32)
}

/// 16-step phrase cursor at the same clock.
pub fn phrase(time: f64) -> usize {
    ((time * 2.0) as usize) % 16
}

/// Click-to-flash: remembers the last click time in temp memory under `id`
/// and returns the decaying flash, so tap buttons read as hits.
pub fn tap_flash(ui: &Ui, id: Id, clicked: bool, time: f64) -> f32 {
    if clicked {
        ui.ctx().data_mut(|d| d.insert_temp(id, time));
    }
    let at: f64 = ui.ctx().data_mut(|d| d.get_temp(id).unwrap_or(-1.0));
    event_flash(time, at)
}

/// Stable pseudo-random in `0..1` per `(seed, k)`, for procedural stand-in
/// thumbnail art.
pub fn hash01(seed: usize, k: usize) -> f32 {
    let mut x = (seed.wrapping_mul(31).wrapping_add(k.wrapping_mul(97))) as u32 ^ 0x9e37_79b9;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    (x % 1000) as f32 / 1000.0
}

/// HSL → sRGB. `h` in degrees (wraps), `s`/`l` in `0..=1`. Palettes defined
/// through this stay coherent under global hue rotation.
pub fn hsl(h: f32, s: f32, l: f32) -> egui::Color32 {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    egui::Color32::from_rgb(
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}

/// The in-demo annotation footer every direction ends with: KEY — text rows
/// in the direction's own muted colors.
pub fn footer(ui: &mut Ui, key_color: egui::Color32, text_color: egui::Color32, lines: &[(&str, &str)]) {
    for (key, text) in lines {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(*key).monospace().size(9.0).color(key_color));
            ui.label(egui::RichText::new(*text).size(11.0).color(text_color));
        });
    }
}
