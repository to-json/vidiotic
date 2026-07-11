# Multi-pass shader effect chain

## Context

Today `Renderer::render()` is single-pass: one fragment shader is the whole
compositor, drawn straight to the swapchain (`src/render.rs:576`). A cue can
override the live shader with **one** pinned pool shader
(`Cue::shader: Option<ShaderId>`, `src/bank.rs:48`), and that override is *not*
persisted (`CueSpec` has no shader field — `project.rs:112`, `to_cue` hardcodes
`shader: None` at `project.rs:403`). There is no way to stack effects.

Goal: turn the single override into an ordered **effect chain** — a stack of
fragment shaders where the live livecoded shader can sit anywhere, with reusable
built-in effects (kaleido/RGB-shift/glitch-style `.frag`s that already ship in
`shaders/`) applied before or after it. Each stage reads the previous stage's
output via a new `prev()` preamble helper while still being able to sample the
untouched source via the existing `video()`.

This revision folds in the FX review (`docs/effect-chain-fx-requests.md`, FR1–7).
The high-value VJ-effect families are kaleido/mirror, RGB-shift/glitch,
tile/pixellate/halftone, strobe/color utilities (all single-pass), plus
trails/feedback and bloom/glow (need machinery beyond one pass). v1 ships the
single-pass families; the structure below is chosen so feedback and multi-pass
built-ins are *not precluded*.

Cost note: `../vidiotic-prep` is the offline bake path. Keep the live chain lean;
a static (non-reactive) effect is a candidate to bake into the clip offline
rather than run every frame. This plan does **not** add baking. `vidiotic-prep`
consumes only `vidiotic::project` and `vidiotic::transcode`, so the render/shader
changes don't affect its build — but the `CueSpec` change in §4 does (see FR2a).

## Core conventions

- **`prev()` = chain input** (previous stage's output; == decoded source for the
  bottom stage via the seed pass). **`video()` = original decoded source**,
  available in every stage for deliberate blend-against-original. State this in
  `preamble.frag` comments. (FR1)
- The renderer's chain is a list of **slots**; each slot resolves to a *list* of
  pipelines (length 1 in v1) so a future bloom/blur built-in can expand to
  several internal passes without reshaping the loop. Ping-pong runs **per pass,
  not per slot**. (FR5)
- **set=2 is "per-pass / per-frame inputs"**: the chain-input texture today, with
  binding numbers ≥2 reserved for a future `stageParams` uniform (FR4) and a
  future `feedback()` texture (FR3). No fourth bind group.

## Design

### 1. Rendering engine — `src/render.rs`

Introduce a pass list executed inside `render()`; the app caller
(`render_output`, `src/app.rs:1117`) and the spike (`src/bin/spike_render.rs:74`)
are unchanged in shape because `render()` still ends at the passed `view`.

- **`render()` gains size.** Signature `render(&self, encoder, view, width,
  height)`. `render_output` passes `g.output.config.width/height`; the spike
  passes `W/H`. Needed to size the intermediate buffers.
- **Ping-pong intermediates.** Two offscreen textures (buffers A/B) in
  `Renderer`, `RENDER_ATTACHMENT | TEXTURE_BINDING`, format = `self.color_format`
  (non-sRGB, matches gfx.rs). Lazily (re)allocate when `width/height` changes and
  **persist across frames** — never transient (feedback will depend on this,
  FR3). Descriptor flags per `spike_render.rs:47-60`.
- **Seed pass (non-empty chain).** First pass writes decoded video into buffer A
  via the existing `passthrough` pipeline (`FragColor = video(uv)`), so `prev()`
  at the bottom stage == decoded source. Uniform semantics regardless of length.
- **Effect passes.** Flatten the resolved chain to a pass list (slots → pipeline
  lists, FR5). For each pass: bind set=2 chain-input to the current source
  buffer, render into the other buffer; ping-pong. The **last** pass renders into
  `view`.
  - Factor this so "last pass → `view`" can later become "last pass → persistent
    target C, then a trivial present pass C → `view`" without reshaping the loop
    (FR3). v1 keeps last-pass → `view` directly.
