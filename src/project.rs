//! The vidiotic project save format (`.viproj`, RON on disk).
//!
//! On-disk "spec" types are deliberately decoupled from the runtime types
//! (`clippool::Clip`, `bank::Cue`, `bank::Bank`): the file owns a stable, flat
//! clip-id space and flattens the runtime `Toggle<T>` knobs to `Option<T>`, so
//! the format can evolve without dragging the engine's in-memory representation
//! along. Both the player (vidiotic) and the authoring tool (vidiotic-prep) load
//! and save through this one module, so the format has a single source of truth.
//!
//! Serialization is `nanoserde` (RON) — no `serde`/`serde_derive` proc-macro. A
//! `.viproj` is read once per open and written once per save, never in a hot
//! loop, so parser speed is irrelevant; RON is chosen for hand-edit ergonomics
//! (comments, native int/float literals, terse enums).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nanoserde::{DeRon, SerRon};

use crate::bank::{Bank, Cue, CueId, Toggle};
use crate::clippool::{Clip, ClipBank};
use crate::commands::{Cadence, ChainSlot, ClipId, SlotRef, TimeSig};
use crate::isf::IsfValue;

/// Bumped on any breaking change to the on-disk shape; [`load`] routes older
/// files through [`migrate`].
pub const FORMAT_VERSION: u32 = 1;

/// A whole saved session: a flat clip pool, named clip-bank groupings over it,
/// and the cue banks the sequencer plays.
#[derive(SerRon, DeRon, Clone, Debug, Default)]
pub struct Project {
    #[nserde(default)]
    pub version: u32,
    #[nserde(default)]
    pub defaults: SessionDefaults,
    /// Flat, global clip pool. `ClipSpec::id` is the stable handle cue/clip-bank
    /// specs reference.
    pub clips: Vec<ClipSpec>,
    /// Named groupings over `clips[].id` — a UI filter, not an ownership tree
    /// (an id may appear in several banks, or none).
    pub clip_banks: Vec<ClipBankSpec>,
    pub cue_banks: Vec<CueBankSpec>,
}

/// On-disk mirror of [`crate::commands::Cadence`].
#[derive(SerRon, DeRon, Clone, Copy, Debug, PartialEq)]
pub enum CadenceSpec {
    Note(u32),
    Bars(u32),
}

impl From<Cadence> for CadenceSpec {
    fn from(c: Cadence) -> Self {
        match c {
            Cadence::Note(t) => Self::Note(t),
            Cadence::Bars(n) => Self::Bars(n),
        }
    }
}

impl From<CadenceSpec> for Cadence {
    fn from(c: CadenceSpec) -> Self {
        match c {
            CadenceSpec::Note(t) => Self::Note(t),
            CadenceSpec::Bars(n) => Self::Bars(n),
        }
    }
}

/// Session-wide playback defaults; mirrors the engine's global knobs.
///
/// `quantum`/`phrase_len`/`loop_len` are the pre-time-signature fields, kept
/// for `vidiotic-prep` compatibility and as the fallback a legacy (pre-`ts_num`)
/// file resolves through. `ts_num == 0` marks a file with no signature written
/// (defaults to 4/4); `phrase_cadence: None` and `!loop_cadence_set` mean
/// "derive from the legacy fields" rather than "use the new ones".
#[derive(SerRon, DeRon, Clone, Debug, Default)]
pub struct SessionDefaults {
    pub bpm: f64,
    pub quantum: f64,
    pub phrase_len: u32,
    #[nserde(default)]
    pub sync: SyncSpec,
    #[nserde(default)]
    pub preserve_playhead: bool,
    /// Forced re-loop grid in 1/32-beat ticks; `None` = loop on EOF only.
    #[nserde(default)]
    pub loop_len: Option<u32>,
    #[nserde(default)]
    pub advanced: bool,
    /// Time signature numerator; `0` = not written (pre-signature file, 4/4).
    #[nserde(default)]
    pub ts_num: u8,
    #[nserde(default)]
    pub ts_den: u8,
    /// The "next every" cadence; `None` = derive from `phrase_len`.
    #[nserde(default)]
    pub phrase_cadence: Option<CadenceSpec>,
    /// Whether `loop_cadence` is authoritative (it may still be `None` = off);
    /// when `false`, derive from `loop_len` instead.
    #[nserde(default)]
    pub loop_cadence_set: bool,
    #[nserde(default)]
    pub loop_cadence: Option<CadenceSpec>,
    /// The live (livecoded) shader file; relative-to-project or absolute.
    #[nserde(default)]
    pub shader_path: Option<String>,
}

