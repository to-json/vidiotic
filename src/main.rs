use std::path::PathBuf;

use clap::Parser;

use vidiotic::analysis::{self, AudioCtl, AudioFrame};
use vidiotic::app::{self, Boot};
use vidiotic::audio;
use vidiotic::clock::InternalClock;
use vidiotic::video::decoder;

const QUANTUM: f64 = 4.0;

#[derive(Parser)]
#[command(name = "vidiotic", version, about = "VJ controller: audio-reactive shader over video clips")]
struct Cli {
    /// Video clip to loop (HAP .mov preferred; H.264/etc. via software decode).
    #[arg(short, long)]
    clip: PathBuf,

    /// Fragment shader: .frag/.fs/.glsl (GLSL) or .wgsl.
    #[arg(short, long)]
    shader: PathBuf,

    /// Initial BPM.
    #[arg(long, default_value_t = 120.0)]
    bpm: f64,

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
    let cli = Cli::parse();

    // Audio analysis handoff: capture ring control channel + wait-free band output.
    let (ctl_tx, ctl_rx) = crossbeam_channel::unbounded::<AudioCtl>();
    let (err_tx, err_rx) = crossbeam_channel::bounded::<cpal::Error>(8);
    let (audio_in, audio_out) = triple_buffer::triple_buffer(&AudioFrame::default());
    std::thread::Builder::new()
        .name("analysis".into())
        .spawn(move || analysis::run(ctl_rx, audio_in))?;

    // Live capture from the chosen (or default) input device.
    let host = cpal::default_host();
    let audio_capture = audio::build_capture(
        &host,
        None,
        cli.audio_device.as_deref(),
        &ctl_tx,
        err_tx,
    )?;
    log::info!("capturing audio from '{}'", audio_capture.device_name);

    // Clip decode worker (loops).
    let decode = decoder::spawn(cli.clip.clone())?;

    let clock = InternalClock::new(cli.bpm, QUANTUM);

    let boot = Boot {
        shader_path: cli.shader,
        windowed: cli.windowed,
        monitor: cli.monitor,
        clock,
        audio_out,
        audio_capture,
        audio_err_rx: err_rx,
        audio_ctl_tx: ctl_tx,
        decode,
    };
    app::run(boot)
}
