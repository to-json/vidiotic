//! Self-contained HAP transcoder: decode any clip with ffmpeg-next, block-
//! compress each frame to DXT1 (texpresso), wrap as a Snappy HAP1 frame, and mux
//! into a `QuickTime` `.mov`. This exists because a stock Homebrew ffmpeg is built
//! without libsnappy and therefore has no `-c:v hap` encoder — so the app ships
//! its own, and the resulting clips play back on the near-zero-CPU HAP path.

use std::path::Path;

use ffmpeg_next as ff;
use texpresso::{Format, Params};

use crate::video::hap;

const HAP1_TAG: u32 = u32::from_le_bytes(*b"Hap1");
const OUT_TIMESCALE: i32 = 1000; // millisecond output time base pre-header

/// Transcode `input` (any decodable video) to a HAP1 `.mov` at `output`.
///
/// # Errors
/// Propagates ffmpeg initialization, demux/decode, and mux/write failures.
///
/// # Panics
/// Panics if the output stream just added to the muxer cannot be read back.
pub fn run(input: &Path, output: &Path) -> anyhow::Result<()> {
    ff::init()?;

    let mut ictx = ff::format::input(input)?;
    let (vid_idx, params, fps) = {
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
        (st.index(), st.parameters(), fps)
    };

    let mut decoder = ff::codec::context::Context::from_parameters(params)?
        .decoder()
        .video()?;
    let (w, h) = (decoder.width(), decoder.height());
    let mut scaler = ff::software::scaling::Context::get(
        decoder.format(),
        w,
        h,
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

    let bc_size = Format::Bc1.compressed_size(w as usize, h as usize);
    let mut bc1 = vec![0u8; bc_size];
    let mut packed = vec![0u8; (w * h * 4) as usize];

    let mut decoded = ff::frame::Video::empty();
    let mut idx: i64 = 0; // output frame index, also the frame count for the log

    let mut process = |decoder: &mut ff::decoder::Video,
                       scaler: &mut ff::software::scaling::Context,
                       octx: &mut ff::format::context::Output|
     -> anyhow::Result<()> {
        while decoder.receive_frame(&mut decoded).is_ok() {
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
            // pts at millisecond timescale, then rescale to the muxer's tb.
            let pts_ms = (idx as f64 * 1000.0 / fps).round() as i64;
            let pts = rescale(pts_ms, (1, OUT_TIMESCALE), out_tb);
            pkt.set_pts(Some(pts));
            pkt.set_dts(Some(pts));
            pkt.set_stream(0);
            pkt.set_flags(ff::codec::packet::Flags::KEY); // HAP is all-intra
            pkt.write_interleaved(octx)?;
            idx += 1;
        }
        Ok(())
    };

    for (stream, packet) in ictx.packets() {
        if stream.index() != vid_idx {
            continue;
        }
        decoder.send_packet(&packet)?;
        process(&mut decoder, &mut scaler, &mut octx)?;
    }
    decoder.send_eof()?;
    process(&mut decoder, &mut scaler, &mut octx)?;

    octx.write_trailer()?;
    log::info!(
        "transcoded {} frames -> {} (Hap1, {w}x{h}, {fps:.2} fps)",
        idx,
        output.display()
    );
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
