use std::path::PathBuf;

use clap::{Parser, Subcommand};

use vidiotic::analysis::{self, AudioCtl, AudioFrame};
use vidiotic::app::{self, Boot};
use vidiotic::audio;
use vidiotic::clippool;
use vidiotic::commands::{ClipId, Command};
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

    /// Fragment shader: .frag/.fs/.glsl (GLSL) or .wgsl.
    #[arg(short, long)]
    shader: PathBuf,

    /// Initial BPM.
    #[arg(long, default_value_t = 120.0)]
    bpm: f64,

    /// Phrase length in beats for auto-transitions (16 or 32).
    #[arg(long, default_value_t = 16)]
    phrase_len: u32,

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

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    match Cli::parse().cmd {
        Cmd::Transcode { input, output } => transcode::run(&input, &output),
        Cmd::Run(args) => run_player(args),
    }
}

fn run_player(cli: RunArgs) -> anyhow::Result<()> {
    // Build the clip pool: a directory, a single --clip, or both.
    let mut clips = Vec::new();
    if let Some(dir) = &cli.clip_dir {
        clips = clippool::scan(dir);
    }
    let mut auto_active: Vec<ClipId> = Vec::new();
    if let Some(single) = &cli.clip {
        let id = clips.len() as ClipId;
        let name = single
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("clip")
            .to_string();
        clips.push(clippool::Clip {
            id,
            path: single.clone(),
            name,
        });
        auto_active.push(id);
    }
    anyhow::ensure!(
        !clips.is_empty(),
        "provide --clip <file> and/or --clip-dir <dir>"
    );
    let clip_dir = cli.clip_dir.clone();
    let thumb_rx = Some(clippool::spawn_thumbnailer(clips.clone()));

    // Audio analysis handoff.
    let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded::<AudioCtl>();
    let (err_tx, err_rx) = crossbeam_channel::bounded::<cpal::Error>(8);
    let (audio_in, audio_out) = triple_buffer::triple_buffer(&AudioFrame::default());
    std::thread::Builder::new()
        .name("analysis".into())
        .spawn(move || analysis::run(ctl_rx, audio_in))?;

    let host = cpal::default_host();
    let audio_devices: Vec<String> = audio::list_input_devices(&host)
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    let audio_capture =
        audio::build_capture(&host, None, cli.audio_device.as_deref(), &ctl_tx, err_tx)?;
    log::info!("capturing audio from '{}'", audio_capture.device_name);

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<Command>();

    let boot = Boot {
        shader_path: cli.shader,
        windowed: cli.windowed,
        monitor: cli.monitor,
        bpm: cli.bpm,
        quantum: QUANTUM,
        phrase_len: cli.phrase_len,
        clip_dir,
        clips,
        auto_active,
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
