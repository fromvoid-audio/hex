//! Unified linear and one-pole exponential parameter smoothers.
//!
//! All smoothers are `Copy`, fixed-size, allocation-free, and strictly
//! validated on construction so they can live inline in real-time audio
//! processor runtimes.

use std::f64::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSmoother;

/// Compute the positive frame count covering `milliseconds` at `sample_rate`
/// (ceiling-rounded, at least 1 frame when `milliseconds > 0`).
pub fn ramp_frames_for_ms(sample_rate: u32, milliseconds: f64) -> Result<u32, InvalidSmoother> {
    if sample_rate == 0 || !milliseconds.is_finite() || milliseconds < 0.0 {
        return Err(InvalidSmoother);
    }
    if milliseconds == 0.0 {
        return Ok(0);
    }
    let frames = (f64::from(sample_rate) * milliseconds / 1_000.0).ceil();
    if !frames.is_finite() || frames < 1.0 || frames > f64::from(u32::MAX) {
        return Err(InvalidSmoother);
    }
    Ok(frames as u32)
}

/// Compute the one-pole feedback pole `exp(-1 / (time_constant_seconds * sample_rate))`.
/// Returns `0.0` for an instant (`0.0` s) transition.
pub fn one_pole_pole(sample_rate: u32, time_constant_seconds: f64) -> Result<f64, InvalidSmoother> {
    if sample_rate == 0 || !time_constant_seconds.is_finite() || time_constant_seconds < 0.0 {
        return Err(InvalidSmoother);
    }
    if time_constant_seconds == 0.0 {
        return Ok(0.0);
    }
    let pole = (-1.0 / (time_constant_seconds * f64::from(sample_rate))).exp();
    if pole.is_finite() && (0.0..=1.0).contains(&pole) {
        Ok(pole)
    } else {
        Err(InvalidSmoother)
    }
}

/// Compute the one-pole forward step coefficient `1 - exp(-1 / (time_constant_seconds * sample_rate))`.
/// Returns `1.0` for an instant (`0.0` s) transition.
pub fn one_pole_coefficient(
    sample_rate: u32,
    time_constant_seconds: f64,
) -> Result<f64, InvalidSmoother> {
    if sample_rate == 0 || !time_constant_seconds.is_finite() || time_constant_seconds < 0.0 {
        return Err(InvalidSmoother);
    }
    if time_constant_seconds == 0.0 {
        return Ok(1.0);
    }
    let coeff = -(-1.0 / (time_constant_seconds * f64::from(sample_rate))).exp_m1();
    if coeff.is_finite() && (0.0..=1.0).contains(&coeff) {
        Ok(coeff)
    } else {
        Err(InvalidSmoother)
    }
}

/// Compute the one-pole filter pole `exp(-TAU * frequency_hz / sample_rate)`.
pub fn frequency_pole(frequency_hz: f64, sample_rate: f64) -> Result<f64, InvalidSmoother> {
    if !sample_rate.is_finite()
        || sample_rate <= 0.0
        || !frequency_hz.is_finite()
        || frequency_hz < 0.0
    {
        return Err(InvalidSmoother);
    }
    let pole = (-TAU * frequency_hz / sample_rate).exp();
    if pole.is_finite() && (0.0..=1.0).contains(&pole) {
        Ok(pole)
    } else {
        Err(InvalidSmoother)
    }
}

/// Finite-frame double-precision linear parameter ramp reaching its exact
/// target on the final frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearRamp {
    current: f64,
    target: f64,
    step: f64,
    remaining: u32,
}

impl LinearRamp {
    pub fn new(initial: f64) -> Result<Self, InvalidSmoother> {
        if !initial.is_finite() {
            return Err(InvalidSmoother);
        }
        Ok(Self::settled(initial))
    }

    pub const fn settled(value: f64) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }

    pub const fn value(self) -> f64 {
        self.current
    }

    pub const fn target(self) -> f64 {
        self.target
    }

    pub const fn step(self) -> f64 {
        self.step
    }

    pub const fn remaining(self) -> u32 {
        self.remaining
    }

    pub const fn is_active(self) -> bool {
        self.remaining > 0
    }

    pub const fn is_settled(self) -> bool {
        self.remaining == 0
    }

    pub fn reset(&mut self, value: f64) {
        *self = Self::settled(value);
    }

    /// Start a linear ramp from the current value to `target` over `frames`.
    /// When `frames == 0` or `self.current == target`, snaps immediately.
    pub fn retarget(&mut self, target: f64, frames: u32) {
        self.target = target;
        if self.current == target || frames == 0 {
            self.current = target;
            self.step = 0.0;
            self.remaining = 0;
        } else {
            self.step = (target - self.current) / f64::from(frames);
            self.remaining = frames;
        }
    }

    /// Advance one frame using fixed per-frame step addition, snapping cleanly
    /// to `target` on the final step.
    #[inline]
    pub fn advance(&mut self) -> f64 {
        if self.remaining == 0 {
            return self.current;
        }
        if self.remaining == 1 {
            self.current = self.target;
            self.step = 0.0;
            self.remaining = 0;
        } else {
            self.current += self.step;
            self.remaining -= 1;
        }
        self.current
    }

    /// Advance one frame using remaining-count division `(target - current) / remaining`.
    #[inline]
    pub fn advance_by_remaining(&mut self) -> f64 {
        if self.remaining == 0 {
            return self.current;
        }
        self.current += (self.target - self.current) / f64::from(self.remaining);
        self.remaining -= 1;
        if self.remaining == 0 {
            self.current = self.target;
            self.step = 0.0;
        }
        self.current
    }

    /// Inherit an active trajectory from `previous`, starting a new ramp over
    /// `frames` only if this ramp's target differs from `previous.target`.
    pub fn inherit_target(&mut self, previous: Self, frames: u32) {
        let target = self.target;
        *self = previous;
        if target != previous.target {
            self.retarget(target, frames);
        }
    }
}

