//! Camera capture (macOS). `AVFoundation` supplies device enumeration, stable
//! identity (`uniqueID`), and TCC permission state; the frames themselves come
//! through ffmpeg's `avfoundation` input device on the already-linked
//! ffmpeg-next, so camera packets ride the same decode surface as file clips.
//!
//! ffmpeg's demuxer selects a device by its position in ffmpeg's own discovery
//! list. [`enumerate`] reproduces that list exactly — same device types in the
//! same order, video devices then muxed devices — so a [`DeviceInfo::index`]
//! can be handed straight to the demuxer as `video_device_index`. The mapping
//! is only valid against a fresh enumeration: resolve uid → index at open
//! time, never cache the index.

use anyhow::ensure;
use ffmpeg_next as ff;
use objc2::rc::Retained;
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureDevice, AVCaptureDeviceDiscoverySession,
    AVCaptureDevicePosition, AVCaptureDeviceType, AVCaptureDeviceTypeBuiltInWideAngleCamera,
    AVCaptureDeviceTypeContinuityCamera, AVCaptureDeviceTypeDeskViewCamera,
    AVCaptureDeviceTypeExternal, AVMediaType, AVMediaTypeMuxed, AVMediaTypeVideo,
};
use objc2_core_media::CMVideoFormatDescriptionGetDimensions;
use objc2_foundation::NSArray;

/// TCC camera permission state, mirroring `AVAuthorizationStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authorization {
    /// The user has never been asked; capture will trigger the system prompt.
    NotDetermined,
    /// Blocked by policy (parental controls / MDM); no prompt possible.
    Restricted,
    /// The user said no; capture yields black frames until they flip it in
    /// System Settings.
    Denied,
    Authorized,
}

/// One capture format a device offers.
#[derive(Debug, Clone)]
pub struct DeviceFormat {
    pub width: u32,
    pub height: u32,
    /// Pixel/codec four-char code as reported by the format description
    /// (e.g. `420v`, `2vuy`, `dmb1` for MJPEG).
    pub fourcc: [u8; 4],
    pub min_fps: f64,
    pub max_fps: f64,
}

impl DeviceFormat {
    /// The four-char code as printable text.
    pub fn fourcc_str(&self) -> String {
        self.fourcc
            .iter()
            .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' })
            .collect()
    }

    /// The ffmpeg `pixel_format` option value matching this format's
    /// `CoreMedia` four-char code, where one exists. Compressed-native devices
    /// (MJPEG four-char `dmb1`) have no mapping — `AVFoundation` decompresses
    /// those to a CV format itself, so the demuxer's default-with-fallback
    /// handles them.
    pub fn ffmpeg_pixel_format(&self) -> Option<&'static str> {
        match &self.fourcc {
            b"420v" | b"420f" => Some("nv12"),
            b"2vuy" => Some("uyvy422"),
            b"yuvs" => Some("yuyv422"),
            b"BGRA" => Some("bgra"),
            _ => None,
        }
    }
}

/// A capture device as seen at one enumeration. Plain data — safe to move
/// across threads, valid until devices are plugged or unplugged.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Position in ffmpeg's avfoundation device list; pass as
    /// `video_device_index`. Valid only against a fresh [`enumerate`].
    pub index: usize,
    /// `AVFoundation` `uniqueID` — the stable identity that survives replugs
    /// and reboots. This is what a project file stores.
    pub uid: String,
    pub name: String,
    pub model_id: String,
    /// The `AVCaptureDeviceType` string, e.g.
    /// `AVCaptureDeviceTypeBuiltInWideAngleCamera` or `...External`.
    pub device_type: String,
    /// True if this came from the muxed (audio+video) discovery pass.
    pub muxed: bool,
    pub formats: Vec<DeviceFormat>,
}

/// The `AVMediaTypeVideo` constant, unwrapped: weak-linked in the bindings but
/// present on every macOS this runs on.
fn media_video() -> &'static AVMediaType {
    // SAFETY: reading a framework string constant.
    unsafe { AVMediaTypeVideo }.expect("AVMediaTypeVideo constant missing")
}

/// The `AVMediaTypeMuxed` constant, unwrapped (see [`media_video`]).
fn media_muxed() -> &'static AVMediaType {
    // SAFETY: reading a framework string constant.
    unsafe { AVMediaTypeMuxed }.expect("AVMediaTypeMuxed constant missing")
}

/// Current camera permission without prompting.
pub fn authorization() -> Authorization {
    // SAFETY: the call has no preconditions beyond a valid media type.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_video()) };
    match status {
        AVAuthorizationStatus::NotDetermined => Authorization::NotDetermined,
        AVAuthorizationStatus::Restricted => Authorization::Restricted,
        AVAuthorizationStatus::Denied => Authorization::Denied,
        AVAuthorizationStatus::Authorized => Authorization::Authorized,
        other => {
            log::warn!("unknown AVAuthorizationStatus {}", other.0);
            Authorization::Denied
        }
    }
}

/// Show the system camera-permission prompt if the state is `NotDetermined`.
/// `on_result` runs on an arbitrary dispatch queue when the user answers (or
/// immediately with the existing answer).
pub fn request_access(on_result: impl Fn(bool) + 'static) {
    let block = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
        on_result(granted.as_bool());
    });
    // SAFETY: the handler block is retained by AVFoundation for the duration
    // of the request.
    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media_video(), &block);
    }
}