- **Chain resolution.** Replace `active_override: Option<ShaderId>` with
  `active_chain: Vec<ChainSlot>` where
  `struct ChainSlot { shader: SlotRef /*, params later */ }` and
  `enum SlotRef { Live, Pool(ShaderId) }`. Resolution: `Live` → live pipeline
  (fallback `passthrough`); `Pool(id)` → pool lookup, **skip the slot** if the id
  no longer resolves (don't abort the chain). The `Live`/`Pool` sentinel stays
  inside the renderer and must **not** leak into the file format (FR2). Empty
  chain → today's behavior: single live/passthrough pass straight to `view` (fast
  path, no seed, no intermediates — preserves the spike and the common
  livecoding case).
- Rename `set_active_shader(Option<ShaderId>)` →
  `set_active_chain(Vec<ChainSlot>)`. Keep `capture_current` / `pool_view`;
  `remove_pool_shader` should also drop any `Pool(id)` slots referencing it from
  `active_chain`.

### 2. Preamble `prev()` helper — `shaders/preamble.frag` + `src/render.rs`

- Add a third bind group (set=2): `inputTex` + `inputSmp`, and preamble helper
  `vec4 prev(vec2 uv)` sampling it (GL-flip like `video()`, `preamble.frag:36`).
  `video()` unchanged. Comment the `prev()`/`video()` convention here (FR1).
- Add `bgl_input` + extend `pipeline_layout` to
  `[bgl_globals, bgl_video, bgl_input]`. Document set=2 as "per-pass inputs" and
  leave binding numbers ≥2 free for `stageParams` (FR4) and feedback (FR3). The
  set=2 bind group is rebuilt cheaply per pass (its texture view changes each
  pass, so it cannot live in the video bind group).
- WGSL shaders (`compile_wgsl_to_module`, `shader.rs:362`) get the same
  documented set=2 contract (no preamble).
- No `shader.rs` preprocessing change needed — it validates nothing about
  bindings; `prev` is just another preamble identifier. Add `prev`/`inputTex`/
  `inputSmp` to the strip lists (`KNOWN_UNIFORMS`/`KNOWN_SAMPLERS`,
  `shader.rs:16-40`) only if user redeclaration collisions appear.

### 3. Built-in effect registry — `src/render.rs` + `src/app.rs`

Built-in `.frag`s are currently not loaded anywhere; load them into the pool at
startup so they're selectable like pinned shaders, addressed by **stable name**.

- Compile a fixed set of bundled effects via `include_str!` into `PooledShader`
  entries at `Renderer::new`, reusing the existing `compile` path. Each carries a
  stable string name (the serialized handle, per FR2). Built-ins already survive
  compile via `bundled_frag_shaders_compile` (`shader.rs:440`).
- **Port each registry effect to sample `prev()` as its input** (FR1) — the four
  shipped as compositors (`kaleido.frag:32`, `chroma-punch.frag:28-30`,
  `spectrum-warp.frag`, `glitch-vhs.frag`) currently sample `video()` and would
  discard upstream output. Keep `video()` available for deliberate
  source-blending.
- They appear in `pool_view()` alongside pinned shaders, so existing pool
  pickers list them for free (name-tagged so built-ins are distinguishable).

### 4. Per-cue chain (config) — `src/bank.rs`, `src/commands.rs`, `src/app.rs`, `src/project.rs`, `../vidiotic-prep`

Extend the per-cue override to an ordered, structured, name-addressed list.

