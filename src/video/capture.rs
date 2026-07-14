//! Camera capture (macOS). `AVFoundation` supplies device enumeration, stable
//! identity (`uniqueID`), and TCC permission state; the frames themselves come
//! through ffmpeg's `avfoundation` input device on the already-linked
//! ffmpeg-next, so camera packets ride the same decode surface as file clips.
//!
//! Device selection: macOS does **not** keep discovery order stable — the
//! same process can see built-in and external devices swap positions between
//! enumerations seconds apart — so an index is racy by the time the demuxer
//! re-enumerates inside open. [`open_device`] therefore selects by
//! `localizedName` (the demuxer prefix-matches it against its own fresh list),
//! falling back to `video_device_index` only for names its URL parser can't
//! express (leading digit, embedded `:`). [`DeviceInfo::index`] is advisory.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

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

use crate::video::frame::{DecodedFrame, PixelData};

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
    /// `CoreMedia` four-char code, where one exists. The name must come from
    /// the avfoundation demuxer's own format table — a name it doesn't know
    /// (even a valid ffmpeg one, e.g. `bgra`) is a hard open error, whereas an
    /// unmapped `None` falls back to the demuxer's default-with-override path.
    /// Compressed-native devices (MJPEG four-char `dmb1`) stay unmapped —
    /// `AVFoundation` decompresses those to a CV format itself.
    pub fn ffmpeg_pixel_format(&self) -> Option<&'static str> {
        match &self.fourcc {
            b"420v" | b"420f" => Some("nv12"),
            b"2vuy" => Some("uyvy422"),
            b"yuvs" => Some("yuyv422"),
            // avfoundation maps kCVPixelFormatType_32BGRA to bgr0, not bgra.
            b"BGRA" => Some("bgr0"),
            _ => None,
        }
    }
}

/// A capture device as seen at one enumeration. Plain data — safe to move
/// across threads, valid until devices are plugged or unplugged.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    /// Position in this enumeration. Advisory (display, spike args): macOS
    /// device order is unstable, so selection goes by name — see
    /// [`open_device`].
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

