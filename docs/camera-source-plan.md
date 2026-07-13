# Camera Source — live capture as a pool clip with voluntary delay

Date: 2026-07-13
Depends on: None (adds the second consumer of the relink surface `recanon` will use)

---

## Context

Vidiotic's sources are file-backed clips: a per-cue decoder thread seeks, decodes,
paces, and pushes `DecodedFrame`s over a bounded(3) channel; the app drains
newest-wins and uploads (`app.rs:1034-1054`). A camera is a frame producer with no
timeline: no seek, no EOF, already real-time paced. The render boundary is clean —
`DecodedFrame` is format-agnostic plain data, `upload_frame` tolerates size/format
changes, newest-wins draining is exactly right for live.

The feature: select a capture device (built-in, virtual cams, UVC capture sticks),
use it as a pool source that cues reference like a clip. Camera cues are **exempt
from forward offset** (`in_sec`, `start_nudge`, loop re-seek — all meaningless
without a timeline) but **subject to voluntary delay**: the feed dialed N
seconds/beats late. Delay ships capped at 3s, then optimize, then scale.

**The load-bearing design decision** (from review): capture must NOT live in the
per-cue decoder map. `App::decoders` is keyed by `CueId` and `retain_decoders`
(`app.rs:435-441`) drops handles the moment a cue is neither current nor armed;
arming leads by a fixed 4 beats (`sequencer.rs:66`) — sub-second at high BPM —
which can never warm a 3s buffer. Two cues on one device would double-open it,
and a single delay knob on a shared worker lets the armed cue clobber the playing
cue's delay. All four failures share one fix: a **persistent per-device capture
service** owning **one multi-reader ring**, with delay as a **per-cue read
offset** — peek by timestamp, never pop.

Decisions locked with the user:
- On-air is an explicit per-device toggle; the camera (and privacy light) stays on
  while toggled, independent of cue rotation.
- Delay changes **slew** toward the target by default; a toggle switches to
  quantized re-targeting at loop-grid boundaries.
- Format-version break is acceptable (no external users); gate with a clear error.
- vidiotic-prep skips camera cues for now (see Followups).

---

## Stream 1: Capture spike (riskiest first)

**Problem**: Device enumeration, stable identity, permissions, frame formats, and
teardown cost are unknowns until run.

**File(s)**: `src/video/capture.rs` (new), a spike bin like `src/bin/spike_render.rs`

### 1.1 Hybrid backend spike
Assume the hybrid from the start (review: the ffmpeg-only arm cannot satisfy
stable identity — its avfoundation demuxer selects by index/name and enumeration
is a log parse):
- **Enumeration + identity + permission**: `objc2-av-foundation` —
  `AVCaptureDevice` discovery (built-in, external/UVC, virtual), `uniqueID` +
  `localizedName`, `authorizationStatus`/`requestAccess`.
- **Capture**: ffmpeg `avfoundation` input via the already-linked `ffmpeg-next`,
  opened by index mapped from `uniqueID` at open time.
- **Fallback**: `nokhwa`, or full objc2 capture, if the ffmpeg arm fights.

### 1.2 Exit criteria
- Enumerate built-in camera, OBS virtual cam, and a UVC capture stick; report
  `uniqueID`/name/supported formats (note MJPEG-native devices — see 2.2).
- Open by uid→index mapping; receive timed frames; log native pixel format.
- Measure session teardown latency (`stopRunning` is commonly 50-300ms — this
  number drives 2.4).
- TCC prompt behavior from a bare cargo binary (attributes to the terminal app —
  acceptable until an app bundle exists; document what breaks without it).
- Informational: behavior of double-opening one device.

### 1.3 Decision gate

**Recorded 2026-07-13 (spike run on the dev machine, macOS 15 / ffmpeg 8.0 via
brew / FaceTime HD Camera only — no virtual cam or UVC stick attached).**

- **Backend combo confirmed**: `objc2-av-foundation` 0.3 enumeration + ffmpeg
  `avfoundation` capture. No nokhwa fallback needed.
- **Device identity**: `uniqueID` works (stable UUID). Device selection uses the
  demuxer's `video_device_index` **option** (not the URL), which sidesteps
  ffmpeg's prefix-based name matching and digit-leading-name hazards entirely.
  Index parity holds because `capture::enumerate()` reproduces ffmpeg 8's exact
  discovery array (`[BuiltInWideAngle, DeskView, Continuity, External]`, video
  then muxed). Resolve uid → index against a fresh enumeration at every open.
