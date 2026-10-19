//! Checked integer enclosure for the actual canonical rounded retime scalars.
//! No floating-point topology decisions and no endpoint-sampling certificate.
use hex_project::audio_retime_scale;

const TERMS: usize = 8;

#[derive(Clone, Copy, Debug, Default)]
struct Term {
    value: u64,
    coefficient: i128,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Rounded {
    denominator: u64,
    constant: i128,
    linear: i128,
    terms: [Term; TERMS],
    divisor: i128,
    ceil: bool,
}

impl Rounded {
    pub(super) fn new(denominator: u64, constant: i128, divisor: i128, ceil: bool) -> Option<Self> {
        (denominator > 0 && divisor > 0).then_some(Self {
            denominator,
            constant,
            linear: 0,
            terms: [Term::default(); TERMS],
            divisor,
            ceil,
        })
    }

    pub(super) fn add_scaled(&mut self, value: u64, coefficient: i128) -> Option<()> {
        // round((q*D+r)*N/D) == q*N + round(r*N/D), exactly.
        // Normalize BEFORE bounding to preserve shared-root cancellations.
        self.linear = self
            .linear
            .checked_add(coefficient.checked_mul(i128::from(value / self.denominator))?)?;
        let residual = value % self.denominator;
        if residual == 0 || coefficient == 0 {
            return Some(());
        }
        let slot = self.terms.iter_mut().find(|term| term.value == residual);
        if let Some(term) = slot {
            term.coefficient = term.coefficient.checked_add(coefficient)?;
            return Some(());
        }
        let term = self.terms.iter_mut().find(|term| term.coefficient == 0)?;
        *term = Term {
            value: residual,
            coefficient,
        };
        Some(())
    }

    fn divide(&self, numerator: i128) -> Option<i128> {
        let quotient = numerator.checked_div_euclid(self.divisor)?;
        quotient.checked_add(i128::from(
            self.ceil && numerator.rem_euclid(self.divisor) != 0,
        ))
    }

    pub(super) fn exact(&self, numerator: u64) -> Option<i128> {
        if numerator == 0 {
            return None;
        }
        let mut sum = self
            .constant
            .checked_add(self.linear.checked_mul(i128::from(numerator))?)?;
        for term in &self.terms {
            if term.coefficient != 0 {
                sum = sum.checked_add(term.coefficient.checked_mul(i128::from(
                    audio_retime_scale(term.value, numerator, self.denominator).ok()?,
                ))?)?;
            }
        }
        self.divide(sum)
    }

    pub(super) fn bounds(&self, low: u64, high: u64) -> Option<(i128, i128)> {
        if low == 0 || low > high {
            return None;
        }
        if low == high {
            let exact = self.exact(low)?;
            return Some((exact, exact));
        }
        let denominator = i128::from(self.denominator);
        let mut slope = self.linear.checked_mul(denominator)?;
        let mut error = 0_i128;
        for term in &self.terms {
            slope = slope.checked_add(term.coefficient.checked_mul(i128::from(term.value))?)?;
            // Each nearest-rounded residual differs from its rational value by
            // strictly less than one. This deliberately outward integer bound
            // remains valid for odd D and signed coefficients.
            error = error.checked_add(term.coefficient.checked_abs()?)?;
        }
        let affine_floor = |n: u64| -> Option<(i128, bool)> {
            // Divide slope first: slope*n may overflow even though its quotient
            // and the real model scalar are representable. Residual multiplication
            // uses full u128, supporting the entire u64 retime domain.
            let quotient = slope.checked_div_euclid(denominator)?;
            let residual = u128::try_from(slope.rem_euclid(denominator)).ok()?;
            let product = residual.checked_mul(u128::from(n))?;
            let fraction = product / u128::from(self.denominator);
            let value = quotient
                .checked_mul(i128::from(n))?
                .checked_add(i128::try_from(fraction).ok()?)?;
            Some((value, product % u128::from(self.denominator) != 0))
        };
        let (near, far) = if slope >= 0 { (low, high) } else { (high, low) };
        let (minimum, _) = affine_floor(near)?;
        let (maximum, fractional) = affine_floor(far)?;
        let minimum = self.constant.checked_add(minimum)?.checked_sub(error)?;
        let maximum = self
            .constant
            .checked_add(maximum)?
            .checked_add(i128::from(fractional))?
            .checked_add(error)?;
        Some((self.divide(minimum)?, self.divide(maximum)?))
    }
}

#[test]
fn every_small_interval_encloses_exact_signed_rounded_phase_expressions() {
    let mut checked = 0_u64;
    for denominator in 1..=13 {
        for value in 0..=2 * denominator {
            for coefficient in [-7, -1, 1, 9] {
                for ceil in [false, true] {
                    let mut expression = Rounded::new(denominator, -13, 7, ceil).unwrap();
                    expression.add_scaled(value, coefficient).unwrap();
                    expression.add_scaled(denominator + 1, -3).unwrap();
                    for low in 1..=16 {
                        for high in low..=16 {
                            let (min, max) = expression.bounds(low, high).unwrap();
                            for n in low..=high {
                                let exact = expression.exact(n).unwrap();
                                assert!(
                                    (min..=max).contains(&exact),
                                    "D={denominator} value={value} c={coefficient} interval={low}..{high} n={n} exact={exact} enclosure={min}..{max}"
                                );
                                checked += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    eprintln!("integer enclosures checked against {checked} exact scalar evaluations");
}

#[test]
fn crop_difference_identity_is_exact_and_large_products_do_not_need_signed_multiplication() {
    for denominator in [1, 2, 3, 13, 1 << 32, u64::MAX / 2] {
        let visible_start = denominator / 3;
        let mut expression = Rounded::new(denominator, 0, 1, false).unwrap();
        expression
            .add_scaled(visible_start + denominator, 1)
            .unwrap();
        expression.add_scaled(visible_start, -1).unwrap();
        for n in [1, 2, 19, u64::MAX / 2] {
            assert_eq!(expression.exact(n), Some(i128::from(n)));
        }
        assert_eq!(
            expression.bounds(1, u64::MAX / 2),
            Some((1, i128::from(u64::MAX / 2)))
        );
    }
    let mut expression = Rounded::new(u64::MAX, 0, 1, false).unwrap();
    expression.add_scaled(u64::MAX - 1, 1_i128 << 40).unwrap();
    let (low, high) = expression.bounds(u64::MAX / 2, u64::MAX - 1).unwrap();
    for n in [u64::MAX / 2, u64::MAX - 1] {
        assert!((low..=high).contains(&expression.exact(n).unwrap()));
    }
}
