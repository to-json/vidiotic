# vidiotic

A VJ controller for DJ sets: a video clip layer with an audio-reactive,
live-reloaded fragment shader composited over it, driven by an app-owned beat
clock. Two windows — a fullscreen output for the projector and an egui control
window on the laptop.

macOS / Apple silicon. Built in Rust on wgpu.

## Build

Needs FFmpeg dev libraries for decode/mux:

```
brew install ffmpeg pkg-config
cargo build --release
```

## Clips: transcode to HAP first

Playback is fastest on HAP-encoded clips (GPU-native block textures, near-zero
CPU decode, instant loop restart). A stock Homebrew FFmpeg has no HAP encoder,
so vidiotic ships its own:

```
vidiotic transcode input.mp4 clip.mov          # -> Hap1 .mov
```

H.264/HEVC/etc. also play directly via software decode (fine at 1080p; HAP is
recommended, especially at 4K).

## Run

```
# single clip, looping
vidiotic run --clip clip.mov --shader shaders/demo.frag

# a directory of clips (double-click them into a cue bank in the control window)
vidiotic run --clip-dir ./clips --shader shaders/demo.frag --bpm 128

# start in Ableton Link mode (follows rekordbox Performance mode, Ableton, etc.)
vidiotic run --clip-dir ./clips --shader shaders/demo.frag --sync link
```

