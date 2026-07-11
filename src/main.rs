use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand, ValueEnum};

use vidiotic::analysis::{self, AudioCtl, AudioFrame};
use vidiotic::app::{self, Boot};
use vidiotic::audio;
use vidiotic::bank::Bank;
use vidiotic::clippool::{self, Clip, ClipBank};
use vidiotic::commands::{ClipId, Command, SyncKind};
use vidiotic::project;
use vidiotic::transcode;

const QUANTUM: f64 = 4.0;

#[derive(Parser)]
#[command(name = "vidiotic", version, about = "VJ controller: audio-reactive shader over video clips")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the VJ player.
    Run(RunArgs),
    /// Transcode a video to a HAP .mov for fast, near-zero-CPU playback.
    Transcode {
        /// Source video (any format ffmpeg can decode).
        input: PathBuf,
        /// Destination .mov (HAP1).
        output: PathBuf,
    },
}

#[derive(Parser)]
struct RunArgs {
    /// A single clip to loop (added to the pool and activated immediately).
    #[arg(short, long)]
    clip: Option<PathBuf>,

    /// A directory of clips to populate the pool (toggle them active in the UI).
    #[arg(short = 'd', long)]
    clip_dir: Option<PathBuf>,

    /// A saved `.viproj` project: clips, clip banks, cue banks, and session
    /// defaults. Mutually exclusive with --clip/--clip-dir.
    #[arg(long)]
    project: Option<PathBuf>,

    /// When loading a --project whose clip files have moved, re-match missing
    /// clips by name under this directory.
    #[arg(long)]
    relink_root: Option<PathBuf>,

    /// Fragment shader: .frag/.fs/.glsl (GLSL) or .wgsl. Optional when a
    /// --project supplies one; required otherwise.
    #[arg(short, long)]
    shader: Option<PathBuf>,

    /// Initial BPM.
    #[arg(long, default_value_t = 120.0)]
    bpm: f64,

    /// Phrase length in beats for auto-transitions (16 or 32).
    #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(u32).range(1..))]
    phrase_len: u32,

    /// Clock sync source at startup.
    #[arg(long, value_enum, default_value = "internal")]
    sync: SyncArg,

    /// Output monitor index for fullscreen (default: first non-primary).
    #[arg(long)]
    monitor: Option<usize>,

    /// Stay windowed instead of going fullscreen after the first frame.
    #[arg(long)]
    windowed: bool,

    /// Input device name substring to capture from (default: system default input).
    #[arg(long)]
    audio_device: Option<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum SyncArg {
    /// App-owned host-time clock.
    Internal,
    /// Follow an Ableton Link session.
    Link,
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    // Quiets libswscale's "No accelerated colorspace conversion found" notice
    // (and similar av_log() chatter) — it's a perf-path note, not an error, and
    // it bypasses our own log filtering since it's FFmpeg's own logger.
    ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Error);
    match Cli::parse().cmd {
        Cmd::Transcode { input, output } => transcode::run(&input, &output),
        Cmd::Run(args) => run_player(args),
    }
}

/// The pool and session state assembled from either a `--project` file or the
/// `--clip`/`--clip-dir` flags, before audio/window plumbing is attached.
struct Loaded {
    clips: Vec<Clip>,
    clip_banks: Vec<ClipBank>,
    cue_banks: Vec<Bank>,
    auto_active: Vec<ClipId>,
    /// Per-clip probe metadata retained for a faithful save; empty for the
    /// `--clip`/`--clip-dir` path (raw files carry no baked metadata).
    clip_meta: HashMap<ClipId, project::ClipMeta>,
    /// The `.viproj` this was loaded from, if any (the default save target).
    project_path: Option<PathBuf>,
    bpm: f64,
    phrase_len: u32,
    sync: SyncKind,
    preserve_playhead: bool,
    loop_len: Option<u32>,
    advanced: bool,
    shader: PathBuf,
}

/// Build the pool from `--clip`/`--clip-dir`: a flat pool wrapped in one clip
/// bank, no cue banks (the engine starts with a default empty bank).
fn load_from_flags(cli: &RunArgs) -> anyhow::Result<Loaded> {
    let mut clips = Vec::new();
    if let Some(dir) = &cli.clip_dir {
        clips = clippool::scan(dir);
    }
    let mut auto_active: Vec<ClipId> = Vec::new();
    if let Some(single) = &cli.clip {
        let id = clips.len() as ClipId;
        let name: Arc<str> = single
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("clip")
            .into();
        clips.push(Clip {
            id,
            path: single.clone(),
            name,
            bpm: None,
        });
        auto_active.push(id);
    }
    anyhow::ensure!(
        !clips.is_empty(),
        "provide --clip <file>, --clip-dir <dir>, or --project <file.viproj>"
    );
    let shader = cli
        .shader
        .clone()
        .ok_or_else(|| anyhow::anyhow!("--shader is required (or load a --project that supplies one)"))?;
    // One clip bank covering the whole flat pool, named for the source folder.
    let name: Arc<str> = cli
        .clip_dir
        .as_ref()
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("clips")
        .into();
    let clip_banks = vec![ClipBank {
        name,
        dir: cli.clip_dir.clone(),
        clip_ids: clips.iter().map(|c| c.id).collect(),
    }];
    Ok(Loaded {
        clips,
        clip_banks,
        cue_banks: Vec::new(),
        auto_active,
        clip_meta: HashMap::new(),
        project_path: None,
        bpm: cli.bpm,
        phrase_len: cli.phrase_len,
        sync: match cli.sync {
            SyncArg::Internal => SyncKind::Internal,
            SyncArg::Link => SyncKind::Link,
        },
        preserve_playhead: true,
        loop_len: None,
        advanced: false,
        shader,
    })
}

