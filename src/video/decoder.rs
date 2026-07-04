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
use crate::video::hap::{self, HapTextureFormat};

/// Handle to a running decode worker. Dropping it stops and joins the thread.
pub struct DecodeHandle {
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

pub fn spawn(path: PathBuf) -> anyhow::Result<DecodeHandle> {
    ff::init()?;
    let (frame_tx, frames) = bounded::<DecodedFrame>(3);
    let (close_tx, close_rx) = bounded::<()>(1);
    let (restart_tx, restart_rx) = bounded::<()>(1);
    let join = std::thread::spawn(move || {
        if let Err(e) = run(&path, &frame_tx, &close_rx, &restart_rx) {
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

fn run(
    path: &PathBuf,
    tx: &Sender<DecodedFrame>,
    close_rx: &Receiver<()>,
    restart_rx: &Receiver<()>,
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
        )
    } else {
        log::info!(
            "clip {}: software decode {width}x{height} ({:?})",
            path.display(),
            params.id()
        );
        run_software(&mut ictx, tx, close_rx, restart_rx, vid_idx, params, tb)
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
) -> anyhow::Result<()> {
    loop {
        let base = Instant::now();
        let mut first_pts = None;
        // Prime the restart signal at the start of each playthrough so a request
        // that arrived during the seek doesn't immediately re-fire.
        take_restart(restart_rx);
        for (stream, packet) in ictx.packets() {
            if should_stop(close_rx) {
                return Ok(());
            }
            if take_restart(restart_rx) {
                break; // musical re-loop: seek to start (handled after the loop)
            }
            if stream.index() != vid_idx {
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
            let pts = packet.pts().unwrap_or(0) as f64 * tb;
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
            if send_or_stop(tx, close_rx, frame) {
                return Ok(());
            }
        }
        // EOF -> loop back to the start.
        let _ = hap_seek_start(ictx);
    }
}

fn hap_seek_start(ictx: &mut ff::format::context::Input) -> anyhow::Result<()> {
    ictx.seek(0, ..)?;
    Ok(())
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

    loop {
        let base = Instant::now();
        let mut first_pts = None;
        let mut decoded = ff::frame::Video::empty();
        let mut restarted = false;
        take_restart(restart_rx);

        for (stream, packet) in ictx.packets() {
            if should_stop(close_rx) {
                return Ok(());
            }
            if take_restart(restart_rx) {
                restarted = true;
                break; // musical re-loop: seek to start without draining the decoder
            }
            if stream.index() != vid_idx {
                continue;
            }
            decoder.send_packet(&packet)?;
            while decoder.receive_frame(&mut decoded).is_ok() {
                let pts = decoded.pts().unwrap_or(0) as f64 * tb;
                pace(base, &mut first_pts, pts);
                let mut rgba = ff::frame::Video::empty();
                scaler.run(&decoded, &mut rgba)?;
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
                if send_or_stop(tx, close_rx, frame) {
                    return Ok(());
                }
            }
        }
        // On natural EOF, flush the decoder's buffered frames before looping.
        // On a musical re-loop we skip the flush and cut straight to the start.
        if !restarted {
            decoder.send_eof()?;
            while decoder.receive_frame(&mut decoded).is_ok() {
                let mut rgba = ff::frame::Video::empty();
                scaler.run(&decoded, &mut rgba)?;
                let stride = rgba.stride(0) as u32;
                let frame = DecodedFrame {
                    pixels: PixelData::Rgba {
                        data: rgba.data(0).to_vec(),
                        stride,
                    },
                    w,
                    h,
                    pts_sec: decoded.pts().unwrap_or(0) as f64 * tb,
                };
                if send_or_stop(tx, close_rx, frame) {
                    return Ok(());
                }
            }
        }
        ictx.seek(0, ..)?;
        decoder.flush();
    }
}

/// True if the format's decoded frames use the BC alpha texture (HapM only).
pub fn is_bc4(format: HapTextureFormat) -> bool {
    matches!(format, HapTextureFormat::Bc4)
}
