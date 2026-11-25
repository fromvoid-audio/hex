//! Band-limited source reconstruction for pitched-up sampler voices, backed by
//! `hex-dsp`'s shared multi-rate Kaiser-windowed sinc polyphase bank.
//!
//! Law (sampler interpolation v2, #4089): a voice whose source step `r`
//! (source frames per output frame) is at most 1 keeps the original two-point
//! linear interpolation exactly. Above unity, each output frame is a normalized
//! Kaiser-windowed sinc sum over the source frames around the exact Q32 source
//! position.
//!
//! [`warm`] builds the shared static bank on control during plan preparation
//! and returns a `&'static Bank` stored on prepared plans and voice kernels.
//! Render-time tap lookup reads that reference directly with bounds-checked
//! slices and no `OnceLock` access, allocation, lock or wait.
#[allow(unused_imports)]
pub(super) use hex_dsp::{
    KAISER_SINC_MAX_HALF_TAPS as MAX_HALF_TAPS, KAISER_SINC_ZERO_CROSSINGS as ZERO_CROSSINGS,
    KaiserSincBank as Bank, SincTaps as Taps,
};

/// Build or borrow the static polyphase bank on control before any voice renders.
pub(super) fn warm() -> &'static Bank {
    Bank::shared()
}

/// Per-voice kernel: the rate class chosen on control from the voice step,
/// paired with the pre-warmed static polyphase bank from `hex-dsp`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Kernel(hex_dsp::KaiserSincKernel);

impl Kernel {
    /// `None` for unity or slower playback, which keeps linear interpolation.
    /// Otherwise the class at or above `rate`, capped at +60 semitones.
    pub(super) fn for_rate_with_bank(rate: f64, bank: &'static Bank) -> Option<Self> {
        hex_dsp::KaiserSincKernel::for_rate_with_bank(rate, bank).map(Self)
    }

    #[cfg(test)]
    pub(super) fn for_rate(rate: f64) -> Option<Self> {
        Self::for_rate_with_bank(rate, warm())
    }

    /// Taps for a source position with `fraction` in `[0, 1)`.
    #[inline]
    pub(super) fn taps(self, fraction: f64) -> Option<Taps> {
        self.0.taps(fraction)
    }
}

#[cfg(test)]
#[path = "interpolation_tests.rs"]
mod tests;
