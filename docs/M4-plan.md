# vidiotic — M4 plan (deferred polish)

Status: M0–M3 + HAP transcode are done and committed. This file plans the five
remaining "later" items. Each is independent — do them in any order. All plug
into the existing architecture; the hook points are named below so you don't
have to re-derive them.

Current shape to know:
- `commands.rs` — `Command` enum (UI/keys/pickers → engine), `UiMirror` (engine → UI).
- `app.rs` — `App` owns everything; `apply_command()` dispatches; `update()` is the
  per-frame engine tick; `Boot` carries startup config into `App::new`.
- `clock.rs` — `ClockSource` trait; `InternalClock`, `LinkClock`. `App.clock` is
  `Box<dyn ClockSource>`; `set_sync_source()` swaps with continuity.
- `render.rs` — `Renderer` (single composite pass, `Globals` uniform, one video
  texture + bind group); `Globals` mirrors `shaders/preamble.frag` (std140, 144 B).
- `video/decoder.rs` — `DecodeHandle` per clip; `app.rs` keeps `decoders:
  HashMap<ClipId, DecodeHandle>` and `current: Option<ClipId>`.

---

## 1. MIDI controller mapping (midir) — the main one

Goal: map a hardware controller's pads/knobs/faders onto the same `Command`s the
UI emits, so tap/nudge/BPM/clip-select work from the controller. Needs the
specific controller (note/CC numbers) — build the engine generic, then fill a
mapping profile.

Dependency: `midir = "0.10"`.

Architecture:
- New `src/midi.rs`. A thread opens the MIDI input port (`midir::MidiInput`,
  `connect(port, name, callback, ())`). The callback translates raw MIDI
  messages into `Command`s and sends them on the existing `cmd_tx`
  (`crossbeam_channel::Sender<Command>`) — the engine already drains this in
  `update()`, so nothing else changes. This is why `Command` was kept UI-agnostic.
- Message decode: status byte high nibble → 0x9 NoteOn (pads), 0xB CC
  (knobs/faders). Note number / CC number + value → look up in the mapping.

Mapping profile (`MidiMap`): a table loaded from config (see item 2), e.g.
```
note 36            -> TapDownbeat
cc   1  (value)    -> SetBpm(range-mapped 60..180)     # a "tempo" knob
cc   2  relative   -> NudgeBpm(+/-0.001 per tick)      # endless encoder
note 37..52        -> ToggleClipActive(note - 37)      # a pad bank = clip grid
note 60            -> SetPhraseLen(16) / 61 -> 32
note 62            -> SetSyncSource(toggle)
```
Represent as `enum MidiBinding { Tap, NudgeUp, NudgeDown, SetBpmFromCc{min,max},
ToggleClip(ClipId), SetPhrase(u32), ... }` keyed by `(kind, number)`.

Two knob modes to support (make it a per-binding flag):
- **Absolute** CC (0..127) → map to a range (`SetBpm`).
- **Relative/endless** encoder → `NudgeBpm`; most controllers send 1/127 or
  65/63 for +/- — detect both conventions.

MIDI-learn (nice, optional): a UI button "Learn" + a pending `MidiBinding`
target; the next incoming message binds to it. Store learned maps in config.

UI: add a "MIDI" section to `control_ui` — input-port dropdown (list via
`MidiInput::ports()` + `port_name`), connect/disconnect, a learn toggle, and a
small activity indicator. Add `Command::SetMidiPort(Option<String>)` and
`Command::MidiLearn(BindingTarget)`.

Files: new `src/midi.rs`; `commands.rs` (+2 commands, `MidiBinding`); `app.rs`
(hold the midi connection + map, apply the new commands); `ui.rs` (MIDI panel);
`main.rs` (`--midi <port substring>` to auto-connect).

Test: without hardware, unit-test the message→Command decoder with raw byte
slices (like the HAP parser tests). With hardware, twist a knob and watch BPM.

---

## 2. Config persistence (serde + toml)

Goal: remember last clip dir, shader, audio device, BPM, phrase length, sync
source, monitor, MIDI port + learned map.

Dependencies: `serde = { version="1", features=["derive"] }`, `toml = "0.8"`,
`directories = "5"` (for the platform config dir).

`src/config.rs`:
```rust
#[derive(Serialize, Deserialize, Default)]
struct Config {
    clip_dir: Option<PathBuf>, shader: Option<PathBuf>,
    audio_device: Option<String>, bpm: Option<f64>, phrase_len: Option<u32>,
    sync: Option<String>, monitor: Option<usize>,
    midi_port: Option<String>, midi_map: MidiMap,
}
fn path() -> PathBuf  // directories::ProjectDirs::from("", "", "vidiotic").config_dir()/config.toml
fn load() -> Config; fn save(&Config)
```
Load in `main.rs` before building `Boot`; CLI flags override config values.
Save on clean shutdown (add `ApplicationHandler::exiting` → `app.save_config()`)
and after any settings change (debounced, or just on exit). Keep it simple: save
on exit + on `SetClipDir`/`SetShaderPath`/`SetAudioDevice`.

