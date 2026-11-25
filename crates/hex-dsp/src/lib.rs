//! # Purpose
//! Own host-independent safe DSP mathematics with explicit gain/frame units and numeric
//! validation.
//!
//! # Dependencies
//! Uses only the standard library; no workspace dependencies.
//!
//! # Does not own
//! Does not own device registration, projects, UI, threads, audio I/O or media storage.
mod biquad;
mod denormal;
mod envelope_follower;
mod gain;
mod modulation_lfo;
mod random_hold;
mod smoother;
mod stereo;
mod svf;
pub use biquad::{Biquad, BiquadCoefficients, InvalidBiquad};
pub use denormal::{
    POWER_FLOOR, QUIET_FLOOR, QUIET_FLOOR_F32, flush_below, flush_quiet, flush_quiet_f32,
    flush_quiet_pair, flush_subnormal,
};
pub use envelope_follower::{EnvelopeFollower, InvalidEnvelopeFollower};
pub use gain::{Decibels, InvalidGain, LinearAmplitude};
pub use modulation_lfo::{InvalidLfo, LfoRate, LfoWaveform, ModulationLfo};
pub use random_hold::{InvalidRandomHold, RandomHold};
pub use smoother::{
    InvalidSmoother, LinearRamp, LinearRampF32, OnePoleSmoother, ParameterSmoother, frequency_pole,
    one_pole_coefficient, one_pole_pole, ramp_frames_for_ms,
};
pub use stereo::{InvalidMatrix, StereoFrame, StereoMatrix};
pub use svf::{InvalidSvf, SvfFilter, SvfFrame, SvfMode, SvfOutputs, SvfSection, SvfSectionF32};