/// One source clip. `path` is relative to the `.viproj`'s directory, or
/// absolute; [`resolve`] turns it into a concrete path and flags misses.
#[derive(SerRon, DeRon, Clone, Debug, Default)]
pub struct ClipSpec {
    pub id: ClipId,
    pub path: String,
    pub name: String,
    #[nserde(default)]
    pub bpm: Option<f64>,
    #[nserde(default)]
    pub fps: Option<f64>,
    #[nserde(default)]
    pub frames: Option<u64>,
    #[nserde(default)]
    pub duration_sec: Option<f64>,
    /// If this clip was baked from a span of a larger source, how it was cut.
    #[nserde(default)]
    pub source: Option<SpanProvenance>,
}

/// How a baked clip was carved out of its pre-transcode original — informational
/// and enough to re-bake. `out_frame` is exclusive.
#[derive(SerRon, DeRon, Clone, Debug, Default)]
pub struct SpanProvenance {
    pub original_path: String,
    pub in_frame: u64,
    pub out_frame: u64,
    pub in_sec: f64,
    pub out_sec: f64,
}

/// A named group of clips, referenced by id. Purely a pool-grid filter.
#[derive(SerRon, DeRon, Clone, Debug, Default)]
pub struct ClipBankSpec {
    pub name: String,
    pub clip_ids: Vec<ClipId>,
}

/// A named, ordered set of cues — the on-disk form of a [`crate::bank::Bank`].
#[derive(SerRon, DeRon, Clone, Debug, Default)]
pub struct CueBankSpec {
    pub name: String,
    pub cues: Vec<CueSpec>,
}

/// One serialized entry in a cue's effect chain. Built-ins are referenced by
/// stable name; the live (livecoded) shader is a tagged position; ISF shaders by
/// file path (relative to the project dir where possible) plus their dialed-in
/// input values. Pinned livecode captures have no stable source and are not
/// serialized (dropped on save), so there is no `Pinned` variant here.
///
/// Not `Eq` because an ISF value can carry an `f32`.
#[derive(SerRon, DeRon, Clone, Debug, PartialEq)]
pub enum CueEffectSpec {
    Live,
    Builtin(String),
    Isf {
        path: String,
        params: Vec<(String, IsfValueSpec)>,
    },
}

/// Serialized ISF input value (mirrors [`crate::isf::IsfValue`]). Colors/points
/// are stored as tuples for nanoserde compatibility.
#[derive(SerRon, DeRon, Clone, Debug, PartialEq)]
pub enum IsfValueSpec {
    Float(f32),
    Bool(bool),
    Long(i32),
    Color(f32, f32, f32, f32),
    Point2D(f32, f32),
}

impl IsfValueSpec {
    fn from_runtime(v: &IsfValue) -> Self {
        match v {
            IsfValue::Float(f) => Self::Float(*f),
            IsfValue::Bool(b) => Self::Bool(*b),
            IsfValue::Long(i) => Self::Long(*i),
            IsfValue::Color([r, g, b, a]) => Self::Color(*r, *g, *b, *a),
            IsfValue::Point2D([x, y]) => Self::Point2D(*x, *y),
        }
    }
    fn to_runtime(&self) -> IsfValue {
        match self {
            Self::Float(f) => IsfValue::Float(*f),
            Self::Bool(b) => IsfValue::Bool(*b),
            Self::Long(i) => IsfValue::Long(*i),
            Self::Color(r, g, b, a) => IsfValue::Color([*r, *g, *b, *a]),
            Self::Point2D(x, y) => IsfValue::Point2D([*x, *y]),
        }
    }
}

/// A cue placement. Runtime `Toggle<T>` advanced knobs are flattened to
/// `Option<T>` (`None` = off; the toggle's retained-off value is not persisted).
#[derive(SerRon, DeRon, Clone, Debug, Default)]
pub struct CueSpec {
    pub clip: ClipId,
    #[nserde(default)]
    pub name: String,
    #[nserde(default)]
    pub in_sec: f64,
    #[nserde(default)]
    pub out_sec: Option<f64>,
    #[nserde(default)]
    pub preserve: Option<bool>,
    #[nserde(default)]
    pub dwell: Option<u32>,
    #[nserde(default)]
    pub loop_len: Option<u32>,
    #[nserde(default)]
    pub loop_phase: Option<i32>,
    #[nserde(default)]
    pub start_nudge: Option<f64>,
    #[nserde(default)]
    pub trig_delay: Option<u32>,
    #[nserde(default)]
    pub bpm: Option<f64>,
    #[nserde(default)]
    pub bpm_sync_on: bool,
    #[nserde(default)]
    pub speed_mul: Option<f64>,
    /// The cue's effect chain, in order. Empty = the live shader. Built-ins by
    /// name; pinned livecode captures are dropped on save.
    #[nserde(default)]
    pub chain: Vec<CueEffectSpec>,
}

