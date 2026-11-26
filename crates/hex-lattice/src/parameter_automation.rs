//! Control-prepared normalized Arrangement envelopes and borrowed runtime queries.
//! Programs own only bounded flat metadata/point vectors. Providers never retain
//! these borrows; the existing prepared/runtime owner inventories their backing.
//! Missing content-origin clocks use base values (None), never performance beats.
use crate::{NativeInstrumentClock, PreparedForeignAutomation};
use hex_lattice_contract::{
    DeviceAutomationTarget, DeviceContractError as Error, HostGain, ParameterAccess,
    ParameterAutomation, ParameterDescriptor, ParameterId, ParameterKind, ParameterSemantics,
    ParameterStepping, ParameterValue,
};

pub const MAX_AUTOMATION_LANES: usize = 256;
pub const MAX_AUTOMATION_POINTS: usize = 65_536;
pub const MAX_AUTOMATION_POINTS_PER_LANE: usize = 1_024;
const MAX_AUTOMATION_TOUCH_STEPS_PER_LANE: usize = 16_384;

/// Validated normalized scalar, independent of project storage and device units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizedAutomationValue(f32);
impl NormalizedAutomationValue {
    pub fn new(value: f32) -> Result<Self, Error> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(Error::InvalidParameterValue);
        }
        Ok(Self(value))
    }
    pub fn get(self) -> f32 {
        self.0
    }
}
/// Borrowed, fully available normalized samples. None means uncovered, not a
/// missing page. Targets must remain sorted and unique throughout the borrow.
pub trait ExternalParameterAutomation {
    fn frames(&self) -> usize;
    fn targets(&self) -> &[DeviceAutomationTarget];
    fn value_at(
        &self,
        target: DeviceAutomationTarget,
        offset: usize,
    ) -> Option<NormalizedAutomationValue>;
}

/// One checked canonical transient control. Host gains never use provider IDs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NativeTransientValue {
    Parameter {
        id: ParameterId,
        value: ParameterValue,
    },
    HostInputGain(HostGain),
    HostOutputGain(HostGain),
}
impl NativeTransientValue {
    pub fn target(self) -> DeviceAutomationTarget {
        match self {
            Self::Parameter { id, .. } => DeviceAutomationTarget::Parameter(id),
            Self::HostInputGain(_) => DeviceAutomationTarget::HostInputGain,
            Self::HostOutputGain(_) => DeviceAutomationTarget::HostOutputGain,
        }
    }
}

/// Explicit terminal restoration is distinct from having no monitored request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NativeTransientOperation {
    Set(NativeTransientValue),
    Clear(DeviceAutomationTarget),
}
impl NativeTransientOperation {
    pub fn target(self) -> DeviceAutomationTarget {
        match self {
            Self::Set(value) => value.target(),
            Self::Clear(target) => target,
        }
    }
    pub fn value(self) -> Option<NativeTransientValue> {
        match self {
            Self::Set(value) => Some(value),
            Self::Clear(_) => None,
        }
    }
}

/// Borrowed final canonical values from the host's shared composition seam.
/// Providers receive typed values and route presence while the host owns the
/// envelopes, modulator graph, and clocks. IDs stay stable during this borrow.
/// None selects immutable base.
pub trait NativeParameterAutomation {
    fn parameter_ids(&self) -> &[ParameterId];

    /// A routed normalized value before the final clamp, if available. A
    /// provider with a subsequent per-occurrence contribution can sum it
    /// without losing offsets that temporarily exceeded the target range.
    fn normalized_before_clamp(&self, _parameter: ParameterId, _offset: usize) -> Option<f64> {
        None
    }

    /// Whether an active sample-rate route targets this value in the block.
    /// A route remains present at zero offset. Authored automation alone
    /// never reports a modulation route.
    fn has_sample_rate_modulation(&self, _parameter: ParameterId) -> bool {
        false
    }

    /// Returns `true` when this provider is backed by a control-prepared
    /// mapping whose emitted values are guaranteed valid against `descriptors`
    /// across `0..frames`, allowing the host to skip redundant full-block
    /// pre-evaluation before provider processing.
    fn guarantees_valid_values(
        &self,
        _descriptors: &[ParameterDescriptor],
        _frames: usize,
    ) -> bool {
        false
    }

    /// Canonical units at this segment's sample offset. None means no authored
    /// lane or unavailable content origin; callers use their immutable base.
    fn value_at(&self, parameter: ParameterId, sample_offset: usize) -> Option<ParameterValue>;

    /// Normalized scalar value in `0.0..=1.0` at `sample_offset` when the
    /// provider can supply it directly without canonical-to-normalized
    /// round-trips (`powf`/`ln`). Returns `None` if unavailable or unsupported.
    fn normalized_value_at(&self, _parameter: ParameterId, _sample_offset: usize) -> Option<f32> {
        None
    }

