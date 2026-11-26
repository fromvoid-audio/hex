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
mod frequency_shifter;
mod gain;
mod halfband;
mod modulation_lfo;
mod oversampling;
mod random_hold;
mod sinc_interpolator;
mod smoother;
mod stereo;
mod svf;
mod window;
pub use biquad::{Biquad, BiquadCoefficients, InvalidBiquad};
pub use denormal::{
    POWER_FLOOR, QUIET_FLOOR, QUIET_FLOOR_F32, flush_below, flush_quiet, flush_quiet_f32,
    flush_quiet_pair, flush_subnormal,
};
pub use envelope_follower::{EnvelopeFollower, InvalidEnvelopeFollower};
pub use frequency_shifter::{
    FrequencyShiftParameters, FrequencyShifter, InvalidFrequencyShifter, ShiftMode,
};
pub use gain::{Decibels, InvalidGain, LinearAmplitude};
pub use halfband::{HALFBAND_BLOCK_CHUNK, Halfband2x, Halfband2xKernel, Halfband2xStereo};
pub use modulation_lfo::{InvalidLfo, LfoRate, LfoWaveform, ModulationLfo};
pub use oversampling::{
    AdaaShaper, KaiserOversample8xKernel, OVERSAMPLE_8X_DRAIN_FRAMES, OVERSAMPLE_8X_FACTOR,
    OVERSAMPLE_8X_LATENCY_FRAMES, OVERSAMPLE_8X_PHASE_TAPS, OVERSAMPLE_8X_TAPS,
    OVERSAMPLE_BLOCK_CHUNK, Oversampler8x, adaa_step,
};
pub use random_hold::{InvalidRandomHold, RandomHold};
pub use sinc_interpolator::{
    BlackmanSincBank, KAISER_SINC_MAX_HALF_TAPS, KAISER_SINC_ZERO_CROSSINGS, KaiserSincBank,
    KaiserSincKernel, SincTaps,
};
pub use smoother::{
    InvalidSmoother, LinearRamp, LinearRampF32, OnePoleSmoother, ParameterSmoother, frequency_pole,
    one_pole_coefficient, one_pole_pole, ramp_frames_for_ms,
};
pub use stereo::{InvalidMatrix, StereoFrame, StereoMatrix};
pub use svf::{InvalidSvf, SvfFilter, SvfFrame, SvfMode, SvfOutputs, SvfSection, SvfSectionF32};
pub use window::{
    bessel_i0, blackman_window, blackman_window_centered, fast_ln_cosh_f64, fast_ln_f64,
    kaiser_window, sinc, warm_ln_cosh_table,
};
