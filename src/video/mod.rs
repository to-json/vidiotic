//! Video subsystem: HAP frame parsing (`hap`), clip decoding workers, and the
//! clip pool. Only `hap` exists at M0; the rest lands in M1.

pub mod hap;