    /// Fill `output` with normalized values (`Some(0.0..=1.0)` or `None` for
    /// base fallback) across `0..output.len()` in one chunk pass, amortizing
    /// lane lookup and curve search. Returns `true` when filled directly in
    /// normalized space; callers fall back to `value_at` when `false`.
    fn fill_normalized_slice(&self, _parameter: ParameterId, _output: &mut [Option<f32>]) -> bool {
        false
    }

    /// Fill `output` with canonical values for `parameter` across
    /// `0..output.len()` in one block pass, amortizing per-sample virtual
    /// dispatch and binary searches.
    fn fill_values(
        &self,
        parameter: ParameterId,
        output: &mut [Option<ParameterValue>],
    ) -> Result<(), Error> {
        for (offset, slot) in output.iter_mut().enumerate() {
            *slot = self.value_at(parameter, offset);
        }
        Ok(())
    }
    /// Query in parameter_ids order, overwriting every output slot. Exact length
    /// is required; a malformed buffer/offset is an error. Unavailable values
    /// are None; the current Arrangement block requires a content origin.
    /// Compatibility providers
    /// may retain scalar implementation; prepared production overrides this.
    fn values_at(
        &self,
        sample_offset: usize,
        output: &mut [Option<ParameterValue>],
    ) -> Result<(), Error> {
        if output.len() != self.parameter_ids().len() {
            return Err(Error::InvalidParameterValue);
        }
        for (slot, &parameter) in output.iter_mut().zip(self.parameter_ids()) {
            *slot = self.value_at(parameter, sample_offset);
        }
        Ok(())
    }
}
/// Monotonic normalized curve shaping matching `hex_project::CurveTension::shape`.
#[inline]
pub fn curve_shape(fraction: f64, tension_millis: i16) -> f64 {
    let t = fraction.clamp(0.0, 1.0);
    if tension_millis == 0 || t <= 0.0 || t >= 1.0 {
        return t;
    }
    let exponent = 2.0_f64.powf((f64::from(tension_millis) / 1_000.0) * 3.0);
    t.powf(exponent).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterAutomationPoint {
    pub beat_raw: u64,
    pub normalized: f32,
    pub step: bool,
    pub curve_tension_millis: i16,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ParameterAutomationLane {
    pub descriptor: ParameterDescriptor,
    pub semantics: ParameterSemantics,
    pub points: std::ops::Range<usize>,
}
/// Host gain staging is independent of the provider parameter ID space.
#[derive(Clone, Debug, PartialEq)]
pub struct HostAutomationLane {
    pub target: DeviceAutomationTarget,
    pub points: std::ops::Range<usize>,
}
/// Exact half-open step range over one native parameter or host trim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterAutomationTouchSpan {
    pub target: DeviceAutomationTarget,
    pub start_beat: u64,
    pub end_beat: u64,
    pub points: std::ops::Range<usize>,
}

/// Opaque, validated runtime mapping. Descriptor text is retained only for
/// control admission; sample queries use this compiled scalar law exclusively.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedParameterAutomationLane {
    descriptor: Option<ParameterDescriptor>,
    mapping: Mapping,
    source: PreparedLaneSource,
    touch: std::ops::Range<usize>,
}
/// BaseFallback is runtime delivery metadata, not an authored empty curve.
/// It keeps a cleared transient target in the provider's stable query inventory.
#[derive(Clone, Debug, PartialEq)]
enum PreparedLaneSource {
    Curve {
        points: std::ops::Range<usize>,
        constant: Option<CanonicalValue>,
    },
    BaseFallback,
}
/// Private validated point payload; canonical held values are prepared once.
/// The public opaque type permits physical backing observations by the existing
/// storage ledger without exposing a second editable envelope representation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedParameterAutomationPoint {
    point: ParameterAutomationPoint,
    canonical: CanonicalValue,
}
/// Private cache units stay explicit. Host amplitude can never leave the
/// provider query seam as a forged ParameterValue or ParameterId. Its value is
/// prepared by the canonical integer-millidB HostGain law exactly once for each
/// authored held/constant point; interpolated host values use that same law.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CanonicalValue {
    Provider(ParameterValue),
    HostGain {
        normalized: NormalizedAutomationValue,
        linear: f32,
    },
}
impl CanonicalValue {
    pub(crate) fn provider(self) -> Option<ParameterValue> {
        match self {
            Self::Provider(value) => Some(value),
            Self::HostGain { .. } => None,
        }
    }
    fn host_gain(self) -> Option<(NormalizedAutomationValue, f32)> {
        match self {
            Self::HostGain { normalized, linear } => Some((normalized, linear)),
            Self::Provider(_) => None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Mapping {
    HostGain,
    Float {
        min: f64,
        max: f64,
        ratio: Option<f64>,
        increment: Option<f64>,
    },
    Bool,
    Choice {
        last: u32,
    },
}
impl Mapping {
    pub(crate) fn prepare(
        descriptor: ParameterDescriptor,
        semantics: ParameterSemantics,
    ) -> Result<Self, Error> {
        use hex_lattice_contract::ParameterNormalization as N;
        match (descriptor.kind, semantics.normalization) {
            (
                ParameterKind::Float { min, max, .. },
                normalization @ (N::Linear | N::PositiveLogarithmic),
            ) => Ok(Self::Float {
                min: f64::from(min),
                max: f64::from(max),
                ratio: if normalization == N::PositiveLogarithmic {
                    Some(f64::from(max) / f64::from(min))
                } else {
                    None
                },
                increment: if let ParameterStepping::Increment(step) = semantics.stepping {
                    Some(f64::from(step))
                } else {
                    None
                },
            }),
            (ParameterKind::Bool, N::Discrete) => Ok(Self::Bool),
            (ParameterKind::Choice { labels }, N::Discrete) => Ok(Self::Choice {
                last: u32::try_from(
                    labels
                        .len()
                        .checked_sub(1)
                        .ok_or(Error::InvalidDescriptor)?,
                )
                .map_err(|_| Error::InvalidDescriptor)?,
            }),
            _ => Err(Error::InvalidDescriptor),
        }
    }
    fn discrete(self) -> bool {
        matches!(self, Self::Bool | Self::Choice { .. })
    }
    pub(crate) fn matches_descriptor(self, descriptor: ParameterDescriptor) -> bool {
        match (self, descriptor.kind) {
            (
                Self::Float { min, max, .. },
                ParameterKind::Float {
                    min: dmin,
                    max: dmax,
                    ..
                },
            ) => min >= f64::from(dmin) && max <= f64::from(dmax),
            (Self::Bool, ParameterKind::Bool) => true,
            (Self::Choice { last }, ParameterKind::Choice { labels }) => {
                (last as usize) < labels.len()
            }
            _ => false,
        }
    }
    pub(crate) fn value(self, normalized: f32) -> Result<CanonicalValue, Error> {
        Ok(match self {
            Self::HostGain => CanonicalValue::HostGain {
                normalized: NormalizedAutomationValue(normalized),
                linear: HostGain::from_normalized(normalized)?.linear(),
            },
            Self::Bool => CanonicalValue::Provider(ParameterValue::Bool(normalized >= 0.5)),
            Self::Choice { last } => CanonicalValue::Provider(ParameterValue::Choice(
                (normalized * last as f32).round() as u32,
            )),
            Self::Float {
                min,
                max,
                ratio,
                increment,
            } => {
                let value = if normalized == 0.0 {
                    min
                } else if normalized == 1.0 {
                    max
                } else if let Some(ratio) = ratio {
                    min * ratio.powf(f64::from(normalized))
                } else {
                    min * (1.0 - f64::from(normalized)) + max * f64::from(normalized)
                };
                let value = increment.map_or(value, |step| {
                    (min + ((value - min) / step).round() * step).clamp(min, max)
                });
                CanonicalValue::Provider(ParameterValue::Float(value as f32))
            }
        })
    }
}
fn compile_points(
    mapping: Mapping,
    points: &[ParameterAutomationPoint],
    compiled: &mut Vec<PreparedParameterAutomationPoint>,
    allow_coincident: bool,
) -> Result<Option<CanonicalValue>, Error> {
    let Some(first) = points.first() else {
        return Ok(None);
    };
    if points.iter().any(|point| {
        !point.normalized.is_finite()
            || !(0.0..=1.0).contains(&point.normalized)
            || !(-1_000..=1_000).contains(&point.curve_tension_millis)
    }) || points.windows(2).any(|pair| {
        pair[0].beat_raw > pair[1].beat_raw
            || (!allow_coincident && pair[0].beat_raw == pair[1].beat_raw)
    }) || points
        .windows(3)
        .any(|run| run[0].beat_raw == run[2].beat_raw)
    {
        return Err(Error::InvalidParameterValue);
    }
    let canonical = mapping.value(first.normalized)?;
    let constant = points
        .iter()
        .all(|point| point.normalized == first.normalized)
        .then_some(canonical);
    for &point in points {
        compiled.push(PreparedParameterAutomationPoint {
            point,
            canonical: mapping.value(point.normalized)?,
        });
    }
    Ok(constant)
}

mod block;
mod prepared;
pub use block::ParameterAutomationBlock;
pub use prepared::PreparedParameterAutomation;

#[cfg(test)]
mod tests;