- **Open must pass explicit `video_size` + `framerate`** from an enumerated
  `DeviceFormat`: the demuxer's NTSC 29.97 default hard-errors on devices
  without a matching frame-rate range. `pixel_format` mapped from the CM
  four-char code (`420v`→`nv12` etc.) avoids the yuv420p→uyvy422 fallback.
- **Native formats**: FaceTime HD is `420v` (NV12) at up to 1920x1080@30.
  AVFoundation decompresses MJPEG-native devices to CV formats itself, so the
  ring only ever sees uncompressed frames — plan step 2.2's "decode MJPEG
  before the ring" is satisfied by the OS.
- **Timing**: open ≈ 1.5 s (session start — confirms the persistent-service
  design; a per-cue open could never work). First frame &lt;1 ms after open.
  Steady 30.00 fps, pts in host-clock microseconds (map pts→wall clock once at
  open). Teardown measured &lt;5 ms on the calling thread — far below the
  feared 50-300 ms, though AVFoundation may finish session shutdown (privacy
  light) asynchronously; detached teardown (2.4) stays as cheap insurance.
- **Double-open of one device works** (two concurrent sessions, both streaming).
  No exclusive-access failure mode to design around.
- **TCC**: running under tmux auto-denies without a prompt (tmux is the
  responsible process and this machine's tmux entry is now *Denied*; flip it in
  System Settings → Privacy & Security → Camera to run vidiotic from tmux).
  From Terminal.app the prompt appears and works. Bare-binary caveat and the
  `NSCameraUseContinuityCameraDeviceType` warning both go away with the app
  bundle followup.
- **Hands-on still needed**: OBS virtual cam (not installed), UVC capture
  stick and Continuity Camera (not attached), privacy-light-off latency.

## Stream 2: CaptureService — per-device worker, multi-reader delay ring

**Problem**: Capture lifetime must be independent of cue rotation; delay must be
per-consumer.

**File(s)**: `src/video/capture.rs`, `src/video/mod.rs`, `src/app.rs` (registry field)

### 2.1 Registry + service
`CaptureRegistry` owned by `App`: `HashMap<DeviceUid, CaptureService>`. A service
= one capture thread + one shared ring, running while the device is on-air.
Not touched by `retain_decoders`.

### 2.2 The ring
- Frames stored in the camera's **native format** with wall-clock timestamps;
  bounded by **time AND bytes** (3s window, byte cap enforced at ship — a 4K60
  BGRA virtual cam is ~6GB/3s uncapped; the cap is a correctness bound, not an
  optimization). Prefer requesting ≤1080p at open where the device allows.
- MJPEG-native devices: decode to NV12 *before* the ring (predictable memory,
  one JPEG decode per captured frame) — revisit only if CPU says otherwise.
- Always retain the full window regardless of current tap delays (a growing
  delay must find old frames — pop-on-emit is wrong). Flush on size/format change.

### 2.3 Taps (per-cue, pull-based)
`CameraTap` = shared handle to the ring + a per-cue effective delay offset. The
app polls it in the frame-drain step: `tap.poll(now)` peeks the frame with
`ts <= now - delay_eff`, converts *that one frame* to RGBA (cached swscale
context), returns a `DecodedFrame`. No channels, no per-tap threads, nothing to
block, armed taps cost nothing. `App::decoders` becomes
`HashMap<CueId, SourceHandle>` with `enum SourceHandle { File(DecodeHandle),
Camera(CameraTap) }` (`DecodeHandle` is private-constructor; the enum is the
integration point, and camera restart/preserve no-ops live in its match arms).

### 2.4 Detached teardown
Toggling off-air moves the session to a reaper thread; nothing joins capture
teardown on the engine tick (file `DecodeHandle::drop` joins in ≤2ms; capture
teardown measured in 1.2 is too slow for the swap boundary).

### 2.5 Delay resolution + slew
Per-cue delay is unit-tagged (seconds | beats). Each tick the app resolves
`target = beats × 60 / live_bpm` (or the literal seconds), **clamped to ring
capacity**, then moves `delay_eff` toward it at a bounded slew rate (constant,
tune by feel; start ~1s per s). A per-cue **quantize toggle** instead applies the
new target exactly at loop-grid boundary crossings (`app.rs:1018-1032` tracker).
BPM drift in beats mode therefore slews or steps musically — never free-jumps.

### 2.6 Demoable milestone
Extend the spike bin: live view and a delayed tap of a hardcoded device through
the real `Renderer` offscreen path. No persistence, no pool UI. This is the
go/no-go artifact.