impl CueSpec {
    /// A whole-clip cue: no trim, every override inherited, no effect chain.
    /// The stable constructor callers (incl. vidiotic-prep) should use instead of
    /// a struct literal, so added fields don't break them.
    pub fn full_length(clip: ClipId, name: String) -> Self {
        Self {
            clip,
            name,
            ..Self::default()
        }
    }
}

impl SessionDefaults {
    /// Resolve the time signature and cadences, falling back to the legacy
    /// `quantum`/`phrase_len`/`loop_len` fields for a file saved before
    /// signatures existed.
    pub fn time_sig(&self) -> TimeSig {
        if self.ts_num > 0 {
            TimeSig { num: self.ts_num, den: self.ts_den.max(1) }.sanitized()
        } else {
            TimeSig::default()
        }
    }

    /// Resolve the "next every" cadence, in 1/32-beat-tick note terms when
    /// falling back to the legacy `phrase_len` (whole beats).
    pub fn phrase_cadence(&self) -> Cadence {
        self.phrase_cadence.map(Cadence::from).unwrap_or_else(|| {
            Cadence::Note(self.phrase_len.max(1) * crate::commands::LOOP_TICKS_PER_BEAT)
        })
    }

    /// Resolve the "loop every" cadence (`None` = loop on EOF only).
    pub fn loop_cadence(&self) -> Option<Cadence> {
        if self.loop_cadence_set {
            self.loop_cadence.map(Cadence::from)
        } else {
            self.loop_len.map(Cadence::Note)
        }
    }
}

/// On-disk mirror of [`crate::commands::SyncKind`], kept separate so the format
/// does not depend on the command enum's layout.
#[derive(SerRon, DeRon, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncSpec {
    #[default]
    Internal,
    Link,
}

// --- load / save ---------------------------------------------------------

/// Serialize `p` to RON and write it to `path`.
///
/// # Errors
/// Propagates the file write failure.
pub fn save(p: &Project, path: &Path) -> anyhow::Result<()> {
    std::fs::write(path, p.serialize_ron())?;
    Ok(())
}

/// Read and parse a `.viproj`, then run version migrations.
///
/// # Errors
/// Propagates read failures and RON parse errors.
pub fn load(path: &Path) -> anyhow::Result<Project> {
    let text = std::fs::read_to_string(path)?;
    let mut p = Project::deserialize_ron(&text)
        .map_err(|e| anyhow::anyhow!("parse {}: {e}", path.display()))?;
    migrate(&mut p);
    Ok(p)
}

/// Upgrade an older `Project` in place. A `version` of 0 (a file with no version
/// field, or a pre-versioning file) is treated as the current version.
fn migrate(p: &mut Project) {
    if p.version == 0 {
        p.version = FORMAT_VERSION;
    }
    // Future breaking changes add their fix-ups here, keyed on p.version.
}

// --- path resolution -----------------------------------------------------

/// Resolve a stored clip path against the project directory: absolute paths pass
/// through, relative ones join `project_dir`.
pub fn resolve_path(project_dir: &Path, stored: &str) -> PathBuf {
    let p = Path::new(stored);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        project_dir.join(p)
    }
}

/// Best-effort absolute form of a path for storage: canonical when the file
/// exists (clean, symlink-resolved), otherwise a lexical absolutize, otherwise
/// the input unchanged. A path scanned from a relative `--clip-dir` is
/// CWD-relative, so it must be absolutized before [`relativize`] or the saved
/// string would resolve against the wrong root on load.
pub fn absolutize(p: &Path) -> PathBuf {
    std::fs::canonicalize(p)
        .or_else(|_| std::path::absolute(p))
        .unwrap_or_else(|_| p.to_path_buf())
}

/// Store `abs` relative to `project_dir` when it lives under it; otherwise keep
/// it absolute. Returns a forward-slash string suitable for the `.viproj`.
pub fn relativize(project_dir: &Path, abs: &Path) -> String {
    abs.strip_prefix(project_dir)
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| abs.to_string_lossy().into_owned())
}

