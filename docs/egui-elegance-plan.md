# egui elegance plan

Visual/UX upgrade of the control window. Goal: from "stock egui debug tool" to
a deliberate, performance-focused VJ console. Pure presentation work — the
command flow, engine, and render paths do not change, except for two small
`UiMirror` additions listed in Phase 3.

## Context the executor needs

- App: two windows — fullscreen wgpu output + an egui control window. egui
  **0.35** integrated as a library (not eframe): `EguiCtl` in `src/ui.rs` owns
  the context/state/renderer and paints via egui-wgpu (`EguiCtl::render`).
- All UI lives in `src/ui.rs` (~800 lines). It reads a `UiMirror` snapshot
  (`src/commands.rs:117`) and emits `Command`s over a channel. Keep this
  one-directional flow: **read mirror → emit Command**. Never store app data in
  UI-local state; egui temp memory is only for display-only toggles (see the
  existing `spectrum_linear_view` pattern).
- The engine requests a control-window redraw every tick (`src/ui.rs:138`), so
  the UI repaints continuously. Animations driven off `m.phase` or
  `ctx.animate_bool` need no extra repaint scheduling.
- There is currently **zero theming**: no `set_style`, `set_fonts`, or
  `set_visuals` anywhere. Colors are scattered raw `Color32` values.
- Keyboard: winit-level in `App::handle_key` (`src/app.rs:921`) — `t` downbeat,
  `b` tap tempo, `+`/`-` bpm ±1, `[`/`]` nudge, `f` fullscreen, digits+Enter
  type a BPM. Space is handled egui-side in the transport panel.

## Ground rules

- Verify exact egui **0.35** API names against docs.rs before use — e.g. this
  version uses `CornerRadius` (u8 fields), `Margin` (i8 fields), the unified
  `egui::Panel` that shows into a `Ui`, and `Frame::new` (not `Frame::none`).
  Don't paste pre-0.31 snippets blind.
- No new dependencies without checking egui-0.35 compatibility in the crate's
  Cargo.toml. If an icon/toast crate doesn't support 0.35, use Unicode glyphs /
  custom paint instead. Nothing in this plan *requires* a new crate.
