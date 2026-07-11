# vidiotic-recanon — re-canonicalize a clip collection after filesystem moves

## Context

A `.viproj` stores clip paths relative to the project dir (or absolute). When the
underlying clip files are **moved or renamed** on disk, the project no longer
resolves: `project::resolve` flags them as `missing` and the live app refuses to
launch ("missing clip files … pass --relink-root"). Today the only repair is the
`--relink-root` flag, which re-matches missing clips by **basename** under a
directory — fragile against renames and basename collisions, and with no GUI.

Goal: a third sibling app (alongside `vidiotic` + `vidiotic-prep`) that opens a
`.viproj`, finds where its clips went, rewrites the paths, and re-saves — plus
**deduplicates** identical clips in the collection. Both jobs want the same
primitive: a stable, cheap **content fingerprint** per clip, so identity survives
renames and duplicates can be detected without trusting the filename.

This is the app foreshadowed in memory `recanon-sibling`; it extends the
extraction work from the live-app save path (`project::from_runtime`,
`absolutize`/`relativize`).

## Where it lives

New crate `../vidiotic-recanon`, an `eframe`/`egui` app modeled on
`../vidiotic-prep` (same shape: `main.rs` + `app.rs` + `ui.rs`, depends only on
the `vidiotic` lib — never `render`/`shader`). Cargo deps mirror prep minus the
transcode/bake stack: `vidiotic`, `eframe`, `egui`, `rfd`, `anyhow`, `log`,
`env_logger`, `crossbeam-channel`. No `ffmpeg` needed unless we later re-probe
clip metadata (fingerprinting reads raw bytes, not frames).

## Shared-lib changes — `vidiotic/src/project.rs`

The reusable primitives go in the shared module (used by recanon, backfilled by
prep, ignorable by the live app), following the existing `resolve` / `relink_*` /
`absolutize` conventions.

### 1. Fingerprint on the model

- New `ClipFingerprint { size: u64, hash: u64 }` (`SerRon`/`DeRon`), and
  `ClipSpec.fingerprint: Option<ClipFingerprint>` with `#[nserde(default)]` — old
  files and unhashed clips stay `None` and degrade to name matching. No format
  break (additive, defaulted); `FORMAT_VERSION` stays 1.
- `pub fn fingerprint(path: &Path) -> io::Result<ClipFingerprint>` — `size` from
  metadata; `hash` = `xxh3` over **three sampled windows: head + middle + tail**
  (~256 KB each, clamped for small files). Three windows, not two: prep-baked
  clips from the same source can share size *and* boundary bytes (same container
  header/trailer) while differing only in the middle span — head+tail alone would
  false-collide there. O(1) reads regardless of file size. Add an `xxhash-rust`
  (xxh3 feature) dep to the `vidiotic` lib.
- `pub fn full_hash(path: &Path) -> io::Result<u64>` — full-file `xxh3`, used
  **only** to confirm a fingerprint collision before anything destructive.

### 2. Fingerprint-first relink

- `pub fn relink_by_fingerprint(r: &ResolvedProject, root: &Path) -> Vec<RelinkCandidate>`
  — walk `root` (like `relink_by_root`), fingerprint each file, and match each
  missing clip whose `ClipSpec.fingerprint` is `Some` by fingerprint; fall back to
  basename (reuse `relink_by_root`'s logic) for clips with no stored fingerprint.
  Reuses the existing `RelinkCandidate` shape and `apply_relink` (which already
  rewrites `ClipSpec.path` so a later `save` persists the fix).
- Fingerprint match beats name match: survives rename, and disambiguates two
  clips that share a basename.

### 3. Dedup primitives

- `pub fn duplicate_groups(r: &ResolvedProject) -> Vec<Vec<ClipId>>` — group
  present clips by `(size, hash)` fingerprint; return groups of ≥2 as
  **candidates** (fingerprint match is necessary, not sufficient).
- `pub fn confirm_duplicate(a: &Path, b: &Path) -> io::Result<bool>` — `full_hash`
  equality; called before any merge/delete. Cheap because it only runs on the
  (rare) candidate groups, not the whole library — the rsync/git/ZFS
  cheap-filter-then-strict pattern.
- `pub fn merge_clip(project: &mut Project, from: ClipId, into: ClipId)` — repoint
  every cue (`cue_banks[].cues[].clip == from → into`) and clip-bank membership
  (`clip_banks[].clip_ids`) off `from`, then drop the `from` `ClipSpec`. This is
  the one project-surgery step; keep it in the shared module so the transform is
  test-covered once. File deletion is the app's choice, never automatic.

## App — `../vidiotic-recanon/src/`

Single-window egui app; worker thread for the directory walk + hashing (a big
collection is I/O-heavy), streaming progress over a channel like prep's export.

Workflow:
1. **Open** a `.viproj` (`rfd` picker / CLI arg) → `project::load` + `resolve`.
   Show the clip table with a status per clip (present / **missing**).
2. **Relink**: pick a search root → `relink_by_fingerprint` → present candidates
   (clip → found file, with match reason: fingerprint vs name) for confirm/skip →
   `apply_relink` the accepted ones.
3. **Backfill fingerprints**: for present clips with `fingerprint: None`, compute
   and store (so future moves relink by content). Offer as a one-click pass.
4. **Dedup**: `duplicate_groups` → show candidate groups → on confirm,
   `confirm_duplicate` (full hash) → choose the canonical clip → `merge_clip` the
   rest → optionally delete the now-orphaned files (explicit, with a count/summary
   first; never silent).
5. **Save**: `project::save` back to the same path (or Save As). Reuses the
   live-app save conventions.

UI notes: plain buttons / tables — do **not** wrap row widgets in
`ui.push_id()` inside scroll areas (memory `egui-push-id-breaks-scroll`). Never
auto-delete or auto-merge; every destructive step is a confirmed action showing
what it will touch.

## Prep integration (later, small)

`../vidiotic-prep/src/export.rs` builds `ClipSpec` by hand — add
`fingerprint: Some(project::fingerprint(&out_path)?)` when it writes each baked
clip, so exported projects ship with fingerprints and relink by content from day
one. Optional, non-blocking for the recanon MVP.

## Verification

- Shared lib (`cargo test` in `vidiotic`):
  - `fingerprint` is stable across a byte-identical copy and differs on a
    middle-only edit (the head+tail-would-miss case).
  - `relink_by_fingerprint` matches a **renamed** file that basename matching
    would miss, and picks the right one of two same-basename files.
  - `merge_clip` repoints cues + clip-bank ids and drops the merged `ClipSpec`;
    round-trip through `save`/`load` holds.
  - Additive-field back-compat: a `fingerprint`-less file still parses.
- App: open a project, move+rename its clips, relink, dedup a duplicated clip,
  save, then load the result in `vidiotic` to confirm it resolves and plays.
- `cargo build` across the workspace (`vidiotic`, `vidiotic-prep`,
  `vidiotic-recanon`) — the new `ClipSpec` field defaults, so prep/live compile
  unchanged.
