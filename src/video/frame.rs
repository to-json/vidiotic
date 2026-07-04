//! Decoded video frames handed from a decode worker to the render thread.
//! Plain data only (no ffmpeg/wgpu types) so it crosses thread boundaries and
//! unit-tests freely.

use super::hap::HapTextureFormat;

pub enum PixelData {
    /// GPU-native block-compressed texels (HAP fast path). `alpha` is the BC4
    /// plane for HapM; `video_mode` is the shader composite mode.
    Bc {
        format: HapTextureFormat,
        data: Vec<u8>,
        alpha: Option<Vec<u8>>,
        video_mode: i32,
    },
    /// Software-decoded RGBA8 (fallback path). `stride` is bytes per row (may
    /// exceed w*4 due to ffmpeg row padding).
    Rgba { data: Vec<u8>, stride: u32 },
}

pub struct DecodedFrame {
    pub pixels: PixelData,
    pub w: u32,
    pub h: u32,
    pub pts_sec: f64,
}

impl PixelData {
    pub fn video_mode(&self) -> i32 {
        match self {
            PixelData::Bc { video_mode, .. } => *video_mode,
            PixelData::Rgba { .. } => 0,
        }
    }
}
