# Third-party font & glyph licenses

Attribution and license texts for the monospace/Nerd-Font stack the
generated-UI work intends to embed. Policy: **OFL / MIT / Apache-2.0 are
acceptable; attribution-required (CC-BY) and unlicensed sets are excluded.**

## The exclusion problem

A stock Nerd-Fonts-patched font *file* contains every aggregated glyph set.
Embedding one redistributes CC-BY outlines even if the app never displays
them — so the excluded ranges must be stripped from the font at build/prep
time (e.g. `pyftsubset --unicodes-file=...`, or running `font-patcher`
without the excluded sets), not merely avoided in code.
`examples/isf_aesthetics/nf.rs` encodes the same ranges for use in code.

Excluded (per nerd-fonts `license-audit.md`, v3.3.0 ranges):

| Set | Codepoints | Why excluded |
|---|---|---|
| Codicons | U+EA60–U+EC1E | CC BY 4.0 (attribution required) |
| Font Awesome | U+ED00–U+F2FF | CC BY 4.0 (attribution required) |
| Font Logos | U+F300–U+F381 | Unlicensed upstream; trademarked brand logos |

## Retained components

| Component | Codepoints | License | File |
|---|---|---|---|
| Iosevka (base font) | — | OFL 1.1 | `iosevka.OFL-1.1.md` |
| Nerd Fonts (patcher/assembly) | — | MIT (combined statement) | `nerd-fonts.combined.md` |
| Seti-UI + Custom | U+E5FA–U+E6B7 | MIT | `seti-ui.MIT.md` |
| Devicons | U+E700–U+E8EF | MIT (README grant) | `devicons.MIT.txt` |
| Material Design Icons | U+F0001–U+F1AF0 | Apache 2.0 (Pictogrammers) | `material-design-icons.Apache-2.0.txt` |
| Weather Icons | U+E300–U+E3E3 | OFL 1.1 (README grant) | `weather-icons.OFL-1.1.txt` |
| Octicons | U+F400–U+F533, U+2665, U+26A1 | MIT | `octicons.MIT.txt` |
| Powerline (+ Extra) | U+E0A0–U+E0D7 | MIT | `powerline-extra-symbols.MIT.txt` |
| IEC Power Symbols | U+23FB–U+23FE, U+2B58 | MIT | `iec-power-symbols.MIT.txt` |
| Pomicons | U+E000–U+E00A | OFL 1.1 | `pomicons.OFL-1.1.txt` |

OFL notes: fonts may be bundled/embedded and sold *with* software, just not
sold standalone; modified versions must drop Reserved Font Names (Nerd Fonts
already renames, e.g. "Iosevka Nerd Font Mono"). Keep these license files in
distributions that embed the font. Apache 2.0 requires retaining the license
text; MIT requires the copyright + permission notice.

The demo (`examples/isf_aesthetics`) does not redistribute any font: it
loads an Iosevka Nerd Font already installed on the machine at runtime, and
falls back to egui's bundled Hack.