// --- resolved form (shared by both apps) --------------------------------

/// A loaded project with each clip id resolved to a concrete path, plus the set
/// of ids whose file is currently missing (candidates for relinking).
#[derive(Clone, Debug)]
pub struct ResolvedProject {
    pub project: Project,
    pub project_dir: PathBuf,
    pub clip_paths: HashMap<ClipId, PathBuf>,
    pub missing: Vec<ClipId>,
}

/// Resolve every clip path and record which ones do not exist on disk.
pub fn resolve(project: Project, project_dir: &Path) -> ResolvedProject {
    let mut clip_paths = HashMap::new();
    let mut missing = Vec::new();
    for c in &project.clips {
        let path = resolve_path(project_dir, &c.path);
        if !path.exists() {
            missing.push(c.id);
        }
        clip_paths.insert(c.id, path);
    }
    ResolvedProject {
        project,
        project_dir: project_dir.to_path_buf(),
        clip_paths,
        missing,
    }
}

// --- relink --------------------------------------------------------------

/// A missing clip and the best re-match found under a candidate root.
#[derive(Clone, Debug)]
pub struct RelinkCandidate {
    pub clip_id: ClipId,
    pub name: String,
    pub found: Option<PathBuf>,
}

/// For each missing clip, look for a file with the same base name anywhere under
/// `new_root`. Does not mutate; the caller applies chosen matches via
/// [`apply_relink`].
pub fn relink_by_root(r: &ResolvedProject, new_root: &Path) -> Vec<RelinkCandidate> {
    let mut by_name: HashMap<String, PathBuf> = HashMap::new();
    let mut stack = vec![new_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                // First match wins; a shallower directory is popped later, but
                // any hit is a reasonable candidate for the user to confirm.
                by_name.entry(name.to_owned()).or_insert(path.clone());
            }
        }
    }
    r.missing
        .iter()
        .map(|&id| {
            let spec = r.project.clips.iter().find(|c| c.id == id);
            let name = spec.map(|c| c.name.clone()).unwrap_or_default();
            let base = spec
                .and_then(|c| Path::new(&c.path).file_name().and_then(|n| n.to_str()))
                .map(str::to_owned)
                .unwrap_or_else(|| name.clone());
            RelinkCandidate {
                clip_id: id,
                name,
                found: by_name.get(&base).cloned(),
            }
        })
        .collect()
}

/// Point a clip at a new file: update its resolved path and drop it from
/// `missing`. Also rewrites the stored `ClipSpec.path` so a subsequent save
/// persists the relink.
pub fn apply_relink(r: &mut ResolvedProject, clip_id: ClipId, path: PathBuf) {
    let stored = relativize(&r.project_dir, &path);
    if let Some(spec) = r.project.clips.iter_mut().find(|c| c.id == clip_id) {
        spec.path = stored;
    }
    r.clip_paths.insert(clip_id, path);
    r.missing.retain(|&id| id != clip_id);
}

// --- gather --------------------------------------------------------------

/// Copy every resolved clip into `dest_dir/clips/` and return a new `Project`
/// whose clip paths are rewritten relative (`clips/<name>`), making the folder
/// self-contained. Clips still missing are left with their original path.
///
/// # Errors
/// Propagates directory-creation and copy failures.
pub fn gather(r: &ResolvedProject, dest_dir: &Path) -> anyhow::Result<Project> {
    let clips_dir = dest_dir.join("clips");
    std::fs::create_dir_all(&clips_dir)?;
    let mut project = r.project.clone();
    let mut used: HashMap<String, ClipId> = HashMap::new();
    for spec in &mut project.clips {
        let Some(src) = r.clip_paths.get(&spec.id) else {
            continue;
        };
        if !src.exists() {
            continue;
        }
        // Dedupe file names across clips: on collision, prefix the id.
        let base = Path::new(&spec.path)
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("clip{}.mov", spec.id));
        let file_name = match used.get(&base) {
            Some(_) => format!("{}_{base}", spec.id),
            None => base.clone(),
        };
        used.insert(base, spec.id);
        std::fs::copy(src, clips_dir.join(&file_name))?;
        spec.path = format!("clips/{file_name}");
    }
    Ok(project)
}

// --- conversions ---------------------------------------------------------

/// Probe metadata attached to a clip when authoring a spec.
#[derive(Clone, Debug, Default)]
pub struct ClipMeta {
    pub fps: Option<f64>,
    pub frames: Option<u64>,
    pub duration_sec: Option<f64>,
    pub source: Option<SpanProvenance>,
}

