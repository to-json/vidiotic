//! Shared Everforest-in-HSL machinery for the terminal-family directions
//! (Phosphor and Hybrid): the semantic palette and its HSL anchors, the
//! dark/light + hue-rotation theme bar, and the character-grid text helpers
//! those directions lay out with. One palette, several hardware dosages.

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, vec2};

use crate::schema::DemoState;
use crate::util;

pub fn mono() -> FontId {
    FontId::monospace(12.0)
}

/// Character-grid metrics for a buffer frame.
pub struct Grid {
    pub cw: f32,
    pub rh: f32,
}

/// The directions' semantic palette, filled from Everforest HSL anchors.
pub struct Theme {
    pub bg: Color32,
    /// Scope glass — always dark.
    pub screen: Color32,
    pub knob: Color32,
    pub frame: Color32,
    pub select: Color32,
    pub fg: Color32,
    pub dim: Color32,
    pub green: Color32,
    pub yellow: Color32,
    pub blue: Color32,
    pub magenta: Color32,
    pub red: Color32,
    /// Scope beam — always the dark-mode green, regardless of mode.
    pub phosphor: Color32,
}

/// Everforest (medium) as HSL anchors, rotated by `hue` degrees.
pub fn theme(dark: bool, hue: f32) -> Theme {
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

/// Paint text at a character column within a row.
pub fn put(ui: &Ui, g: &Grid, row: Rect, col: f32, line: usize, text: &str, color: Color32) -> Rect {
    let pos = Pos2::new(row.min.x + col * g.cw, row.min.y + (line as f32 + 0.5) * g.rh);
    let galley = ui.painter().layout_no_wrap(text.to_string(), mono(), color);
    let size = galley.size();
    ui.painter().galley(pos - vec2(0.0, size.y * 0.5), galley, color);
    Rect::from_min_size(pos - vec2(0.0, g.rh * 0.5), vec2(size.x, g.rh))
}

/// Header row: `[dark]/[light]` selector and the hue-rotation slider, in the
/// buffer's own idiom. Mutations land next frame (the demo repaints
/// continuously).
pub fn theme_bar(ui: &mut Ui, g: &Grid, width: f32, st: &mut DemoState, th: &Theme) {
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
