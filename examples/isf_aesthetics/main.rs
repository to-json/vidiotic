//! Speculative aesthetics research for vidiotic's generated ISF param
//! controls: seven visual directions (Console baseline, Elektron, Moog,
//! Make Noise, Terminal, a Terminal/hardware Hybrid, and Phosphor at the
//! midpoint between those last two), each rendering
//! the full set of ISF input types
//! (<https://docs.isf.video/ref_json.html>) as live epaint widgets over one
//! shared mock state, with MIDI bindability as a first-class affordance.
//!
//! Run with `cargo run --example isf_aesthetics`. This example stands alone:
//! it does not link `vidiotic` (the ISF backend is in-flight work).

mod console;
mod elektron;
mod everforest;
mod hybrid;
mod makenoise;
mod moog;
mod nf;
mod phosphor;
mod schema;
mod terminal;
mod util;

use egui::{Align2, Color32, CornerRadius, FontId, Pos2, Sense, Stroke, StrokeKind, vec2};
use schema::DemoState;

const CHROME_BG: Color32 = Color32::from_rgb(15, 15, 19);
const CHROME_PANEL: Color32 = Color32::from_rgb(20, 20, 26);
const CHROME_FG: Color32 = Color32::from_rgb(232, 232, 237);
const CHROME_DIM: Color32 = Color32::from_rgb(102, 102, 116);
const CHROME_ACCENT: Color32 = Color32::from_rgb(82, 191, 255);
const CHROME_ACCENT_DIM: Color32 = Color32::from_rgb(36, 63, 83);
const LEARN_RED: Color32 = Color32::from_rgb(255, 99, 99);

const DIRECTIONS: [&str; 7] = ["CONSOLE", "ELEKTRON", "MOOG", "MAKE NOISE", "TERMINAL", "PHOSPHOR", "HYBRID"];

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 800.0])
            .with_title("vidiotic · ISF control aesthetics"),
        ..Default::default()
    };
    eframe::run_native(
        "isf_aesthetics",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            let nerd = install_nerd_font(&cc.egui_ctx);
            Ok(Box::new(App::new(nerd)))
        }),
    )
}

const VIEWS: [&str; 2] = ["PARAMS", "WIDGETS"];

struct App {
    st: DemoState,
    direction: usize,
    /// 0 = ISF param controls, 1 = the app's shared widget vocabulary.
    view: usize,
}

impl App {
    /// Optional CLI arg picks the starting direction, by index or prefix
    /// (`cargo run --example isf_aesthetics -- moog`).
    fn new(nerd: bool) -> Self {
        let direction = std::env::args()
            .nth(1)
            .map(|arg| {
                arg.parse::<usize>().unwrap_or_else(|_| {
                    DIRECTIONS
                        .iter()
                        .position(|d| d.to_lowercase().starts_with(&arg.to_lowercase()))
                        .unwrap_or(0)
                })
            })
            .unwrap_or(0)
            .min(DIRECTIONS.len() - 1);
        let mut st = DemoState::new();
        st.nerd = nerd;
        // A `light` arg anywhere starts the Everforest directions in light
        // mode; a `widgets` arg starts on the widgets view.
        st.dark = !std::env::args().any(|a| a.eq_ignore_ascii_case("light"));
        let view = usize::from(std::env::args().any(|a| a.eq_ignore_ascii_case("widgets")));
        Self { st, direction, view }
    }
}

/// Put an installed OFL nerd-patched mono at the front of the monospace
/// family (the demo doesn't redistribute a font — see `licenses/README.md`).
/// Falls back silently to egui's bundled Hack.
fn install_nerd_font(ctx: &egui::Context) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    for name in ["IosevkaNerdFontMono-Regular.ttf", "JetBrainsMonoNerdFontMono-Regular.ttf"] {
        let path = std::path::Path::new(&home).join("Library/Fonts").join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("nerd".into(), egui::FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "nerd".into());
        ctx.set_fonts(fonts);
        return true;
    }
    false
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Meters and mock CC activity animate continuously.
        root.ctx().request_repaint();
        self.st.time = root.ctx().input(|i| i.time);

        egui::Panel::top("bar")
            .frame(egui::Frame::new().fill(CHROME_PANEL).inner_margin(8.0))
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("ISF CONTROL AESTHETICS")
                            .small()
                            .color(CHROME_DIM),
                    );
                    ui.add_space(12.0);
                    tabs(ui, &mut self.direction);
                    ui.add_space(16.0);
                    view_tabs(ui, &mut self.view);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        learn_toggle(ui, &mut self.st.midi.armed, self.st.time);
                    });
                });
            });

        egui::Panel::bottom("hints")
            .frame(egui::Frame::new().fill(CHROME_PANEL).inner_margin(6.0))
            .show(root, |ui| {
                ui.label(
                    egui::RichText::new(
                        "drag knobs/bars vertically or along their track · click LEARN then a \
                         control to (un)bind · bound controls flash on mock CC input · values \
                         are shared across directions",
                    )
                    .small()
                    .color(CHROME_DIM),
                );
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(CHROME_BG).inner_margin(16.0))
            .show(root, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    match (self.view, self.direction) {
                        (0, 0) => console::show(ui, &mut self.st),
                        (0, 1) => elektron::show(ui, &mut self.st),
                        (0, 2) => moog::show(ui, &mut self.st),
                        (0, 3) => makenoise::show(ui, &mut self.st),
                        (0, 4) => terminal::show(ui, &mut self.st),
                        (0, 5) => phosphor::show(ui, &mut self.st),
                        (0, _) => hybrid::show(ui, &mut self.st),
                        (_, 0) => console::show_widgets(ui, &mut self.st),
                        (_, 1) => elektron::show_widgets(ui, &mut self.st),
                        (_, 2) => moog::show_widgets(ui, &mut self.st),
                        (_, 3) => makenoise::show_widgets(ui, &mut self.st),
                        (_, 4) => terminal::show_widgets(ui, &mut self.st),
                        (_, 5) => phosphor::show_widgets(ui, &mut self.st),
                        _ => hybrid::show_widgets(ui, &mut self.st),
                    }
                });
            });
    }
}

