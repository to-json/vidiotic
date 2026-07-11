//! Visual theme for the control window: the Everforest palette in HSL,
//! generated from a dark/light switch and a global hue rotation (the
//! "phosphor" direction from the ISF aesthetics study). Every `Color32`
//! used under `src/ui/` should come from [`palette`] rather than being
//! constructed ad hoc, so the look stays coherent as panels are added and
//! survives hue rotation.

use std::sync::Mutex;

use egui::{Color32, Context, CornerRadius, FontFamily, FontId, Stroke, TextStyle};

/// Semantic color roles for the control window.
#[derive(Clone, Copy)]
pub struct Palette {
    /// Window/panel fill.
    pub bg_base: Color32,
    /// Side/top panel fill.
    pub bg_panel: Color32,
    /// Hover and popup fill.
    pub bg_elevated: Color32,
    /// Meter wells, text-edit fills, and thumbnail placeholders.
    pub bg_inset: Color32,
    pub fg_primary: Color32,
    pub fg_secondary: Color32,
    pub fg_muted: Color32,
    /// Selection and interactive accent (Everforest yellow).
    pub accent: Color32,
    /// Selection fill — the statusline background.
    pub accent_dim: Color32,
    pub playing: Color32,
    pub armed: Color32,
    pub error: Color32,
    pub border: Color32,
    pub blue: Color32,
    pub magenta: Color32,
    /// Meter/beam green — always the dark-mode anchor, regardless of mode.
    pub phosphor: Color32,
}

/// The theme switchboard: dark/light and a global hue rotation in degrees.
#[derive(Clone, Copy, PartialEq)]
pub struct ThemeState {
    pub dark: bool,
    pub hue: f32,
}

impl Default for ThemeState {
    fn default() -> Self {
        Self { dark: true, hue: 0.0 }
    }
}

/// Everforest (medium) as HSL anchors, rotated by `state.hue` degrees.
pub fn palette_for(state: ThemeState) -> Palette {
    let h = |base: f32| base + state.hue;
    let phosphor = hsl(h(83.0), 0.34, 0.63);
    if state.dark {
        Palette {
            bg_base: hsl(h(206.0), 0.13, 0.20),
            bg_panel: hsl(h(205.0), 0.13, 0.18),
            bg_elevated: hsl(h(199.0), 0.13, 0.24),
            bg_inset: hsl(h(202.0), 0.14, 0.14),
            fg_primary: hsl(h(41.0), 0.32, 0.75),
            fg_secondary: hsl(h(139.0), 0.06, 0.55),
            fg_muted: hsl(h(150.0), 0.06, 0.42),
            accent: hsl(h(40.0), 0.56, 0.68),
            accent_dim: hsl(h(199.0), 0.12, 0.27),
            playing: phosphor,
            armed: hsl(h(24.0), 0.60, 0.67),
            error: hsl(h(359.0), 0.68, 0.70),
            border: hsl(h(201.0), 0.11, 0.31),
            blue: hsl(h(172.0), 0.31, 0.62),
            magenta: hsl(h(332.0), 0.43, 0.72),
            phosphor,
        }
    } else {
        Palette {
            bg_base: hsl(h(44.0), 0.87, 0.94),
            bg_panel: hsl(h(44.0), 0.60, 0.91),
            bg_elevated: hsl(h(43.0), 0.67, 0.92),
            bg_inset: hsl(h(45.0), 0.45, 0.86),
            fg_primary: hsl(h(202.0), 0.11, 0.40),
            fg_secondary: hsl(h(111.0), 0.07, 0.55),
            fg_muted: hsl(h(111.0), 0.06, 0.66),
            accent: hsl(h(43.0), 1.0, 0.44),
            accent_dim: hsl(h(43.0), 0.57, 0.89),
            playing: hsl(h(68.0), 0.99, 0.32),
            armed: hsl(h(24.0), 0.75, 0.45),
            error: hsl(h(1.0), 0.92, 0.60),
            border: hsl(h(55.0), 0.26, 0.78),
            blue: hsl(h(201.0), 0.55, 0.50),
            magenta: hsl(h(319.0), 0.65, 0.64),
            phosphor,
        }
    }
}

/// The palette last applied by [`sync`], readable from anywhere in the UI.
static CURRENT: Mutex<Option<Palette>> = Mutex::new(None);

/// The current frame's palette.
pub fn palette() -> Palette {
    CURRENT
        .lock()
        .unwrap()
        .unwrap_or_else(|| palette_for(ThemeState::default()))
}

/// The theme switchboard state, kept in egui memory (UI-local, not project
/// data).
pub fn state(ctx: &Context) -> ThemeState {
    ctx.data_mut(|d| d.get_temp(egui::Id::new("theme_state")))
        .unwrap_or_default()
}

pub fn set_state(ctx: &Context, st: ThemeState) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("theme_state"), st));
}

/// Grid metrics shared by the glyph widgets: the buffer row height.
pub const ROW: f32 = 18.0;

