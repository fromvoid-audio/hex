//! Host-owned ramp of a changed static device matrix across a compatible
//! revision handoff.
//!
//! A memoryless device (for example an unautomated Utility) is prepared as a
//! static [`StereoMatrix`] on its chain entry, with no processor runtime to
//! smooth a direct edit. When history inheritance pairs a candidate entry with
//! its old same-identity entry and the static matrix changed, the candidate
//! ramps every coefficient linearly from the old entry's effective matrix to
//! its own over [`matrix_ramp_frames`] (5 ms at any rate, the Compressor and
//! Saturator law). The ramp position is a pure function of elapsed frames, so
//! block partitioning cannot change output, and the final frame applies the
//! target matrix exactly.
//!
//! No ramp starts when either side is bypassed or automated (an automated
//! entry applies its law in the processor with an identity host matrix), or
//! when the old entry is excluded as a retained tail. Reset (seek and other
//! hard discontinuities) and freshly prepared chains apply the target exactly.
//! The state is `Copy`, lives in the chain's committed/scratch twins and never
//! allocates.

use hex_dsp::{StereoFrame, StereoMatrix};

/// Frames for a static matrix ramp: 5 ms rounded up, at least one frame.
pub(crate) fn matrix_ramp_frames(sample_rate: u32) -> u32 {
    sample_rate.div_ceil(200).max(1)
}

/// Fixed ramp position. Inactive when `elapsed >= total`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MatrixRamp {
    from: [[f32; 2]; 2],
    elapsed: u32,
    total: u32,
}

impl MatrixRamp {
    pub(crate) const INACTIVE: Self = Self {
        from: [[1.0, 0.0], [0.0, 1.0]],
        elapsed: 0,
        total: 0,
    };

    pub(crate) fn is_active(self) -> bool {
        self.elapsed < self.total
    }

    /// Effective coefficients after `elapsed` frames toward `target`.
    pub(crate) fn current(self, target: StereoMatrix) -> [[f32; 2]; 2] {
        if !self.is_active() {
            return target.coefficients();
        }
        self.coefficients_at(target.coefficients(), self.elapsed)
    }

    fn coefficients_at(self, target: [[f32; 2]; 2], elapsed: u32) -> [[f32; 2]; 2] {
        let position = f64::from(elapsed) / f64::from(self.total);
        let mut result = target;
        for (row, output) in result.iter_mut().enumerate() {
            for (column, value) in output.iter_mut().enumerate() {
                let from = f64::from(self.from[row][column]);
                let to = f64::from(target[row][column]);
                *value = (from + (to - from) * position) as f32;
            }
        }
        result
    }

    /// Choose the candidate ramp from the old entry's committed state. `ramps`
    /// is false when either side is bypassed/automated or the old entry is not
    /// a compatible history donor; the candidate then applies its target.
    pub(crate) fn inherited(
        old: Self,
        old_matrix: StereoMatrix,
        candidate_matrix: StereoMatrix,
        sample_rate: u32,
        ramps: bool,
    ) -> Self {
        if !ramps {
            return Self::INACTIVE;
        }
        if candidate_matrix == old_matrix {
            // A twin or unchanged revision continues the same trajectory.
            return old;
        }
        let from = old.current(old_matrix);
        if from == candidate_matrix.coefficients() {
            return Self::INACTIVE;
        }
        Self {
            from,
            elapsed: 0,
            total: matrix_ramp_frames(sample_rate),
        }
    }

    /// Apply the matrix to a block, advancing an active ramp one step per
    /// frame. Allocation-free and bounded by `frames.len()`.
    pub(crate) fn apply_block(&mut self, target: StereoMatrix, frames: &mut [StereoFrame]) {
        let mut ramped = 0;
        if self.is_active() {
            let coefficients = target.coefficients();
            for frame in frames.iter_mut() {
                if !self.is_active() {
                    break;
                }
                self.elapsed += 1;
                *frame = if self.is_active() {
                    let m = self.coefficients_at(coefficients, self.elapsed);
                    [
                        m[0][0] * frame[0] + m[0][1] * frame[1],
                        m[1][0] * frame[0] + m[1][1] * frame[1],
                    ]
                } else {
                    target.apply(*frame)
                };
                ramped += 1;
            }
        }
        if let Some(rest) = frames.get_mut(ramped..) {
            target.apply_block(rest);
        }
    }
}

#[cfg(test)]
mod tests;
