//! Self-contained HAP transcoder: decode any clip with ffmpeg-next, block-
//! compress each frame to DXT1 (texpresso), wrap as a Snappy HAP1 frame, and mux
//! into a `QuickTime` `.mov`. This exists because a stock Homebrew ffmpeg is built
//! without libsnappy and therefore has no `-c:v hap` encoder — so the app ships
//! its own, and the resulting clips play back on the near-zero-CPU HAP path.
//!
//! [`run_span`] additionally bakes a frame-accurate sub-range of the source,
//! which the `vidiotic-prep` authoring tool uses to export selected spans as
//! standalone clips.

use std::path::Path;

use ffmpeg_next as ff;
use texpresso::{Format, Params};

use crate::video::hap;

const HAP1_TAG: u32 = u32::from_le_bytes(*b"Hap1");
const OUT_TIMESCALE: i32 = 1000; // millisecond output time base pre-header

/// What a transcode produced: baked dimensions (after the 4-px alignment crop),
/// frame rate, emitted frame count, and duration. `vidiotic-prep` records these
/// as clip metadata.
#[derive(Clone, Copy, Debug)]
pub struct TranscodeReport {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub frames: u64,
    pub duration_sec: f64,
}

/// Transcode `input` (any decodable video) to a HAP1 `.mov` at `output`.
///
/// # Errors
/// Propagates ffmpeg initialization, demux/decode, and mux/write failures.
///
/// # Panics
/// Panics if the output stream just added to the muxer cannot be read back.
pub fn run(input: &Path, output: &Path) -> anyhow::Result<()> {
    run_span(input, output, 0.0, None).map(|_| ())
}

/// [`run_span_with`] without progress reporting, at [`BakeQuality::High`]
/// (the pre-existing quality of whole-file transcodes).
///
/// # Errors
/// See [`run_span_with`].
pub fn run_span(
    input: &Path,
    output: &Path,
    in_sec: f64,
    out_sec: Option<f64>,
) -> anyhow::Result<TranscodeReport> {
    run_span_with(input, output, in_sec, out_sec, BakeQuality::High, |_| {})
}

/// Live position of an in-flight [`run_span_with`] bake, reported once per
/// decoded frame. `src_sec` advances even while pre-in frames are being
/// skipped, so a caller can distinguish "decoding toward the in-point" (or a
/// pts mismatch) from a stall.
#[derive(Clone, Copy, Debug)]
pub struct BakeUpdate {
    /// Frames emitted to the output so far.
    pub emitted: u64,
    /// Source timestamp of the frame just decoded.
    pub src_sec: f64,
}


