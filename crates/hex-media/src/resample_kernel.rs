//! Explicitly shared immutable storage for one v1 resampling rate pair.
use crate::ImportError;
use std::{fmt, sync::Arc};

pub(crate) const PHASE_COUNT: usize = 1_024;
const BASE_SUPPORT_RADIUS: u64 = 32;
const MAX_KERNEL_TABLE_BYTES: usize = 13 * 1_024 * 1_024;

/// Actual reserved weight data and one shared kernel object, excluding Arc/
/// allocator metadata and stack. Failed weight storage has been reclaimed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KernelBuildCapacity {
    pub weight_slots: usize,
    pub shared_object_bytes: usize,
    pub reservation_attempted: bool,
}
impl KernelBuildCapacity {
    pub fn weight_bytes(self) -> Option<usize> {
        self.weight_slots.checked_mul(std::mem::size_of::<f64>())
    }
    pub fn data_bytes(self) -> Option<usize> {
        self.weight_bytes()?.checked_add(self.shared_object_bytes)
    }
}
#[derive(Debug)]
pub struct KernelBuildFailure {
    error: ImportError,
    capacity: KernelBuildCapacity,
}
impl KernelBuildFailure {
    pub fn into_parts(self) -> (ImportError, KernelBuildCapacity) {
        (self.error, self.capacity)
    }
}
impl fmt::Display for KernelBuildFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}
impl std::error::Error for KernelBuildFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// Cloneable, control-side handle to a prepared v1 sinc kernel.
///
/// Clones share one immutable table. There is no global cache; callers decide
/// when to reuse a kernel and must keep its final drop off the audio callback.
#[derive(Clone)]
pub struct ResampleKernel {
    pub(crate) inner: Arc<KernelData>,
}

pub(crate) struct KernelData {
    pub(crate) source_rate: u32,
    pub(crate) target_rate: u32,
    pub(crate) radius: usize,
    pub(crate) taps: usize,
    pub(crate) weights: Vec<f64>,
}

impl fmt::Debug for ResampleKernel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResampleKernel")
            .field("source_rate", &self.source_rate())
            .field("target_rate", &self.target_rate())
            .field("table_payload_bytes", &self.table_payload_bytes())
            .finish()
    }
}

impl ResampleKernel {
    /// Prepares the v1 table for one source/target rate pair.
    pub fn new(source_rate: u32, target_rate: u32) -> Result<Self, ImportError> {
        Self::new_reported(source_rate, target_rate)
            .map(|(kernel, _)| kernel)
            .map_err(|failure| failure.into_parts().0)
    }

    /// Same v1 kernel math and limits, with actual construction data capacities
    /// preserved on API-reported refusal. No process/allocator overhead claim.
    pub fn new_reported(
        source_rate: u32,
        target_rate: u32,
    ) -> Result<(Self, KernelBuildCapacity), KernelBuildFailure> {
        Self::build_reported(source_rate, target_rate, |weights, count| {
            weights
                .try_reserve_exact(count)
                .map_err(|_| ImportError::AllocationFailed)
        })
    }

    fn build_reported(
        source_rate: u32,
        target_rate: u32,
        reserve: impl FnOnce(&mut Vec<f64>, usize) -> Result<(), ImportError>,
    ) -> Result<(Self, KernelBuildCapacity), KernelBuildFailure> {
        let mut capacity = KernelBuildCapacity::default();
        match Self::build(source_rate, target_rate, &mut capacity, reserve) {
            Ok(kernel) => Ok((kernel, capacity)),
            Err(error) => Err(KernelBuildFailure { error, capacity }),
        }
    }

    fn build(
        source_rate: u32,
        target_rate: u32,
        capacity: &mut KernelBuildCapacity,
        reserve: impl FnOnce(&mut Vec<f64>, usize) -> Result<(), ImportError>,
    ) -> Result<Self, ImportError> {
        validate_rates(source_rate, target_rate)?;
        let (radius, taps, weights) = if source_rate == target_rate {
            (0, 0, Vec::new())
        } else {
            let ratio = if target_rate < source_rate {
                f64::from(target_rate) / f64::from(source_rate)
            } else {
                1.0
            };
            let radius = support_radius(source_rate, target_rate)?;
            let taps = radius
                .checked_mul(2)
                .and_then(|count| count.checked_add(1))
                .ok_or(ImportError::Invalid("resampling kernel size overflow"))?;
            let weight_count = PHASE_COUNT
                .checked_mul(taps)
                .ok_or(ImportError::Invalid("resampling table size overflow"))?;
            let requested_bytes = weight_count
                .checked_mul(std::mem::size_of::<f64>())
                .ok_or(ImportError::Invalid("resampling table size overflow"))?;
            if requested_bytes > MAX_KERNEL_TABLE_BYTES {
                return Err(ImportError::LimitExceeded("resampling kernel table"));
            }
            let weights = build_kernel_table(radius, taps, ratio, weight_count, capacity, reserve)?;
            let allocated_bytes = weights
                .capacity()
                .checked_mul(std::mem::size_of::<f64>())
                .ok_or(ImportError::Invalid("resampling table size overflow"))?;
            if allocated_bytes > MAX_KERNEL_TABLE_BYTES {
                return Err(ImportError::LimitExceeded("resampling kernel table"));
            }
            (radius, taps, weights)
        };
        capacity.shared_object_bytes = std::mem::size_of::<KernelData>();
        Ok(Self {
            inner: Arc::new(KernelData {
                source_rate,
                target_rate,
                radius,
                taps,
                weights,
            }),
        })
    }

