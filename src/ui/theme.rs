//! Visual theme for the control window: a fixed dark palette and spacing
//! scale, applied once to egui's style. Every `Color32` used under
//! `src/ui/` should come from [`PALETTE`] rather than being constructed
//! ad hoc, so the look stays coherent as panels are added.

use egui::{Color32, Context, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme};

/// Semantic color roles for the control window.
pub struct Palette {
    /// Window/panel fill.
    pub bg_base: Color32,
    /// Side/top panel fill.
    pub bg_panel: Color32,
    /// Hover and popup fill.
    pub bg_elevated: Color32,
    /// Meter wells and thumbnail placeholders.
    pub bg_inset: Color32,
    pub fg_primary: Color32,
    pub fg_secondary: Color32,
    pub fg_muted: Color32,
    /// Selection and interactive accent.
    pub accent: Color32,
    /// `accent` blended over `bg_panel` at ~25%, precomputed opaque.
    pub accent_dim: Color32,
    pub playing: Color32,
    pub armed: Color32,
    pub error: Color32,
    pub border: Color32,
}

/// The control window's fixed dark palette.
pub const PALETTE: Palette = Palette {
    bg_base: Color32::from_rgb(15, 15, 19),
    bg_panel: Color32::from_rgb(20, 20, 26),
    bg_elevated: Color32::from_rgb(31, 31, 39),
    bg_inset: Color32::from_rgb(9, 9, 12),
    fg_primary: Color32::from_rgb(232, 232, 237),
    fg_secondary: Color32::from_rgb(158, 158, 170),
    fg_muted: Color32::from_rgb(102, 102, 116),
    accent: Color32::from_rgb(82, 191, 255),
    accent_dim: Color32::from_rgb(36, 63, 83),
    playing: Color32::from_rgb(84, 230, 150),
    armed: Color32::from_rgb(255, 187, 74),
    error: Color32::from_rgb(255, 99, 99),
    border: Color32::from_rgb(45, 45, 56),
};

pub const SP_XS: f32 = 2.0;
pub const SP_SM: f32 = 4.0;
pub const SP_MD: f32 = 8.0;
pub const SP_LG: f32 = 16.0;

/// A palette (or `Color32::BLACK`/`WHITE`) color at a different alpha, for
/// derived translucent overlays — hover brighten, tile scrims, beat-pulse
/// fades — that aren't a new color, just an existing one made partly
/// transparent.
pub fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
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

/// Apply the dark theme to `ctx`. Call once, at `EguiCtl` construction.
pub fn apply(ctx: &Context) {
    let p = &PALETTE;

    // The control window never follows the OS light/dark preference.
    ctx.set_theme(Theme::Dark);

    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = p.bg_panel;
        v.window_fill = p.bg_elevated;
        v.extreme_bg_color = p.bg_inset;
        v.selection.bg_fill = p.accent_dim;
        v.selection.stroke = Stroke::new(1.0, p.accent);
        v.hyperlink_color = p.accent;
        v.error_fg_color = p.error;

        let radius = CornerRadius::same(4);
        v.widgets.noninteractive.corner_radius = radius;
        v.widgets.noninteractive.bg_fill = p.bg_panel;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.fg_secondary);

        v.widgets.inactive.corner_radius = radius;
        v.widgets.inactive.weak_bg_fill = p.bg_elevated;
        v.widgets.inactive.bg_fill = p.bg_elevated;
        v.widgets.inactive.bg_stroke = Stroke::NONE;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.fg_primary);

        v.widgets.hovered.corner_radius = radius;
        v.widgets.hovered.weak_bg_fill = p.border;
        v.widgets.hovered.bg_fill = p.border;
        v.widgets.hovered.bg_stroke = Stroke::NONE;
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.fg_primary);

        v.widgets.active.corner_radius = radius;
        v.widgets.active.weak_bg_fill = p.accent_dim;
        v.widgets.active.bg_fill = p.accent_dim;
        v.widgets.active.bg_stroke = Stroke::NONE;
        v.widgets.active.fg_stroke = Stroke::new(1.0, p.fg_primary);

        v.widgets.open.corner_radius = radius;
        v.widgets.open.weak_bg_fill = p.bg_elevated;
        v.widgets.open.bg_fill = p.bg_elevated;
        v.widgets.open.bg_stroke = Stroke::new(1.0, p.border);
        v.widgets.open.fg_stroke = Stroke::new(1.0, p.fg_primary);

        style.spacing.item_spacing = egui::vec2(SP_MD, SP_SM + 2.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;

        style.text_styles = [
            (TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
            (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
        ]
        .into();

        style.animation_time = 0.12;
    });
}