- `src/bank.rs:48`: `shader: Option<ShaderId>` → `chain: Vec<ChainSlot>` (empty =
  live shader, matching today's `None`). Update `Cue::default` (`bank.rs:82`).
- `src/commands.rs`: replace `SetCueShader(CueId, Option<ShaderId>)` with chain
  ops — `PushCueEffect(CueId, SlotRef)`, `RemoveCueEffect(CueId, usize)`,
  `MoveCueEffect(CueId, usize, usize)`. Update the mirror `CueView`
  (`commands.rs:129,182`) to carry the resolved chain (names for display).
- `src/app.rs:886-894` (step 6b): resolve the playing cue's `chain` to
  `Vec<ChainSlot>` and call `r.set_active_chain(...)`; empty → live shader.
  Handle the new commands in `apply` (`app.rs:784-790`).
- `src/project.rs`: **new** `chain` field on `CueSpec` (`project.rs:115`) — there
  is no old shader field to migrate (the v1-absent comment at `project.rs:112`).
  Serialized slot is tagged by kind and by *name* for built-ins, e.g.
  `enum CueEffectSpec { Live, Builtin(String) }`; `Vec<CueEffectSpec>`. Pinned
  (`Pool`) shaders are **skipped or warned at save** until pinned sources are
  persistable. Add `#[nserde(default)]` so older `.viproj` parse. Update `to_cue`
  (`project.rs:395`) to build the runtime chain (resolving `Builtin(name)` →
  registry id) and the save path to lower runtime slots back to specs.
- **FR2a — prep build compat.** `full_length_cue` (`vidiotic-prep/src/export.rs:19`)
  is a bare `CueSpec { ... }` literal; a new field breaks prep's build. Give
  `CueSpec` a `Default` impl (or a `CueSpec::full_length(clip, name)` constructor
  in `vidiotic::project`) and switch prep to it. Known/accepted: prep's retrim
  path (`reopen_project`) already discards cue banks, so chains are lossy through
  a retrim round-trip — no new work, just documented.

### 5. Minimal UI — `src/ui/`

Reuse the existing per-cue shader picker: render the chain as an ordered list of
slots, each a picker over the pool (built-ins by name + `Live` + pinned), with
add / remove / reorder (◀▶ like `MoveCue`). Do **not** wrap slot rows in
`ui.push_id()` inside scroll areas (kills `ScrollArea` wheel input — known
gotcha). Optional later: a hint on a `Live` slot that isn't first ("uses
`video()`, may ignore upstream", FR1).

## Not-in-v1, not-precluded (structural reservations)

- **Feedback / trails** (FR3): last-pass-to-persistent-target factoring + a
  reserved set=2 `feedback()` binding. Highest-value effect family; v1 leaves the
  door open, ships nothing.
- **Per-stage params** (FR4): a `vec4[2]` `stageParams` uniform at a reserved
  set=2 binding when knobs arrive. v1 effects self-animate off `Globals`.
- **Multi-pass built-ins** (FR5): slot → pipeline-list already in the types;
  bloom/blur (possibly half-res) later.

## v1 built-in set (FR6)

Ship the four planned (ported to `prev()`), then prioritize — each single-pass,
parameterless-viable via `Globals` beat/lvl/freqs:
pixellate/mosaic, strobe/invert-flash, tile/grid replicate, posterize+hue-rotate
("color crush"), edge-neon (Sobel + tint), wave-warp (sine UV displacement).
Every added `.frag` is covered by `bundled_frag_shaders_compile` for free.

## Verification

- `cargo test` — `bundled_frag_shaders_compile` + shader.rs tests still pass; add
  a test that a 2-stage chain (built-in → live) resolves, that `prev()` compiles,
  and that a `.viproj` round-trips a `Builtin(name)` chain.
- `cargo run --bin spike_render` — single-effect fast path still reads back the
  expected center pixel (engine unchanged when chain is empty). Extend it with
  one 2-stage chain case so it can't rot into a `video()`-only harness (FR7).
- `cargo run` (real app): load a clip, build a cue chain `[builtin kaleido] →
  [Live]`, confirm the built-in's output feeds the live shader (live shader does
  `FragColor = prev(uv)`; you see the kaleido). Reorder to `[Live] → [builtin]`
  and confirm the post-effect wraps the live output. Save/reload the `.viproj`;
  confirm the chain survives. Empty chain falls back to the live shader.
- `cargo build` in `../vidiotic-prep` succeeds against the new `CueSpec`
  (`Default`/constructor in place).
