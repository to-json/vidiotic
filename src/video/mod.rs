//! Video subsystem: HAP frame parsing (`hap`), per-clip decode workers
//! (`decoder`), and the plain-data frame types they hand to the renderer
//! (`frame`).

pub mod decoder;
pub mod frame;
pub mod hap;