/// Transcode the `[in_sec, out_sec)` span of `input` to a HAP1 `.mov`,
/// invoking `progress` once per decoded frame.
///
/// The demuxer seeks to the keyframe at or before `in_sec`; frames whose source
/// pts precede `in_sec` are decoded (for inter-frame correctness) but not
/// emitted, and decoding stops once a frame's pts reaches `out_sec`. Output pts
/// is re-baselined so the file always begins at t=0. Pass `in_sec = 0.0` and
/// `out_sec = None` for a whole-file transcode.
///
/// # Errors
/// Propagates ffmpeg initialization, seek, demux/decode, and mux/write failures.
///
/// # Panics
/// Panics if the output stream just added to the muxer cannot be read back.
pub fn run_span_with(
    input: &Path,
    output: &Path,
    in_sec: f64,
    out_sec: Option<f64>,
    quality: BakeQuality,
    mut progress: impl FnMut(BakeUpdate),
) -> anyhow::Result<TranscodeReport> {
    ff::init()?;
    let started = std::time::Instant::now();
    let bc1_params = quality.params();

    let mut ictx = ff::format::input(input)?;
    let (vid_idx, params, fps, in_tb) = {
        let st = ictx
            .streams()
            .best(ff::media::Type::Video)
            .ok_or_else(|| anyhow::anyhow!("no video stream in {}", input.display()))?;
        let rate = st.avg_frame_rate();
        let fps = if rate.denominator() != 0 && rate.numerator() != 0 {
            rate.numerator() as f64 / rate.denominator() as f64
        } else {
            30.0
        };
        // Stream time base, for turning a decoded frame's pts into seconds.
        let tb = st.time_base();
        let in_tb = if tb.denominator() != 0 {
            tb.numerator() as f64 / tb.denominator() as f64
        } else {
            0.0
        };
        (st.index(), st.parameters(), fps, in_tb)
    };

    let mut dec_ctx = ff::codec::context::Context::from_parameters(params)?;
    // Frame-threaded decoding (count 0 = auto): the source codec (h264 etc.) is
    // often the bake bottleneck, not BC1.
    dec_ctx.set_threading(ff::codec::threading::Config::kind(
        ff::codec::threading::Type::Frame,
    ));
    let mut decoder = dec_ctx.decoder().video()?;
    // HAP/BC1 works on 4×4 blocks and the render path copies block rows assuming
    // aligned dimensions, so crop to a multiple of 4 (at most 3 px per side).
    let (sw, sh) = (decoder.width(), decoder.height());
    let (w, h) = (sw & !3, sh & !3);
    anyhow::ensure!(w >= 4 && h >= 4, "video too small to transcode: {sw}x{sh}");
    let mut scaler = ff::software::scaling::Context::get(
        decoder.format(),
        sw,
        sh,
        ff::format::Pixel::RGBA,
        w,
        h,
        ff::software::scaling::Flags::BILINEAR,
    )?;

    // --- output ---
    let mut octx = ff::format::output(output)?;
    {
        let mut stream = octx.add_stream(ff::codec::Id::HAP)?;
        stream.set_time_base((1, OUT_TIMESCALE));
        // add_stream(Id::HAP) creates a null-codec stream (no encoder exists);
        // fill its codec parameters directly so the mov muxer tags it 'Hap1'.
        unsafe {
            let st = stream.as_mut_ptr();
            let par = (*st).codecpar;
            (*par).codec_type = ff::sys::AVMediaType::AVMEDIA_TYPE_VIDEO;
            (*par).codec_id = ff::sys::AVCodecID::AV_CODEC_ID_HAP;
            (*par).codec_tag = HAP1_TAG;
            (*par).width = w as i32;
            (*par).height = h as i32;
            (*par).format = ff::sys::AVPixelFormat::AV_PIX_FMT_RGBA as i32;
        }
    }
    octx.write_header()?;
    // The muxer may pick its own timescale; capture it to rescale packet pts.
    let out_tb = octx.stream(0).expect("stream 0 was just added").time_base();

    // Seek to (or just before) the in-point; flush so no pre-seek frames leak.
    if in_sec > 0.0 {
        seek_secs(&mut ictx, in_sec)?;
        decoder.flush();
    }

    let bc_size = Format::Bc1.compressed_size(w as usize, h as usize);
    let mut bc1 = vec![0u8; bc_size];
    let mut packed = vec![0u8; (w * h * 4) as usize];

    let mut decoded = ff::frame::Video::empty();
    let mut idx: i64 = 0; // count of *emitted* frames — the re-baselined pts index
    let mut skipped: u64 = 0; // decoded-but-dropped pre-in frames
    let mut stages = StageTimes::default();

    // Returns Ok(true) once a frame at/after out_sec is seen (stop the demux).
    let mut process = |decoder: &mut ff::decoder::Video,
                       scaler: &mut ff::software::scaling::Context,
                       octx: &mut ff::format::context::Output,
                       stages: &mut StageTimes|
     -> anyhow::Result<bool> {
        loop {
            let t0 = std::time::Instant::now();
            let got = decoder.receive_frame(&mut decoded).is_ok();
            stages.decode += t0.elapsed();
            if !got {
                return Ok(false);
            }
            let src_sec = decoded.pts().unwrap_or(0) as f64 * in_tb;
            progress(BakeUpdate { emitted: idx as u64, src_sec });
            // Seek lands on a keyframe ≤ in_sec; skip anything before the in-point.
            if src_sec + 1e-6 < in_sec {
                skipped += 1;
                continue;
            }
            // Reached the out-point: nothing more to emit.
            if out_sec.is_some_and(|o| src_sec >= o) {
                return Ok(true);
            }

            let t0 = std::time::Instant::now();
            let mut rgba = ff::frame::Video::empty();
            scaler.run(&decoded, &mut rgba)?;

            // Repack rows to a tight width*4 stride for texpresso.
            let src = rgba.data(0);
            let stride = rgba.stride(0);
            let row = (w * 4) as usize;
            for y in 0..h as usize {
                packed[y * row..(y + 1) * row]
                    .copy_from_slice(&src[y * stride..y * stride + row]);
            }
            stages.scale += t0.elapsed();

            let t0 = std::time::Instant::now();
            Format::Bc1.compress(&packed, w as usize, h as usize, bc1_params, &mut bc1);
            stages.bc1 += t0.elapsed();

            let t0 = std::time::Instant::now();
            let hap_frame = hap::encode_hap1_frame(&bc1);
            let mut pkt = ff::codec::packet::Packet::copy(&hap_frame);
            // Re-baselined pts at millisecond timescale, rescaled to the muxer's tb.
            let pts_ms = (idx as f64 * 1000.0 / fps).round() as i64;
            let pts = rescale(pts_ms, (1, OUT_TIMESCALE), out_tb);
            pkt.set_pts(Some(pts));
            pkt.set_dts(Some(pts));
            pkt.set_stream(0);
            pkt.set_flags(ff::codec::packet::Flags::KEY); // HAP is all-intra
            pkt.write_interleaved(octx)?;
            stages.mux += t0.elapsed();
            idx += 1;
        }
    };

    let mut reached_out = false;
    for (stream, packet) in ictx.packets() {
        if stream.index() != vid_idx {
            continue;
        }
        let t0 = std::time::Instant::now();
        decoder.send_packet(&packet)?;
        stages.decode += t0.elapsed();
        if process(&mut decoder, &mut scaler, &mut octx, &mut stages)? {
            reached_out = true;
            break;
        }
    }
    if !reached_out {
        decoder.send_eof()?;
        process(&mut decoder, &mut scaler, &mut octx, &mut stages)?;
    }
    stages.log(idx.max(1) as u32);

    octx.write_trailer()?;
    let duration_sec = if fps > 0.0 { idx as f64 / fps } else { 0.0 };
    let elapsed = started.elapsed().as_secs_f64();
    log::info!(
        "transcoded {idx} frames (skipped {skipped} pre-in) -> {} (Hap1, {w}x{h}, {fps:.2} fps) in {elapsed:.1}s = {:.1} enc f/s",
        output.display(),
        idx as f64 / elapsed.max(1e-9),
    );
    if idx == 0 {
        log::warn!(
            "bake emitted 0 frames: source pts never reached [{in_sec:.3}..{:?})s — \
             check the source's timestamps against the requested span",
            out_sec
        );
    }
    Ok(TranscodeReport {
        width: w,
        height: h,
        fps,
        frames: idx as u64,
        duration_sec,
    })
}

