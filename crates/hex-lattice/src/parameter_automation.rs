//! Control-prepared normalized Arrangement envelopes and borrowed runtime queries.
//! Programs own only bounded flat metadata/point vectors. Providers never retain
//! these borrows; the existing prepared/runtime owner inventories their backing.
//! Missing content-origin clocks use base values (None), never performance beats.
use crate::NativeInstrumentClock;
use hex_lattice_contract::{
    DeviceContractError as Error, ParameterAccess, ParameterAutomation, ParameterDescriptor,
    ParameterId, ParameterKind, ParameterSemantics, ParameterStepping, ParameterValue,
};

pub const MAX_AUTOMATION_LANES: usize = 256;
pub const MAX_AUTOMATION_POINTS: usize = 65_536;
pub const MAX_AUTOMATION_POINTS_PER_LANE: usize = 1_024;

/// Borrowed final canonical values, independent of their producing control
/// source. The historical automation name remains the shared delivery seam for
/// future host composition; providers never inspect envelopes or modulators.
/// IDs remain stable throughout this borrow. None selects immutable base.
pub trait NativeParameterAutomation {
    fn parameter_ids(&self) -> &[ParameterId];
    /// Canonical units at this segment's sample offset. None means no authored
    /// lane or unavailable content origin; callers use their immutable base.
    fn value_at(&self, parameter: ParameterId, sample_offset: usize) -> Option<ParameterValue>;
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
/// Opaque, validated runtime mapping. Descriptor text is retained only for
/// control admission; sample queries use this compiled scalar law exclusively.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedParameterAutomationLane {
    descriptor: ParameterDescriptor,
    mapping: Mapping,
    points: std::ops::Range<usize>,
    constant: Option<ParameterValue>,
}
/// Private validated point payload; canonical held values are prepared once.
/// The public opaque type permits physical backing observations by the existing
/// storage ledger without exposing a second editable envelope representation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedParameterAutomationPoint {
    point: ParameterAutomationPoint,
    canonical: ParameterValue,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mapping {
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
    fn value(self, normalized: f32) -> ParameterValue {
        match self {
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
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedParameterAutomation {
    ids: Vec<ParameterId>,
    lanes: Vec<PreparedParameterAutomationLane>,
    points: Vec<PreparedParameterAutomationPoint>,
}
impl PreparedParameterAutomation {
    pub fn new(
        lanes: Vec<ParameterAutomationLane>,
        points: Vec<ParameterAutomationPoint>,
    ) -> Result<Self, Error> {
        if lanes.len() > MAX_AUTOMATION_LANES || points.len() > MAX_AUTOMATION_POINTS {
            return Err(Error::RuntimeBudgetExceeded);
        }
        let mut ids = Vec::new();
        ids.try_reserve_exact(lanes.len())
            .map_err(|_| Error::RuntimeAllocation)?;
        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(lanes.len())
            .map_err(|_| Error::RuntimeAllocation)?;
        let mut compiled_points = Vec::new();
        compiled_points
            .try_reserve_exact(points.len())
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
            let mapping = Mapping::prepare(lane.descriptor, lane.semantics)?;
            let first = values.first().ok_or(Error::InvalidDescriptor)?;
            let constant = values
                .iter()
                .all(|point| point.normalized == first.normalized)
                .then(|| mapping.value(first.normalized));
            compiled_points.extend(
                values
                    .iter()
                    .map(|&point| PreparedParameterAutomationPoint {
                        point,
                        canonical: mapping.value(point.normalized),
                    }),
            );
            prepared.push(PreparedParameterAutomationLane {
                descriptor: lane.descriptor,
                mapping,
                points: lane.points.clone(),
                constant,
            });
            end = lane.points.end;
        }
        if end != points.len() {
            return Err(Error::InvalidDescriptor);
        }
        Ok(Self {
            ids,
            lanes: prepared,
            points: compiled_points,
        })
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn parameter_ids(&self) -> &[ParameterId] {
        &self.ids
    }
    pub fn descriptor(&self, id: ParameterId) -> Option<ParameterDescriptor> {
        let index = self.ids.binary_search(&id).ok()?;
        self.lanes.get(index).map(|lane| lane.descriptor)
    }
    /// Actual owned flat-vector observations for the existing control ledger.
    pub fn storage_vectors(
        &self,
    ) -> (
        &Vec<ParameterId>,
        &Vec<PreparedParameterAutomationLane>,
        &Vec<PreparedParameterAutomationPoint>,
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
                    .checked_mul(std::mem::size_of::<PreparedParameterAutomationPoint>())?,
            )
    }
    pub fn quoted_storage_bytes(lanes: usize, points: usize) -> Option<usize> {
        lanes
            .checked_mul(
                std::mem::size_of::<ParameterId>()
                    .checked_add(std::mem::size_of::<PreparedParameterAutomationLane>())?,
            )?
            .checked_add(
                points.checked_mul(std::mem::size_of::<PreparedParameterAutomationPoint>())?,
            )
    }
    /// Staging lane metadata coexists with retained compiled backing only while
    /// this control-side constructor validates and converts its input vector.
    pub fn quoted_construction_overlap_bytes(lanes: usize, points: usize) -> Option<usize> {
        lanes
            .checked_mul(std::mem::size_of::<ParameterAutomationLane>())?
            .checked_add(points.checked_mul(std::mem::size_of::<ParameterAutomationPoint>())?)
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
    fn value_at_beat(&self, index: usize, beat: u64) -> Option<ParameterValue> {
        let lane = self.program.lanes.get(index)?;
        if let Some(value) = lane.constant {
            return Some(value);
        }
        let points = self.program.points.get(lane.points.clone())?;
        let next = points.partition_point(|point| point.point.beat_raw <= beat);
        if next == 0 {
            return Some(points.first()?.canonical);
        }
        let left = points.get(next - 1)?;
        match points.get(next) {
            Some(right)
                if !left.point.step && !lane.mapping.discrete() && beat != left.point.beat_raw =>
            {
                let span = right.point.beat_raw.checked_sub(left.point.beat_raw)?;
                let elapsed = beat.checked_sub(left.point.beat_raw)?;
                let normalized = (f64::from(left.point.normalized)
                    + (f64::from(right.point.normalized) - f64::from(left.point.normalized))
                        * elapsed as f64
                        / span as f64) as f32;
                Some(lane.mapping.value(normalized))
            }
            _ => Some(left.canonical),
        }
    }
}
impl NativeParameterAutomation for ParameterAutomationBlock<'_> {
    fn parameter_ids(&self) -> &[ParameterId] {
        self.program.parameter_ids()
    }
    fn value_at(&self, parameter: ParameterId, offset: usize) -> Option<ParameterValue> {
        if offset >= self.frames {
            return None;
        }
        let index = self.program.ids.binary_search(&parameter).ok()?;
        self.value_at_beat(index, self.clock?.arrangement_beat_at_offset(offset)?)
    }
    fn values_at(&self, offset: usize, output: &mut [Option<ParameterValue>]) -> Result<(), Error> {
        if output.len() != self.program.ids.len() || offset >= self.frames {
            return Err(Error::InvalidParameterValue);
        }
        let beat = self
            .clock
            .and_then(|clock| clock.arrangement_beat_at_offset(offset));
        for (index, slot) in output.iter_mut().enumerate() {
            *slot = beat.and_then(|beat| self.value_at_beat(index, beat));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
