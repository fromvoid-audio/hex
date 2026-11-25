//! Thin crate-local re-exports of `hex-dsp`'s shared 8x Kaiser-sinc polyphase
//! oversampler with first-order ADAA for native nonlinear devices (`overdrive`,
//! `saturator`), plus test-only spectrum measurement helpers.
#[allow(unused_imports)]
pub(crate) use hex_dsp::{
    AdaaShaper as Shaper, KaiserOversample8xKernel as Kernel,
    OVERSAMPLE_8X_DRAIN_FRAMES as DRAIN_FRAMES, OVERSAMPLE_8X_FACTOR as FACTOR,
    OVERSAMPLE_8X_LATENCY_FRAMES as LATENCY_FRAMES, OVERSAMPLE_8X_TAPS as TAPS,
    OVERSAMPLE_BLOCK_CHUNK as BLOCK_CHUNK, Oversampler8x as Oversampler,
};

#[cfg(test)]
pub(crate) mod spectrum;