Useful flags: `--windowed` (don't take over a display), `--monitor <i>`,
`--phrase-len 16|32`, `--audio-device <name substring>`.

## Shaders

Write a fragment shader that composites the clip. The clip is available through
`video(uv)`; audio and beat state through uniforms. Existing OpenGL-3.3-style
`.frag` files (e.g. Shadertoy-ish) work nearly unchanged — `#version`, known
`uniform` declarations, and the `in`/`out` varyings are stripped automatically,
and `freqs1[i]` is rewritten to `fftBand(i)`. `.wgsl` is also accepted.

Uniforms in scope:

| name | meaning |
|---|---|
| `video(uv)` | sample the current clip (vec4) |
| `time` / `iTime` | seconds since start |
| `resolution` / `iResolution` | output size |
| `lvl` | overall audio level |
| `fftBand(i)` | one of 21 log-spaced FFT bands (0..20) |
| `iChannel0` | Shadertoy audio texture (see below) |
| `fftAt(x)` / `waveAt(x)` | spectrum / waveform at `x` ∈ 0..1 |
| `beat` | continuous beats since the downbeat |
| `bar_phase` | 0..1 across each bar (flash on the downbeat) |
| `phrase_phase` | 0..1 across each phrase |
| `bpm`, `mouse` | current tempo, cursor |

**Shadertoy audio compat**: `iChannel0` is a 512×2 audio texture using the
Shadertoy convention — row 0 (`vec2(x, 0.25)`) is the FFT spectrum (linear
frequency, DC→Nyquist, normalized 0..1); row 1 (`vec2(x, 0.75)`) is the
waveform (0..1, silence at 0.5). So `texture(iChannel0, vec2(uv.x, 0.25)).x`
reads the spectrum exactly as on shadertoy.com; `fftAt(x)`/`waveAt(x)` are
shorthands. Pasted Shadertoy audio shaders that declare `uniform sampler2D
iChannel0;` have that line stripped automatically. (`fftBand(i)` is the older,
native 21-log-band API and still works — the control-window spectrum uses it.)

The shader file is watched — save it and the output updates live. A compile
error keeps the last good shader and shows the error in the control window.

**Pin** (control window) freezes the current shader's last good compile into a
pool. A cue can then render with a pinned shader as an override (see below), so
you can keep livecoding the main shader while a specific clip runs a fixed look.

### Bundled demo shaders

Point `--shader` at any of these (in `shaders/`), or load them live from the
control window. All react to the beat clock even in silence, and harder with a
loud input.

| shader | vibe |
|---|---|
| `demo.frag` | reference: bass zoom, downbeat flash, spectrum ribbon |
| `chroma-punch.frag` | bass zoom-punch, chromatic aberration, beat shake, scanlines |
| `kaleido.frag` | rotating kaleidoscope; segments step up each phrase |
| `tunnel.frag` | infinite tunnel; depth scrolls on the beat, treble sparkle rings |
| `spectrum-warp.frag` | the FFT spectrum ripples and rainbow-stains each row |
| `glitch-vhs.frag` | datamosh/VHS: bass tears rows, RGB split, tracking bar |
| `audio-scope.frag` | Shadertoy `iChannel0` reference: FFT bars + waveform scope |

## Control window

- **BPM** readout + drag, **±0.1%** nudge.
- **DOWNBEAT** (or spacebar) snaps the downbeat to now — phase only (nearest
  bar), tempo unchanged.
- **RESET** hard-resets the grid to bar 1, beat 1 (phrase 1); tempo unchanged.
- **TEMPO** (or `b`) is a traditional tap tempo: tap it 2+ times and the BPM is
  set from the average interval (taps more than 2 s apart start fresh).
- **Next every** *(bars)*: how often the sequencer advances to the next cue,
  quantized to the beat grid.
- **Loop every** *(bars, or off)*: force the current clip back to its in-point on
  that beat grid, so the video re-loops on a musical boundary regardless of its
  own length. `off` = let the clip play through and loop on its own EOF.
- **preserve playhead**: on a cut, carry the playhead into the next clip (it
  comes in already running, phase-locked to the outgoing clip). Off = the next
  clip restarts from its in-point on the cut. A cue can override this per-cue.
- **Sync**: Internal or **Link** (peer count shown).

### Clips, cues, and banks

The sequencer plays **cues**, not raw clips. A cue is a placement of a source
clip with its own **in/out** trim points and preserve-playhead override.

- **Clips** grid (the source pool): double-click a clip to add it as a cue to the
  **edit bank**. Markers: gold = has a cue in the live bank, ▶ = playing,
  ○ = armed.
- **Banks** bar: cues live in banks. `●` marks the **live** bank (what plays);
  click a bank tab to make it the **edit** bank shown below; `▶` sends a bank
  live (it takes over at the next phrase). `＋` adds a bank. Because live and edit
  are independent, you can build/tweak one bank while another plays.
- **Cue list** (the edit bank): click a cue to edit it in the side panel; `✕`
  removes it. With two or more cues in the live bank, the sequencer round-robins
  them on the **Next every** boundary, pre-arming the next a bar early.
- **Cue editor** (side panel): set **In**/**Out** points (in seconds, or `⏺` to
  snap to the current playhead), the per-cue **preserve** override
  (Inherit / On / Off), and a **Shader** override — render this cue with a pinned
  pool shader instead of the live one. Trim and preserve apply the next time the
  cue is triggered; the shader override applies immediately while it plays.
- **Shader…** / **Folder…** pickers; audio input device selector (mic, line-in,
  BlackHole loopback); spectrum meter with a **21·log / 512·lin** toggle (the
  perceptual `fftBand` view vs. the linear `iChannel0` view).

Output-window hotkeys: `t` snap downbeat · `b` tap tempo · `+`/`-` bpm ±1 ·
`[`/`]` nudge ∓/±0.1% · digits then `Enter` set BPM · `f` fullscreen · `Cmd+Q`
quit.

## Notes on sync gear

- **rekordbox 6+ Performance mode**, Ableton Live, and many apps speak Ableton
  Link — set Sync to Link and they share tempo/phase.
- A **Pioneer XDJ-RX2** emits no usable sync (no Pro DJ Link, no MIDI clock), so
  on that gear the workflow is internal clock + read the BPM off the screen +
  tap + nudge. The `ClockSource` trait is kept small so Pro DJ Link (real
  CDJs/DJM) can be added later.