/// BC1 encoder quality/speed trade-off. `texpresso` is rayon-parallel either
/// way; at 1080p `Draft` (`RangeFit`) block compression is ~6x faster than
/// `High` (`ClusterFit`) and the difference dominates bake time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BakeQuality {
    /// `RangeFit`: fast, slightly worse gradients. Right for iterating.
    #[default]
    Draft,
    /// `ClusterFit`: texpresso's default quality, several times slower.
    High,
}

impl BakeQuality {
    fn params(self) -> Params {
        let algorithm = match self {
            Self::Draft => texpresso::Algorithm::RangeFit,
            Self::High => texpresso::Algorithm::ClusterFit,
        };
        Params { algorithm, ..Params::default() }
    }
}

/// Wall-clock spent per bake stage, for the debug-level breakdown log.
#[derive(Default)]
struct StageTimes {
    decode: std::time::Duration,
    scale: std::time::Duration,
    bc1: std::time::Duration,
    mux: std::time::Duration,
}

impl StageTimes {
    fn log(&self, frames: u32) {
        log::debug!(
            "bake stages (ms/frame over {frames}): decode {:.1}, scale+pack {:.1}, bc1 {:.1}, hap+mux {:.1}",
            self.decode.as_secs_f64() * 1000.0 / f64::from(frames),
            self.scale.as_secs_f64() * 1000.0 / f64::from(frames),
            self.bc1.as_secs_f64() * 1000.0 / f64::from(frames),
            self.mux.as_secs_f64() * 1000.0 / f64::from(frames),
        );
    }
}

/// Seek the demuxer to `secs` (clamped at 0), in the container's own timeline.
fn seek_secs(ictx: &mut ff::format::context::Input, secs: f64) -> anyhow::Result<()> {
    let ts = (secs.max(0.0) * 1_000_000.0) as i64; // AV_TIME_BASE microseconds
    ictx.seek(ts, ..)?;
    Ok(())
}

fn rescale(pts: i64, from: (i32, i32), to: ff::Rational) -> i64 {
    let num = from.0 as i128 * to.denominator() as i128;
    let den = from.1 as i128 * to.numerator() as i128;
    if den == 0 {
        return pts;
    }
    ((pts as i128 * num) / den) as i64
}