- Do NOT wrap clip/cue tiles in `ui.push_id(clip.id)` / `ui.push_id(cue.id)`.
  Tried in Phase 1: it silently breaks mouse-wheel scrolling in any
  `ScrollArea` + `horizontal_wrapped` combo (confirmed by hand — `cargo
  check`/`clippy` don't catch it). If per-item widget-state stability is
  ever needed, use an explicit `.id_salt()` on the individual widget instead
  of scoping the whole tile, and re-verify scrolling by hand.
- Comment standards: doc comments on all public items; no point-in-time,
  milestone, or provenance comments.
- One commit per phase, imperative summary line, matching repo history style.
- After each phase: `cargo check --all-targets && cargo clippy --all-targets`
  (must be warning-free), then `cargo run` and eyeball the described result.
  The app runs with no args; load a clip folder via "Folder…" to exercise
  tiles/thumbnails.

| Phase | Deliverable | Files |
|-------|-------------|-------|
| 1 | Module split + theme foundation | src/ui/ (new dir), lib.rs |
| 2 | Shared custom widgets | src/ui/widgets.rs |
| 3 | Transport redesign | src/ui/transport.rs, commands.rs, app.rs |
| 4 | Clip pool + cue chips as painted tiles | src/ui/library.rs |
| 5 | Bank bar → underline tabs | src/ui/library.rs |
| 6 | Cue editor polish | src/ui/editor.rs |
| 7 | Bottom panel → status bar + meters | src/ui/status.rs |
| 8 | Polish pass | all of src/ui/ |

---

## Phase 1 — Module split + theme foundation

**Split.** Convert `src/ui.rs` into `src/ui/`:

- `mod.rs` — `EguiCtl`, `control_ui` (now just panel orchestration), `pick_file`.
- `theme.rs` — palette, spacing, typography, `apply(ctx)`.
- `widgets.rs` — shared custom widgets (Phase 2).
- `transport.rs`, `library.rs` (clip pool + banks + cue list), `editor.rs`
  (cue editor), `status.rs` (bottom panel).

`lib.rs`'s `pub mod ui;` is unchanged. This phase moves code verbatim into the
new files (plus visibility/doc-comment fixes); the theme is the only behavior
change.

**Palette** (`theme.rs`). Semantic roles, referenced everywhere; after this
plan no raw `Color32::from_*` may remain outside `theme.rs`:

```rust
pub struct Palette {
    pub bg_base: Color32,      // window/panel fill        — rgb(15, 15, 19)
    pub bg_panel: Color32,     // side/top panels          — rgb(20, 20, 26)
    pub bg_elevated: Color32,  // hover, popups             — rgb(31, 31, 39)
    pub bg_inset: Color32,     // meter wells, thumbnails   — rgb(9, 9, 12)
    pub fg_primary: Color32,   // rgb(232, 232, 237)
    pub fg_secondary: Color32, // rgb(158, 158, 170)
    pub fg_muted: Color32,     // rgb(102, 102, 116)
    pub accent: Color32,       // selection/interactive     — rgb(82, 191, 255)
    pub accent_dim: Color32,   // accent at ~25% over bg (precomputed, opaque)
    pub playing: Color32,      // rgb(84, 230, 150)
    pub armed: Color32,        // rgb(255, 187, 74)
    pub error: Color32,        // rgb(255, 99, 99)
    pub border: Color32,       // rgb(45, 45, 56)
}
pub const PALETTE: Palette = /* const-construct the above */;

pub const SP_XS: f32 = 2.0;
pub const SP_SM: f32 = 4.0;
pub const SP_MD: f32 = 8.0;
pub const SP_LG: f32 = 16.0;
```

The clear color in `EguiCtl::render` (0.02/0.02/0.03) should become the wgpu
equivalent of `bg_base` so panel gaps don't flash a different black.

**`theme::apply(ctx: &egui::Context)`** — called once in `EguiCtl::new`. Set:

- `visuals.panel_fill = bg_panel`, `visuals.window_fill = bg_elevated`,
  `visuals.extreme_bg_color = bg_inset`, `visuals.selection.bg_fill =
  accent_dim`, `visuals.selection.stroke = accent`, `visuals.hyperlink_color =
  accent`, `visuals.error_fg_color = error`.
- `visuals.widgets.{noninteractive,inactive,hovered,active,open}`: fills from
  the bg ramp (inactive `bg_elevated`, hovered one step brighter, active
  `accent_dim`), `fg_stroke` from the fg ramp, corner radius 4 across the
  board, no `bg_stroke` on inactive widgets (borderless buttons; hover state
  carries the affordance).
- `spacing.item_spacing = vec2(SP_MD, SP_SM + 2.0)`, `spacing.button_padding =
  vec2(10.0, 5.0)`, `spacing.interact_size.y = 26.0`.
- Text styles: Heading 17, Body 13, Monospace 13, Button 13, Small 11.
- `style.animation_time = 0.12` (snappy hover fades).

**Typography (optional — skip cleanly if offline).** Create `assets/fonts/`,
bundle Inter (UI text) and JetBrains Mono (numbers/code) via `include_bytes!`
and `ctx.set_fonts` — Inter at proportional index 0, JetBrains Mono at
monospace index 0. Sources: github.com/rsms/inter and
github.com/JetBrains/JetBrainsMono releases (both OFL; commit the .ttf files).
If fonts can't be fetched, do everything else and note the skip in the commit
message body — nothing downstream depends on the fonts.

**Also in this phase:** delete decorative `ui.separator()` calls (keep only
semantic boundaries — e.g. between the pinned-shader list and the spectrum);
replace them with `ui.add_space(SP_MD)`. Replace every magic spacing number
with the SP_* constants.

**Verify:** app launches with the new dark theme; buttons show a hover fade;
no raw colors left outside `theme.rs` (`grep -rn "Color32::from" src/ui/`
shows only theme.rs).

---

## Phase 2 — Shared custom widgets (`widgets.rs`)

These are the building blocks that make the rest look designed instead of
default. All take `&Palette` (or read `theme::PALETTE` directly — pick one and
be consistent).

**`segmented`** — a pill-group replacing rows of `selectable_label`:

```rust
/// One-of-N segmented control. Returns the clicked index, if any.
pub fn segmented(ui: &mut Ui, id: impl std::hash::Hash, labels: &[&str],
                 selected: Option<usize>) -> Option<usize>
```

Custom paint: allocate one rect per label (text width + 2×SP_MD padding,
height 22), flush together (`item_spacing.x = 0`) inside a rounded
`bg_inset` well; selected segment gets an `accent_dim` fill + `fg_primary`
text; others `fg_secondary`, hover → `bg_elevated`. Corner radius only on the
outer ends. This replaces the "Next every" / "Loop every" rows, the sync
picker, and the cue-editor Inherit/On/Off row.

**`section_label`** — `pub fn section_label(ui: &mut Ui, text: &str)`:
uppercase, Small size, `fg_muted`, letter-spaced if cheap (else plain). Used
for "NEXT EVERY", "LOOP EVERY", "PRESERVE PLAYHEAD", "SHADER", etc.

**`chip`** — small rounded pill with optional remove ✕:

```rust
pub struct ChipResponse { pub clicked: bool, pub removed: bool }
pub fn chip(ui: &mut Ui, text: &str, tint: Option<Color32>, removable: bool) -> ChipResponse
```

`bg_elevated` fill (or `tint.linear_multiply(0.15)` with `tint` text), radius
half-height, Small text; the ✕ only paints while the chip is hovered. Used for
pinned shaders, cue metadata badges (trim/keep/fx), and the audio-error badge.

**`media_tile`** — the clip/cue tile, fully painted:

```rust
pub struct TileSpec<'a> {
    pub name: &'a str,
    pub tex: Option<&'a egui::TextureHandle>,
    pub role: ClipRole,
    pub selected: bool,       // accent ring
    pub active: bool,         // in-pool "part of a cue" marker
    pub beat_pulse: f32,      // 0..1, drives the playing-glow decay
    pub size: Vec2,           // 128×86 clip pool, 146×98 cue list
}
pub struct TileResponse { pub clicked: bool, pub double_clicked: bool, pub hovered: bool }
pub fn media_tile(ui: &mut Ui, spec: &TileSpec) -> TileResponse
```

Paint order: rounded-rect clip region (radius 4) → thumbnail image stretched
to fill (or `bg_inset` + a muted "decoding…" label while `tex` is `None`) →
bottom scrim (vertical gradient to ~70% black over the lower 22px; a
`rect_filled` with two stacked translucent rects is fine if a real gradient
mesh is fiddly) → name in Small over the scrim, truncated by width, not by
`ellipsize` char count → role badge top-left: small filled circle + glyph
(`▶` playing, `○` armed) in `playing`/`armed` color with a dark halo →
selection: 2px `accent` stroke; hover: 1px `border` stroke + slight image
brighten (`Image::tint` with a >1 multiplier or a translucent white overlay).

**Playing pulse:** when `role == Playing`, stroke the tile with `playing` at
alpha `(beat_pulse.powi(2) * 160.0) as u8` — the tile visibly breathes on the
beat grid. Callers pass `beat_pulse = 1.0 - (m.phase.fract() as f32)`.

**`transport_button`** — uniform big button for the transport row:

```rust
pub fn transport_button(ui: &mut Ui, label: &str, size: Vec2, flash: f32) -> Response
```

Painted: `bg_elevated` fill, radius 4, Button-size strong text, hover →
brighter fill, press → `accent_dim`; `flash` (0..1) overlays `accent` at
`flash * 120` alpha so taps read as hits (see Phase 3).

**Verify:** unit-test nothing (pure painting); wire one widget (e.g. replace
the "Loop every" row with `segmented`) and confirm it renders and clicks
correctly before building the rest of the phases on top.

---

## Phase 3 — Transport redesign (`transport.rs`)

Layout stays a top panel, restructured into: **[BPM hero] [tap buttons]
[beat/phrase indicator] [sync]** on the first row, cadence controls on the
second, and a phrase-progress strip as the panel's bottom edge.

- **BPM hero.** Keep the 40pt monospace readout. Below it in Small/`fg_muted`:
  the drag-value (styled by the theme) and nudge. While a keyboard BPM entry
  is pending, render the entry string *in place of* the readout in `accent`
  with a trailing `▏` caret and a Small "Enter to set" hint — this state is
  currently invisible to the user.
  - Plumbing: add `pub bpm_entry: Option<String>` to `UiMirror`
    (`commands.rs`), set `self.mirror.bpm_entry = self.bpm_entry.clone();` in
    `build_mirror` (`app.rs:740`). Field-by-field mirror build makes this safe.
- **Tap buttons.** DOWNBEAT / RESET / TEMPO via `transport_button`, uniform
  46px height. On click (or key), set `ctx.animate_bool`-driven flash: store
  a `f32` decay in egui temp memory keyed per button, set to 1.0 on trigger,
  multiply by ~0.85 per frame (continuous repaint makes this trivial). TEMPO
  also flashes on `b`, DOWNBEAT on `t`/Space — trigger the flash where the
  Command is sent egui-side; winit-side keys can skip the flash (not worth
  plumbing).
  Keep the existing Space handling and its
  `egui_wants_keyboard_input` guard.
- **Beat indicator.** Replace the pulse dot: paint `m.quantum as usize` dots
  (typically 4), 8px, in a row. Dot `floor(m.phase)` is lit `accent` with
  brightness `1.0 - fract(phase)` eased (`powi(2)`); others `bg_elevated`.
  Downbeat dot (index 0) uses `playing` instead of `accent` so bar starts
  read at a glance. Keep the `bar N/M` label next to it in Small/`fg_muted`.
- **Phrase progress strip.** 3px full-width strip at the panel's bottom:
  fraction = `(m.bar_in_phrase as f64 * m.quantum + m.phase) / m.phrase_len
  as f64` (phrase_len is in beats), filled `accent_dim` with an `accent` head;
  tick marks at each bar boundary in `border`. This shows time-to-next-cut —
  the single most useful glanceable in the app.
- **Sync.** `segmented(["Internal", "Link"])`. When Link is active and
  `m.peers > 0`, a `chip` showing `{peers} peers` in `playing` tint
  (`m.peers` is currently collected but never displayed).
- **Cadence rows.** `section_label("NEXT EVERY")` + `segmented` over
  `CADENCE_BARS`; `section_label("LOOP EVERY")` + `segmented` over
  `["off", …LOOP_CADENCE labels]`. Keep the existing hover texts on the
  group (attach to the section label). Preserve-playhead becomes a small
  toggle at the row's right end (checkbox is fine once themed; keep the
  hover text).

