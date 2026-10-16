//! Explicitly shared immutable storage for one v1 resampling rate pair.
use crate::ImportError;
use std::{fmt, sync::Arc};

pub(crate) const PHASE_COUNT: usize = 1_024;
const BASE_SUPPORT_RADIUS: u64 = 32;
const MAX_KERNEL_TABLE_BYTES: usize = 13 * 1_024 * 1_024;

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
            let weights = build_kernel_table(radius, taps, ratio, weight_count)?;
            let allocated_bytes = weights
                .capacity()
                .checked_mul(std::mem::size_of::<f64>())
                .ok_or(ImportError::Invalid("resampling table size overflow"))?;
            if allocated_bytes > MAX_KERNEL_TABLE_BYTES {
                return Err(ImportError::LimitExceeded("resampling kernel table"));
            }
            (radius, taps, weights)
        };
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
) -> Result<Vec<f64>, ImportError> {
    // c is normalized to Nyquist; with normalized sinc, the cycles/sample cutoff is c / 2.
    let cutoff_nyquist = 0.94 * ratio;
    let mut weights = Vec::new();
    weights
        .try_reserve_exact(weight_count)
        .map_err(|_| ImportError::AllocationFailed)?;
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

/// Real sized owner payload and actual Vec backing, not allocator/Arc overhead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResampleKernelDataLayout {
    pub owner_payload_bytes: usize,
    pub owner_payload_alignment: usize,
    pub weight_capacity: usize,
    pub weight_element_bytes: usize,
}
impl ResampleKernel {
    pub fn control_data_layout(&self) -> ResampleKernelDataLayout {
        let KernelData {
            source_rate: _,
            target_rate: _,
            radius: _,
            taps: _,
            weights,
        } = self.inner.as_ref();
        ResampleKernelDataLayout {
            owner_payload_bytes: std::mem::size_of::<KernelData>(),
            owner_payload_alignment: std::mem::align_of::<KernelData>(),
            weight_capacity: weights.capacity(),
            weight_element_bytes: std::mem::size_of::<f64>(),
        }
    }
    pub fn shares_allocation_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}
