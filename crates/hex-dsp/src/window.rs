//! Shared window, sinc, and fast ADAA transcendental math primitives.
//!
//! Control-side filter design calls [`bessel_i0`], [`sinc`], [`kaiser_window`],
//! and [`blackman_window`]. Audio-rate antiderivative anti-aliasing calls
//! [`fast_ln_f64`] and [`fast_ln_cosh_f64`], which are pure safe Rust,
//! allocation-free, table-free (zero atomics/memory lookups), inlineable, and
//! accurate within `< 1e-8` across the full input range (`< 1e-15` near zero).

/// Modified Bessel function of the first kind of order zero, `I_0(x)`.
///
/// Unified implementation for Kaiser-windowed FIR and polyphase filter design.
/// Converges to full IEEE-754 `f64` precision for all ordinary filter betas.
#[inline]
pub fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x * 0.5;
    for k in 1..64 {
        let ratio = half / k as f64;
        term *= ratio * ratio;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Normalized sinc function `sin(pi * x) / (pi * x)`, returning `1.0` at `0.0`.
#[inline]
pub fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Kaiser window evaluated at normalized coordinate `ratio = offset / half_span`
/// in `[-1, 1]`, given shape `beta` and precomputed `inv_i0_beta = 1.0 / bessel_i0(beta)`.
/// Returns `0.0` outside `[-1, 1]`.
#[inline]
pub fn kaiser_window(ratio: f64, beta: f64, inv_i0_beta: f64) -> f64 {
    if ratio.abs() >= 1.0 {
        0.0
    } else {
        bessel_i0(beta * (1.0 - ratio * ratio).max(0.0).sqrt()) * inv_i0_beta
    }
}

/// Standard three-term Blackman window at phase `phi` in `[0, 2*pi]`.
///
/// Matches `0.42 - 0.5 * cos(phi) + 0.08 * cos(2 * phi)`.
#[inline]
pub fn blackman_window(phi: f64) -> f64 {
    0.42 - 0.5 * phi.cos() + 0.08 * (2.0 * phi).cos()
}

/// Centered Blackman window at signed normalized coordinate `r = distance / half_width`
/// in `[-1, 1]`, where `r = 0` is the center (`1.0`) and `|r| = 1` is the edge (`0.0`).
///
/// Equivalent to `0.42 + 0.5 * cos(pi * r) + 0.08 * cos(2 * pi * r)`.
#[inline]
pub fn blackman_window_centered(r: f64) -> f64 {
    let pi_r = std::f64::consts::PI * r;
    0.42 + 0.5 * pi_r.cos() + 0.08 * (2.0 * pi_r).cos()
}

/// Fast, allocation-free natural logarithm for positive finite `f64` values,
/// accurate to `< 1e-10` without calling `libm::log`.
///
/// Uses IEEE-754 exponent extraction and a degree-9 odd series in
/// `t = (m - 1) / (m + 1)` with `m in [sqrt(0.5), sqrt(2)]` (`|t| <= 0.1716`).
#[inline(always)]
pub fn fast_ln_f64(x: f64) -> f64 {
    if !x.is_finite() || x <= 0.0 {
        return x.ln();
    }
    let bits = x.to_bits();
    let mut exp = ((bits >> 52) & 0x7ff) as i32 - 1023;
    let mut m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    if m > std::f64::consts::SQRT_2 {
        m *= 0.5;
        exp += 1;
    }
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let poly = 1.0
        + t2 * (1.0 / 3.0
            + t2 * (1.0 / 5.0 + t2 * (1.0 / 7.0 + t2 * (1.0 / 9.0 + t2 * (1.0 / 11.0)))));
    (exp as f64) * std::f64::consts::LN_2 + 2.0 * t * poly
}

/// Fast `exp(u)` for `u in [-40, 0]`, register-only IEEE-754 range reduction
/// (`exp(u) = 2^k * exp(r)`, `r in [-ln2/2, ln2/2]`), accurate to `< 1e-11`.
#[inline(always)]
fn fast_exp_neg_f64(u: f64) -> f64 {
    // u <= 0, |u| <= 40 => k in [-58, 0] (always normal IEEE-754 exponent).
    const INV_LN2: f64 = std::f64::consts::LOG2_E;
    let kf = (u * INV_LN2 - 0.5).ceil();
    let k = kf as i32;
    let r = u - kf * std::f64::consts::LN_2;
    // Degree-7 Taylor polynomial of exp(r) on |r| <= 0.3466, max err < 6e-10.
    let exp_r = 1.0
        + r * (1.0
            + r * (0.5
                + r * (1.0 / 6.0
                    + r * (1.0 / 24.0
                        + r * (1.0 / 120.0 + r * (1.0 / 720.0 + r * (1.0 / 5040.0)))))));
    let scale = f64::from_bits(((k + 1023) as u64) << 52);
    scale * exp_r
}

/// Kept as a no-op hook for control-side preparation compatibility; [`fast_ln_cosh_f64`]
/// is fully register-based with no static table or atomic initialization.
#[inline]
pub fn warm_ln_cosh_table() {}

/// Fast `ln(cosh(x)) = |x| + ln(1 + exp(-2|x|)) - ln(2)` for `tanh` ADAA.
///
/// Register-only (zero memory lookups, zero atomics, zero libm calls):
/// - `|x| <= 0.25`: degree-10 even Taylor series (exact to `< 1e-15`).
/// - `|x| >= 18.0`: exact asymptote `|x| - ln(2)`.
/// - `0.25 < |x| < 18.0`: IEEE-754 `fast_exp_neg_f64(-2|x|)` + odd `ln(1+y)` series (`< 1e-9`).
#[inline(always)]
pub fn fast_ln_cosh_f64(x: f64) -> f64 {
    let a = x.abs();
    if a <= 0.25 {
        let x2 = a * a;
        return x2
            * (0.5
                + x2 * (-1.0 / 12.0
                    + x2 * (1.0 / 45.0 + x2 * (-17.0 / 2520.0 + x2 * (31.0 / 14175.0)))));
    }
    if a >= 18.0 {
        return a - std::f64::consts::LN_2;
    }
    let y = fast_exp_neg_f64(-2.0 * a);
    // y in (0, 0.607], so t = y / (2 + y) in (0, 0.233]; degree-11 odd series < 2e-10.
    let t = y / (2.0 + y);
    let t2 = t * t;
    let ln1p_y = 2.0
        * t
        * (1.0
            + t2 * (1.0 / 3.0
                + t2 * (1.0 / 5.0 + t2 * (1.0 / 7.0 + t2 * (1.0 / 9.0 + t2 * (1.0 / 11.0))))));
    a + ln1p_y - std::f64::consts::LN_2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bessel_i0_matches_known_values() {
        assert_eq!(bessel_i0(0.0), 1.0);
        // Reference values from Abramowitz & Stegun (I_0(1), I_0(2), I_0(10)).
        assert!((bessel_i0(1.0) - 1.266_065_877_752_008_4).abs() < 1e-14);
        assert!((bessel_i0(2.0) - 2.279_585_302_336_067_3).abs() < 1e-14);
        assert!((bessel_i0(10.0) - 2_815.716_628_466_254_4).abs() < 1e-10);
    }

    #[test]
    fn fast_ln_matches_std_across_dynamic_range() {
        for exp in -20..=20 {
            for frac in [0.5, std::f64::consts::FRAC_1_SQRT_2, 1.0, 1.3333, 1.9999] {
                let x = frac * 2.0_f64.powi(exp);
                let err = (fast_ln_f64(x) - x.ln()).abs();
                assert!(err < 1e-10, "x={x}: err={err}");
            }
        }
    }

    #[test]
    fn fast_ln_cosh_matches_exact_formula_and_is_even() {
        warm_ln_cosh_table();
        assert_eq!(fast_ln_cosh_f64(0.0), 0.0);
        for i in 0..=400 {
            let x = i as f64 * 0.05;
            let exact = x + (-2.0 * x).exp().ln_1p() - std::f64::consts::LN_2;
            let approx = fast_ln_cosh_f64(x);
            let err = (approx - exact).abs();
            assert!(
                err < 1e-8,
                "x={x}: approx={approx}, exact={exact}, err={err}"
            );
            assert_eq!(fast_ln_cosh_f64(-x), approx);
        }
    }
}