**Verify:** run; tap `t`/`b`/Space and watch flashes; type digits — pending
entry shows in accent; beat dots track the metronome; phrase strip resets on
each auto-advance; Link peers chip appears when a Link peer joins (or skip if
no peer available — code-review the path instead).

---

## Phase 4 — Clip pool + cue chips (`library.rs`)

- Replace `clip_tile` internals with `media_tile` (size 128×86, name in the
  scrim — drop the label line under the tile; the colored-marker information
  moves into the role badge and the active ring). `active` (in-pool clips
  referenced by cues) renders as a 1px `armed`-tinted stroke, distinct from
  the selection ring. Double-click behavior unchanged.
- Replace `cue_chip` internals with `media_tile` (146×98) plus a metadata
  row *below* the tile: `chip`s for trim (`0:01.20–end`), `keep`/`cut`, and
  `fx` (tinted `accent` when a shader override is set). The remove ✕ is a
  hover-only small button at the tile's top-right (paint inside
  `media_tile`'s hover state? No — keep `media_tile` generic; overlay a small
  ✕ button positioned via `ui.put` over the tile rect when
  `TileResponse::hovered`). Selection = accent ring via `spec.selected`.
- Do not scope each tile in its own `Ui`/id (see the ground rules' scrolling
  warning); if `media_tile` needs a stable per-item id, thread it in as an
  explicit id salt argument instead.
