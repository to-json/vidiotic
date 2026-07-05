//! Audio analysis thread: pull mono samples from the capture ring, window +
//! FFT them, bin into 21 log-spaced perceptual bands with fast-attack/slow-decay
//! smoothing, and publish to the render thread wait-free. The band math is
//! ported from throw-shade/src/audio.rs, with the sample rate parameterized so
//! band bin edges follow the actual capture device.

use std::sync::Arc;
use std::time::Duration;

use rustfft::{num_complex::Complex, Fft, FftPlanner};

pub const FFT_SIZE: usize = 2048;
pub const NUM_BANDS: usize = 21;
const ATTACK: f32 = 0.7; // throw-shade values, unchanged
const DECAY: f32 = 0.88;

/// Shadertoy-style audio texture geometry: 512 texels wide, 2 rows.
/// Row 0 = FFT spectrum (sampled at y=0.25), row 1 = waveform (y=0.75).
pub const AUDIO_TEX_W: usize = 512;
pub const AUDIO_TEX_LEN: usize = AUDIO_TEX_W * 2;
/// Compression divisor for the normalized spectrum row (matches the demo
/// shaders' `log(1+mag)/8` convention so raw magnitudes land in 0..1).
const SPEC_LOG_SCALE: f32 = 8.0;

#[derive(Clone, Copy)]
pub struct AudioFrame {
    pub bands: [f32; NUM_BANDS],
    pub level: f32,
    /// Packed 512x2 R8 audio texture, row-major: `[0..512]` = FFT spectrum
    /// (linear frequency DC..Nyquist, normalized 0..1), `[512..1024]` = waveform
    /// (raw PCM, 0..1 centered on 0.5). Uploaded verbatim as a Shadertoy iChannel.
    pub audio_tex: [u8; AUDIO_TEX_LEN],
}

impl Default for AudioFrame {
    fn default() -> Self {
        let mut audio_tex = [0u8; AUDIO_TEX_LEN];
        // Silence: flat spectrum (0), waveform centered at 0.5.
        for w in &mut audio_tex[AUDIO_TEX_W..] {
            *w = 128;
        }
        AudioFrame {
            bands: [0.0; NUM_BANDS],
            level: 0.0,
            audio_tex,
        }
    }
}

/// Control messages from the main thread to the analysis thread.
pub enum AudioCtl {
    /// A new capture source: its ring consumer and sample rate. The old consumer
    /// is dropped, band edges recomputed, and smoothing state reset.
    SwapSource {
        consumer: rtrb::Consumer<f32>,
        sample_rate: u32,
    },
    Shutdown,
}

/// Log-spaced band boundaries (FFT bin ranges), 20 Hz..20 kHz. Ported verbatim
/// from throw-shade with the sample rate as a parameter.
fn log_bands(sample_rate: f32) -> [(usize, usize); NUM_BANDS] {
    let mut bounds = [(0usize, 0usize); NUM_BANDS];
    let (log_min, log_max) = (20.0f32.ln(), 20000.0f32.ln());
    for i in 0..NUM_BANDS {
        let f_lo = (log_min + (log_max - log_min) * i as f32 / NUM_BANDS as f32).exp();
        let f_hi = (log_min + (log_max - log_min) * (i + 1) as f32 / NUM_BANDS as f32).exp();
        let b_lo = (f_lo * FFT_SIZE as f32 / sample_rate).round() as usize;
        let b_hi = (f_hi * FFT_SIZE as f32 / sample_rate).round() as usize;
        bounds[i] = (b_lo.max(1), b_hi.max(b_lo + 1).min(FFT_SIZE / 2));
    }
    bounds
}

