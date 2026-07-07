# vidiotic

VJ controller for DJ sets: a video clip layer with an audio-reactive,
live-reloaded fragment shader composited over it, driven by an app-owned beat
clock. Two windows — fullscreen output for the projector, egui control window
on the laptop.

macOS. Built in Rust on wgpu.

## Why did you do this, why are you doing this?

~~Because we hate you~~ Because I've been involved in electronic
musical performance for quite a while, and the music making has gotten
better, and the djing has gotten better, and even the access to space
has gotten better, but cheap vizualization tools are still about as
good as Milkdrop, a thing that is a quarter century old.

## Build

```
brew install ffmpeg pkg-config
cargo build --release
```

## Transcode clips to HAP

HAP-encoded clips play fastest (GPU-native block textures, near-zero CPU
decode, instant loop restart). Stock Homebrew FFmpeg has no HAP encoder, so
vidiotic ships its own:

```
vidiotic transcode input.mp4 clip.mov
```

H.264/HEVC/etc. also play directly via software decode (fine at 1080p; HAP
recommended at 4K).

## Run

```
# single clip, looping
vidiotic run --clip clip.mov --shader shaders/demo.frag

# a directory of clips (double-click into a cue bank in the control window)
vidiotic run --clip-dir ./clips --shader shaders/demo.frag --bpm 128

# follow Ableton Link (rekordbox Performance mode, Ableton Live, etc.)
vidiotic run --clip-dir ./clips --shader shaders/demo.frag --sync link
```

Flags: `--windowed` (don't take over a display), `--monitor <i>`,
`--phrase-len 16|32`, `--audio-device <name substring>`.

## Shaders

Write a fragment shader that composites the clip via `video(uv)`; audio and
beat state come in through uniforms. Existing OpenGL-3.3-style `.frag` files
(Shadertoy-ish) work nearly unchanged — `#version`, known `uniform`
declarations, and `in`/`out` varyings are stripped automatically, and
`freqs1[i]` is rewritten to `fftBand(i)`. `.wgsl` is also accepted.

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

**Shadertoy audio compat**: `iChannel0` is a 512×2 texture, Shadertoy
convention — row 0 (`vec2(x, 0.25)`) is the FFT spectrum (linear frequency,
DC→Nyquist, 0..1), row 1 (`vec2(x, 0.75)`) is the waveform (0..1, silence at
0.5). `fftAt(x)`/`waveAt(x)` are shorthands for reading it. A pasted `uniform
sampler2D iChannel0;` declaration is stripped automatically.

The shader file is watched — save it and the output updates live. A compile
error keeps the last good shader and shows the error in the control window.

**Pin** (control window) freezes the current shader's last good compile into a
pool, so a cue can render with a pinned shader while you keep livecoding the
main one.

### Bundled demo shaders

In `shaders/`, load live from the control window or via `--shader`. All react
to the beat clock even in silence, harder with a loud input.

| shader | vibe |
|---|---|
| `demo.frag` | reference: bass zoom, downbeat flash, spectrum ribbon |
| `chroma-punch.frag` | bass zoom-punch, chromatic aberration, beat shake, scanlines |
| `kaleido.frag` | rotating kaleidoscope; segments step up each phrase |
| `tunnel.frag` | infinite tunnel; depth scrolls on the beat, treble sparkle rings |
| `spectrum-warp.frag` | FFT spectrum ripples and rainbow-stains each row |
| `glitch-vhs.frag` | datamosh/VHS: bass tears rows, RGB split, tracking bar |
| `audio-scope.frag` | Shadertoy `iChannel0` reference: FFT bars + waveform scope |

## Control window

- **BPM** readout + drag, **±0.1%** nudge.
- **DOWNBEAT** (or spacebar) snaps the downbeat to now — phase only, tempo
  unchanged.
- **RESET** hard-resets the grid to bar 1, beat 1, phrase 1; tempo unchanged.
- **TEMPO** (or `b`): tap tempo — 2+ taps sets BPM from the average interval
  (a gap over 2 s starts fresh).
- **Next every** *(bars)*: how often the sequencer advances to the next cue,
  quantized to the beat grid.
- **Loop every** *(bars, or off)*: force the current clip back to its in-point
  on that grid, so it re-loops on a musical boundary regardless of length.
  `off` lets it play through and loop on its own EOF.
- **preserve playhead**: on a cut, carry the playhead into the next clip
  (phase-locked, already running). Off restarts the next clip from its
  in-point. Per-cue override available.
- **Sync**: Internal or **Link** (peer count shown).

### Clips, cues, and banks

The sequencer plays **cues**, not raw clips — a cue is a placement of a source
clip with its own in/out trim and preserve-playhead override.

- **Clips** grid: double-click a clip to add it as a cue to the **edit bank**.
  Gold = has a cue in the live bank, ▶ = playing, ○ = armed.
- **Banks** bar: cues live in banks. `●` marks the **live** bank; click a tab
  to make it the **edit** bank; `▶` sends a bank live at the next phrase; `＋`
  adds a bank.
- **Cue list** (edit bank): click a cue to edit it in the side panel; `✕`
  removes it. 2+ cues round-robin on the **Next every** boundary, pre-arming
  the next a bar early.
- **Cue editor**: **In**/**Out** points (seconds, or `⏺` to snap to the
  playhead), per-cue **preserve** override (Inherit/On/Off), and a **Shader**
  override to render the cue with a pinned pool shader. Trim/preserve apply
  next trigger; shader override applies immediately.
- **Shader…** / **Folder…** pickers; audio input device selector (mic,
  line-in, BlackHole loopback); spectrum meter (**21·log / 512·lin** toggle).

Output-window hotkeys: `t` snap downbeat · `b` tap tempo · `+`/`-` bpm ±1 ·
`[`/`]` nudge ∓/±0.1% · digits then `Enter` set BPM · `f` fullscreen · `Cmd+Q`
quit.

## Sync

Set **Sync** to **Link** to follow Ableton Link (rekordbox Performance mode,
Ableton Live, etc.) — tempo and phase are shared with peers. Otherwise it
runs on internal clock: tap tempo + nudge to match ungeared decks.
