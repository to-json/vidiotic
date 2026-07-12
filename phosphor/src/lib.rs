//! Phosphor: the character-grid "buffer" UI idiom for egui — an Everforest
//! HSL palette with a global hue rotation, monospace glyph widgets (bracket
//! buttons, glyph checkboxes and faders, eighth-block meters), and square
//! corners everywhere. Grew out of the "phosphor" direction of vidiotic's
//! ISF control-aesthetics study (`examples/isf_aesthetics`), then hardened
//! in its control window; extracted here so any egui app can wear it.
//!
//! Wiring: call [`theme::apply`] once at context setup and [`theme::sync`]
//! at the top of every frame; read colors through [`theme::palette`].

pub mod theme;
pub mod widgets;