impl ClipSpec {
    /// Build a spec from a runtime clip, storing its path relative to
    /// `project_dir` where possible.
    ///
    /// The runtime path is [`absolutize`]d first (a clip pool scanned from a
    /// relative `--clip-dir` holds CWD-relative paths); otherwise saving into a
    /// different directory would emit a string that resolves against the wrong
    /// root on load.
    pub fn from_clip(c: &Clip, project_dir: &Path, meta: ClipMeta) -> Self {
        let path = match c.file_path() {
            Some(p) => relativize(project_dir, &absolutize(p)),
            // Camera clips have no file; their identity is the CameraSpec.
            None => String::new(),
        };
        Self {
            id: c.id,
            path,
            name: c.name.to_string(),
            bpm: c.bpm,
            fps: meta.fps,
            frames: meta.frames,
            duration_sec: meta.duration_sec,
            source: meta.source,
        }
    }

    /// Build a runtime clip from a spec with its already-resolved absolute path.
    pub fn to_clip(&self, resolved: PathBuf) -> Clip {
        Clip {
            id: self.id,
            source: crate::clippool::ClipSource::File(resolved),
            name: self.name.as_str().into(),
            bpm: self.bpm,
        }
    }
}

impl CueSpec {
    /// Snapshot a runtime cue. Drops the runtime `id` (reassigned on load) and
    /// maps each `Toggle` to `Some(val)` only when on. Chain slots serialize by
    /// stable name (built-ins) or file path relative to `dir` (ISF, with their
    /// param overrides); pinned livecode captures have no stable source, so they
    /// are dropped (with a warning).
    pub fn from_cue(c: &Cue, dir: &Path) -> Self {
        let chain = c
            .chain
            .iter()
            .filter_map(|slot| match &slot.shader {
                SlotRef::Live => Some(CueEffectSpec::Live),
                SlotRef::Builtin(name) => Some(CueEffectSpec::Builtin(name.to_string())),
                SlotRef::Isf(path) => Some(CueEffectSpec::Isf {
                    path: relativize(dir, &absolutize(Path::new(path.as_ref()))),
                    params: slot
                        .params
                        .iter()
                        .map(|(n, v)| (n.to_string(), IsfValueSpec::from_runtime(v)))
                        .collect(),
                }),
                SlotRef::Pinned(id) => {
                    log::warn!("dropping pinned shader {id} from saved cue chain (not persistable)");
                    None
                }
            })
            .collect();
        Self {
            clip: c.clip,
            name: c.name.to_string(),
            in_sec: c.in_sec,
            out_sec: c.out_sec,
            preserve: c.preserve,
            dwell: c.dwell,
            loop_len: c.loop_len,
            loop_phase: c.loop_phase.on.then_some(c.loop_phase.val),
            start_nudge: c.start_nudge.on.then_some(c.start_nudge.val),
            trig_delay: c.trig_delay.on.then_some(c.trig_delay.val),
            bpm: c.bpm,
            bpm_sync_on: c.bpm_sync_on,
            speed_mul: c.speed_mul.on.then_some(c.speed_mul.val),
            chain,
        }
    }

    /// Rebuild a runtime cue with the caller-assigned `id`. Absent toggles come
    /// back off, carrying the same retained defaults as [`Cue::new`]. ISF paths
    /// resolve against `dir` back to absolute, so the pool can load them.
    pub fn to_cue(&self, id: CueId, dir: &Path) -> Cue {
        let chain = self
            .chain
            .iter()
            .map(|e| match e {
                CueEffectSpec::Live => ChainSlot::new(SlotRef::Live),
                CueEffectSpec::Builtin(name) => ChainSlot::new(SlotRef::Builtin(name.as_str().into())),
                CueEffectSpec::Isf { path, params } => {
                    let abs = resolve_path(dir, path);
                    ChainSlot {
                        shader: SlotRef::Isf(abs.to_string_lossy().as_ref().into()),
                        params: params
                            .iter()
                            .map(|(n, v)| (n.as_str().into(), v.to_runtime()))
                            .collect(),
                    }
                }
            })
            .collect();
        Cue {
            id,
            clip: self.clip,
            name: self.name.as_str().into(),
            in_sec: self.in_sec,
            out_sec: self.out_sec,
            preserve: self.preserve,
            chain,
            dwell: self.dwell,
            loop_len: self.loop_len,
            loop_phase: toggle(self.loop_phase, 0),
            start_nudge: toggle(self.start_nudge, 0.0),
            trig_delay: toggle(self.trig_delay, 0),
            bpm: self.bpm,
            bpm_sync_on: self.bpm_sync_on,
            speed_mul: toggle(self.speed_mul, 1.0),
            delay: crate::bank::CamDelay::default(),
        }
    }
}

