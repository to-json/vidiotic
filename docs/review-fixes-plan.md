# Review-fixes plan

Fixes from a bug/consistency review of the repo. Ordered by priority. Each item
is self-contained: problem, exact location, the change, and how to verify.

Work top-down. After each group, run `cargo check --all-targets` and
`cargo test`. Do **not** refactor beyond what each item states — these are
surgical fixes. `ClipId`, `CueId`, `ShaderId` are all `u32` aliases, so type
mismatches compile silently; respect the intended alias in each edit.

| # | Item | Priority | File |
|---|------|----------|------|
| 1 | `log_bands` usize underflow panic | **P0 — breaking** | analysis.rs |
| 2 | `phrase_len = 0` divide-by-zero | P1 | main.rs, sequencer.rs |
| 3 | `retain_decoders` wrong type annotation | P1 | app.rs |
| 4 | Set-to-playhead uses wrong clip | P1 | app.rs |
| 5 | Out ≤ In: display disagrees with playback | P1 | app.rs |
| 6 | Space-bar downbeat fires while typing | P2 | ui.rs |
| 7 | Decode worker dies on one bad packet | P2 | video/decoder.rs |
| 8 | HapM alpha-format guarded only by `debug_assert` | P2 | video/hap.rs |
| 9 | No "default" audio-device option | P3 | ui.rs |
| 10 | Bank names collide after 26 banks | P3 | app.rs |
| 11 | `memcheck-*` artifacts untracked | P3 | .gitignore |

---

## P0

### 1. `log_bands` usize underflow panic on low-sample-rate devices

**Problem.** `log_bands` (`src/analysis.rs`) clamps the high bin to `FFT_SIZE/2`
but never clamps the low bin. On any capture device below ~28.8 kHz (e.g. a
Bluetooth mic in HFP mode at 8/16 kHz — selectable live via `switch_audio_device`)
some bands come out inverted, e.g. at 16 kHz bands 19–20 are `(1326, 1024)` and
`(1842, 1024)`. The consumer then does `let count = (hi - lo).max(1)` which
**panics on subtraction overflow in debug** (killing the analysis thread, so
audio reactivity freezes for the session) and wraps to a garbage count in release.

**Fix A — `log_bands`, the `bounds[i] = ...` line.** Replace:

```rust
        bounds[i] = (b_lo.max(1), b_hi.max(b_lo + 1).min(FFT_SIZE / 2));
```

with:

```rust
        let half = FFT_SIZE / 2; // usable bins DC..Nyquist
        let b_lo = b_lo.clamp(1, half - 1);
        let b_hi = b_hi.clamp(b_lo + 1, half);
        bounds[i] = (b_lo, b_hi);
```

This guarantees `1 <= b_lo < b_hi <= FFT_SIZE/2` for every band at every sample rate.

**Fix B — defense in depth in the consumer.** In `run`, the band-sum loop, change:

```rust
            let count = (hi - lo).max(1) as f32;
```

to:

```rust
            let count = hi.saturating_sub(lo).max(1) as f32;
```

**Fix C — add a regression test** in `analysis.rs`'s test module (create a
`#[cfg(test)] mod tests` if none exists):

```rust
#[test]
fn log_bands_valid_across_sample_rates() {
    for sr in [8000.0, 16000.0, 22050.0, 24000.0, 32000.0, 44100.0, 48000.0, 96000.0] {
        for (lo, hi) in log_bands(sr) {
            assert!(hi > lo, "sr {sr}: band {lo}..{hi} inverted");
            assert!((1..=FFT_SIZE / 2).contains(&lo));
            assert!(hi <= FFT_SIZE / 2);
        }
    }
}
```

**Verify.** `cargo test log_bands_valid_across_sample_rates` passes.

---

## P1

### 2. `phrase_len = 0` divide-by-zero

**Problem.** `--phrase-len 0` is accepted by clap (`src/main.rs`). The sequencer
then computes `(snap.beat / self.phrase_len).floor()` (`src/sequencer.rs`),
producing inf/NaN and breaking all clip transitions. `SetPhraseLen(0)` (not
reachable from the current UI) has the same effect.

**Fix A — CLI validation (`src/main.rs`, `phrase_len` arg).** Change:

```rust
    #[arg(long, default_value_t = 16)]
    phrase_len: u32,
```

to:

```rust
    #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(u32).range(1..))]
    phrase_len: u32,
```

