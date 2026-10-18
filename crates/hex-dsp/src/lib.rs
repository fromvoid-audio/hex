//! Host-independent, safe DSP mathematics with explicit units and numeric policy.
mod gain;
mod stereo;
pub use gain::{Decibels, InvalidGain, LinearAmplitude};
pub use stereo::{InvalidMatrix, StereoFrame, StereoMatrix};