impl ClipBankSpec {
    /// Snapshot a runtime clip bank. `dir` (a scan source) is not persisted — a
    /// saved bank is just its name and clip-id membership.
    pub fn from_bank(b: &ClipBank) -> Self {
        Self {
            name: b.name.to_string(),
            clip_ids: b.clip_ids.clone(),
        }
    }
}

impl CueBankSpec {
    /// Snapshot a runtime cue bank, converting each cue via [`CueSpec::from_cue`].
    /// `dir` (the save directory) relativizes ISF shader paths.
    pub fn from_bank(b: &Bank, dir: &Path) -> Self {
        Self {
            name: b.name.to_string(),
            cues: b.cues.iter().map(|c| CueSpec::from_cue(c, dir)).collect(),
        }
    }
}

impl Project {
    /// Assemble a `Project` from live runtime state, ready to [`save`]. Clip paths
    /// are stored relative to `dir` (the save directory) where possible.
    ///
    /// `clip_meta` supplies probe data the runtime [`Clip`] does not retain
    /// (`fps`/`frames`/`duration_sec`/`source`); clips absent from the map — e.g.
    /// added at runtime from a folder scan — fall back to [`ClipMeta::default`]
    /// and are re-probed on the next load. Clip ids are stable across a
    /// load/save round-trip, so clip-bank membership references stay valid.
    ///
    /// This is the shared inverse of the load path in the binary: any consumer of
    /// the `vidiotic` lib that holds runtime `Clip`/`ClipBank`/`Bank` state can
    /// build a savable project through it.
    pub fn from_runtime(
        dir: &Path,
        clips: &[Clip],
        clip_banks: &[ClipBank],
        cue_banks: &[Bank],
        clip_meta: &HashMap<ClipId, ClipMeta>,
        defaults: SessionDefaults,
    ) -> Self {
        Self {
            version: FORMAT_VERSION,
            defaults,
            clips: clips
                .iter()
                .map(|c| {
                    ClipSpec::from_clip(c, dir, clip_meta.get(&c.id).cloned().unwrap_or_default())
                })
                .collect(),
            clip_banks: clip_banks.iter().map(ClipBankSpec::from_bank).collect(),
            cue_banks: cue_banks.iter().map(|b| CueBankSpec::from_bank(b, dir)).collect(),
        }
    }
}

