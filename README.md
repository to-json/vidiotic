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

# a directory of clips to sequence (toggle them active in the control window)
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
| `beat` | continuous beats since the downbeat |
| `bar_phase` | 0..1 across each bar (flash on the downbeat) |
| `phrase_phase` | 0..1 across each phrase |
| `bpm`, `mouse` | current tempo, cursor |

The shader file is watched — save it and the output updates live. A compile
error keeps the last good shader and shows the error in the control window.

## Control window

- **BPM** readout + drag, **±0.1%** nudge, **TAP** (or spacebar) to set the
  downbeat, phrase length **16/32**.
- **Sync**: Internal or **Link** (peer count shown).
- **Clips** grid: click to toggle a clip in/out of the loop rotation. With two or
  more active, the sequencer cuts between them on phrase boundaries, pre-arming
  the next clip a bar early. Markers: gold = active, ▶ = playing, ○ = armed.
- **Shader…** / **Folder…** pickers; audio input device selector (mic, line-in,
  BlackHole loopback); 21-band spectrum.

Output-window hotkeys: `t` tap · `+`/`-` bpm ±1 · `[`/`]` nudge ∓/±0.1% ·
digits then `Enter` set BPM · `f` fullscreen · `Cmd+Q` quit.

## Notes on sync gear

- **rekordbox 6+ Performance mode**, Ableton Live, and many apps speak Ableton
  Link — set Sync to Link and they share tempo/phase.
- A **Pioneer XDJ-RX2** emits no usable sync (no Pro DJ Link, no MIDI clock), so
  on that gear the workflow is internal clock + read the BPM off the screen +
  tap + nudge. The `ClockSource` trait is kept small so Pro DJ Link (real
  CDJs/DJM) can be added later.
