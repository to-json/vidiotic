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

/// Transcode the `[in_sec, out_sec)` span of `input` to a HAP1 `.mov`.
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
pub fn run_span(
    input: &Path,
    output: &Path,
    in_sec: f64,
    out_sec: Option<f64>,
) -> anyhow::Result<TranscodeReport> {
    ff::init()?;

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

    let mut decoder = ff::codec::context::Context::from_parameters(params)?
        .decoder()
        .video()?;
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

    // Returns Ok(true) once a frame at/after out_sec is seen (stop the demux).
    let mut process = |decoder: &mut ff::decoder::Video,
                       scaler: &mut ff::software::scaling::Context,
                       octx: &mut ff::format::context::Output|
     -> anyhow::Result<bool> {
        while decoder.receive_frame(&mut decoded).is_ok() {
            let src_sec = decoded.pts().unwrap_or(0) as f64 * in_tb;
            // Seek lands on a keyframe ≤ in_sec; skip anything before the in-point.
            if src_sec + 1e-6 < in_sec {
                continue;
            }
            // Reached the out-point: nothing more to emit.
            if out_sec.is_some_and(|o| src_sec >= o) {
                return Ok(true);
            }

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
            Format::Bc1.compress(&packed, w as usize, h as usize, Params::default(), &mut bc1);

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
            idx += 1;
        }
        Ok(false)
    };

    let mut reached_out = false;
    for (stream, packet) in ictx.packets() {
        if stream.index() != vid_idx {
            continue;
        }
        decoder.send_packet(&packet)?;
        if process(&mut decoder, &mut scaler, &mut octx)? {
            reached_out = true;
            break;
        }
    }
    if !reached_out {
        decoder.send_eof()?;
        process(&mut decoder, &mut scaler, &mut octx)?;
    }

    octx.write_trailer()?;
    let duration_sec = if fps > 0.0 { idx as f64 / fps } else { 0.0 };
    log::info!(
        "transcoded {} frames -> {} (Hap1, {w}x{h}, {fps:.2} fps)",
        idx,
        output.display()
    );
    Ok(TranscodeReport {
        width: w,
        height: h,
        fps,
        frames: idx as u64,
        duration_sec,
    })
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
