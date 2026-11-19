//! Explicitly shared immutable storage for one resampling rate pair.
//!
//! **v1** (`SINC_CONVERSION_V1`): Hann-windowed sinc, base radius 32, cutoff
//! 0.94 of the lower Nyquist, 1,024 phase rows chosen by rounding. Its output
//! is pinned bit-for-bit and must never change.
//!
//! **v2**: Kaiser-windowed sinc designed for [`V2_STOPBAND_DB`] attenuation,
//! base radius 64, 512 phase rows plus a closing row, linearly interpolated
//! between adjacent rows by the exact rational phase. The transition band is
//! placed below the lower Nyquist so every frequency that would alias into the
//! output lies in the stopband. Measured limits live in `resample_quality_tests`.
use crate::ImportError;
use std::{fmt, sync::Arc};

pub(crate) const PHASE_COUNT: usize = 1_024;
pub(crate) const V2_PHASE_COUNT: usize = 512;
const BASE_SUPPORT_RADIUS: u64 = 32;
const V2_BASE_SUPPORT_RADIUS: u64 = 64;
/// Design attenuation for the v2 Kaiser window (dB).
pub const V2_STOPBAND_DB: f64 = 100.0;
const MAX_KERNEL_TABLE_BYTES: usize = 13 * 1_024 * 1_024;

/// Versioned sample-rate conversion law. Each version's output is fixed once
/// shipped. Callers must add saved-format support before persisting v2 media.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResampleVersion {
    V1,
    V2,
}

impl ResampleVersion {
    /// The conversion law a saved asset's `conversion_version` names.
    /// Identity assets never reach kernel weights, so they select v1, whose
    /// identity plan shares the input unchanged. Unknown versions refuse.
    pub fn for_conversion(conversion_version: u32) -> Result<Self, ImportError> {
        match conversion_version {
            hex_project::IDENTITY_CONVERSION | hex_project::SINC_CONVERSION_V1 => Ok(Self::V1),
            hex_project::SINC_CONVERSION_V2 => Ok(Self::V2),
            _ => Err(ImportError::Invalid("unsupported media conversion version")),
        }
    }
}

/// Cloneable, control-side handle to a prepared sinc kernel.
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
    pub(crate) version: ResampleVersion,
    pub(crate) radius: usize,
    pub(crate) taps: usize,
    pub(crate) weights: Vec<f64>,
}

impl fmt::Debug for ResampleKernel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResampleKernel")
            .field("source_rate", &self.source_rate())
            .field("target_rate", &self.target_rate())
            .field("version", &self.version())
            .field("table_payload_bytes", &self.table_payload_bytes())
            .finish()
    }
}

impl ResampleKernel {
    /// Prepares the v1 table for one source/target rate pair.
    pub fn new(source_rate: u32, target_rate: u32) -> Result<Self, ImportError> {
        Self::with_version(source_rate, target_rate, ResampleVersion::V1)
    }

