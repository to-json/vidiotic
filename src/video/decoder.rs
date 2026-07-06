//! Clip decode worker. One thread per active clip: demux with ffmpeg-next, take
//! the HAP fast path (parse packet -> BC bytes, near-zero CPU) or the software
//! RGBA fallback for other codecs, loop at EOF, and hand frames to the render
//! thread over a small bounded channel, paced to the clip's own timeline.

use std::path::PathBuf;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender, TrySendError};
use ffmpeg_next as ff;

use crate::video::frame::{DecodedFrame, PixelData};
use crate::video::hap;

/// Handle to a running decode worker. Dropping it stops and joins the thread.
pub struct DecodeHandle {
    /// Decoded frames, paced to the clip's timeline (bounded; drain for newest).
    pub frames: Receiver<DecodedFrame>,
    restart_tx: Sender<()>,
    close_tx: Option<Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl DecodeHandle {
    /// Ask the worker to seek back to the clip's start on its next packet — used
    /// by the musical re-loop grid. Non-blocking; a coalesced no-op if pending.
    pub fn request_restart(&self) {
        let _ = self.restart_tx.try_send(());
    }
}

impl Drop for DecodeHandle {
    fn drop(&mut self) {
        // Signal by dropping the sender so the worker's close check disconnects,
        // then join. (Struct fields drop *after* this runs, so drop explicitly.)
        self.close_tx.take();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Spawn a decode worker for one cue. `in_sec`/`out_sec` trim the loop: playback
/// (and every restart) begins at `in_sec` and loops back once it reaches
/// `out_sec` (or the clip's natural end when `out_sec` is `None`).
pub fn spawn(path: PathBuf, in_sec: f64, out_sec: Option<f64>) -> anyhow::Result<DecodeHandle> {
    ff::init()?;
    let (frame_tx, frames) = bounded::<DecodedFrame>(3);
    let (close_tx, close_rx) = bounded::<()>(1);
    let (restart_tx, restart_rx) = bounded::<()>(1);
    let join = std::thread::spawn(move || {
        if let Err(e) = run(&path, &frame_tx, &close_rx, &restart_rx, in_sec, out_sec) {
            log::error!("decode worker for {}: {e:#}", path.display());
        }
    });
    Ok(DecodeHandle {
        frames,
        restart_tx,
        close_tx: Some(close_tx),
        join: Some(join),
    })
}

fn should_stop(close_rx: &Receiver<()>) -> bool {
    !matches!(close_rx.try_recv(), Err(crossbeam_channel::TryRecvError::Empty))
}

/// Drain any pending restart requests; true if at least one was waiting.
fn take_restart(restart_rx: &Receiver<()>) -> bool {
    let mut hit = false;
    while restart_rx.try_recv().is_ok() {
        hit = true;
    }
    hit
}

/// Send a frame, blocking on a full channel but bailing if asked to stop.
/// Returns true if the worker should exit.
fn send_or_stop(tx: &Sender<DecodedFrame>, close_rx: &Receiver<()>, frame: DecodedFrame) -> bool {
    let mut f = frame;
    loop {
        match tx.try_send(f) {
            Ok(()) => return false,
            Err(TrySendError::Disconnected(_)) => return true,
            Err(TrySendError::Full(returned)) => {
                if should_stop(close_rx) {
                    return true;
                }
                f = returned;
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}

/// Sleep so the frame at `pts` seconds appears at the right wall-clock time,
/// relative to the first frame of this playthrough.
fn pace(base: Instant, first_pts: &mut Option<f64>, pts: f64) {
    let fp = *first_pts.get_or_insert(pts);
    let target = base + Duration::from_secs_f64((pts - fp).max(0.0));
    let now = Instant::now();
    if target > now {
        std::thread::sleep(target - now);
    }
}

/// Seek the demuxer to `secs` (clamped at 0), in the container's own timeline.
fn seek_secs(ictx: &mut ff::format::context::Input, secs: f64) -> anyhow::Result<()> {
    let ts = (secs.max(0.0) * 1_000_000.0) as i64; // AV_TIME_BASE microseconds
    ictx.seek(ts, ..)?;
    Ok(())
}

fn run(
    path: &PathBuf,
    tx: &Sender<DecodedFrame>,
    close_rx: &Receiver<()>,
    restart_rx: &Receiver<()>,
    in_sec: f64,
    out_sec: Option<f64>,
) -> anyhow::Result<()> {
    let mut ictx = ff::format::input(path)?;

    let (vid_idx, params, time_base) = {
        let st = ictx
            .streams()
            .best(ff::media::Type::Video)
            .ok_or_else(|| anyhow::anyhow!("no video stream"))?;
        (st.index(), st.parameters(), st.time_base())
    };
    let is_hap = params.id() == ff::codec::Id::HAP;
    let (fourcc, width, height) = unsafe {
        let p = params.as_ptr();
        (
            (*p).codec_tag.to_le_bytes(),
            (*p).width as u32,
            (*p).height as u32,
        )
    };
    let tb = time_base.numerator() as f64 / time_base.denominator() as f64;

    if is_hap {
        let texture_count = if &fourcc == b"HapM" { 2 } else { 1 };
        log::info!(
            "clip {}: HAP {:?} {width}x{height}, {texture_count} texture(s)",
            path.display(),
            std::str::from_utf8(&fourcc).unwrap_or("?")
        );
        run_hap(
            &mut ictx, tx, close_rx, restart_rx, vid_idx, tb, width, height, texture_count,
            in_sec, out_sec,
        )
    } else {
        log::info!(
            "clip {}: software decode {width}x{height} ({:?})",
            path.display(),
            params.id()
        );
        run_software(
            &mut ictx, tx, close_rx, restart_rx, vid_idx, params, tb, in_sec, out_sec,
        )
    }
}

/// Avoid a hot seek-loop when a trim yields no frames (e.g. an in-point past the
/// clip's end): pause briefly before retrying the empty playthrough.
fn guard_empty_playthrough(sent_any: bool) {
    if !sent_any {
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[allow(clippy::too_many_arguments)]
fn run_hap(
    ictx: &mut ff::format::context::Input,
    tx: &Sender<DecodedFrame>,
    close_rx: &Receiver<()>,
    restart_rx: &Receiver<()>,
    vid_idx: usize,
    tb: f64,
    width: u32,
    height: u32,
    texture_count: u8,
    in_sec: f64,
    out_sec: Option<f64>,
) -> anyhow::Result<()> {
    loop {
        // Position at the cue's in-point for this playthrough (also the target
        // of an EOF loop, out-point loop, or musical re-loop restart).
        let _ = seek_secs(ictx, in_sec);
        let base = Instant::now();
        let mut first_pts = None;
        let mut sent_any = false;
        // Prime the restart signal so a request that arrived during the seek
        // doesn't immediately re-fire.
        take_restart(restart_rx);
        for (stream, packet) in ictx.packets() {
            if should_stop(close_rx) {
                return Ok(());
            }
            if take_restart(restart_rx) {
                break; // musical re-loop: reseek at the top of the loop
            }
            if stream.index() != vid_idx {
                continue;
            }
            let pts = packet.pts().unwrap_or(0) as f64 * tb;
            // Loop back once we reach the out-point.
            if out_sec.is_some_and(|o| pts >= o) {
                break;
            }
            // Skip anything the seek landed before the in-point.
            if pts + 1e-6 < in_sec {
                continue;
            }
            let Some(bytes) = packet.data() else { continue };

            let mut main = Vec::new();
            let mut alpha = Vec::new();
            let meta = match hap::decode_frame(bytes, texture_count, &mut main, &mut alpha) {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("HAP frame parse failed: {e}");
                    continue;
                }
            };
            pace(base, &mut first_pts, pts);

            let frame = DecodedFrame {
                pixels: PixelData::Bc {
                    format: meta.format,
                    data: main,
                    alpha: if meta.has_alpha { Some(alpha) } else { None },
                    video_mode: meta.video_mode,
                },
                w: width,
                h: height,
                pts_sec: pts,
            };
            sent_any = true;
            if send_or_stop(tx, close_rx, frame) {
                return Ok(());
            }
        }
        guard_empty_playthrough(sent_any);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_software(
    ictx: &mut ff::format::context::Input,
    tx: &Sender<DecodedFrame>,
    close_rx: &Receiver<()>,
    restart_rx: &Receiver<()>,
    vid_idx: usize,
    params: ff::codec::Parameters,
    tb: f64,
    in_sec: f64,
    out_sec: Option<f64>,
) -> anyhow::Result<()> {
    use ff::format::Pixel;
    use ff::software::scaling;

    let mut decoder = ff::codec::context::Context::from_parameters(params)?
        .decoder()
        .video()?;
    let (w, h) = (decoder.width(), decoder.height());
    let mut scaler = scaling::Context::get(
        decoder.format(),
        w,
        h,
        Pixel::RGBA,
        w,
        h,
        scaling::Flags::BILINEAR,
    )?;

    let send_rgba = |decoded: &ff::frame::Video,
                         scaler: &mut scaling::Context,
                         base: Instant,
                         first_pts: &mut Option<f64>,
                         pts: f64,
                         pace_it: bool|
     -> anyhow::Result<bool> {
        if pace_it {
            pace(base, first_pts, pts);
        }
        let mut rgba = ff::frame::Video::empty();
        scaler.run(decoded, &mut rgba)?;
        let stride = rgba.stride(0) as u32;
        let frame = DecodedFrame {
            pixels: PixelData::Rgba {
                data: rgba.data(0).to_vec(),
                stride,
            },
            w,
            h,
            pts_sec: pts,
        };
        Ok(send_or_stop(tx, close_rx, frame))
    };

    loop {
        // Seek+flush to the in-point for this playthrough.
        let _ = seek_secs(ictx, in_sec);
        decoder.flush();
        let base = Instant::now();
        let mut first_pts = None;
        let mut decoded = ff::frame::Video::empty();
        let mut restarted = false;
        let mut hit_out = false;
        let mut sent_any = false;
        take_restart(restart_rx);

        for (stream, packet) in ictx.packets() {
            if should_stop(close_rx) {
                return Ok(());
            }
            if take_restart(restart_rx) {
                restarted = true;
                break; // musical re-loop: reseek at the top of the loop
            }
            if stream.index() != vid_idx {
                continue;
            }
            if let Err(e) = decoder.send_packet(&packet) {
                log::warn!("decode send_packet failed, skipping packet: {e}");
                continue;
            }
            while decoder.receive_frame(&mut decoded).is_ok() {
                let pts = decoded.pts().unwrap_or(0) as f64 * tb;
                if out_sec.is_some_and(|o| pts >= o) {
                    hit_out = true;
                    break;
                }
                if pts + 1e-6 < in_sec {
                    continue; // seek landed before the in-point; drop
                }
                sent_any = true;
                if send_rgba(&decoded, &mut scaler, base, &mut first_pts, pts, true)? {
                    return Ok(());
                }
            }
            if hit_out {
                break;
            }
        }
        // Natural EOF (not a restart or out-point cut): drain buffered frames.
        if !restarted && !hit_out {
            decoder.send_eof()?;
            while decoder.receive_frame(&mut decoded).is_ok() {
                let pts = decoded.pts().unwrap_or(0) as f64 * tb;
                if out_sec.is_some_and(|o| pts >= o) {
                    break;
                }
                if pts + 1e-6 < in_sec {
                    continue;
                }
                sent_any = true;
                if send_rgba(&decoded, &mut scaler, base, &mut first_pts, pts, true)? {
                    return Ok(());
                }
            }
        }
        guard_empty_playthrough(sent_any);
    }
}