`App` needs the current values it doesn't already track as a unit (most are in
fields or the mirror already) — assemble a `Config` from `App` state in
`save_config()`.

---

## 3. Monitor hot-plug fallback

Goal: if the fullscreen output monitor is unplugged mid-set, fall back to
windowed on the primary instead of a black/stale window.

Hook: `winit` doesn't give a clean monitor-removed event on all platforms, but
you can re-check on relevant events. In `app.rs`:
- On `WindowEvent::Occluded`, on a periodic tick, or on the next
  `ToggleFullscreen`/`SetOutputMonitor`, re-enumerate
  `window.available_monitors()`. If the target index is gone, call
  `set_fullscreen(None)` and set `self.fullscreen = false`,
  `mirror.fullscreen = false`, log it.
- `pick_monitor_from_window` already handles a missing index by returning `None`
  (→ current monitor); make the fullscreen re-apply path detect the None and go
  windowed instead.

Small; mostly defensive. Also handle `SetOutputMonitor(i)` as a proper command
(currently monitor selection is only at startup) — add it to the UI monitor
dropdown and `apply_command`.

---

## 4. MIDI-clock ClockSource (for gear that emits it)

Goal: a third `ClockSource` that follows 24-ppqn MIDI clock (some controllers /
drum machines send it; the XDJ-RX2 does not).

`src/clock/midiclock.rs` (or extend `clock.rs`): `MidiClock` implements
`ClockSource`. It receives MIDI clock bytes (0xF8 tick, 0xFA start, 0xFC stop)
from the MIDI thread (share the port with item 1, or a dedicated clock port).
- Maintain a ring of recent tick timestamps; BPM = 60 / (avg tick interval × 24).
- Smooth heavily (median or EMA over ~24–48 ticks) — MIDI clock is jittery.
- `beat` accumulates 1/24 per tick from a start anchor; phase derived like the
  others. `set_bpm`/`nudge_bpm`/`tap` are no-ops or best-effort (it's a follower);
  advertise `can_set_tempo=false` in `caps()` so the UI greys those controls
  (add that greying to `ui.rs`, reading `mirror` caps — currently the UI always
  shows tempo controls).
- Add `SyncKind::MidiClock` and a picker entry; `set_sync_source` builds it.

This shares the trait cleanly — `App.clock` is already `Box<dyn ClockSource>`.

---

## 5. Crossfade transitions (optional, nicest visually)

Goal: clip changes crossfade over ~1 beat instead of hard-cutting.

The decode architecture already supports two live clips (`decoders` map keeps
current + armed). What's missing is a second video texture + a mix in the shader.

Render changes (`render.rs`):
- Add a second video texture set (`videoTexB`, its own bind group slots) OR a
  second bind group toggled per draw. Add `mix: f32` and a second `videoModeB`
  to `Globals` (grow the std140 layout — bump the struct + `preamble.frag` in
  lockstep; re-assert `size_of` in the test).
- Preamble: `videoB(uv)` helper + expose `mix`. The user's `video(uv)` stays the
  "A" clip; a default compositor does `mix(video(uv), videoB(uv), mix)`. Or make
  `video()` itself blend when `mix > 0` so existing shaders crossfade for free —
  cleaner for users. Decide: auto-blend inside `video()`.
- `Renderer` uploads both current and outgoing frames; `upload_frame` gains a
  slot arg (A/B).

Engine changes (`app.rs`):
- On `SwapTo(next)`, don't drop the old current immediately; keep decoding it as
  "B", set `mix` ramping 0→1 over N beats (drive from `snap.beat`), then drop B
  and reset mix. Track a `Transition { from, to, start_beat, beats }`.
- The sequencer stays as-is (it already emits Arm/Swap); the crossfade is an
  engine-level animation on top of `SwapTo`.

More involved than the others; hard cuts already look fine on the downbeat, so
this is genuinely optional.

---

## Suggested order if you come back to it

1. Config persistence (2) — small, immediately nice, and item 1 wants it for the map.
2. MIDI mapping (1) — the big value; needs your controller's note/CC numbers.
3. Monitor hot-plug (3) — quick defensive polish.
4. Crossfade (5) — visual upgrade when you want it.
5. MIDI-clock (4) — only if you have gear that emits it.