## Stream 3: Cue semantics + UI

**Problem**: Seek-dependent knobs must degrade honestly; the device and delay
need controls.

**File(s)**: `src/app.rs`, `src/ui/editor.rs`, `src/ui/library.rs`, `src/clippool.rs`

### 3.1 Forward-offset exemption
Camera cues: `in_sec`/`out_sec`/`start_nudge`/`speed_mul`/`bpm_sync` **and
`preserve`** inert + greyed (preserve is dead once restart is a no-op); trim UI
hidden; loop-grid restarts skipped; hard reset = no-op on the tap. `trig_delay`,
`dwell`, effect chain unchanged — they're sequencer-level and source-agnostic.

### 3.2 Pool: cameras section
Enumerated device rows (manual refresh) with the **on-air toggle** and a static
camera-glyph thumbnail; filter camera entries out of `spawn_thumbnailer`'s input.
Cues are created from a device row exactly like from a clip tile.

### 3.3 Delay control
Cue editor: delay fader with a sec/beats unit toggle and the quantize toggle
(2.5), phosphor-styled like the cadence controls. Beats mode shows its clamp when
`beats × 60/bpm` exceeds ring capacity. Range 0-3s until the scale-up pass.

## Stream 4: Model + persistence (last — after camera renders correctly)

**Problem**: Everything assumes a file path; nanoserde makes format changes a
hard break; `ClipSpec.source` is already taken by bake provenance.

**File(s)**: `src/clippool.rs`, `src/project.rs`, `src/main.rs`, `src/app.rs:262`,
`../vidiotic-prep` (guards only)

### 4.1 `ClipSource`
`Clip.source: ClipSource { File(PathBuf), Camera { uid: String, name: String } }`.
`clip_path`/`ensure_decoder` branch on it; camera cues route to the registry.
Update vidiotic-prep call sites to compile (guards, not features).

### 4.2 `.viproj`
- New field `camera: Option<CameraSpec>` on `ClipSpec` (NOT `source` — that's
  bake provenance), `path` empty + ignored when camera. `ClipSpec::from_clip`
  and `relativize`/`absolutize`/`gather` skip camera clips.
- Bump `FORMAT_VERSION`; `migrate()` accepts old files; new files refuse old
  binaries with a clear versioned error (accepted break).

### 4.3 Missing devices ≠ missing files
`resolve()` never path-checks camera clips; after first enumeration, absent uids
mark the clip **missing-device**: load proceeds (no `main.rs` bail), the cue
renders black with a status notice, and relink = pick-a-device UI matching by
name (basename relink and `gather` are file-only).

## Sequence integration

Independent of effect-chain/UI plans. Touches the relink surface `recanon` will
formalize — the missing-device flow (4.3) should be written as the second case of
"clip whose backing store is gone" so recanon inherits it. vidiotic-prep is
compile-guarded in 4.1 but gains no camera features (Followups).

## Staging

1. **Ship**: streams 1-4; 3s byte-capped native-format CPU ring; ≤1080p request.
2. **Optimize**: measure convert/copy; candidates — downscale-before-ring option,
   zero-copy CVPixelBuffer path. (GPU-side texture ring is explicitly a
   *separate multi-week project* — the render path is CPU `write_texture`;
   don't smuggle it in as a task.)
3. **Scale**: raise the delay cap to what measurements afford; revisit beats-range UI.

## Followups (out of scope, recorded)

- **Teach prep about non-clip-reliant cues.** Direction: the user wants preppable
  cue varieties that don't reference a media file (camera today; generative/
  shader-only sources later). Prep currently reconstructs sessions from
  `SpanProvenance` and decodes file paths; a "sourceless cue" concept there is
  its own plan.
- Live thumbnail on the camera pool tile.
- App bundle / Info.plist for proper TCC attribution.

## Risks

- **Backend friction** (uid→index races on hotplug, virtual-cam quirks) —
  spike-first with fallback; hotplug re-maps on refresh only, not continuously.
- **Memory**: byte cap + resolution request bound it; failure mode is shorter
  ring window, never OOM.
- **Slew feel**: the slew constant and quantize interaction need hands-on tuning;
  budget a feel pass in 3.3.
- **Teardown latency** unknown until 1.2; if reaper-thread teardown still
  glitches capture-adjacent state, keep services alive off-air and only pause.
- **Format churn** from virtual cams mid-stream: ring flush + `upload_frame`
  already tolerate it; verify in the spike.
