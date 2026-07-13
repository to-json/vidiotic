//! Video subsystem: HAP frame parsing (`hap`), per-clip decode workers
//! (`decoder`), camera capture (`capture`), and the plain-data frame types
//! they hand to the renderer (`frame`).

#[cfg(target_os = "macos")]
pub mod capture;
pub mod decoder;
pub mod frame;
pub mod hap;
