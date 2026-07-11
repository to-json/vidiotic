//! Mock ISF input schema and shared demo state.
//!
//! Deliberately independent of `vidiotic::isf` (in-flight work): this file
//! mirrors the ISF JSON input types (<https://docs.isf.video/ref_json.html>)
//! with a local enum so the demo never blocks on, or breaks, the real
//! transpiler. One fake shader — "Kaleido Bloom" — declares all nine types
//! with realistic MIN/MAX/DEFAULT/VALUES/LABELS.

use std::collections::HashMap;

use egui::ecolor::Hsva;

/// The nine ISF `INPUTS` types, with the attributes that shape a control.
pub enum MockKind {
    /// Momentary click button.
    Event,
    /// Checkbox / toggle.
    Bool { default: bool },
    /// Pop-up menu over `VALUES` with display `LABELS`.
    Long { values: Vec<i32>, labels: Vec<&'static str>, default: usize },
    /// Slider between `MIN` and `MAX`.
    Float { min: f32, max: f32, default: f32 },
    /// 2D coordinate, both axes in `min..=max`.
    Point2D { min: [f32; 2], max: [f32; 2], default: [f32; 2] },
    /// RGBA color (stored HSV so per-channel controls stay stable).
    Color { default: Hsva },
    /// Image stream input (source routing, not a knob).
    Image,
    /// Audio waveform stream.
    Audio,
    /// Audio FFT stream.
    AudioFft,
}

/// One declared shader input.
pub struct MockInput {
    /// Uniform name, as the JSON `NAME` attribute.
    pub name: &'static str,
    /// Human label, as the JSON `LABEL` attribute.
    pub label: &'static str,
    pub kind: MockKind,
}

/// Live value for one input, parallel to the schema vec.
#[derive(Clone, Copy)]
pub enum Value {
    /// Image/audio inputs carry no scalar value.
    None,
    Bool(bool),
    /// Index into `Long::values`.
    LongIdx(usize),
    Float(f32),
    Point([f32; 2]),
    Color(Hsva),
    /// Time the event last fired, for flash decay. Negative = never.
    EventAt(f64),
}

/// The fake shader: every ISF input type at realistic ranges.
pub fn kaleido_bloom() -> Vec<MockInput> {
    use MockKind as K;
    vec![
        MockInput { name: "inputImage", label: "Input", kind: K::Image },
        MockInput { name: "audio", label: "Audio", kind: K::Audio },
        MockInput { name: "audioFFT", label: "Spectrum", kind: K::AudioFft },
        MockInput { name: "burst", label: "Burst", kind: K::Event },
        MockInput { name: "mirror", label: "Mirror", kind: K::Bool { default: true } },
        MockInput {
            name: "sides",
            label: "Sides",
            kind: K::Long {
                values: vec![3, 4, 6, 8, 12],
                labels: vec!["3", "4", "6", "8", "12"],
                default: 2,
            },
        },
        MockInput {
            name: "angle",
            label: "Angle",
            kind: K::Float { min: 0.0, max: 360.0, default: 45.0 },
        },
        MockInput {
            name: "zoom",
            label: "Zoom",
            kind: K::Float { min: 0.25, max: 4.0, default: 1.0 },
        },
        MockInput {
            name: "feedback",
            label: "Feedback",
            kind: K::Float { min: -1.0, max: 1.0, default: 0.0 },
        },
        MockInput {
            name: "center",
            label: "Center",
            kind: K::Point2D { min: [0.0, 0.0], max: [1.0, 1.0], default: [0.5, 0.5] },
        },
        MockInput {
            name: "tint",
            label: "Tint",
            kind: K::Color { default: Hsva::new(0.07, 0.8, 1.0, 1.0) },
        },
    ]
}

/// Build the starting values for a schema.
pub fn default_values(inputs: &[MockInput]) -> Vec<Value> {
    inputs
        .iter()
        .map(|inp| match &inp.kind {
            MockKind::Event => Value::EventAt(-1.0),
            MockKind::Bool { default } => Value::Bool(*default),
            MockKind::Long { default, .. } => Value::LongIdx(*default),
            MockKind::Float { default, .. } => Value::Float(*default),
            MockKind::Point2D { default, .. } => Value::Point(*default),
            MockKind::Color { default } => Value::Color(*default),
            MockKind::Image | MockKind::Audio | MockKind::AudioFft => Value::None,
        })
        .collect()
}

/// Role marker mirroring the app's `ClipRole`, for the widgets view.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MockRole {
    Playing,
    Armed,
    None,
}

/// One fake clip-pool entry for the widgets view's media tiles.
pub struct MockClip {
    pub name: &'static str,
    pub role: MockRole,
    /// Selection ring (cue list: selected for editing).
    pub selected: bool,
    /// Seeds the procedural stand-in thumbnail.
    pub seed: usize,
}

