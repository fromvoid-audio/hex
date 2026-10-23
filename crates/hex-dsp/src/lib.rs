//! # Purpose
//! Own host-independent safe DSP mathematics with explicit gain/frame units and numeric
//! validation.
//!
//! # Dependencies
//! Uses only the standard library; no workspace dependencies.
//!
//! # Does not own
//! Does not own device registration, projects, UI, threads, audio I/O or media storage.
mod gain;
mod stereo;
pub use gain::{Decibels, InvalidGain, LinearAmplitude};
pub use stereo::{InvalidMatrix, StereoFrame, StereoMatrix};