    /// Prepares the table of `version` for one source/target rate pair.
    pub fn with_version(
        source_rate: u32,
        target_rate: u32,
        version: ResampleVersion,
    ) -> Result<Self, ImportError> {
        validate_rates(source_rate, target_rate)?;
        let (base_radius, rows) = match version {
            ResampleVersion::V1 => (BASE_SUPPORT_RADIUS, PHASE_COUNT),
            ResampleVersion::V2 => (V2_BASE_SUPPORT_RADIUS, V2_PHASE_COUNT + 1),
        };
        let (radius, taps, weights) = if source_rate == target_rate {
            (0, 0, Vec::new())
        } else {
            let ratio = if target_rate < source_rate {
                f64::from(target_rate) / f64::from(source_rate)
            } else {
                1.0
            };
            let radius = support_radius(base_radius, source_rate, target_rate)?;
            let taps = radius
                .checked_mul(2)
                .and_then(|count| count.checked_add(1))
                .ok_or(ImportError::Invalid("resampling kernel size overflow"))?;
            let weight_count = rows
                .checked_mul(taps)
                .ok_or(ImportError::Invalid("resampling table size overflow"))?;
            let requested_bytes = weight_count
                .checked_mul(std::mem::size_of::<f64>())
                .ok_or(ImportError::Invalid("resampling table size overflow"))?;
            if requested_bytes > MAX_KERNEL_TABLE_BYTES {
                return Err(ImportError::LimitExceeded("resampling kernel table"));
            }
            let weights = match version {
                ResampleVersion::V1 => build_kernel_table(radius, taps, ratio, weight_count)?,
                ResampleVersion::V2 => build_v2_kernel_table(radius, taps, ratio, weight_count)?,
            };
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
                version,
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

    pub fn version(&self) -> ResampleVersion {
        self.inner.version
    }

    /// Source frames read on each side of an output frame's center (already
    /// scaled for downsampling); callers size source halos from this.
    pub fn support_radius(&self) -> usize {
        self.inner.radius
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

fn support_radius(base: u64, source_rate: u32, target_rate: u32) -> Result<usize, ImportError> {
    let radius = if target_rate >= source_rate {
        base
    } else {
        let numerator = base
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

/// Kaiser-windowed sinc rows for phases `0, 1/P, .., 1` (P = [`V2_PHASE_COUNT`]);
/// the closing row lets the renderer interpolate every fractional phase.
fn build_v2_kernel_table(
    radius: usize,
    taps: usize,
    ratio: f64,
    weight_count: usize,
) -> Result<Vec<f64>, ImportError> {
    let beta = 0.1102 * (V2_STOPBAND_DB - 8.7);
    // Kaiser estimate of the transition width for 2 * radius taps, as a
    // fraction of the source Nyquist, placed wholly below the lower Nyquist.
    let transition = (V2_STOPBAND_DB - 7.95) / (2.285 * 2.0 * radius as f64 * std::f64::consts::PI);
    let cutoff_nyquist = ratio - transition / 2.0;
    if cutoff_nyquist.is_nan() || cutoff_nyquist <= 0.0 {
        return Err(ImportError::Invalid("resampling kernel cutoff is empty"));
    }
    let normalizer = bessel_i0(beta);
    let mut weights = Vec::new();
    weights
        .try_reserve_exact(weight_count)
        .map_err(|_| ImportError::AllocationFailed)?;
    for phase_index in 0..=V2_PHASE_COUNT {
        let phase = phase_index as f64 / V2_PHASE_COUNT as f64;
        let row_start = weights.len();
        let mut sum = 0.0_f64;
        for tap_index in 0..taps {
            let offset = tap_index as i64 - radius as i64;
            let distance = offset as f64 - phase;
            let relative = distance / radius as f64;
            let weight = if relative.abs() < 1.0 {
                let window = bessel_i0(beta * (1.0 - relative * relative).sqrt()) / normalizer;
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

/// Zeroth-order modified Bessel function of the first kind, by its power series.
fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
    let mut term = 1.0_f64;
    let mut sum = 1.0_f64;
    for k in 1..64 {
        term *= quarter / (k * k) as f64;
        sum += term;
        if term < sum * 1.0e-17 {
            break;
        }
    }
    sum
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
    /// Fixed sized DATA payload for one constructor kernel allocation, excluding
    /// its separately bounded table and opaque Arc/allocator metadata.
    pub const fn owner_payload_bytes() -> usize {
        std::mem::size_of::<KernelData>()
    }
    /// Constructor table ceiling used before any source worker starts.
    pub const fn max_table_payload_bytes() -> usize {
        MAX_KERNEL_TABLE_BYTES
    }
    pub fn control_data_layout(&self) -> ResampleKernelDataLayout {
        let KernelData {
            source_rate: _,
            target_rate: _,
            version: _,
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
