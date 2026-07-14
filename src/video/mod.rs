//! Video subsystem: HAP frame parsing (`hap`), per-clip decode workers
//! (`decoder`), camera capture (`capture`), and the plain-data frame types
//! they hand to the renderer (`frame`).

#[cfg(target_os = "macos")]
pub mod capture;
pub mod decoder;
pub mod frame;
pub mod hap;

/// Non-macOS stub with the same shape as `capture`, so the app compiles
/// without platform cfg noise: no devices enumerate, taps never yield, the
/// registry is inert.
#[cfg(not(target_os = "macos"))]
pub mod capture {
    use std::time::Instant;

    use crate::video::frame::DecodedFrame;

    pub const DELAY_CAP: f64 = 3.0;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Authorization {
        NotDetermined,
        Restricted,
        Denied,
        Authorized,
    }

    #[derive(Debug, Clone)]
    pub struct DeviceFormat {
        pub width: u32,
        pub height: u32,
        pub fourcc: [u8; 4],
        pub min_fps: f64,
        pub max_fps: f64,
    }

    #[derive(Debug, Clone)]
    pub struct DeviceInfo {
        pub index: usize,
        pub uid: String,
        pub name: String,
        pub model_id: String,
        pub device_type: String,
        pub muxed: bool,
        pub formats: Vec<DeviceFormat>,
    }

    #[derive(Debug, Clone)]
    pub enum ServiceStatus {
        Starting,
        Running { width: u32, height: u32, fps: f64 },
        Failed(String),
    }

    pub struct CameraTap {
        pub delay_eff: f64,
    }

    impl CameraTap {
        pub fn poll(&mut self, _now: Instant) -> Option<DecodedFrame> {
            None
        }
    }

    #[derive(Default)]
    pub struct CaptureRegistry;

    impl CaptureRegistry {
        pub fn set_on_air(&mut self, _uid: &str, _on: bool) {}
        pub fn is_on_air(&self, _uid: &str) -> bool {
            false
        }
        pub fn tap(&self, _uid: &str) -> Option<CameraTap> {
            None
        }
        pub fn status(&self, _uid: &str) -> Option<ServiceStatus> {
            None
        }
    }

    pub fn authorization() -> Authorization {
        Authorization::Denied
    }

    pub fn request_access(_on_result: impl Fn(bool) + 'static) {}

    pub fn enumerate() -> Vec<DeviceInfo> {
        Vec::new()
    }

    pub fn slew(current: f64, target: f64, dt: f64, rate: f64) -> f64 {
        let step = (rate * dt).max(0.0);
        let diff = target - current;
        if diff.abs() <= step {
            target
        } else {
            current + step * diff.signum()
        }
    }
}

use std::time::Instant;

use frame::DecodedFrame;

/// A cue's frame source: a per-cue file decode worker, or a per-cue tap onto a
/// shared camera capture service. The match arms are where camera cues'
/// timeline exemptions live — a live feed has nothing to seek.
pub enum SourceHandle {
    File(decoder::DecodeHandle),
    Camera(capture::CameraTap),
}

impl SourceHandle {
    /// Musical re-loop / hard-reset restart. No-op for cameras: no timeline.
    pub fn request_restart(&self) {
        match self {
            Self::File(h) => h.request_restart(),
            Self::Camera(_) => {}
        }
    }

    /// The newest frame available right now, if any: drains the decode channel
    /// newest-wins for files, polls the delay ring for cameras.
    pub fn poll_newest(&mut self, now: Instant) -> Option<DecodedFrame> {
        match self {
            Self::File(h) => {
                let mut newest = None;
                while let Ok(f) = h.frames.try_recv() {
                    newest = Some(f);
                }
                newest
            }
            Self::Camera(tap) => tap.poll(now),
        }
    }

    /// The camera tap, when this source is one.
    pub fn camera_mut(&mut self) -> Option<&mut capture::CameraTap> {
        match self {
            Self::File(_) => None,
            Self::Camera(tap) => Some(tap),
        }
    }
}