pub fn run(ctl_rx: crossbeam_channel::Receiver<AudioCtl>, mut tri_in: triple_buffer::Input<AudioFrame>) {
    let mut planner = FftPlanner::<f32>::new();
    let fft: Arc<dyn Fft<f32>> = planner.plan_fft_forward(FFT_SIZE);
    let mut scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
    let mut buf = [Complex { re: 0.0, im: 0.0 }; FFT_SIZE];

    let mut window = [0.0f32; FFT_SIZE];
    for (i, w) in window.iter_mut().enumerate() {
        *w = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos());
    }

    let mut samples = [0.0f32; FFT_SIZE]; // sliding window of the most recent input
    let mut smoothed = [0.0f32; NUM_BANDS];
    let mut spec_smoothed = [0.0f32; AUDIO_TEX_W]; // per-bin state for the FFT texture row
    let mut cons: Option<rtrb::Consumer<f32>> = None;
    let mut bands = log_bands(48000.0);
    let mut hop = 48000usize / 60; // ~60 Hz updates

    loop {
        match ctl_rx.try_recv() {
            Ok(AudioCtl::SwapSource {
                consumer,
                sample_rate,
            }) => {
                cons = Some(consumer);
                bands = log_bands(sample_rate as f32);
                hop = (sample_rate as usize / 60).max(64);
                samples.fill(0.0);
                smoothed.fill(0.0);
                spec_smoothed.fill(0.0);
            }
            Ok(AudioCtl::Shutdown) => return,
            Err(crossbeam_channel::TryRecvError::Empty) => {}
            Err(crossbeam_channel::TryRecvError::Disconnected) => return,
        }

        let Some(c) = cons.as_mut() else {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        if c.slots() < hop {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }

        // Slide the window left by `hop` and append the newest `hop` samples.
        samples.copy_within(hop.., 0);
        if let Ok(chunk) = c.read_chunk(hop) {
            let (a, b) = chunk.as_slices();
            let dst = &mut samples[FFT_SIZE - hop..];
            dst[..a.len()].copy_from_slice(a);
            dst[a.len()..a.len() + b.len()].copy_from_slice(b);
            chunk.commit_all();
        }

        for i in 0..FFT_SIZE {
            buf[i] = Complex {
                re: samples[i] * window[i],
                im: 0.0,
            };
        }
        fft.process_with_scratch(&mut buf, &mut scratch);

        let mut band_mag = [0.0f32; NUM_BANDS];
        for (i, &(lo, hi)) in bands.iter().enumerate() {
            let mut sum = 0.0f32;
            let count = (hi - lo).max(1) as f32;
            for bin in lo..hi {
                let c = buf[bin];
                sum += (c.re * c.re + c.im * c.im).sqrt();
            }
            band_mag[i] = sum / count;
        }
        for i in 0..NUM_BANDS {
            if band_mag[i] > smoothed[i] {
                smoothed[i] = smoothed[i] * (1.0 - ATTACK) + band_mag[i] * ATTACK;
            } else {
                smoothed[i] *= DECAY;
            }
        }

        // Shadertoy-style 512x2 audio texture.
        let mut audio_tex = [0u8; AUDIO_TEX_LEN];
        // Row 0: linear-frequency spectrum. The 2048-pt FFT gives 1024 usable
        // bins (DC..Nyquist); average adjacent pairs down to 512, log-compress
        // to 0..1, and apply the same attack/decay smoothing as the bands.
        for i in 0..AUDIO_TEX_W {
            let a = buf[2 * i];
            let b = buf[2 * i + 1];
            let mag = 0.5
                * ((a.re * a.re + a.im * a.im).sqrt() + (b.re * b.re + b.im * b.im).sqrt());
            let v = ((1.0 + mag).ln() / SPEC_LOG_SCALE).clamp(0.0, 1.0);
            if v > spec_smoothed[i] {
                spec_smoothed[i] = spec_smoothed[i] * (1.0 - ATTACK) + v * ATTACK;
            } else {
                spec_smoothed[i] *= DECAY;
            }
            audio_tex[i] = (spec_smoothed[i] * 255.0).round() as u8;
        }
        // Row 1: waveform = the most recent 512 raw (un-windowed) samples,
        // mapped to 0..1 centered on 0.5.
        for i in 0..AUDIO_TEX_W {
            let s = samples[FFT_SIZE - AUDIO_TEX_W + i];
            audio_tex[AUDIO_TEX_W + i] = ((s * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
        }

        tri_in.write(AudioFrame {
            bands: smoothed,
            level: smoothed[0] + smoothed[1] + smoothed[2],
            audio_tex,
        });
    }
}
