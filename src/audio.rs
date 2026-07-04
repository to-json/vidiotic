//! Live audio capture via cpal. Captures from a selectable input device (mic,
//! line-in, or a loopback device like BlackHole), downmixes to mono in the
//! realtime callback with no allocation, and hands samples to the analysis
//! thread through an rtrb ring. The app plays no audio itself — it only listens.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};

use crate::analysis::AudioCtl;

/// Owns the live capture stream. Dropping it stops the callback.
pub struct AudioCapture {
    pub stream: cpal::Stream,
    pub sample_rate: u32,
    pub device_id: Option<cpal::DeviceId>,
    pub device_name: String,
}

/// Enumerate input devices as (stringified id, human name) for the UI/CLI.
pub fn list_input_devices(host: &cpal::Host) -> Vec<(cpal::DeviceId, String)> {
    host.input_devices()
        .map(|it| {
            it.filter_map(|d| {
                let id = d.id().ok()?;
                let name = d
                    .description()
                    .ok()
                    .map(|desc| desc.name().to_string())
                    .unwrap_or_else(|| "unknown".to_string());
                Some((id, name))
            })
            .collect()
        })
        .unwrap_or_default()
}

/// Pick an input device: explicit id, else a case-insensitive substring match on
/// the name, else the default input.
fn resolve_device(
    host: &cpal::Host,
    id: Option<&cpal::DeviceId>,
    name_match: Option<&str>,
) -> anyhow::Result<cpal::Device> {
    if let Some(id) = id {
        return host
            .device_by_id(id)
            .ok_or_else(|| anyhow::anyhow!("input device not available"));
    }
    if let Some(needle) = name_match {
        let needle = needle.to_lowercase();
        if let Ok(devs) = host.input_devices() {
            for d in devs {
                if let Ok(desc) = d.description() {
                    if desc.name().to_lowercase().contains(&needle) {
                        return Ok(d);
                    }
                }
            }
        }
        log::warn!("no input device matched '{needle}', using default");
    }
    host.default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no default input device"))
}

/// Build and start a capture stream. Sends the analysis thread a `SwapSource`
/// with the fresh ring consumer before returning.
pub fn build_capture(
    host: &cpal::Host,
    id: Option<&cpal::DeviceId>,
    name_match: Option<&str>,
    ctl_tx: &crossbeam_channel::Sender<AudioCtl>,
    err_tx: crossbeam_channel::Sender<cpal::Error>,
) -> anyhow::Result<AudioCapture> {
    let device = resolve_device(host, id, name_match)?;
    let device_name = device
        .description()
        .ok()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let supported = device.default_input_config()?;
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.config();
    let channels = config.channels as usize;
    let sample_rate = config.sample_rate;

    // ~1s mono ring; enough to absorb scheduling jitter, small enough to stay live.
    let (mut prod, cons) = rtrb::RingBuffer::<f32>::new(sample_rate as usize);
    // The SendError payload holds the (!Sync) Consumer, so it can't become an
    // anyhow error — discard it and report a plain message.
    ctl_tx
        .send(AudioCtl::SwapSource {
            consumer: cons,
            sample_rate,
        })
        .map_err(|_| anyhow::anyhow!("analysis thread is gone"))?;

    let err_cb = move |e: cpal::Error| {
        let _ = err_tx.try_send(e);
    };
    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream::<f32, _, _>(
            config,
            move |data, _| push_mono_f32(&mut prod, data, channels),
            err_cb,
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream::<i16, _, _>(
            config,
            move |data, _| push_mono_i16(&mut prod, data, channels),
            err_cb,
            None,
        )?,
        SampleFormat::U16 => device.build_input_stream::<u16, _, _>(
            config,
            move |data, _| push_mono_u16(&mut prod, data, channels),
            err_cb,
            None,
        )?,
        other => anyhow::bail!("unsupported sample format {other:?}"),
    };
    stream.play()?; // cpal 0.18 streams start paused

    Ok(AudioCapture {
        stream,
        sample_rate,
        device_id: device.id().ok(),
        device_name,
    })
}

// Realtime callbacks: no allocation, no locking. On a full ring, drop samples —
// a missed hop is invisible in the visualization.
fn push_mono_f32(prod: &mut rtrb::Producer<f32>, data: &[f32], channels: usize) {
    let frames = (data.len() / channels).min(prod.slots());
    if frames == 0 {
        return;
    }
    if let Ok(chunk) = prod.write_chunk_uninit(frames) {
        if channels == 1 {
            chunk.fill_from_iter(data[..frames].iter().copied());
        } else {
            chunk.fill_from_iter(
                data.chunks_exact(channels)
                    .take(frames)
                    .map(|f| f.iter().sum::<f32>() / channels as f32),
            );
        }
    }
}

fn push_mono_i16(prod: &mut rtrb::Producer<f32>, data: &[i16], channels: usize) {
    let frames = (data.len() / channels).min(prod.slots());
    if frames == 0 {
        return;
    }
    if let Ok(chunk) = prod.write_chunk_uninit(frames) {
        chunk.fill_from_iter(
            data.chunks_exact(channels)
                .take(frames)
                .map(|f| f.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / channels as f32),
        );
    }
}

fn push_mono_u16(prod: &mut rtrb::Producer<f32>, data: &[u16], channels: usize) {
    let frames = (data.len() / channels).min(prod.slots());
    if frames == 0 {
        return;
    }
    if let Ok(chunk) = prod.write_chunk_uninit(frames) {
        chunk.fill_from_iter(
            data.chunks_exact(channels)
                .take(frames)
                .map(|f| f.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).sum::<f32>() / channels as f32),
        );
    }
}