/// The device-type array ffmpeg 8's avfoundation demuxer uses for video
/// devices on macOS 14+ deployment targets, in ffmpeg's order. Index parity
/// with ffmpeg depends on this list matching `getDevicesWithMediaType` in
/// libavdevice/avfoundation.m.
fn ffmpeg_video_device_types() -> Retained<NSArray<AVCaptureDeviceType>> {
    // SAFETY: framework string constants.
    unsafe {
        NSArray::from_slice(&[
            AVCaptureDeviceTypeBuiltInWideAngleCamera,
            AVCaptureDeviceTypeDeskViewCamera,
            AVCaptureDeviceTypeContinuityCamera,
            AVCaptureDeviceTypeExternal,
        ])
    }
}

/// The device-type array for ffmpeg's muxed (audio+video) pass.
fn ffmpeg_muxed_device_types() -> Retained<NSArray<AVCaptureDeviceType>> {
    // SAFETY: framework string constant.
    unsafe { NSArray::from_slice(&[AVCaptureDeviceTypeExternal]) }
}

fn discover(
    types: &NSArray<AVCaptureDeviceType>,
    media: &AVMediaType,
) -> Retained<NSArray<AVCaptureDevice>> {
    // SAFETY: valid device-type array and media type; Unspecified position
    // matches ffmpeg's discovery call.
    unsafe {
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            types,
            Some(media),
            AVCaptureDevicePosition::Unspecified,
        )
        .devices()
    }
}

fn device_formats(dev: &AVCaptureDevice) -> Vec<DeviceFormat> {
    let mut out = Vec::new();
    // SAFETY: reading immutable format descriptions off a discovered device.
    unsafe {
        for f in dev.formats().iter() {
            let desc = f.formatDescription();
            let sub = desc.media_sub_type();
            let dims = CMVideoFormatDescriptionGetDimensions(&desc);
            let (mut min_fps, mut max_fps) = (f64::INFINITY, 0.0f64);
            for range in f.videoSupportedFrameRateRanges().iter() {
                min_fps = min_fps.min(range.minFrameRate());
                max_fps = max_fps.max(range.maxFrameRate());
            }
            out.push(DeviceFormat {
                width: dims.width.max(0) as u32,
                height: dims.height.max(0) as u32,
                fourcc: sub.to_be_bytes(),
                min_fps: if min_fps.is_finite() { min_fps } else { 0.0 },
                max_fps,
            });
        }
    }
    out
}

/// Enumerate capture devices in ffmpeg's avfoundation order (video devices,
/// then muxed devices), so each entry's `index` is directly usable as the
/// demuxer's `video_device_index`.
pub fn enumerate() -> Vec<DeviceInfo> {
    let mut out = Vec::new();
    for (is_muxed, types, media) in [
        (false, ffmpeg_video_device_types(), media_video()),
        (true, ffmpeg_muxed_device_types(), media_muxed()),
    ] {
        for dev in discover(&types, media).iter() {
            // SAFETY: reading immutable identity properties off a discovered
            // device.
            let (uid, name, model_id, device_type) = unsafe {
                (
                    dev.uniqueID().to_string(),
                    dev.localizedName().to_string(),
                    dev.modelID().to_string(),
                    dev.deviceType().to_string(),
                )
            };
            out.push(DeviceInfo {
                index: out.len(),
                uid,
                name,
                model_id,
                device_type,
                muxed: is_muxed,
                formats: device_formats(&dev),
            });
        }
    }
    out
}

/// Pick the format to request from a device: the largest format whose height
/// fits `max_h` (largest area, then highest frame rate as tie-break), falling
/// back to the smallest offered if everything is bigger.
pub fn pick_format(formats: &[DeviceFormat], max_h: u32) -> Option<&DeviceFormat> {
    let key = |f: &DeviceFormat| (u64::from(f.width) * u64::from(f.height), f.max_fps as u64);
    formats
        .iter()
        .filter(|f| f.height <= max_h)
        .max_by_key(|f| key(f))
        .or_else(|| formats.iter().min_by_key(|f| key(f)))
}

/// Open a capture device through ffmpeg's avfoundation demuxer. `index` must
/// come from a fresh [`enumerate`]. `video_size` and `framerate` should come
/// from one of the device's [`DeviceFormat`]s — the demuxer rejects frame
/// rates that don't match a supported range's max (its NTSC default fails on
/// most devices), while an unsupported `pixel_format` merely falls back with a
/// log line (pass [`DeviceFormat::ffmpeg_pixel_format`] to skip the fallback).
///
/// Dropping the returned context stops the capture session; measure that cost
/// before putting it on a latency-sensitive path.
///
/// # Errors
/// Fails if ffmpeg lacks the avfoundation input device, the index is stale,
/// the format/framerate combination is rejected, or TCC denies capture.
pub fn open_by_index(
    index: usize,
    video_size: (u32, u32),
    framerate: f64,
    pixel_format: Option<&str>,
) -> anyhow::Result<ff::format::context::Input> {
    ff::init()?;
    ff::device::register_all();
    // SAFETY: av_find_input_format does a name lookup in a static table.
    let fmt_ptr = unsafe { ff::ffi::av_find_input_format(c"avfoundation".as_ptr()) };
    ensure!(
        !fmt_ptr.is_null(),
        "ffmpeg was built without the avfoundation input device"
    );
    // SAFETY: non-null AVInputFormat from the lookup above; input formats are
    // static and never freed.
    let input = unsafe { ff::format::Input::wrap(fmt_ptr.cast_mut()) };

    let mut opts = ff::Dictionary::new();
    opts.set("video_device_index", &index.to_string());
    opts.set("video_size", &format!("{}x{}", video_size.0, video_size.1));
    opts.set("framerate", &format!("{framerate:.4}"));
    if let Some(pf) = pixel_format {
        opts.set("pixel_format", pf);
    }

    match ff::format::open_with("", &ff::Format::Input(input), opts)? {
        ff::format::Context::Input(i) => Ok(i),
        ff::format::Context::Output(_) => unreachable!("opened an input format"),
    }
}
