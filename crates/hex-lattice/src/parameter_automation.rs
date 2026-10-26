//! Control-prepared normalized Arrangement envelopes and borrowed runtime queries.
//! Programs own only bounded flat metadata/point vectors. Providers never retain
//! these borrows; the existing prepared/runtime owner inventories their backing.
//! Missing content-origin clocks use base values (None), never performance beats.
use crate::NativeInstrumentClock;
use hex_lattice_contract::{
    DeviceAutomationTarget, DeviceContractError as Error, HostGain, ParameterAccess,
    ParameterAutomation, ParameterDescriptor, ParameterId, ParameterKind, ParameterSemantics,
    ParameterStepping, ParameterValue,
};

pub const MAX_AUTOMATION_LANES: usize = 256;
pub const MAX_AUTOMATION_POINTS: usize = 65_536;
pub const MAX_AUTOMATION_POINTS_PER_LANE: usize = 1_024;

pub trait NativeParameterAutomation {
    fn parameter_ids(&self) -> &[ParameterId];
    /// Canonical units at this segment's sample offset. None means no authored
    /// lane or unavailable content origin; callers use their immutable base.
    fn value_at(&self, parameter: ParameterId, sample_offset: usize) -> Option<ParameterValue>;
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterAutomationPoint {
    pub beat_raw: u64,
    pub normalized: f32,
    pub step: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ParameterAutomationLane {
    pub descriptor: ParameterDescriptor,
    pub semantics: ParameterSemantics,
    pub points: std::ops::Range<usize>,
}
/// Host gains use the same normalized point backing, with a fixed host-owned
/// -60..24 dB, one-millidB canonical stepping law. No provider ID is forged.
#[derive(Clone, Debug, PartialEq)]
pub struct HostAutomationLane {
    pub target: DeviceAutomationTarget,
    pub points: std::ops::Range<usize>,
}
/// Opaque, validated runtime mapping. Descriptor text is retained only for
/// control admission; sample queries use this compiled scalar law exclusively.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedParameterAutomationLane {
    descriptor: Option<ParameterDescriptor>,
    mapping: Mapping,
    points: std::ops::Range<usize>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mapping {
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
    fn prepare(
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
    // Provider queries never expose a host target as a ParameterValue. Host
    // queries below decode the same borrowed normalized value through HostGain.
    fn value(self, normalized: f32) -> Result<ParameterValue, Error> {
        Ok(match self {
            Self::HostGain => return Err(Error::InvalidDescriptor),
            Self::Bool => ParameterValue::Bool(normalized >= 0.5),
            Self::Choice { last } => {
                ParameterValue::Choice((normalized * last as f32).round() as u32)
            }
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
                ParameterValue::Float(value as f32)
            }
        })
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedParameterAutomation {
    ids: Vec<ParameterId>,
    lanes: Vec<PreparedParameterAutomationLane>,
    points: Vec<ParameterAutomationPoint>,
    host_input: Option<usize>,
    host_output: Option<usize>,
}
impl PreparedParameterAutomation {
    pub fn new(
        lanes: Vec<ParameterAutomationLane>,
        points: Vec<ParameterAutomationPoint>,
    ) -> Result<Self, Error> {
        Self::new_with_host(lanes, Vec::new(), points)
    }
    pub fn new_with_host(
        lanes: Vec<ParameterAutomationLane>,
        host: Vec<HostAutomationLane>,
        points: Vec<ParameterAutomationPoint>,
    ) -> Result<Self, Error> {
        let count = lanes
            .len()
            .checked_add(host.len())
            .ok_or(Error::RuntimeBudgetExceeded)?;
        if count > MAX_AUTOMATION_LANES || points.len() > MAX_AUTOMATION_POINTS {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let mut ids = Vec::new();
        ids.try_reserve_exact(lanes.len())
            .map_err(|_| Error::RuntimeAllocation)?;
        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(count)
            .map_err(|_| Error::RuntimeAllocation)?;
        let mut end = 0;
        for lane in &lanes {
            lane.semantics.validate(lane.descriptor)?;
            if lane.semantics.access != ParameterAccess::ReadWrite
                || lane.semantics.automation != ParameterAutomation::DeviceArrangementEnvelope
                || matches!(
                    lane.semantics.normalization,
                    hex_lattice_contract::ParameterNormalization::Unspecified
                )
                || matches!(lane.semantics.stepping, ParameterStepping::Unspecified)
                || ids.last().is_some_and(|id| *id >= lane.descriptor.id)
                || lane.points.start != end
                || lane.points.start >= lane.points.end
                || lane.points.len() > MAX_AUTOMATION_POINTS_PER_LANE
            {
                return Err(Error::InvalidDescriptor);
            }
            let values = points
                .get(lane.points.clone())
                .ok_or(Error::InvalidDescriptor)?;
            if values.iter().any(|point| {
                !point.normalized.is_finite() || !(0.0..=1.0).contains(&point.normalized)
            }) || values
                .windows(2)
                .any(|pair| pair[0].beat_raw >= pair[1].beat_raw)
            {
                return Err(Error::InvalidParameterValue);
            }
            ids.push(lane.descriptor.id);
            prepared.push(PreparedParameterAutomationLane {
                descriptor: Some(lane.descriptor),
                mapping: Mapping::prepare(lane.descriptor, lane.semantics)?,
                points: lane.points.clone(),
            });
            end = lane.points.end;
        }
        let mut host_input = None;
        let mut host_output = None;
        let mut previous = None;
        for lane in host {
            if !lane.target.is_host()
                || previous.is_some_and(|target| target >= lane.target)
                || lane.points.start != end
                || lane.points.is_empty()
                || lane.points.len() > MAX_AUTOMATION_POINTS_PER_LANE
            {
                return Err(Error::InvalidDescriptor);
            }
            let values = points
                .get(lane.points.clone())
                .ok_or(Error::InvalidDescriptor)?;
            if values.iter().any(|point| {
                !point.normalized.is_finite() || !(0.0..=1.0).contains(&point.normalized)
            }) || values
                .windows(2)
                .any(|pair| pair[0].beat_raw >= pair[1].beat_raw)
            {
                return Err(Error::InvalidParameterValue);
            }
            previous = Some(lane.target);
            match lane.target {
                DeviceAutomationTarget::HostInputGain => host_input = Some(prepared.len()),
                DeviceAutomationTarget::HostOutputGain => host_output = Some(prepared.len()),
                DeviceAutomationTarget::Parameter(_) => return Err(Error::InvalidDescriptor),
            }
            end = lane.points.end;
            prepared.push(PreparedParameterAutomationLane {
                descriptor: None,
                mapping: Mapping::HostGain,
                points: lane.points,
            });
        }
        if end != points.len() {
            return Err(Error::InvalidDescriptor);
        }
        Ok(Self {
            ids,
            lanes: prepared,
            points,
            host_input,
            host_output,
        })
    }
    pub fn is_empty(&self) -> bool {
        self.lanes.is_empty()
    }
    pub fn has_host_target(&self, target: DeviceAutomationTarget) -> bool {
        match target {
            DeviceAutomationTarget::HostInputGain => self.host_input.is_some(),
            DeviceAutomationTarget::HostOutputGain => self.host_output.is_some(),
            DeviceAutomationTarget::Parameter(_) => false,
        }
    }
    pub fn has_host_targets(&self) -> bool {
        self.host_input.is_some() || self.host_output.is_some()
    }
    pub fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }
    pub fn descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        let index = self.ids.binary_search(&id).ok()?;
        self.lanes.get(index).and_then(|lane| lane.descriptor)
    }
    /// Actual owned flat-vector observations for the existing control ledger.
    pub fn storage_vectors(
        &self,
    ) -> (
        &Vec<ParameterId>,
        &Vec<PreparedParameterAutomationLane>,
        &Vec<ParameterAutomationPoint>,
    ) {
        (&self.ids, &self.lanes, &self.points)
    }
    pub fn storage_bytes(&self) -> Option<usize> {
        self.ids
            .capacity()
            .checked_mul(std::mem::size_of::<ParameterId>())?
            .checked_add(
                self.lanes
                    .capacity()
                    .checked_mul(std::mem::size_of::<PreparedParameterAutomationLane>())?,
            )?
            .checked_add(
                self.points
                    .capacity()
                    .checked_mul(std::mem::size_of::<ParameterAutomationPoint>())?,
            )
    }
    pub fn quoted_storage_bytes(lanes: usize, points: usize) -> Option<usize> {
        Self::quoted_storage_bytes_with_host(lanes, 0, points)
    }
    pub fn quoted_storage_bytes_with_host(
        provider: usize,
        host: usize,
        points: usize,
    ) -> Option<usize> {
        provider
            .checked_mul(std::mem::size_of::<ParameterId>())?
            .checked_add(
                provider
                    .checked_add(host)?
                    .checked_mul(std::mem::size_of::<PreparedParameterAutomationLane>())?,
            )?
            .checked_add(points.checked_mul(std::mem::size_of::<ParameterAutomationPoint>())?)
    }
    /// Staging lane metadata coexists with retained compiled backing only while
    /// this control-side constructor validates and converts its input vector.
    pub fn quoted_construction_overlap_bytes(lanes: usize) -> Option<usize> {
        lanes.checked_mul(std::mem::size_of::<ParameterAutomationLane>())
    }
    pub fn block<'a>(
        &'a self,
        clock: Option<&'a dyn NativeInstrumentClock>,
        frames: usize,
    ) -> ParameterAutomationBlock<'a> {
        ParameterAutomationBlock {
            program: self,
            clock,
            frames,
        }
    }
}
pub struct ParameterAutomationBlock<'a> {
    program: &'a PreparedParameterAutomation,
    clock: Option<&'a dyn NativeInstrumentClock>,
    frames: usize,
}
impl ParameterAutomationBlock<'_> {
    /// Effective host trim in linear amplitude units. Missing clock falls back
    /// to immutable base at the chain owner; no provider receives host targets.
    /// Authored normalized values have already passed new_with_host validation;
    /// this uses the same integer millidB decode and linear law as canonical state.
    pub fn host_gain_at(&self, target: DeviceAutomationTarget, offset: usize) -> Option<f32> {
        let index = match target {
            DeviceAutomationTarget::HostInputGain => self.program.host_input?,
            DeviceAutomationTarget::HostOutputGain => self.program.host_output?,
            DeviceAutomationTarget::Parameter(_) => return None,
        };
        HostGain::from_normalized(self.normalized_at_index(index, offset)?)
            .ok()
            .map(HostGain::linear)
    }
    fn value_at_index(&self, index: usize, offset: usize) -> Option<ParameterValue> {
        self.program
            .lanes
            .get(index)?
            .mapping
            .value(self.normalized_at_index(index, offset)?)
            .ok()
    }
    fn normalized_at_index(&self, index: usize, offset: usize) -> Option<f32> {
        if offset >= self.frames {
            return None;
        }
        let beat = self.clock?.arrangement_beat_at_offset(offset)?;
        let lane = self.program.lanes.get(index)?;
        let points = self.program.points.get(lane.points.clone())?;
        let next = points.partition_point(|point| point.beat_raw <= beat);
        let normalized = if next == 0 {
            points.first()?.normalized
        } else {
            let left = points.get(next - 1)?;
            match points.get(next) {
                Some(right) if !left.step && !lane.mapping.discrete() && beat != left.beat_raw => {
                    let span = right.beat_raw.checked_sub(left.beat_raw)?;
                    let elapsed = beat.checked_sub(left.beat_raw)?;
                    (f64::from(left.normalized)
                        + (f64::from(right.normalized) - f64::from(left.normalized))
                            * elapsed as f64
                            / span as f64) as f32
                }
                _ => left.normalized,
            }
        };
        Some(normalized)
    }
}
impl NativeParameterAutomation for ParameterAutomationBlock<'_> {
    fn parameter_ids(&self) -> &[ParameterId] {
        self.program.parameter_ids()
    }
    fn value_at(&self, parameter: ParameterId, offset: usize) -> Option<ParameterValue> {
        let index = self.program.ids.binary_search(&parameter).ok()?;
        self.value_at_index(index, offset)
    }
}

#[cfg(test)]
mod tests;