/// Load a `.viproj`: resolve clip paths (relinking missing ones under
/// `--relink-root` if given), then rebuild the flat pool, clip banks, and cue
/// banks with fresh ids.
fn load_from_project(cli: &RunArgs, path: &Path) -> anyhow::Result<Loaded> {
    let project = project::load(path)?;
    let project_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut resolved = project::resolve(project, project_dir);

    if !resolved.missing.is_empty() {
        if let Some(root) = &cli.relink_root {
            for cand in project::relink_by_root(&resolved, root) {
                if let Some(found) = cand.found {
                    project::apply_relink(&mut resolved, cand.clip_id, found);
                }
            }
        }
        if !resolved.missing.is_empty() {
            let names: Vec<String> = resolved
                .missing
                .iter()
                .filter_map(|id| resolved.project.clips.iter().find(|c| &c.id == id))
                .map(|c| c.name.clone())
                .collect();
            anyhow::bail!(
                "missing clip files (pass --relink-root <dir> to re-locate): {}",
                names.join(", ")
            );
        }
    }

    let d = &resolved.project.defaults;
    let clips: Vec<Clip> = resolved
        .project
        .clips
        .iter()
        .map(|spec| spec.to_clip(resolved.clip_paths[&spec.id].clone()))
        .collect();
    let clip_banks: Vec<ClipBank> = resolved
        .project
        .clip_banks
        .iter()
        .map(|b| ClipBank {
            name: b.name.as_str().into(),
            dir: None,
            clip_ids: b.clip_ids.clone(),
        })
        .collect();
    // Assign cue ids sequentially across all banks.
    let mut next_cue = 1u32;
    let cue_banks: Vec<Bank> = resolved
        .project
        .cue_banks
        .iter()
        .map(|cb| {
            let cues = cb
                .cues
                .iter()
                .map(|cs| {
                    let id = next_cue;
                    next_cue += 1;
                    cs.to_cue(id)
                })
                .collect();
            Bank {
                name: cb.name.as_str().into(),
                cues,
            }
        })
        .collect();

    // Shader: prefer the project's, fall back to --shader.
    let shader = d
        .shader_path
        .as_ref()
        .map(|s| project::resolve_path(project_dir, s))
        .or_else(|| cli.shader.clone())
        .ok_or_else(|| anyhow::anyhow!("project has no shader; pass --shader"))?;

    // Retain per-clip probe metadata the runtime `Clip` drops, so a later save
    // round-trips fps/frames/duration/provenance instead of blanking them.
    let clip_meta: HashMap<ClipId, project::ClipMeta> = resolved
        .project
        .clips
        .iter()
        .map(|c| {
            (
                c.id,
                project::ClipMeta {
                    fps: c.fps,
                    frames: c.frames,
                    duration_sec: c.duration_sec,
                    source: c.source.clone(),
                },
            )
        })
        .collect();

    Ok(Loaded {
        clips,
        clip_banks,
        cue_banks,
        auto_active: Vec::new(),
        clip_meta,
        project_path: Some(path.to_path_buf()),
        bpm: if d.bpm > 0.0 { d.bpm } else { cli.bpm },
        phrase_len: if d.phrase_len > 0 { d.phrase_len } else { cli.phrase_len },
        sync: match d.sync {
            project::SyncSpec::Internal => SyncKind::Internal,
            project::SyncSpec::Link => SyncKind::Link,
        },
        preserve_playhead: d.preserve_playhead,
        loop_len: d.loop_len,
        advanced: d.advanced,
        shader,
    })
}

fn run_player(cli: RunArgs) -> anyhow::Result<()> {
    anyhow::ensure!(
        cli.project.is_none() || (cli.clip.is_none() && cli.clip_dir.is_none()),
        "--project is mutually exclusive with --clip/--clip-dir"
    );
    let loaded = match &cli.project {
        Some(path) => load_from_project(&cli, path)?,
        None => load_from_flags(&cli)?,
    };
    let thumb_rx = Some(clippool::spawn_thumbnailer(loaded.clips.clone()));

    // Audio analysis handoff.
    let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded::<AudioCtl>();
    let (err_tx, err_rx) = crossbeam_channel::bounded::<cpal::Error>(8);
    let (audio_in, audio_out) = triple_buffer::triple_buffer(&AudioFrame::default());
    std::thread::Builder::new()
        .name("analysis".into())
        .spawn(move || analysis::run(ctl_rx, audio_in))?;

    let host = cpal::default_host();
    let audio_devices: Vec<Arc<str>> = audio::list_input_devices(&host)
        .into_iter()
        .map(|(_, name)| name.into())
        .collect();
    let audio_capture =
        audio::build_capture(&host, None, cli.audio_device.as_deref(), &ctl_tx, err_tx)?;
    log::info!("capturing audio from '{}'", audio_capture.device_name);

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<Command>();

    let boot = Boot {
        shader_path: loaded.shader,
        windowed: cli.windowed,
        monitor: cli.monitor,
        bpm: loaded.bpm,
        quantum: QUANTUM,
        phrase_len: loaded.phrase_len,
        initial_sync: loaded.sync,
        clips: loaded.clips,
        clip_banks: loaded.clip_banks,
        cue_banks: loaded.cue_banks,
        auto_active: loaded.auto_active,
        clip_meta: loaded.clip_meta,
        project_path: loaded.project_path,
        preserve_playhead: loaded.preserve_playhead,
        loop_len: loaded.loop_len,
        advanced: loaded.advanced,
        thumb_rx,
        audio_out,
        audio_capture,
        audio_err_rx: err_rx,
        audio_ctl_tx: ctl_tx,
        host,
        audio_devices,
        cmd_tx,
        cmd_rx,
    };
    app::run(boot)
}
