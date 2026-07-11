//! Nerd Font glyph policy: which patched-in icon sets the app may use.
//!
//! With OFL fonts accepted, the constraint moves to the individual glyph
//! sets a Nerd Font aggregates: two are CC BY 4.0 (attribution required)
//! and one is unlicensed, so their codepoint ranges are banned outright.
//! Ranges and licenses per nerd-fonts v3.3.0 (`license-audit.md` and the
//! "Glyph Sets and Code Points" wiki); license texts and the full retained
//! table live in `licenses/` at the repo root. Note the exclusion also
//! applies to font *files*: embedding a stock patched font redistributes
//! CC-BY outlines even if they're never displayed, so production must strip
//! these ranges when preparing the font asset.

/// Codepoint ranges that must not appear in vidiotic text (inclusive).
pub const EXCLUDED: &[(u32, u32, &str)] = &[
    (0xEA60, 0xEC1E, "Codicons — CC BY 4.0"),
    (0xED00, 0xF2FF, "Font Awesome — CC BY 4.0"),
    (0xF300, 0xF381, "Font Logos — unlicensed + trademarked logos"),
];

/// Whether every char in `s` is license-clean; see [`EXCLUDED`].
pub fn allowed(s: &str) -> bool {
    s.chars()
        .all(|c| EXCLUDED.iter().all(|&(lo, hi, _)| !(lo..=hi).contains(&(c as u32))))
}

/// Debug-asserting pass-through: the demo's cheap enforcement point.
pub fn check(s: &str) -> &str {
    debug_assert!(allowed(s), "CC-BY/unlicensed nerd-font glyph in {s:?}");
    s
}

// Glyphs used by the Terminal direction — all Material Design Icons
// (Apache 2.0, U+F0001–U+F1AF0).
pub const IMAGE: &str = "\u{f02e9}"; // nf-md-image
pub const AUDIO: &str = "\u{f147d}"; // nf-md-waveform
pub const FFT: &str = "\u{f0128}"; // nf-md-chart_bar
pub const MIDI: &str = "\u{f08f1}"; // nf-md-midi
pub const FLASH: &str = "\u{f0241}"; // nf-md-flash
