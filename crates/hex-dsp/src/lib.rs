//! # Purpose
//! Own host-independent safe DSP mathematics with explicit gain/frame units and numeric
//! validation.
//!
//! # Dependencies
//! Uses only the standard library; no workspace dependencies.
//!
//! # Does not own
//! Does not own device registration, projects, UI, threads, audio I/O or media storage.
mod envelope_follower;
mod gain;
mod modulation_lfo;
mod stereo;
pub use envelope_follower::{EnvelopeFollower, InvalidEnvelopeFollower};
pub use gain::{Decibels, InvalidGain, LinearAmplitude};
pub use modulation_lfo::{InvalidLfo, LfoRate, ModulationLfo};
pub use stereo::{InvalidMatrix, StereoFrame, StereoMatrix};