/// Enumerate capture devices with the same discovery scope as ffmpeg's
/// avfoundation demuxer (video device types, then muxed). Order is whatever
/// macOS returns this instant — selection goes by name, not position.
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
    // The demuxer prefix-matches names, so a device whose full name prefixes
    // another's is ambiguous to select. Rare; surface it rather than guess.
    for a in &out {
        if out.iter().any(|b| b.uid != a.uid && b.name.starts_with(&a.name)) {
            log::warn!(
                "capture device name {:?} is a prefix of another device's name; \
                 selection may pick the wrong one",
                a.name
            );
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

/// True when the demuxer's URL parser can express this device name: a leading
/// digit would parse as a device index, and `:` splits the video/audio parts.
fn name_selectable(name: &str) -> bool {
    !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) && !name.contains(':')
}

/// Open a capture device through ffmpeg's avfoundation demuxer. Selection is
/// by the device's `localizedName` (prefix-matched by the demuxer against its
/// own fresh enumeration) because macOS device *order* is unstable — an index
/// can point at a different device by the time the demuxer looks. Names the
/// URL parser can't express fall back to `video_device_index` best-effort.
///
/// `video_size` and `framerate` should come from one of the device's
/// [`DeviceFormat`]s — the demuxer rejects frame rates that don't match a
/// supported range's max (its NTSC default fails on most devices), while an
/// unsupported `pixel_format` merely falls back with a log line (pass
/// [`DeviceFormat::ffmpeg_pixel_format`] to skip the fallback).
///
/// Dropping the returned context stops the capture session; measure that cost
/// before putting it on a latency-sensitive path.
///
/// # Errors
/// Fails if ffmpeg lacks the avfoundation input device, the device vanished,
/// the format/framerate combination is rejected, or TCC denies capture.
pub fn open_device(
    dev: &DeviceInfo,
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
    opts.set("video_size", &format!("{}x{}", video_size.0, video_size.1));
    opts.set("framerate", &format!("{framerate:.4}"));
    if let Some(pf) = pixel_format {
        opts.set("pixel_format", pf);
    }
    let url = if name_selectable(&dev.name) {
        dev.name.clone()
    } else {
        log::warn!(
            "device name {:?} can't be matched by the demuxer; \
             falling back to the racy index {}",
            dev.name,
            dev.index
        );
        opts.set("video_device_index", &dev.index.to_string());
        String::new()
    };

    match ff::format::open_with(&url, &ff::Format::Input(input), opts)? {
        ff::format::Context::Input(i) => Ok(i),
        ff::format::Context::Output(_) => unreachable!("opened an input format"),
    }
}

/// Maximum voluntary delay a cue can dial in, seconds. The ship cap from the
/// plan's staging; raising it is the "scale" pass.
pub const DELAY_CAP: f64 = 3.0;

/// How much history a service's ring retains. Slightly above [`DELAY_CAP`] so
/// a maxed-out delay still finds a frame.
const RING_WINDOW: Duration = Duration::from_millis(3200);

/// Hard memory bound per ring. A correctness bound, not a tuning knob: an
/// uncapped 3 s of 4K BGRA is ~6 GB. When the cap bites, the effective delay
/// window shrinks instead of memory growing.
const RING_BYTE_CAP: usize = 384 << 20;

/// Requested capture height ceiling; larger device formats are skipped at open.
const CAPTURE_MAX_H: u32 = 1080;

/// Retry cadence when a device is missing or fails to open.
const REOPEN_DELAY: Duration = Duration::from_millis(500);

/// One captured frame in the delay ring, decoded but still in the camera's
/// native pixel format.
struct RingFrame {
    /// Arrival wall-clock time; tap delays are offsets against this.
    wall: Instant,
    /// Seconds since the service started, for display.
    pts_sec: f64,
    frame: ff::frame::Video,
    bytes: usize,
}

fn frame_bytes(f: &ff::frame::Video) -> usize {
    (0..f.planes()).map(|i| f.data(i).len()).sum()
}

/// The delay ring proper: newest at the back. Frames are retained for the full
/// window regardless of tap positions (a growing delay must find old frames),
/// evicted only by age or the byte cap, and flushed wholesale when the device
/// changes size or format mid-stream.
struct RingState {
    frames: VecDeque<RingFrame>,
    bytes: usize,
    window: Duration,
    byte_cap: usize,
}

impl RingState {
    fn new(window: Duration, byte_cap: usize) -> Self {
        Self { frames: VecDeque::new(), bytes: 0, window, byte_cap }
    }

    fn push(&mut self, wall: Instant, pts_sec: f64, frame: ff::frame::Video) {
        if let Some(back) = self.frames.back() {
            let b = &back.frame;
            if (b.width(), b.height(), b.format())
                != (frame.width(), frame.height(), frame.format())
            {
                self.frames.clear();
                self.bytes = 0;
            }
        }
        let bytes = frame_bytes(&frame);
        self.bytes += bytes;
        self.frames.push_back(RingFrame { wall, pts_sec, frame, bytes });
        while self.frames.len() > 1 {
            let front = &self.frames[0];
            if self.bytes > self.byte_cap || wall.duration_since(front.wall) > self.window {
                self.bytes -= front.bytes;
                self.frames.pop_front();
            } else {
                break;
            }
        }
    }

    /// The newest frame at or before `target`, falling back to the oldest when
    /// the requested moment predates the ring (warm-up, or a delay past the
    /// byte-capped window).
    fn peek_at(&self, target: Instant) -> Option<&RingFrame> {
        self.frames
            .iter()
            .rev()
            .find(|f| f.wall <= target)
            .or_else(|| self.frames.front())
    }
}

/// What a capture service is currently doing, for status UI.
#[derive(Debug, Clone)]
pub enum ServiceStatus {
    Starting,
    Running { width: u32, height: u32, fps: f64 },
    /// Open or capture failed (device missing, TCC denied, ...). The worker
    /// keeps retrying while this is shown.
    Failed(String),
}

struct Ring {
    state: Mutex<RingState>,
    status: Mutex<ServiceStatus>,
}

/// Lock, recovering from poisoning — a panicked capture thread must not take
/// the render thread down with it.
fn lock_unpoisoned<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A per-cue read handle onto a device's delay ring. Pull-based: the app polls
/// it on the frame-drain tick; nothing blocks and idle taps cost nothing. Each
/// tap has its own delay offset and swscale cache, so cues on the same device
/// don't interfere.
pub struct CameraTap {
    ring: Arc<Ring>,
    /// Effective delay in seconds. The app owns moving this (slew/quantize);
    /// the tap just reads frames `delay_eff` behind the live edge.
    pub delay_eff: f64,
    last_wall: Option<Instant>,
    scaler: Option<(ff::software::scaling::Context, (u32, u32, ff::format::Pixel))>,
}

impl CameraTap {
    /// The newest frame at `now - delay_eff`, converted to RGBA, or `None` if
    /// that position hasn't advanced since the last poll (or the ring is
    /// empty). Conversion touches only the emitted frame.
    pub fn poll(&mut self, now: Instant) -> Option<DecodedFrame> {
        let target = now.checked_sub(Duration::from_secs_f64(self.delay_eff.max(0.0)))?;
        let state = lock_unpoisoned(&self.ring.state);
        let picked = state.peek_at(target)?;
        if self.last_wall == Some(picked.wall) {
            return None;
        }
        let (w, h, fmt) = (picked.frame.width(), picked.frame.height(), picked.frame.format());
        if self.scaler.as_ref().is_none_or(|(_, key)| *key != (w, h, fmt)) {
            let ctx = ff::software::scaling::Context::get(
                fmt,
                w,
                h,
                ff::format::Pixel::RGBA,
                w,
                h,
                ff::software::scaling::Flags::BILINEAR,
            )
            .ok()?;
            self.scaler = Some((ctx, (w, h, fmt)));
        }
        let mut rgba = ff::frame::Video::empty();
        let (scaler, _) = self.scaler.as_mut()?;
        if let Err(e) = scaler.run(&picked.frame, &mut rgba) {
            log::warn!("camera tap convert failed: {e}");
            return None;
        }
        self.last_wall = Some(picked.wall);
        let pts_sec = picked.pts_sec;
        drop(state);
        Some(DecodedFrame {
            pixels: PixelData::Rgba {
                data: rgba.data(0).to_vec(),
                stride: rgba.stride(0) as u32,
            },
            w,
            h,
            pts_sec,
        })
    }
}

/// A running capture on one device: a worker thread feeding the shared delay
/// ring. Lifetime follows the device's on-air toggle, not cue rotation. The
/// worker re-resolves uid → index on every (re)open, so replugs and stale
/// indexes self-heal on retry.
pub struct CaptureService {
    ring: Arc<Ring>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl CaptureService {
    /// Start capturing from the device with `AVFoundation` uid `uid`.
    pub fn start(uid: String) -> Self {
        let ring = Arc::new(Ring {
            state: Mutex::new(RingState::new(RING_WINDOW, RING_BYTE_CAP)),
            status: Mutex::new(ServiceStatus::Starting),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let join = {
            let (ring, stop) = (Arc::clone(&ring), Arc::clone(&stop));
            std::thread::spawn(move || worker(&uid, &ring, &stop))
        };
        Self { ring, stop, join: Some(join) }
    }

    /// A fresh zero-delay tap onto this service's ring.
    pub fn tap(&self) -> CameraTap {
        CameraTap {
            ring: Arc::clone(&self.ring),
            delay_eff: 0.0,
            last_wall: None,
            scaler: None,
        }
    }

    pub fn status(&self) -> ServiceStatus {
        lock_unpoisoned(&self.ring.status).clone()
    }
}

impl Drop for CaptureService {
    fn drop(&mut self) {
        // Detached teardown: session stop must never ride the engine tick, so
        // a reaper thread absorbs the join.
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            std::thread::spawn(move || {
                let _ = j.join();
            });
        }
    }
}

fn set_status(ring: &Ring, s: ServiceStatus) {
    *lock_unpoisoned(&ring.status) = s;
}

/// Sleep in stop-checkable slices before an open retry.
fn retry_wait(stop: &AtomicBool) {
    let deadline = Instant::now() + REOPEN_DELAY;
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn worker(uid: &str, ring: &Ring, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        if let Err(e) = capture_once(uid, ring, stop) {
            set_status(ring, ServiceStatus::Failed(format!("{e:#}")));
            log::warn!("capture worker for {uid}: {e:#}");
            retry_wait(stop);
        }
    }
}

/// One open-and-capture session; returns Ok on a requested stop, Err on
/// anything that warrants a reopen.
fn capture_once(uid: &str, ring: &Ring, stop: &AtomicBool) -> anyhow::Result<()> {
    let devices = enumerate();
    let dev = devices
        .iter()
        .find(|d| d.uid == uid)
        .ok_or_else(|| anyhow::anyhow!("device not connected"))?;
    let fmt = pick_format(&dev.formats, CAPTURE_MAX_H)
        .ok_or_else(|| anyhow::anyhow!("device reports no formats"))?;
    let mut ictx = open_device(
        dev,
        (fmt.width, fmt.height),
        fmt.max_fps,
        fmt.ffmpeg_pixel_format(),
    )?;

    let (vid_idx, params) = {
        let st = ictx
            .streams()
            .best(ff::media::Type::Video)
            .ok_or_else(|| anyhow::anyhow!("no video stream on capture input"))?;
        (st.index(), st.parameters())
    };
    let mut decoder = ff::codec::context::Context::from_parameters(params)?
        .decoder()
        .video()?;
    set_status(
        ring,
        ServiceStatus::Running {
            width: decoder.width(),
            height: decoder.height(),
            fps: fmt.max_fps,
        },
    );

    let start = Instant::now();
    let mut ring_scaler: Option<(ff::software::scaling::Context, (u32, u32, ff::format::Pixel))> =
        None;
    for (stream, packet) in ictx.packets() {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if stream.index() != vid_idx {
            continue;
        }
        if let Err(e) = decoder.send_packet(&packet) {
            log::warn!("capture decode send_packet failed, skipping: {e}");
            continue;
        }
        let mut decoded = ff::frame::Video::empty();
        while decoder.receive_frame(&mut decoded).is_ok() {
            let wall = Instant::now();
            let pts_sec = wall.duration_since(start).as_secs_f64();
            let frame = std::mem::replace(&mut decoded, ff::frame::Video::empty());
            let frame = normalize_for_ring(frame, &mut ring_scaler);
            lock_unpoisoned(&ring.state).push(wall, pts_sec, frame);
        }
    }
    // The packet iterator only ends on read error / EOF — a live input
    // shouldn't do either, so treat it as a fault and reopen.
    anyhow::bail!("capture input ended unexpectedly")
}

/// Re-encode fat pixel layouts to NV12 before they enter the ring. Packed RGB
/// (e.g. OBS's forced 1080p60 BGRA) is 4 B/px — against the byte cap that
/// shrinks the delay window under a second, while NV12's 1.5 B/px keeps the
/// full window. Compact YUV layouts pass through untouched; on conversion
/// failure the original frame is ringed rather than dropped.
fn normalize_for_ring(
    frame: ff::frame::Video,
    scaler: &mut Option<(ff::software::scaling::Context, (u32, u32, ff::format::Pixel))>,
) -> ff::frame::Video {
    use ff::format::Pixel;
    let (w, h, fmt) = (frame.width(), frame.height(), frame.format());
    if matches!(fmt, Pixel::NV12 | Pixel::YUV420P | Pixel::UYVY422 | Pixel::YUYV422) {
        return frame;
    }
    if scaler.as_ref().is_none_or(|(_, key)| *key != (w, h, fmt)) {
        match ff::software::scaling::Context::get(
            fmt,
            w,
            h,
            Pixel::NV12,
            w,
            h,
            ff::software::scaling::Flags::BILINEAR,
        ) {
            Ok(ctx) => *scaler = Some((ctx, (w, h, fmt))),
            Err(e) => {
                log::warn!("ring NV12 convert unavailable for {fmt:?}: {e}");
                return frame;
            }
        }
    }
    let Some((ctx, _)) = scaler.as_mut() else { return frame };
    let mut nv12 = ff::frame::Video::empty();
    match ctx.run(&frame, &mut nv12) {
        Ok(()) => nv12,
        Err(e) => {
            log::warn!("ring NV12 convert failed: {e}");
            frame
        }
    }
}

/// The on-air registry: one persistent [`CaptureService`] per toggled-on
/// device, keyed by `AVFoundation` uid. Owned by the app; deliberately not
/// touched by cue-lifetime bookkeeping like `retain_decoders`.
#[derive(Default)]
pub struct CaptureRegistry {
    services: HashMap<String, CaptureService>,
}

impl CaptureRegistry {
    /// Toggle a device on or off air. Turning off tears down detached; frames
    /// already tapped keep rendering until their cues drop.
    pub fn set_on_air(&mut self, uid: &str, on: bool) {
        if on {
            self.services
                .entry(uid.to_owned())
                .or_insert_with(|| CaptureService::start(uid.to_owned()));
        } else {
            self.services.remove(uid);
        }
    }

    pub fn is_on_air(&self, uid: &str) -> bool {
        self.services.contains_key(uid)
    }

    /// A fresh tap for a cue on this device, if it's on air.
    pub fn tap(&self, uid: &str) -> Option<CameraTap> {
        self.services.get(uid).map(CaptureService::tap)
    }

    pub fn status(&self, uid: &str) -> Option<ServiceStatus> {
        self.services.get(uid).map(CaptureService::status)
    }
}

/// Move `current` toward `target` by at most `rate * dt` (all in seconds).
/// The app runs each camera cue's effective delay through this every tick so
/// delay changes glide instead of jumping.
pub fn slew(current: f64, target: f64, dt: f64, rate: f64) -> f64 {
    let step = (rate * dt).max(0.0);
    let diff = target - current;
    if diff.abs() <= step {
        target
    } else {
        current + step * diff.signum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nv12(w: u32, h: u32) -> ff::frame::Video {
        ff::frame::Video::new(ff::format::Pixel::NV12, w, h)
    }

    fn ring_for_test() -> RingState {
        RingState::new(Duration::from_secs(3), usize::MAX)
    }

    #[test]
    fn ring_evicts_by_age_keeping_window() {
        let mut r = ring_for_test();
        let t0 = Instant::now();
        for i in 0..10 {
            r.push(t0 + Duration::from_millis(i * 500), i as f64 * 0.5, nv12(4, 4));
        }
        // 10 frames over 4.5s; the 3s window keeps those within 3s of the last.
        let newest = t0 + Duration::from_millis(9 * 500);
        assert!(r.frames.iter().all(|f| newest.duration_since(f.wall).as_secs_f64() <= 3.0));
        assert!(r.frames.len() >= 6);
    }

    #[test]
    fn ring_evicts_by_byte_cap() {
        let per_frame = frame_bytes(&nv12(16, 16));
        let mut r = RingState::new(Duration::from_secs(60), per_frame * 3);
        let t0 = Instant::now();
        for i in 0..10 {
            r.push(t0 + Duration::from_millis(i * 10), 0.0, nv12(16, 16));
        }
        assert_eq!(r.frames.len(), 3);
        assert!(r.bytes <= per_frame * 3);
    }

    #[test]
    fn ring_flushes_on_format_change() {
        let mut r = ring_for_test();
        let t0 = Instant::now();
        r.push(t0, 0.0, nv12(16, 16));
        r.push(t0 + Duration::from_millis(10), 0.01, nv12(16, 16));
        r.push(t0 + Duration::from_millis(20), 0.02, nv12(8, 8));
        assert_eq!(r.frames.len(), 1);
        assert_eq!(r.bytes, frame_bytes(&nv12(8, 8)));
    }

    #[test]
    fn ring_never_evicts_the_only_frame() {
        let mut r = RingState::new(Duration::from_millis(1), 1);
        let t0 = Instant::now();
        r.push(t0, 0.0, nv12(16, 16));
        r.push(t0 + Duration::from_secs(5), 5.0, nv12(16, 16));
        assert_eq!(r.frames.len(), 1);
    }

    #[test]
    fn peek_selects_newest_at_or_before_target() {
        let mut r = ring_for_test();
        let t0 = Instant::now();
        for i in 0..3 {
            r.push(t0 + Duration::from_secs(i), i as f64, nv12(4, 4));
        }
        let hit = r.peek_at(t0 + Duration::from_millis(1500)).unwrap();
        assert!((hit.pts_sec - 1.0).abs() < 1e-9);
        // A target past the newest frame gets the newest.
        let hit = r.peek_at(t0 + Duration::from_secs(10)).unwrap();
        assert!((hit.pts_sec - 2.0).abs() < 1e-9);
    }

    #[test]
    fn peek_clamps_to_oldest_during_warmup() {
        let mut r = ring_for_test();
        let t0 = Instant::now();
        r.push(t0 + Duration::from_secs(1), 1.0, nv12(4, 4));
        // Delay target predates everything in the ring: clamp, don't starve.
        let hit = r.peek_at(t0).unwrap();
        assert!((hit.pts_sec - 1.0).abs() < 1e-9);
    }

    #[test]
    fn normalize_converts_packed_rgb_to_nv12() {
        let mut scaler = None;
        let bgra = ff::frame::Video::new(ff::format::Pixel::BGRA, 16, 16);
        let out = normalize_for_ring(bgra, &mut scaler);
        assert_eq!(out.format(), ff::format::Pixel::NV12);
        assert_eq!((out.width(), out.height()), (16, 16));
        // Compact YUV passes through untouched.
        let mut scaler = None;
        let out = normalize_for_ring(nv12(16, 16), &mut scaler);
        assert_eq!(out.format(), ff::format::Pixel::NV12);
        assert!(scaler.is_none());
    }

    #[test]
    fn name_selectable_flags_url_hazards() {
        assert!(name_selectable("OBS Virtual Camera"));
        assert!(!name_selectable("4K Capture Stick"));
        assert!(!name_selectable("Cam: Pro"));
        assert!(!name_selectable(""));
    }

    #[test]
    fn slew_steps_and_lands() {
        assert!((slew(0.0, 1.0, 0.1, 1.0) - 0.1).abs() < 1e-9);
        assert!((slew(0.9, 1.0, 0.5, 1.0) - 1.0).abs() < 1e-9);
        assert!((slew(2.0, 1.0, 0.1, 1.0) - 1.9).abs() < 1e-9);
        // Zero rate holds position.
        assert!((slew(0.5, 1.0, 0.1, 0.0) - 0.5).abs() < 1e-9);
    }
}