/// The widgets view's clip pool: one tile per interesting state.
pub fn mock_clips() -> Vec<MockClip> {
    vec![
        MockClip { name: "warehouse-loop.mp4", role: MockRole::Playing, selected: false, seed: 1 },
        MockClip { name: "strobe-cuts.mov", role: MockRole::Armed, selected: false, seed: 2 },
        MockClip { name: "ferns-macro.mp4", role: MockRole::None, selected: true, seed: 3 },
        MockClip { name: "vhs-noise.mp4", role: MockRole::None, selected: false, seed: 4 },
    ]
}

/// Whether a binding drives a CC or a note.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BindMode {
    Cc,
    Note,
}

/// One mock MIDI binding: `span` consecutive controller numbers starting at
/// `num` (point2D binds a CC pair, color an HSV triple).
#[derive(Clone, Copy)]
pub struct Binding {
    pub ch: u8,
    pub num: u8,
    pub span: u8,
    pub mode: BindMode,
}

impl Binding {
    /// Compact display form: `CC74`, `CC20+2`, `NT36` (all on `ch`).
    pub fn label(&self) -> String {
        let prefix = match self.mode {
            BindMode::Cc => "CC",
            BindMode::Note => "NT",
        };
        if self.span > 1 {
            format!("{prefix}{}+{}", self.num, self.span - 1)
        } else {
            format!("{prefix}{}", self.num)
        }
    }
}

/// The mock MIDI-map: learn arming plus per-input bindings, keyed by the
/// input's index in the schema — the demo stand-in for the real system's
/// stable `cue/slot/input-name` param address.
pub struct MidiState {
    pub armed: bool,
    next_cc: u8,
    bindings: HashMap<usize, Binding>,
}

impl MidiState {
    /// A couple of pre-bound params so the at-rest affordance shows
    /// immediately: `angle` → CC74, `center` → CC20 pair.
    pub fn prebound(inputs: &[MockInput]) -> Self {
        let mut bindings = HashMap::new();
        for (i, inp) in inputs.iter().enumerate() {
            match inp.name {
                "angle" => {
                    bindings.insert(i, Binding { ch: 1, num: 74, span: 1, mode: BindMode::Cc });
                }
                "center" => {
                    bindings.insert(i, Binding { ch: 1, num: 20, span: 2, mode: BindMode::Cc });
                }
                _ => {}
            }
        }
        Self { armed: false, next_cc: 40, bindings }
    }

    /// Stream inputs aren't bindable; everything with a value is.
    pub fn bindable(kind: &MockKind) -> bool {
        !matches!(kind, MockKind::Image | MockKind::Audio | MockKind::AudioFft)
    }

    pub fn binding(&self, idx: usize) -> Option<Binding> {
        self.bindings.get(&idx).copied()
    }

    /// A learn-mode click on control `idx`: bound → unbind, unbound → bind
    /// the next free CC (events take a note instead).
    pub fn learn_click(&mut self, idx: usize, kind: &MockKind) {
        if self.bindings.remove(&idx).is_some() {
            return;
        }
        let (mode, span): (BindMode, u8) = match kind {
            MockKind::Event => (BindMode::Note, 1),
            MockKind::Point2D { .. } => (BindMode::Cc, 2),
            MockKind::Color { .. } => (BindMode::Cc, 3),
            _ => (BindMode::Cc, 1),
        };
        let num = if mode == BindMode::Note {
            36
        } else {
            let n = self.next_cc;
            self.next_cc += span;
            n
        };
        self.bindings.insert(idx, Binding { ch: 1, num, span, mode });
    }

    /// Fake "incoming CC" activity for a bound control: a short flash every
    /// few seconds, staggered by controller number. `0..=1`, decaying.
    pub fn activity(&self, idx: usize, time: f64) -> f32 {
        let Some(b) = self.bindings.get(&idx) else { return 0.0 };
        let phase = (time * 0.35 + f64::from(b.num) * 0.217).fract() as f32;
        (1.0 - phase * 5.0).max(0.0)
    }

    /// Where a mock incoming absolute CC currently "is" (`0..=1`), for
    /// soft-pickup ghosting on absolute-knob directions.
    pub fn incoming_pos(&self, idx: usize, time: f64) -> Option<f32> {
        let b = self.bindings.get(&idx)?;
        let t = (time * 0.11 + f64::from(b.num) * 0.5).sin() as f32;
        Some(t * 0.5 + 0.5)
    }
}

/// Everything the direction panels read and mutate.
pub struct DemoState {
    pub inputs: Vec<MockInput>,
    pub values: Vec<Value>,
    pub midi: MidiState,
    /// `ui.input(|i| i.time)`, sampled once per frame.
    pub time: f64,
    /// Make Noise panel inversion: false = ink on paper, true = film negative.
    pub ink_film: bool,
    /// Whether a nerd-patched mono loaded, so directions may use icon glyphs.
    pub nerd: bool,
    /// Hybrid theme: Everforest dark (true) or light (false).
    pub dark: bool,
    /// Hybrid theme: global hue rotation in degrees.
    pub hue: f32,
}

impl DemoState {
    pub fn new() -> Self {
        let inputs = kaleido_bloom();
        let values = default_values(&inputs);
        let midi = MidiState::prebound(&inputs);
        Self { inputs, values, midi, time: 0.0, ink_film: false, nerd: false, dark: true, hue: 0.0 }
    }
}