/// `Some(v)` → an on toggle carrying `v`; `None` → off carrying `default`.
fn toggle<T>(opt: Option<T>, default: T) -> Toggle<T> {
    match opt {
        Some(val) => Toggle { on: true, val },
        None => Toggle { on: false, val: default },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Project {
        Project {
            version: FORMAT_VERSION,
            defaults: SessionDefaults {
                bpm: 128.0,
                quantum: 3.5,
                phrase_len: 16,
                sync: SyncSpec::Link,
                preserve_playhead: true,
                loop_len: Some(128),
                advanced: false,
                ts_num: 7,
                ts_den: 8,
                phrase_cadence: Some(CadenceSpec::Bars(2)),
                loop_cadence_set: true,
                loop_cadence: Some(CadenceSpec::Note(16)),
                shader_path: Some("shaders/demo.frag".into()),
            },
            clips: vec![ClipSpec {
                id: 0,
                path: "clips/kick.mov".into(),
                name: "kick.mov".into(),
                bpm: Some(128.0),
                fps: Some(30.0),
                frames: Some(64),
                duration_sec: Some(2.133),
                source: Some(SpanProvenance {
                    original_path: "/src/drums.mov".into(),
                    in_frame: 10,
                    out_frame: 74,
                    in_sec: 0.333,
                    out_sec: 2.466,
                }),
            }],
            clip_banks: vec![ClipBankSpec {
                name: "drums".into(),
                clip_ids: vec![0],
            }],
            cue_banks: vec![CueBankSpec {
                name: "A".into(),
                cues: vec![CueSpec {
                    clip: 0,
                    name: "kick".into(),
                    in_sec: 0.0,
                    out_sec: Some(2.0),
                    preserve: Some(false),
                    dwell: Some(64),
                    loop_len: None,
                    loop_phase: Some(-4),
                    start_nudge: None,
                    trig_delay: None,
                    bpm: Some(128.0),
                    bpm_sync_on: true,
                    speed_mul: Some(1.5),
                    chain: vec![CueEffectSpec::Builtin("kaleido".into()), CueEffectSpec::Live],
                }],
            }],
        }
    }

    #[test]
    fn round_trips_through_ron() {
        let p = sample();
        let text = p.serialize_ron();
        let back = Project::deserialize_ron(&text).expect("parse");
        assert_eq!(back.version, p.version);
        assert_eq!(back.clips.len(), 1);
        assert_eq!(back.clips[0].name, "kick.mov");
        assert_eq!(back.clips[0].source.as_ref().unwrap().in_frame, 10);
        assert_eq!(back.clip_banks[0].clip_ids, vec![0]);
        assert_eq!(back.defaults.sync, SyncSpec::Link);
        assert_eq!(back.defaults.ts_num, 7);
        assert_eq!(back.defaults.ts_den, 8);
        assert_eq!(back.defaults.time_sig(), TimeSig { num: 7, den: 8 });
        assert_eq!(back.defaults.phrase_cadence(), Cadence::Bars(2));
        assert_eq!(back.defaults.loop_cadence(), Some(Cadence::Note(16)));
        let cue = &back.cue_banks[0].cues[0];
        assert_eq!(cue.loop_phase, Some(-4));
        assert_eq!(cue.start_nudge, None);
        assert_eq!(cue.speed_mul, Some(1.5));
    }

    #[test]
    fn cue_toggle_round_trip() {
        let cue = sample().cue_banks[0].cues[0].clone();
        let dir = Path::new("/proj");
        let runtime = cue.to_cue(7, dir);
        assert_eq!(runtime.id, 7);
        assert!(runtime.loop_phase.on && runtime.loop_phase.val == -4);
        assert!(!runtime.start_nudge.on && runtime.start_nudge.val == 0.0);
        assert!(runtime.speed_mul.on && runtime.speed_mul.val == 1.5);
        let back = CueSpec::from_cue(&runtime, dir);
        assert_eq!(back.loop_phase, Some(-4));
        assert_eq!(back.start_nudge, None);
        assert_eq!(back.speed_mul, Some(1.5));
    }

    #[test]
    fn isf_effect_spec_round_trips() {
        let dir = Path::new("/proj");
        let spec = CueSpec {
            clip: 0,
            chain: vec![CueEffectSpec::Isf {
                path: "fx/hue.fs".into(),
                params: vec![
                    ("gain".into(), IsfValueSpec::Float(1.5)),
                    ("tint".into(), IsfValueSpec::Color(0.1, 0.2, 0.3, 1.0)),
                ],
            }],
            ..Default::default()
        };

        // to runtime: path resolves to absolute (so the pool can load it),
        // params come back as runtime values.
        let runtime = spec.to_cue(9, dir);
        match &runtime.chain[0].shader {
            SlotRef::Isf(p) => assert_eq!(p.as_ref(), "/proj/fx/hue.fs"),
            other => panic!("expected ISF slot, got {other:?}"),
        }
        assert_eq!(runtime.chain[0].param("gain"), Some(&IsfValue::Float(1.5)));

        // back to spec: absolute path relativizes against the save dir; params
        // preserved.
        let back = CueSpec::from_cue(&runtime, dir);
        assert_eq!(back.chain, spec.chain);

        // And the on-disk RON text round-trips.
        let text = spec.serialize_ron();
        let parsed = CueSpec::deserialize_ron(&text).expect("parse");
        assert_eq!(parsed.chain, spec.chain);
    }

    #[test]
    fn from_runtime_round_trips_through_save() {
        let dir = std::env::temp_dir().join("vidiotic_proj_test_from_runtime");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("clips")).unwrap();

        // Runtime state: one clip, one clip bank, one cue bank whose sole cue
        // carries a `Builtin("kaleido") → Live` chain (the feature we must persist).
        let clips = vec![Clip {
            id: 0,
            source: crate::clippool::ClipSource::File(dir.join("clips/kick.mov")),
            name: "kick.mov".into(),
            bpm: Some(128.0),
        }];
        let clip_banks = vec![ClipBank {
            name: "drums".into(),
            dir: None,
            clip_ids: vec![0],
        }];
        let cue = sample().cue_banks[0].cues[0].clone().to_cue(1, &dir);
        let cue_banks = vec![Bank {
            name: "A".into(),
            cues: vec![cue],
        }];
        // The metadata a runtime `Clip` drops but a faithful save must retain.
        let clip_meta = HashMap::from([(
            0,
            ClipMeta {
                fps: Some(30.0),
                frames: Some(64),
                duration_sec: Some(2.133),
                source: Some(SpanProvenance {
                    original_path: "/src/drums.mov".into(),
                    in_frame: 10,
                    out_frame: 74,
                    in_sec: 0.333,
                    out_sec: 2.466,
                }),
            },
        )]);
        let defaults = SessionDefaults {
            bpm: 128.0,
            quantum: 4.0,
            phrase_len: 16,
            sync: SyncSpec::Link,
            preserve_playhead: true,
            loop_len: Some(128),
            advanced: false,
            ts_num: 4,
            ts_den: 4,
            phrase_cadence: Some(CadenceSpec::Bars(4)),
            loop_cadence_set: true,
            loop_cadence: Some(CadenceSpec::Bars(4)),
            shader_path: Some("shaders/demo.frag".into()),
        };

        let proj = Project::from_runtime(&dir, &clips, &clip_banks, &cue_banks, &clip_meta, defaults);
        let path = dir.join("out.viproj");
        save(&proj, &path).expect("save");
        let back = load(&path).expect("load");

        // Clip path relativized against the save dir; retained metadata survives.
        assert_eq!(back.clips[0].path, "clips/kick.mov");
        assert_eq!(back.clips[0].fps, Some(30.0));
        assert_eq!(back.clips[0].source.as_ref().unwrap().in_frame, 10);
        // A clip with no meta entry falls back to blank probe data (no panic).
        assert_eq!(back.clip_banks[0].clip_ids, vec![0]);
        // The effect chain round-trips intact.
        assert_eq!(
            back.cue_banks[0].cues[0].chain,
            vec![CueEffectSpec::Builtin("kaleido".into()), CueEffectSpec::Live]
        );
        assert_eq!(back.defaults.bpm, 128.0);
        assert_eq!(back.defaults.sync, SyncSpec::Link);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn from_clip_absolutizes_relative_path() {
        // A clip scanned from a relative `--clip-dir` holds a CWD-relative path.
        // Saving into an unrelated directory must not emit that string verbatim
        // (it would resolve against the wrong root on load) — from_clip absolutizes
        // first, so relativizing against a foreign dir yields an absolute path.
        let clip = Clip {
            id: 0,
            source: crate::clippool::ClipSource::File("some/relative/clip.mov".into()),
            name: "clip.mov".into(),
            bpm: None,
        };
        let spec = ClipSpec::from_clip(&clip, Path::new("/elsewhere/proj"), ClipMeta::default());
        assert!(
            Path::new(&spec.path).is_absolute(),
            "expected absolute path, got {:?}",
            spec.path
        );
        assert!(spec.path.ends_with("some/relative/clip.mov"));
    }

    #[test]
    fn missing_default_version_is_current() {
        // A hand-written file with no `version` field parses and migrates to 1.
        let text = r#"(
            defaults: (bpm: 120.0, quantum: 4.0, phrase_len: 16),
            clips: [],
            clip_banks: [],
            cue_banks: [],
        )"#;
        let mut p = Project::deserialize_ron(text).expect("parse hand-written");
        migrate(&mut p);
        assert_eq!(p.version, FORMAT_VERSION);
        assert!(p.clips.is_empty());
        // No ts_num/phrase_cadence written: resolves through the legacy fields.
        assert_eq!(p.defaults.time_sig(), TimeSig::default());
        assert_eq!(
            p.defaults.phrase_cadence(),
            Cadence::Note(16 * crate::commands::LOOP_TICKS_PER_BEAT)
        );
        assert_eq!(p.defaults.loop_cadence(), None);
    }

    #[test]
    fn resolve_flags_missing_and_relinks() {
        let dir = std::env::temp_dir().join("vidiotic_proj_test_relink");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("moved")).unwrap();
        // The project points at clips/kick.mov (absent); the real file is under moved/.
        std::fs::write(dir.join("moved/kick.mov"), b"x").unwrap();

        let mut project = sample();
        project.clips[0].source = None;
        let r = resolve(project, &dir);
        assert_eq!(r.missing, vec![0]);

        let cands = relink_by_root(&r, &dir.join("moved"));
        assert_eq!(cands.len(), 1);
        let found = cands[0].found.clone().expect("re-matched kick.mov");

        let mut r = r;
        apply_relink(&mut r, 0, found);
        assert!(r.missing.is_empty());
        assert!(r.clip_paths[&0].ends_with("moved/kick.mov"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