**Fix B — clamp at the sequencer chokepoints (`src/sequencer.rs`).**
In `Sequencer::new`, store `phrase_len: phrase_len.max(1.0)`. In `set_phrase_len`,
change the first line to `self.phrase_len = beats.max(1) as f64;`.

**Verify.** `cargo run -- run --shader shaders/<any>.frag --clip <any> --phrase-len 0`
starts and transitions normally (clamped to 1) instead of stalling.

### 3. `retain_decoders` wrong type annotation

**Problem.** In `App::retain_decoders` (`src/app.rs`) the keep-set is annotated
`Vec<ClipId>` but actually holds `CueId`s (decoders are keyed by cue). It
compiles only because both alias `u32`; it will silently do the wrong thing if
either becomes a newtype.

**Fix.** Change `let keep: Vec<ClipId> = ...` to `let keep: Vec<CueId> = ...`.
`CueId` is already imported in `app.rs`.

**Verify.** `cargo check`.

### 4. Set-in/out-to-playhead uses the wrong clip's playhead

**Problem.** `current_pts` is the playhead of the **playing** cue (`self.current`),
but `SetCueInToPlayhead` / `SetCueOutToPlayhead` target the **selected** cue in
the edit bank (`src/app.rs`, `apply_command`). Snapping a trim on an idle cue
captures a different clip's timestamp.

**Fix.** Guard both arms so the snap only applies when the target cue is the one
currently playing (the only cue the playhead is meaningful for):

```rust
            Command::SetCueInToPlayhead(id) => {
                if self.current == Some(id) {
                    let p = self.current_pts.max(0.0);
                    self.edit_cue(id, |c| c.in_sec = p);
                }
            }
            Command::SetCueOutToPlayhead(id) => {
                if self.current == Some(id) {
                    let p = self.current_pts.max(0.0);
                    self.edit_cue(id, |c| c.out_sec = Some(p));
                }
            }
```

(Then apply the trim normalization from item 5 — see below.)

**Verify.** `cargo check`. Manual: with a clip playing, select a *different*
idle cue and click the ⏺ buttons; its in/out must not change. Select the playing
cue and the buttons snap to its live playhead.

### 5. Out ≤ In: the editor shows a trim the decoder ignores

**Problem.** `ensure_decoder` (`src/app.rs`) discards an out-point with
`out_sec.filter(|&o| o > in_sec)`, so a cue whose out ≤ in plays to clip end —
but the cue editor still displays the stored out-point. Reachable by dragging Out
below In, or by set-out-to-playhead when the playhead is before the in-point.

**Fix.** Normalize stored trim so it matches the decoder's rule exactly: an
out-point that isn't strictly after the in-point is stored as `None` (untrimmed).
Add a free function to `app.rs`:

```rust
/// Keep stored trim consistent with the decoder's rule (`ensure_decoder` only
/// honors an out-point strictly after the in-point): collapse an out ≤ in to
/// "untrimmed" so the editor never shows a trim that playback ignores.
fn normalize_cue_trim(cue: &mut Cue) {
    if cue.out_sec.is_some_and(|o| o <= cue.in_sec) {
        cue.out_sec = None;
    }
}
```

Then run it after every in/out mutation in `apply_command`. Update these arms:

```rust
            Command::SetCueIn(id, s) => {
                self.edit_cue(id, |c| { c.in_sec = s.max(0.0); normalize_cue_trim(c); })
            }
            Command::SetCueOut(id, s) => {
                self.edit_cue(id, |c| { c.out_sec = s; normalize_cue_trim(c); })
            }
```

and the two `...ToPlayhead` closures from item 4:

```rust
                    self.edit_cue(id, |c| { c.in_sec = p; normalize_cue_trim(c); });
                    // ...
                    self.edit_cue(id, |c| { c.out_sec = Some(p); normalize_cue_trim(c); });
```

**Verify.** `cargo check`. Manual: in the cue editor drag Out below In — the Out
field should snap back to "clip end" rather than showing an inverted range.

---

## P2

### 6. Space-bar downbeat fires while typing in the control window

**Problem.** In `control_ui`'s transport panel (`src/ui.rs`) the DOWNBEAT button
also triggers on `ui.input(|i| i.key_pressed(egui::Key::Space))` with no focus
check, so pressing Space while editing a `DragValue`/text field snaps the downbeat
mid-set.

**Fix.** Gate the Space shortcut on keyboard focus. Change:

