//! Clip pool: scan a directory for video clips and extract first-frame
//! thumbnails on a background thread. Thumbnails are delivered over a channel so
//! a large pool never blocks the UI.

use std::path::{Path, PathBuf};

use ffmpeg_next as ff;

use crate::commands::ClipId;

const VIDEO_EXTS: &[&str] = &["mov", "mp4", "mkv", "m4v", "avi", "webm", "hap"];
const THUMB_W: u32 = 192;
const THUMB_H: u32 = 108;

/// One source video in the pool.
#[derive(Clone, Debug)]
pub struct Clip {
    pub id: ClipId,
    pub path: PathBuf,
    pub name: String,
}

/// A decoded first-frame preview, delivered from the thumbnailer thread.
pub struct Thumbnail {
    pub id: ClipId,
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

/// List video clips in `dir` (non-recursive), sorted by name, with stable ids.
pub fn scan(dir: &Path) -> Vec<Clip> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| VIDEO_EXTS.contains(&e.to_lowercase().as_str()))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .enumerate()
        .map(|(i, path)| {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("clip")
                .to_string();
            Clip {
                id: i as ClipId,
                path,
                name,
            }
        })
        .collect()
}

/// Spawn a worker that extracts a thumbnail for each clip and streams results.
/// The returned receiver closes when all clips are processed.
pub fn spawn_thumbnailer(clips: Vec<Clip>) -> crossbeam_channel::Receiver<Thumbnail> {
    let (tx, rx) = crossbeam_channel::unbounded();
    std::thread::Builder::new()
        .name("thumbnailer".into())
        .spawn(move || {
            let _ = ff::init();
            for clip in clips {
                match first_frame_rgba(&clip.path, THUMB_W, THUMB_H) {
                    Ok((w, h, rgba)) => {
                        let _ = tx.send(Thumbnail {
                            id: clip.id,
                            w,
                            h,
                            rgba,
                        });
                    }
                    Err(e) => log::warn!("thumbnail failed for {}: {e}", clip.name),
                }
            }
        })
        .ok();
    rx
}

/// Decode the first frame of a clip and scale it to a tight RGBA thumbnail.
fn first_frame_rgba(path: &Path, tw: u32, th: u32) -> anyhow::Result<(usize, usize, Vec<u8>)> {
    let mut ictx = ff::format::input(path)?;
    let (vid_idx, params) = {
        let st = ictx
            .streams()
            .best(ff::media::Type::Video)
            .ok_or_else(|| anyhow::anyhow!("no video stream"))?;
        (st.index(), st.parameters())
    };
    let mut decoder = ff::codec::context::Context::from_parameters(params)?
        .decoder()
        .video()?;
    let mut scaler = ff::software::scaling::Context::get(
        decoder.format(),
        decoder.width(),
        decoder.height(),
        ff::format::Pixel::RGBA,
        tw,
        th,
        ff::software::scaling::Flags::BILINEAR,
    )?;

    let mut frame = ff::frame::Video::empty();
    for (stream, packet) in ictx.packets() {
        if stream.index() != vid_idx {
            continue;
        }
        decoder.send_packet(&packet)?;
        if decoder.receive_frame(&mut frame).is_ok() {
            let mut rgba = ff::frame::Video::empty();
            scaler.run(&frame, &mut rgba)?;
            let stride = rgba.stride(0);
            let row = (tw * 4) as usize;
            let mut packed = vec![0u8; row * th as usize];
            let src = rgba.data(0);
            for y in 0..th as usize {
                packed[y * row..(y + 1) * row]
                    .copy_from_slice(&src[y * stride..y * stride + row]);
            }
            return Ok((tw as usize, th as usize, packed));
        }
    }
    anyhow::bail!("no decodable frame")
}