pub const SP_XS: f32 = 2.0;
pub const SP_SM: f32 = 4.0;
pub const SP_MD: f32 = 8.0;
pub const SP_LG: f32 = 16.0;

/// The buffer font every glyph widget lays out with.
pub fn mono() -> FontId {
    FontId::monospace(12.0)
}

/// A palette (or `Color32::BLACK`/`WHITE`) color at a different alpha, for
/// derived translucent overlays — hover brighten, tile scrims, beat-pulse
/// fades — that aren't a new color, just an existing one made partly
/// transparent.
pub fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// HSL → sRGB. `h` in degrees (wraps), `s`/`l` in `0..=1`. Palettes defined
/// through this stay coherent under global hue rotation.
pub fn hsl(h: f32, s: f32, l: f32) -> Color32 {
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
    Color32::from_rgb(
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}

/// Convert a palette color to a wgpu clear color. The render targets in this
/// app are non-sRGB (see `gfx::WindowSurface::configure`), so a `LoadOp::Clear`
/// writes each channel's `0..1` fraction straight into the framebuffer with no
/// gamma conversion — matching how egui-wgpu paints `Color32` onto the same
/// non-sRGB target.
pub fn wgpu_clear_color(c: Color32) -> wgpu::Color {
    wgpu::Color {
        r: c.r() as f64 / 255.0,
        g: c.g() as f64 / 255.0,
        b: c.b() as f64 / 255.0,
        a: 1.0,
    }
}

/// Apply the theme to `ctx` at construction. Per-frame theme edits (the
/// statusline's dark/light and hue controls) land through [`sync`].
pub fn apply(ctx: &Context) {
    set_state(ctx, ThemeState::default());
    apply_style(ctx, ThemeState::default());
}

/// Re-derive the palette and egui style when the theme state changed since
/// the last frame. Call once per frame, before building the UI.
pub fn sync(ctx: &Context) {
    let st = state(ctx);
    let applied_id = egui::Id::new("theme_applied");
    let applied: Option<ThemeState> = ctx.data_mut(|d| d.get_temp(applied_id));
    if applied != Some(st) {
        apply_style(ctx, st);
        ctx.data_mut(|d| d.insert_temp(applied_id, st));
    }
}

fn apply_style(ctx: &Context, st: ThemeState) {
    let p = palette_for(st);
    *CURRENT.lock().unwrap() = Some(p);

    // The control window follows its own switch, never the OS preference.
    ctx.set_theme(if st.dark { egui::Theme::Dark } else { egui::Theme::Light });

    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.dark_mode = st.dark;
        v.panel_fill = p.bg_panel;
        v.window_fill = p.bg_elevated;
        v.extreme_bg_color = p.bg_inset;
        v.selection.bg_fill = p.accent_dim;
        v.selection.stroke = Stroke::new(1.0, p.accent);
        v.hyperlink_color = p.blue;
        v.error_fg_color = p.error;

        // The buffer aesthetic is square: no rounded corners anywhere.
        let radius = CornerRadius::ZERO;
        v.widgets.noninteractive.corner_radius = radius;
        v.widgets.noninteractive.bg_fill = p.bg_panel;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.fg_primary);

        v.widgets.inactive.corner_radius = radius;
        v.widgets.inactive.weak_bg_fill = p.bg_elevated;
        v.widgets.inactive.bg_fill = p.bg_elevated;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.fg_primary);

        v.widgets.hovered.corner_radius = radius;
        v.widgets.hovered.weak_bg_fill = p.bg_elevated;
        v.widgets.hovered.bg_fill = p.bg_elevated;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.fg_secondary);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.accent);

        v.widgets.active.corner_radius = radius;
        v.widgets.active.weak_bg_fill = p.accent_dim;
        v.widgets.active.bg_fill = p.accent_dim;
        v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
        v.widgets.active.fg_stroke = Stroke::new(1.0, p.fg_primary);

        v.widgets.open.corner_radius = radius;
        v.widgets.open.weak_bg_fill = p.bg_elevated;
        v.widgets.open.bg_fill = p.bg_elevated;
        v.widgets.open.bg_stroke = Stroke::new(1.0, p.border);
        v.widgets.open.fg_stroke = Stroke::new(1.0, p.fg_primary);

        style.spacing.item_spacing = egui::vec2(SP_MD, SP_SM + SP_XS);
        style.spacing.button_padding = egui::vec2(6.0, 3.0);
        style.spacing.interact_size.y = 22.0;

        // One face: everything is buffer text.
        style.text_styles = [
            (TextStyle::Heading, FontId::new(14.0, FontFamily::Monospace)),
            (TextStyle::Body, FontId::new(12.0, FontFamily::Monospace)),
            (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
            (TextStyle::Button, FontId::new(12.0, FontFamily::Monospace)),
            (TextStyle::Small, FontId::new(10.0, FontFamily::Monospace)),
        ]
        .into();

        style.animation_time = 0.12;
    });
}