- **Empty states.** No clips loaded: center of the scroll area, `fg_muted`
  two-line prompt ("No clips loaded" / "Pick a folder to fill the pool") with
  a real "Folder…" button — not a bare weak label. Empty bank: same
  treatment with the double-click hint.
- Section headers: "CLIPS" / bank area get `section_label` treatment; the
  clip-dir path stays `fg_muted` Small, truncated from the left (paths
  overflow — `RichText` + `Label::truncate`).
- Beat pulse: pass `beat_pulse` from `m.phase` so the playing tile breathes.

**Verify:** load a clip folder; tiles show thumbnails with scrims and badges;
hover brightens; the playing tile pulses on the beat; double-click adds a cue;
cue chips show metadata chips; ✕ appears only on hover; empty states render
before a folder is picked.

---

## Phase 5 — Bank bar → underline tabs (`library.rs`)

Replace `bank_bar`'s selectable-labels with custom-painted tabs (allocate
text-width+24 × 26 rects, flush): active edit bank = `fg_primary` text + 2px
`accent` underline; inactive = `fg_secondary`, hover fill `bg_elevated`. The
live bank shows a 5px `playing` dot before its name (replaces `●` text). Cue
count as `fg_muted` Small within the tab. The "make live" `▶` paints only on
hover of a non-live tab (same hit-area trick as the tab close button pattern:
a small rect at the tab's right edge). `＋` stays a small button at the end.

**Verify:** switching edit bank moves the underline; hovering a non-live tab
reveals ▶; clicking it moves the live dot at the next phrase.

---

## Phase 6 — Cue editor polish (`editor.rs`)

- Header: cue name strong Body, then `chip`s for role (`playing` tint when
  playing, `armed` when armed, plain when idle) and `#id`. Playhead readout
  moves here: monospace `fg_secondary`, it's about *this* panel's context.
- In/Out rows in a 2-column `Grid` (labels `fg_muted` right-aligned,
  monospace drag-values): the ⏺ set-to-playhead buttons keep their hover
  text; give them `accent` text color so they read as actions.
- `section_label`s for PRESERVE PLAYHEAD and SHADER; preserve row →
  `segmented(["Inherit", "On", "Off"])`.
- Shader combo unchanged functionally; the "no pinned shaders" hint stays
  `fg_muted` Small.
- "Remove cue" → error-styled: `error`-tinted text on `error.linear_multiply
  (0.12)` fill, full width, bottom-anchored
  (`ui.with_layout(Layout::bottom_up(..))`) so destructive is spatially
  isolated from editing controls.
- No-cue-selected empty state: centered `fg_muted` prompt, same voice as
  Phase 4's empty states.

**Verify:** select a cue; trim edits still send commands on change; preserve
segmented reflects and sets state; Remove sits at the panel bottom in red.

---

## Phase 7 — Bottom panel → status bar + meters (`status.rs`)

Restructure into one dense bar plus an optional error drawer:

- **Left cluster:** "Shader…" picker button; current shader name monospace
  (`fg_primary` when compiling clean, `error` when `m.shader_error` is
  Some — the name itself is the status light); 📌 Pin button; pinned shaders
  as removable `chip`s (replaces the label+✕+separator run).
- **Right cluster** (`Layout::right_to_left`): audio device combo; then the
  meters:
  - Spectrum: keep the existing painting logic and the 21/512 toggle,
    recolored — bars `accent`, well `bg_inset`, and the toggle as a `chip`.
  - Overall level: 4px vertical bar next to the spectrum fed by `m.level`
    (collected, currently unused) in `playing` → `armed` → `error` bands.
  - `m.audio_error` (collected, currently **never displayed**): when Some,
    an `error`-tinted chip "audio ⚠" whose hover text is the full error.
- **Error drawer:** when `m.shader_error` is Some, the scroll area appears
  under the bar inside a `Frame` filled `error.linear_multiply(0.08)` with a
  2px `error` left border, monospace `fg_primary` text (red-on-dark full text
  was unreadable; only the accent should be red). Cap height 96 as today.
  Wrap the appearance in `ctx.animate_bool` for a slide-open.

**Verify:** break a shader on disk — name turns red, drawer slides open with
readable text; fix it — drawer closes. Select audio devices; meters move.

---

## Phase 8 — Polish pass

- Sweep all of `src/ui/` for: leftover `ui.separator()` decoration, magic
  spacing numbers, raw `Color32`, char-count `ellipsize` (replace with
  width-based truncation; delete the helper if unused).
- Tooltip audit: every button/toggle has hover text; add the key hint to each
  tooltip that has a binding (`t / Space`, `b`, `[ ]`, `+ −`, `f`) in the
  existing "Key: x" style.
- Hover audit: every interactive element visibly changes on hover.
- Resize the control window narrow: nothing clips or overlaps; the cue-editor
  panel min width still works; transport wraps acceptably (if the tap-button
  row collides with the BPM hero below ~700px, let the cadence rows wrap
  first — `horizontal_wrapped`).
- `cargo clippy --all-targets` warning-free; doc comments on every public
  item in `src/ui/`.
- Update the module doc comment in `src/ui/mod.rs` to describe the new
  layout, and README screenshots if any exist (none currently — skip).

**Verify:** full manual pass — load clips, build a bank, play, switch banks,
trim a cue, pin a shader, break a shader, type a BPM, tap tempo, toggle
fullscreen — everything functions exactly as before this plan, only better
dressed.
