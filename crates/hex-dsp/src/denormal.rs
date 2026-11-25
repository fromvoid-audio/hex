//! Standardized subnormal and near-silent state flushing helpers.
//!
//! Hex does not configure CPU flush-to-zero / denormals-are-zero modes and does
//! not assume host floating-point flags. Recursive filter, envelope, and
//! dynamics histories call these helpers explicitly so silent tails retire to
//! exact zero without accumulating subnormals.

/// Standard amplitude floor (`-400 dBFS`) for flushing recursive signal and
/// filter histories to exact zero well above `f64` and `f32` subnormals.
pub const QUIET_FLOOR: f64 = 1.0e-20;

/// Single-precision amplitude floor matching [`QUIET_FLOOR`].
pub const QUIET_FLOOR_F32: f32 = 1.0e-20;

/// Deep history / squared-energy floor (`sqrt(1e-30) = -300 dBFS`) for RMS
/// power integrators and deep double-precision filter histories.
pub const POWER_FLOOR: f64 = 1.0e-30;

/// Flush near-silent or subnormal `f64` values below [`QUIET_FLOOR`] to exact `0.0`.
/// Non-finite values (`NaN`, `±inf`) are preserved untouched so caller guards
/// can still detect them.
#[inline]
pub fn flush_quiet(value: f64) -> f64 {
    if value.abs() < QUIET_FLOOR {
        0.0
    } else {
        value
    }
}

/// Flush near-silent or subnormal `f32` values below [`QUIET_FLOOR_F32`] to exact `0.0`.
#[inline]
pub fn flush_quiet_f32(value: f32) -> f32 {
    if value.abs() < QUIET_FLOOR_F32 {
        0.0
    } else {
        value
    }
}

/// Flush `f64` values whose magnitude is strictly below `floor` to exact `0.0`.
#[inline]
pub fn flush_below(value: f64, floor: f64) -> f64 {
    if value.abs() < floor { 0.0 } else { value }
}

/// Clear a two-element state pair together when both entries are below [`QUIET_FLOOR`].
///
/// Independently snapping one component of a two-pole high-pass recurrence can
/// sustain a near-floor limit cycle; clearing the pair together guarantees
/// quiet convergence to exact zero.
#[inline]
pub fn flush_quiet_pair(state: &mut [f64; 2]) {
    if state[0].abs() < QUIET_FLOOR && state[1].abs() < QUIET_FLOOR {
        *state = [0.0; 2];
    }
}

/// Flush only IEEE-754 `f64` subnormals to exact `0.0`, leaving normal values intact.
#[inline]
pub fn flush_subnormal(value: f64) -> f64 {
    if value.is_subnormal() { 0.0 } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flush_quiet_clears_subnormals_and_values_below_floor_only() {
        assert_eq!(flush_quiet(0.0), 0.0);
        assert_eq!(flush_quiet(-0.0), 0.0);
        assert_eq!(flush_quiet(f64::MIN_POSITIVE * 0.5), 0.0);
        assert_eq!(flush_quiet(-f64::MIN_POSITIVE * 0.5), 0.0);
        assert_eq!(flush_quiet(1.0e-25), 0.0);
        assert_eq!(flush_quiet(-9.9e-21), 0.0);
        assert_eq!(flush_quiet(QUIET_FLOOR), QUIET_FLOOR);
        assert_eq!(flush_quiet(-QUIET_FLOOR), -QUIET_FLOOR);
        assert_eq!(flush_quiet(0.25), 0.25);
        assert!(flush_quiet(f64::NAN).is_nan());
        assert_eq!(flush_quiet(f64::INFINITY), f64::INFINITY);
    }

    #[test]
    fn flush_quiet_f32_and_power_floor_behave_consistently() {
        assert_eq!(flush_quiet_f32(1.0e-25_f32), 0.0);
        assert_eq!(flush_quiet_f32(-1.0e-21_f32), 0.0);
        assert_eq!(flush_quiet_f32(QUIET_FLOOR_F32), QUIET_FLOOR_F32);
        assert_eq!(flush_below(1.0e-31, POWER_FLOOR), 0.0);
        assert_eq!(flush_below(POWER_FLOOR, POWER_FLOOR), POWER_FLOOR);
        assert_eq!(flush_subnormal(f64::MIN_POSITIVE * 0.5), 0.0);
        assert_eq!(flush_subnormal(f64::MIN_POSITIVE), f64::MIN_POSITIVE);
    }

    #[test]
    fn flush_quiet_pair_clears_only_when_both_entries_are_quiet() {
        let mut mixed = [1.0e-25, 1.0e-10];
        flush_quiet_pair(&mut mixed);
        assert_eq!(mixed, [1.0e-25, 1.0e-10]);

        let mut both_quiet = [1.0e-25, -5.0e-21];
        flush_quiet_pair(&mut both_quiet);
        assert_eq!(both_quiet, [0.0, 0.0]);
    }
}