    pub fn source_rate(&self) -> u32 {
        self.inner.source_rate
    }

    pub fn target_rate(&self) -> u32 {
        self.inner.target_rate
    }

    /// Actual capacity of the immutable f64 weight allocation, excluding Arc metadata.
    pub fn table_payload_bytes(&self) -> usize {
        self.inner.weights.capacity() * std::mem::size_of::<f64>()
    }

    /// Data size of the one shared kernel object, excluding its separately
    /// reported weight allocation and opaque standard-library Arc header.
    /// Count once across handles; this borrowed query creates no new owner.
    pub const fn shared_object_bytes(&self) -> usize {
        std::mem::size_of::<KernelData>()
    }
}

pub(crate) fn validate_rates(source_rate: u32, target_rate: u32) -> Result<(), ImportError> {
    if !(8_000..=192_000).contains(&source_rate) || !(8_000..=192_000).contains(&target_rate) {
        return Err(ImportError::Invalid(
            "sample rates must be within 8000..192000 hz",
        ));
    }
    Ok(())
}

fn support_radius(source_rate: u32, target_rate: u32) -> Result<usize, ImportError> {
    let radius = if target_rate >= source_rate {
        BASE_SUPPORT_RADIUS
    } else {
        let numerator = BASE_SUPPORT_RADIUS
            .checked_mul(u64::from(source_rate))
            .ok_or(ImportError::Invalid("resampling support radius overflow"))?;
        numerator.div_ceil(u64::from(target_rate))
    };
    usize::try_from(radius)
        .map_err(|_| ImportError::Invalid("resampling support radius exceeds addressable range"))
}

fn build_kernel_table(
    radius: usize,
    taps: usize,
    ratio: f64,
    weight_count: usize,
    capacity: &mut KernelBuildCapacity,
    reserve: impl FnOnce(&mut Vec<f64>, usize) -> Result<(), ImportError>,
) -> Result<Vec<f64>, ImportError> {
    // c is normalized to Nyquist; with normalized sinc, the cycles/sample cutoff is c / 2.
    let cutoff_nyquist = 0.94 * ratio;
    let mut weights = Vec::new();
    capacity.reservation_attempted = true;
    let result = reserve(&mut weights, weight_count);
    capacity.weight_slots = weights.capacity();
    result?;
    for phase_index in 0..PHASE_COUNT {
        let phase = phase_index as f64 / PHASE_COUNT as f64;
        let row_start = weights.len();
        let mut sum = 0.0_f64;
        for tap_index in 0..taps {
            let offset = tap_index as i64 - radius as i64;
            let distance = offset as f64 - phase;
            let weight = if distance.abs() < radius as f64 {
                let window = 0.5 * (1.0 + (std::f64::consts::PI * distance / radius as f64).cos());
                cutoff_nyquist * sinc(cutoff_nyquist * distance) * window
            } else {
                0.0
            };
            if !weight.is_finite() {
                return Err(ImportError::Invalid("resampling kernel is non-finite"));
            }
            sum += weight;
            weights.push(weight);
        }
        if !sum.is_finite() || sum.abs() < f64::EPSILON {
            return Err(ImportError::Invalid(
                "resampling kernel cannot be normalized",
            ));
        }
        for weight in &mut weights[row_start..row_start + taps] {
            *weight /= sum;
        }
    }
    Ok(weights)
}

fn sinc(value: f64) -> f64 {
    if value.abs() < 1.0e-12 {
        1.0
    } else {
        let angle = std::f64::consts::PI * value;
        angle.sin() / angle
    }
}

#[cfg(test)]
mod reporting_tests {
    use super::*;
    #[test]
    fn reported_build_matches_legacy_and_preserves_refused_capacity() {
        for (source, target) in [(48000, 48000), (44100, 48000), (48000, 16000)] {
            let legacy = ResampleKernel::new(source, target).unwrap();
            let (reported, capacity) = ResampleKernel::new_reported(source, target).unwrap();
            assert_eq!(legacy.inner.weights, reported.inner.weights);
            assert_eq!(
                capacity.weight_bytes(),
                Some(reported.table_payload_bytes())
            );
            assert_eq!(capacity.shared_object_bytes, reported.shared_object_bytes());
        }
        let failure =
            ResampleKernel::build_reported(44100, 48000, |_, _| Err(ImportError::AllocationFailed))
                .unwrap_err();
        let (error, capacity) = failure.into_parts();
        assert!(matches!(error, ImportError::AllocationFailed));
        assert!(capacity.reservation_attempted);
        assert_eq!(capacity.weight_slots, 0);
        let mut observed = 0;
        let failure = ResampleKernel::build_reported(44100, 48000, |weights, count| {
            weights.try_reserve_exact(count).unwrap();
            observed = weights.capacity();
            Err(ImportError::AllocationFailed)
        })
        .unwrap_err();
        assert_eq!(failure.into_parts().1.weight_slots, observed);
        assert_eq!(
            KernelBuildCapacity {
                weight_slots: usize::MAX,
                ..KernelBuildCapacity::default()
            }
            .data_bytes(),
            None
        );
    }
}