/// Compact 16-byte single-precision linear ramp with clamped intermediate steps
/// and exact endpoint arrival (for high-parameter-count runtimes like Reverb).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearRampF32 {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
}

impl LinearRampF32 {
    pub fn new(initial: f32) -> Result<Self, InvalidSmoother> {
        if !initial.is_finite() {
            return Err(InvalidSmoother);
        }
        Ok(Self::settled(initial))
    }

    pub const fn settled(value: f32) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }

    pub const fn value(self) -> f32 {
        self.current
    }

    pub const fn target(self) -> f32 {
        self.target
    }

    pub const fn remaining(self) -> u32 {
        self.remaining
    }

    pub const fn is_active(self) -> bool {
        self.remaining > 0
    }

    pub fn retarget(&mut self, target: f32, frames: u32) {
        self.target = target;
        if frames == 0 {
            self.current = target;
            self.step = 0.0;
            self.remaining = 0;
        } else {
            self.step = (target - self.current) / frames as f32;
            self.remaining = frames;
        }
    }

    #[inline]
    pub fn advance(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            self.current = if self.remaining == 0 {
                self.target
            } else if self.step >= 0.0 {
                (self.current + self.step).min(self.target)
            } else {
                (self.current + self.step).max(self.target)
            };
        }
        self.current
    }

    pub fn inherit(&mut self, previous: Self, frames: u32) {
        let target = self.target;
        *self = previous;
        if target != previous.target {
            self.retarget(target, frames);
        }
    }
}

/// One-pole exponential parameter smoother with optional target snap epsilon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnePoleSmoother {
    current: f64,
    target: f64,
    coefficient: f64,
    snap_epsilon: f64,
}

impl OnePoleSmoother {
    pub fn new(
        initial: f64,
        sample_rate: u32,
        time_constant_seconds: f64,
    ) -> Result<Self, InvalidSmoother> {
        if !initial.is_finite() {
            return Err(InvalidSmoother);
        }
        let coefficient = one_pole_coefficient(sample_rate, time_constant_seconds)?;
        Ok(Self {
            current: initial,
            target: initial,
            coefficient,
            snap_epsilon: 0.0,
        })
    }

    pub fn from_coefficient(initial: f64, coefficient: f64) -> Result<Self, InvalidSmoother> {
        if !initial.is_finite() || !coefficient.is_finite() || !(0.0..=1.0).contains(&coefficient) {
            return Err(InvalidSmoother);
        }
        Ok(Self {
            current: initial,
            target: initial,
            coefficient,
            snap_epsilon: 0.0,
        })
    }

    pub fn with_snap_epsilon(mut self, epsilon: f64) -> Result<Self, InvalidSmoother> {
        if !epsilon.is_finite() || epsilon < 0.0 {
            return Err(InvalidSmoother);
        }
        self.snap_epsilon = epsilon;
        Ok(self)
    }

    pub const fn value(self) -> f64 {
        self.current
    }

    pub const fn target(self) -> f64 {
        self.target
    }

    pub const fn coefficient(self) -> f64 {
        self.coefficient
    }

    pub fn is_settled(self) -> bool {
        self.current == self.target
    }

    pub fn set_target(&mut self, target: f64) {
        self.target = target;
    }

    pub fn reset(&mut self, value: f64) {
        self.current = value;
        self.target = value;
    }

    pub fn inherit_value(&mut self, previous: &Self) {
        self.current = previous.current;
    }

    #[inline]
    pub fn advance(&mut self) -> f64 {
        if self.current == self.target {
            return self.current;
        }
        let next = self.current + (self.target - self.current) * self.coefficient;
        self.current = if self.snap_epsilon > 0.0 && (next - self.target).abs() < self.snap_epsilon
        {
            self.target
        } else {
            next
        };
        self.current
    }

    #[inline]
    pub fn advance_toward(&mut self, target: f64) -> f64 {
        self.target = target;
        self.advance()
    }
}