/// Direction switcher, in the chrome's own (console) language.
fn tabs(ui: &mut egui::Ui, direction: &mut usize) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, label) in DIRECTIONS.iter().enumerate() {
            let galley = ui.painter().layout_no_wrap(
                (*label).to_string(),
                FontId::proportional(11.0),
                CHROME_FG,
            );
            let (rect, resp) =
                ui.allocate_exact_size(galley.size() + vec2(20.0, 10.0), Sense::click());
            if resp.clicked() {
                *direction = i;
            }
            let selected = *direction == i;
            let last = DIRECTIONS.len() - 1;
            let radius = CornerRadius {
                nw: u8::from(i == 0) * 4,
                sw: u8::from(i == 0) * 4,
                ne: u8::from(i == last) * 4,
                se: u8::from(i == last) * 4,
            };
            let fill = if selected {
                CHROME_ACCENT_DIM
            } else if resp.hovered() {
                Color32::from_rgb(31, 31, 39)
            } else {
                Color32::from_rgb(9, 9, 12)
            };
            ui.painter().rect_filled(rect, radius, fill);
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                *label,
                FontId::proportional(11.0),
                if selected { CHROME_ACCENT } else { CHROME_DIM },
            );
        }
    });
}

/// View switcher: the ISF param research vs the app's widget vocabulary.
fn view_tabs(ui: &mut egui::Ui, view: &mut usize) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, label) in VIEWS.iter().enumerate() {
            let galley = ui.painter().layout_no_wrap(
                (*label).to_string(),
                FontId::proportional(11.0),
                CHROME_FG,
            );
            let (rect, resp) =
                ui.allocate_exact_size(galley.size() + vec2(16.0, 10.0), Sense::click());
            if resp.clicked() {
                *view = i;
            }
            let selected = *view == i;
            let radius = CornerRadius {
                nw: u8::from(i == 0) * 4,
                sw: u8::from(i == 0) * 4,
                ne: u8::from(i == 1) * 4,
                se: u8::from(i == 1) * 4,
            };
            let fill = if selected {
                Color32::from_rgb(46, 40, 22)
            } else if resp.hovered() {
                Color32::from_rgb(31, 31, 39)
            } else {
                Color32::from_rgb(9, 9, 12)
            };
            ui.painter().rect_filled(rect, radius, fill);
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                *label,
                FontId::proportional(11.0),
                if selected { Color32::from_rgb(229, 192, 123) } else { CHROME_DIM },
            );
        }
    });
}

/// Global MIDI-learn arm/disarm, with a status LED.
fn learn_toggle(ui: &mut egui::Ui, armed: &mut bool, time: f64) {
    let (rect, resp) = ui.allocate_exact_size(vec2(96.0, 24.0), Sense::click());
    if resp.clicked() {
        *armed = !*armed;
    }
    let painter = ui.painter();
    let fill = if *armed {
        Color32::from_rgb(58, 26, 26)
    } else if resp.hovered() {
        Color32::from_rgb(31, 31, 39)
    } else {
        Color32::from_rgb(9, 9, 12)
    };
    painter.rect_filled(rect, CornerRadius::same(4), fill);
    if *armed {
        let pulse = 0.5 + 0.5 * ((time * 6.0).sin() as f32);
        painter.rect_stroke(
            rect,
            CornerRadius::same(4),
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 99, 99, 120 + (pulse * 130.0) as u8)),
            StrokeKind::Inside,
        );
    }
    let led = Pos2::new(rect.min.x + 12.0, rect.center().y);
    painter.circle_filled(led, 3.5, if *armed { LEARN_RED } else { CHROME_DIM });
    painter.text(
        Pos2::new(led.x + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        "MIDI LEARN",
        FontId::proportional(10.0),
        if *armed { CHROME_FG } else { CHROME_DIM },
    );
}
