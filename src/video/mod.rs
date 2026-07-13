//! Video subsystem: HAP frame parsing (`hap`), per-clip decode workers
//! (`decoder`), camera capture (`capture`), and the plain-data frame types
//! they hand to the renderer (`frame`).

#[cfg(target_os = "macos")]
pub mod capture;
pub mod decoder;
pub mod frame;
pub mod hap;

use std::time::Instant;

use frame::DecodedFrame;

/// A cue's frame source: a per-cue file decode worker, or a per-cue tap onto a
/// shared camera capture service. The match arms are where camera cues'
/// timeline exemptions live — a live feed has nothing to seek.
pub enum SourceHandle {
    File(decoder::DecodeHandle),
    #[cfg(target_os = "macos")]
    Camera(capture::CameraTap),
}

impl SourceHandle {
    /// Musical re-loop / hard-reset restart. No-op for cameras: no timeline.
    pub fn request_restart(&self) {
        match self {
            Self::File(h) => h.request_restart(),
            #[cfg(target_os = "macos")]
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
            #[cfg(target_os = "macos")]
            Self::Camera(tap) => tap.poll(now),
        }
    }
}
