//! Memoryless stereo kernels extracted without changing arithmetic or input policy.
use std::fmt;

/// One interleaved stereo frame represented as left and right samples.
pub type StereoFrame = [f32; 2];

/// A coefficient or composition failed finite single-precision validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMatrix(pub &'static str);

impl fmt::Display for InvalidMatrix {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for InvalidMatrix {}

/// An immutable 2×2 transform whose coefficients are finite `f32` values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StereoMatrix {
    coefficients: [[f32; 2]; 2],
}

impl StereoMatrix {
    /// Identity preserves each input sample bit-for-bit in [`Self::apply`].
    pub const fn identity() -> Self {
        Self {
            coefficients: [[1.0, 0.0], [0.0, 1.0]],
        }
    }

    /// Construct from output-row/input-column coefficients after finite checks.
    pub fn new(coefficients: [[f32; 2]; 2]) -> Result<Self, InvalidMatrix> {
        if coefficients
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(InvalidMatrix("stereo matrix coefficients must be finite"));
        }
        Ok(Self { coefficients })
    }

    /// Return the immutable output-row/input-column coefficient array.
    pub const fn coefficients(self) -> [[f32; 2]; 2] {
        self.coefficients
    }

    /// Report the fixed delay of this memoryless matrix transform.
    pub const fn latency_frames(self) -> u32 {
        0
    }

    /// Apply this matrix to one stereo frame.
    pub fn apply(self, input: StereoFrame) -> StereoFrame {
        if self.is_identity() {
            return input;
        }
        let matrix = self.coefficients;
        [
            matrix[0][0] * input[0] + matrix[0][1] * input[1],
            matrix[1][0] * input[0] + matrix[1][1] * input[1],
        ]
    }

    /// Apply this matrix in place to a caller-owned block, without allocation.
    pub fn apply_block(self, frames: &mut [StereoFrame]) {
        if self.is_identity() {
            return;
        }
        for frame in frames {
            *frame = self.apply(*frame);
        }
    }

    /// Compose in signal order: `first.then(next)` applies `first`, then `next`.
    /// The result is refused if any composed `f32` coefficient is non-finite.
    pub fn then(self, next: Self) -> Result<Self, InvalidMatrix> {
        if self.is_identity() {
            return Ok(next);
        }
        if next.is_identity() {
            return Ok(self);
        }
        let a = self.coefficients;
        let b = next.coefficients;
        let mut result = [[0.0; 2]; 2];
        for (row, output) in result.iter_mut().enumerate() {
            for (column, coefficient) in output.iter_mut().enumerate() {
                let value = f64::from(b[row][0]) * f64::from(a[0][column])
                    + f64::from(b[row][1]) * f64::from(a[1][column]);
                if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
                    return Err(InvalidMatrix("stereo matrix composition overflowed"));
                }
                *coefficient = value as f32;
                if !coefficient.is_finite() {
                    return Err(InvalidMatrix("stereo matrix composition overflowed"));
                }
            }
        }
        Self::new(result)
    }

    /// Whether coefficients are exactly unity with no cross-channel contribution.
    pub fn is_identity(self) -> bool {
        self.coefficients == [[1.0, 0.0], [0.0, 1.0]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_is_a_zero_latency_bit_preserving_fast_path() {
        let identity = StereoMatrix::identity();
        assert_eq!(identity.latency_frames(), 0);
        let input = [f32::from_bits(0x8000_0000), f32::from_bits(0x7fc0_0123)];
        let output = identity.apply(input);
        assert_eq!(output[0].to_bits(), input[0].to_bits());
        assert_eq!(output[1].to_bits(), input[1].to_bits());
        let mut block = [input, [1.0, -1.0]];
        identity.apply_block(&mut block);
        assert_eq!(block[0][1].to_bits(), input[1].to_bits());
    }

    #[test]
    fn coefficients_are_finite_and_composition_overflow_is_refused() {
        assert!(StereoMatrix::new([[f32::NAN, 0.0], [0.0, 1.0]]).is_err());
        assert!(StereoMatrix::new([[f32::INFINITY, 0.0], [0.0, 1.0]]).is_err());
        let large = StereoMatrix::new([[f32::MAX, 0.0], [0.0, f32::MAX]]).unwrap();
        let scale = StereoMatrix::new([[2.0, 0.0], [0.0, 2.0]]).unwrap();
        assert!(large.then(scale).is_err());
    }

    #[test]
    fn composition_preserves_serial_noncommutative_order() {
        let swap = StereoMatrix::new([[0.0, 1.0], [1.0, 0.0]]).unwrap();
        let left_gain = StereoMatrix::new([[2.0, 0.0], [0.0, 1.0]]).unwrap();
        let swapped_then_gain = swap.then(left_gain).unwrap();
        let gain_then_swap = left_gain.then(swap).unwrap();
        assert_eq!(swapped_then_gain.apply([1.0, 3.0]), [6.0, 1.0]);
        assert_eq!(gain_then_swap.apply([1.0, 3.0]), [3.0, 2.0]);
    }
}
