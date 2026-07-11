# FX feature requests against the effect-chain plan

Requests from the effects work (see the VJ-effects survey: the high-value
families are kaleido/mirror, RGB-shift/glitch, tile/pixellate/halftone,
strobe/color utilities — all single-pass — plus trails/feedback and bloom/glow,
which need machinery beyond a single pass). Each request says what the chain
plan must do *now* versus merely *not preclude*.

FR1–FR2 are correctness/format issues that block the plan as written.
FR3–FR5 are cheap structural accommodations. FR6–FR7 are content and roadmap.

## FR1 — Effects must read `prev()`, not `video()` (blocking)

The bundled `.frag`s the plan loads into the registry (`kaleido.frag`,
`glitch-vhs.frag`, `chroma-punch.frag`, `spectrum-warp.frag`) were written as
full compositors: they sample `video()`. Loaded verbatim as chain stages, each
stage re-samples the raw clip and discards the previous stage's output — the
chain composes nothing.

**Ask:**
- Port each registry effect to sample `prev()` as its input. `video()` stays
  available and meaningful (deliberate access to the untouched source, e.g.
  blending original against processed), but an effect's *input* is `prev()`.
- State the convention in the plan and in `preamble.frag`'s comments:
  `prev()` = chain input, `video()` = original decoded source.
- The live shader has the same issue when placed mid-chain. Existing live
  shaders use `video()`; that stays correct when the live slot is first (seed
  pass makes `prev()` == source, but `video()` == source too). Mid-chain, a
  `video()`-based live shader silently bypasses upstream effects. At minimum
  document this; a UI hint on the live slot ("uses video(), ignores upstream")
  can come later.

## FR2 — Chain slots must be structured and name-addressed, not `Vec<ShaderId>` (blocking, format)

The chain gets serialized into `.viproj`, so its shape is a format commitment.
Three problems with bare `Vec<ShaderId>`:

1. **Pool ids don't persist.** Ids are runtime-assigned and pinned shaders'
   sources are never saved; a stored id is meaningless next session. Built-ins
   must be referenced by stable *name*, not by reserved numeric id.
2. **Effects will grow parameters.** Every surveyed VJ effect carries 1–4 knobs
   (amount, divisions, angle, beat-sync rate). v1 ships zero knobs (effects
   self-animate off `Globals`), but if the serialized slot is a bare id, adding
   params later is a format break.
3. **The live slot is data, not a magic number.** Sentinel `0` is fine inside
   the renderer; it should not leak into the file format.

**Ask:**
- Runtime chain entry: a struct, e.g.
  `ChainSlot { shader: SlotRef, /* params later */ }` with
  `enum SlotRef { Live, Pool(ShaderId) }`. Renderer resolution: `Live` → live
  pipeline (fallback passthrough); `Pool(id)` → pool lookup, **skip** the slot
  if the id no longer resolves.
- Serialized form (in `CueSpec`): tagged by kind and by *name* for built-ins,
  e.g. `Live | Builtin("kaleido")`. Pinned shaders are skipped (or warned) at
  save time until pinned sources are persistable.
- Note for the plan's §4: `CueSpec` has **no existing shader field**
  (`project.rs` — `to_cue` hardcodes `shader: None`), so there is no
  back-compat read to write; the field is new.

### FR2a — vidiotic-prep build compatibility

Prep constructs `CueSpec` as a full struct literal (`vidiotic-prep/src/export.rs:19`,
`full_length_cue`). Adding a field breaks prep's build — the plan's "prep is
unaffected" claim holds for render/shader but not for §4.

**Ask:** give `CueSpec` a `Default` impl (or a `CueSpec::full_length(clip, name)`
constructor in `vidiotic::project`) and switch prep to use it, plus
`#[nserde(default)]` on the new field so older `.viproj` files parse. Known and
accepted: prep's retrim path (`reopen_project`) already discards cue banks, so
chains are lossy through a retrim round-trip — no new work, but say so in the
plan.

## FR3 — Leave room for a persistent feedback texture (don't preclude)

Trails / echo / feedback / motion-blur / datamosh-fakes are the single
highest-value effect family in every VJ tool, and all need *last frame's final
output* to survive into this frame. Two design choices in the current plan
would block it if made carelessly:

1. The final stage renders directly into the swapchain `view`, which can't be
   sampled or (typically) copied from. Feedback needs the final composite to
   also exist in a sampleable texture.
2. Ping-pong buffers must persist across frames (the plan's lazy-alloc-on-size-
   change already implies this — keep it; never make them transient).

**Ask (structure only, no feedback feature in v1):**
- Factor `render()` so "last stage → `view`" can later become "last stage →
  persistent target C, trivial present pass C → `view`" without reshaping the
  pass loop.
- Reserve the binding story: the future `feedback()` helper will need one more
  texture binding; keep set=2 as *the per-pass/per-frame texture set* so it
  lands as another binding there rather than a fourth bind group.

## FR4 — Reserve a per-stage params binding (don't preclude)

When knobs arrive (FR2 item 2), each stage needs a small uniform block (a
`vec4[2]` of knobs covers every surveyed effect) bound per pass. That is
naturally set=2 (the only per-pass group).

**Ask:** when defining `bgl_input`, document set=2 as "per-pass inputs" and
keep binding numbers ≥2 free for `stageParams` (and FR3's feedback texture).
Costs nothing now; avoids a layout/preamble migration for every shipped effect
later.

## FR5 — Allow one chain slot to expand to multiple internal passes (don't preclude)

Bloom/glow (near-mandatory for the rave aesthetic) and real blur are separable
multi-pass effects; a single `.frag` can't express them, and v1's single-pass
approximations will eventually disappoint.

**Ask:** in the renderer, resolve a slot to a *list* of pipelines (length 1 for
everything in v1) rather than exactly one, and run the ping-pong per pass, not
per slot. This is a types-level choice inside `render()`/the registry with no
behavior change today, and it's the difference between "add a bloom built-in"
and "refactor the pass loop" later. (Multi-pass built-ins may also want
half-res intermediates — explicitly out of scope, fine.)

## FR6 — v1 built-in set (content request)

Ship the four planned registry effects (per FR1, ported to `prev()`), then
prioritize these next — each is single-pass, parameterless-viable (self-animate
off `Globals` beat/lvl/freqs), and present in effectively every VJ tool:

1. **Pixellate/mosaic** — beat-droppable resolution crush.
2. **Strobe / invert-flash** — beat-gated; the rave staple.
3. **Tile/grid replicate** — pairs with kaleido.
4. **Posterize + hue-rotate** (one "color crush" effect).
5. **Edge-neon** (Sobel + glow-ish tint) — outline aesthetic without real bloom.
6. **Wave warp** — sine UV displacement, audio-reactive amplitude.

Every added `.frag` is covered by `bundled_frag_shaders_compile` for free.

## FR7 — Bake-path alignment (roadmap note)

The plan flags offline baking as the escape hatch for static effects. For prep
to ever bake a chain into a clip it needs (a) name-stable effect references in
`.viproj` — FR2 provides this — and (b) a headless render path. The
`spike_render.rs` offscreen pattern is that path's seed; keep it compiling
against the chain API (the plan already updates it). No further work requested
now; just don't let the spike rot into a video()-only harness — once chains
land, extend it with one 2-stage chain case (also satisfies the plan's
verification item).