/// Unified parameter smoother supporting either finite linear ramps or one-pole
/// exponential smoothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParameterSmoother {
    Linear(LinearRamp),
    OnePole(OnePoleSmoother),
}

impl ParameterSmoother {
    pub fn linear(initial: f64) -> Result<Self, InvalidSmoother> {
        Ok(Self::Linear(LinearRamp::new(initial)?))
    }

    pub fn one_pole(
        initial: f64,
        sample_rate: u32,
        time_constant_seconds: f64,
    ) -> Result<Self, InvalidSmoother> {
        Ok(Self::OnePole(OnePoleSmoother::new(
            initial,
            sample_rate,
            time_constant_seconds,
        )?))
    }

    pub const fn value(self) -> f64 {
        match self {
            Self::Linear(ramp) => ramp.value(),
            Self::OnePole(smoother) => smoother.value(),
        }
    }

    pub const fn target(self) -> f64 {
        match self {
            Self::Linear(ramp) => ramp.target(),
            Self::OnePole(smoother) => smoother.target(),
        }
    }

    pub fn is_settled(self) -> bool {
        match self {
            Self::Linear(ramp) => ramp.is_settled(),
            Self::OnePole(smoother) => smoother.is_settled(),
        }
    }

    pub fn retarget_linear(&mut self, target: f64, frames: u32) {
        match self {
            Self::Linear(ramp) => ramp.retarget(target, frames),
            Self::OnePole(smoother) => smoother.set_target(target),
        }
    }

    pub fn set_target(&mut self, target: f64) {
        match self {
            Self::Linear(ramp) => ramp.retarget(target, 0),
            Self::OnePole(smoother) => smoother.set_target(target),
        }
    }

    #[inline]
    pub fn advance(&mut self) -> f64 {
        match self {
            Self::Linear(ramp) => ramp.advance(),
            Self::OnePole(smoother) => smoother.advance(),
        }
    }

    pub fn reset(&mut self, value: f64) {
        match self {
            Self::Linear(ramp) => ramp.reset(value),
            Self::OnePole(smoother) => smoother.reset(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_ramp_reaches_exact_endpoint_and_handles_retarget() {
        let mut ramp = ParameterSmoother::linear(0.0).unwrap();
        ramp.retarget_linear(1.0, 4);
        assert!(!ramp.is_settled());
        assert!((ramp.advance() - 0.25).abs() < 1e-12);
        assert!((ramp.advance() - 0.5).abs() < 1e-12);
        assert!((ramp.advance() - 0.75).abs() < 1e-12);
        assert_eq!(ramp.advance(), 1.0);
        assert!(ramp.is_settled());
        assert_eq!(ramp.advance(), 1.0);

        ramp.retarget_linear(0.2, 0);
        assert_eq!(ramp.value(), 0.2);
        assert!(ramp.is_settled());
    }

    #[test]
    fn one_pole_smoother_matches_exponential_law_and_snaps() {
        let mut smoother = OnePoleSmoother::new(0.0, 48_000, 0.010)
            .unwrap()
            .with_snap_epsilon(1.0e-6)
            .unwrap();
        smoother.set_target(1.0);
        for _ in 0..480 {
            smoother.advance();
        }
        let expected = 1.0 - (-1.0_f64).exp();
        assert!((smoother.value() - expected).abs() < 1e-4);
        for _ in 0..10_000 {
            smoother.advance();
        }
        assert_eq!(smoother.value(), 1.0);
        assert!(smoother.is_settled());
    }

    #[test]
    fn linear_ramp_f32_clamps_and_inherits() {
        let mut a = LinearRampF32::new(0.0).unwrap();
        a.retarget(1.0, 5);
        a.advance();
        a.advance();
        let mut b = LinearRampF32::new(1.0).unwrap();
        b.inherit(a, 5);
        assert_eq!(b.remaining(), 3);
        let mut c = LinearRampF32::new(2.0).unwrap();
        c.inherit(a, 10);
        assert_eq!(c.remaining(), 10);
        assert_eq!(c.target(), 2.0);
    }

    #[test]
    fn rejects_invalid_parameters() {
        assert_eq!(LinearRamp::new(f64::NAN), Err(InvalidSmoother));
        assert_eq!(LinearRampF32::new(f32::INFINITY), Err(InvalidSmoother));
        assert_eq!(OnePoleSmoother::new(0.0, 0, 0.01), Err(InvalidSmoother));
        assert_eq!(
            OnePoleSmoother::new(0.0, 48_000, -0.01),
            Err(InvalidSmoother)
        );
        assert_eq!(ramp_frames_for_ms(0, 5.0), Err(InvalidSmoother));
        assert_eq!(ramp_frames_for_ms(48_000, -1.0), Err(InvalidSmoother));
        assert_eq!(ramp_frames_for_ms(48_000, 5.0), Ok(240));
        assert_eq!(frequency_pole(-1.0, 48_000.0), Err(InvalidSmoother));
    }
}