```rust
                || ui.input(|i| i.key_pressed(egui::Key::Space))
```

to:

```rust
                || (!ui.ctx().wants_keyboard_input()
                    && ui.input(|i| i.key_pressed(egui::Key::Space)))
```

**Verify.** `cargo check`. Manual: focus the BPM DragValue, press Space — downbeat
must not fire. Click empty canvas, press Space — it fires.

### 7. Decode worker exits permanently on one corrupt packet

**Problem.** In `run_software` (`src/video/decoder.rs`) `decoder.send_packet(&packet)?`
propagates any decode error out of the worker `run`, which logs and exits — the
clip freezes with no respawn. A single malformed packet ends playback for the
whole session.

**Fix.** Make a send-packet failure skip the packet instead of killing the thread.
Change:

```rust
            decoder.send_packet(&packet)?;
```

to:

```rust
            if let Err(e) = decoder.send_packet(&packet) {
                log::warn!("decode send_packet failed, skipping packet: {e}");
                continue;
            }
```

Leave the HAP path as-is (it already `continue`s past unparseable packets).

**Verify.** `cargo check`. (No easy unit test; the change is a localized
error-handling swap.)

### 8. HapM alpha-format check is `debug_assert` only

**Problem.** `decode_frame` (`src/video/hap.rs`) assumes the second texture of a
HapM packet is BC4 via `debug_assert_eq!`. In a release build a non-BC4 alpha
plane flows into `upload_bc` with a wrong-size payload and becomes a wgpu
validation error mid-show.

**Fix.** Add an error variant and return it. In `enum HapErr` add:

```rust
    /// HapM second texture wasn't the expected BC4 alpha plane.
    UnexpectedAlpha,
```

Add its `Display` arm alongside the others:

```rust
            HapErr::UnexpectedAlpha => write!(f, "HapM alpha plane was not BC4"),
```

In `decode_frame`, replace the `debug_assert_eq!(alpha_fmt, HapTextureFormat::Bc4);`
line with a real check:

```rust
        if alpha_fmt != HapTextureFormat::Bc4 {
            return Err(HapErr::UnexpectedAlpha);
        }
```

**Verify.** `cargo test` (existing `hapm_two_textures` still passes — it supplies
a real BC4 plane).

---

## P3 (nice-to-have; skip if time-boxed)

### 9. No path back to the default audio device

**Problem.** The audio combo in `control_ui` (`src/ui.rs`) only ever sends
`SetAudioDevice(Some(name))`; `None` (system default) is unreachable from the UI.
(Separately, device identity is a case-insensitive substring match on the name in
`audio.rs::resolve_device`, and `mirror.audio_devices` is a `(name, name)` pair —
both known fragilities, out of scope for this pass.)

**Fix.** Add a "Default" entry as the first item inside the `.show_ui` closure of
the audio `ComboBox`:

```rust
                    if ui
                        .selectable_label(false, "Default")
                        .on_hover_text("system default input")
                        .clicked()
                    {
                        let _ = tx.send(Command::SetAudioDevice(None));
                    }
```

(The selection highlight won't reflect "is default" since `current_device` always
holds a resolved name — acceptable for this pass.)

**Verify.** `cargo check`; manual: picking "Default" re-resolves to the system
default input.

### 10. Bank names collide after 26 banks

**Problem.** `add_bank` (`src/app.rs`) names banks `(b'A' + len % 26)`, so the
27th bank is a second "A", indistinguishable in the bank bar.

**Fix.** Suffix a number past Z:

```rust
        let n = self.banks.len();
        let name = if n < 26 {
            ((b'A' + n as u8) as char).to_string()
        } else {
            format!("{}{}", (b'A' + (n % 26) as u8) as char, n / 26)
        };
```

**Verify.** `cargo check`.

### 11. `memcheck-*` artifacts untracked at repo root

**Problem.** `memcheck-2026*/` dirs and `memcheck.sh` (leftover from the
occlusion-leak investigation) sit untracked in the repo root.

**Fix (non-destructive).** Append to `.gitignore`:

```
/memcheck-*/
/memcheck.sh
```

Do **not** delete the files — leave that to the user.

**Verify.** `git status` no longer lists the memcheck entries.

---

## Final checks

Run all of:

```
cargo check --all-targets
cargo test
cargo clippy --all-targets   # if available; no new warnings from touched files
```

All existing tests must still pass; item 1 adds one new test.
