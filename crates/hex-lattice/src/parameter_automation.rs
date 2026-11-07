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
enum CanonicalValue {
    Provider(ParameterValue),
    HostGainLinear(f32),
}
impl CanonicalValue {
    fn provider(self) -> Option<ParameterValue> {
        match self {
            Self::Provider(value) => Some(value),
            Self::HostGainLinear(_) => None,
        }
    }
    fn host_gain(self) -> Option<f32> {
        match self {
            Self::HostGainLinear(value) => Some(value),
            Self::Provider(_) => None,
        }
    }
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
    fn value(self, normalized: f32) -> Result<CanonicalValue, Error> {
        Ok(match self {
            Self::HostGain => {
                CanonicalValue::HostGainLinear(HostGain::from_normalized(normalized)?.linear())
            }
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
) -> Result<Option<CanonicalValue>, Error> {
    let Some(first) = points.first() else {
        return Ok(None);
    };
    if points
        .iter()
        .any(|point| !point.normalized.is_finite() || !(0.0..=1.0).contains(&point.normalized))
        || points
            .windows(2)
            .any(|pair| pair[0].beat_raw >= pair[1].beat_raw)
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedParameterAutomation {
    ids: Vec<ParameterId>,
    lanes: Vec<PreparedParameterAutomationLane>,
    points: Vec<PreparedParameterAutomationPoint>,
    host_input: Option<usize>,
    host_output: Option<usize>,
    input_target: Option<DeviceAutomationTarget>,
    foreign: Option<PreparedForeignAutomation>,
    touch: Vec<ParameterAutomationTouchSpan>,
}
impl PreparedParameterAutomation {
    /// A foreign processor consumes the SDK queue rather than the native
    /// canonical parameter or host trim queries.
    pub fn for_foreign(program: PreparedForeignAutomation) -> Self {
        Self {
            foreign: Some(program),
            ..Self::default()
        }
    }

    pub fn foreign(&self) -> Option<&PreparedForeignAutomation> {
        self.foreign.as_ref()
    }

    /// Carry the exact predecessor's immutable foreign quantum programs while
    /// preparing a new graph on control. Native/host envelopes stay unchanged.
    /// The host must obtain the optional frontier from this original worker.
    pub fn retain_foreign_history_on_control(
        &mut self,
        previous: &Self,
        received: Option<std::num::NonZeroU64>,
    ) -> Result<(), Error> {
        let current = self
            .foreign
            .as_mut()
            .ok_or(Error::ForeignProcessingFailed)?;
        let previous = previous
            .foreign
            .as_ref()
            .ok_or(Error::ForeignProcessingFailed)?;
        if current.shares_storage_with(previous) {
            return Ok(());
        }
        if current.borrow().revision <= previous.borrow().revision {
            return Err(Error::ForeignProcessingFailed);
        }
        match received {
            Some(received) => current.retain_received_on_control(previous, received),
            None => current.retain_previous_on_control(previous),
        }
    }

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
        Self::new_with_host_and_touch(lanes, host, points, Vec::new())
    }

    pub fn new_with_host_and_touch(
        lanes: Vec<ParameterAutomationLane>,
        host: Vec<HostAutomationLane>,
        points: Vec<ParameterAutomationPoint>,
        touch: Vec<ParameterAutomationTouchSpan>,
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
                || lane.points.start > lane.points.end
                || lane.points.len() > MAX_AUTOMATION_POINTS_PER_LANE
            {
                return Err(Error::InvalidDescriptor);
            }
            let values = points
                .get(lane.points.clone())
                .ok_or(Error::InvalidDescriptor)?;
            ids.push(lane.descriptor.id);
            let mapping = Mapping::prepare(lane.descriptor, lane.semantics)?;
            let constant = compile_points(mapping, values, &mut compiled_points)?;
            prepared.push(PreparedParameterAutomationLane {
                descriptor: Some(lane.descriptor),
                mapping,
                touch: 0..0,
                source: PreparedLaneSource::Curve {
                    points: lane.points.clone(),
                    constant,
                },
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
                || lane.points.start > lane.points.end
                || lane.points.len() > MAX_AUTOMATION_POINTS_PER_LANE
            {
                return Err(Error::InvalidDescriptor);
            }
            let values = points
                .get(lane.points.clone())
                .ok_or(Error::InvalidDescriptor)?;
            let constant = compile_points(Mapping::HostGain, values, &mut compiled_points)?;
            previous = Some(lane.target);
            match lane.target {
                DeviceAutomationTarget::HostInputGain => host_input = Some(prepared.len()),
                DeviceAutomationTarget::HostOutputGain => host_output = Some(prepared.len()),
                DeviceAutomationTarget::Parameter(_) | DeviceAutomationTarget::Vst3 { .. } => {
                    return Err(Error::InvalidDescriptor);
                }
            }
            end = lane.points.end;
            prepared.push(PreparedParameterAutomationLane {
                descriptor: None,
                mapping: Mapping::HostGain,
                touch: 0..0,
                source: PreparedLaneSource::Curve {
                    points: lane.points,
                    constant,
                },
            });
        }
        let mut result = Self {
            ids,
            lanes: prepared,
            points: compiled_points,
            host_input,
            host_output,
            input_target: None,
            foreign: None,
            touch,
        };
        result.compile_touch(&points, end)?;
        Ok(result)
    }

    fn compile_touch(
        &mut self,
        points: &[ParameterAutomationPoint],
        mut end: usize,
    ) -> Result<(), Error> {
        let mut previous: Option<&ParameterAutomationTouchSpan> = None;
        let mut lane_count = 0;
        for (index, span) in self.touch.iter().enumerate() {
            if span.start_beat >= span.end_beat || span.points.start != end {
                return Err(Error::InvalidDescriptor);
            }
            let lane_index = match span.target {
                DeviceAutomationTarget::Parameter(id) => self.ids.binary_search(&id).ok(),
                DeviceAutomationTarget::HostInputGain => self.host_input,
                DeviceAutomationTarget::HostOutputGain => self.host_output,
                DeviceAutomationTarget::Vst3 { .. } => None,
            };
            let lane = self
                .lanes
                .get_mut(lane_index.ok_or(Error::InvalidDescriptor)?)
                .ok_or(Error::InvalidDescriptor)?;
            let PreparedLaneSource::Curve { points: base, .. } = &lane.source else {
                return Err(Error::InvalidDescriptor);
            };
            if previous.is_some_and(|prior| {
                prior.target > span.target
                    || (prior.target == span.target && prior.end_beat > span.start_beat)
            }) {
                return Err(Error::InvalidDescriptor);
            }
            if previous.is_none_or(|prior| prior.target != span.target) {
                lane.touch = index..index;
                lane_count = base.len();
            }
            lane_count = lane_count
                .checked_add(span.points.len())
                .ok_or(Error::RuntimeBudgetExceeded)?;
            if lane_count > MAX_AUTOMATION_POINTS_PER_LANE || lane.touch.len() == 128 {
                return Err(Error::RuntimeBudgetExceeded);
            }
            let values = points
                .get(span.points.clone())
                .ok_or(Error::InvalidDescriptor)?;
            if values
                .first()
                .is_none_or(|first| first.beat_raw != span.start_beat)
                || values
                    .last()
                    .is_some_and(|last| last.beat_raw >= span.end_beat)
                || values.iter().any(|point| !point.step)
            {
                return Err(Error::InvalidDescriptor);
            }
            compile_points(lane.mapping, values, &mut self.points)?;
            lane.touch.end += 1;
            previous = Some(span);
            end = span.points.end;
        }
        if end != points.len() || self.lanes.iter().any(|lane| {
            matches!(&lane.source, PreparedLaneSource::Curve { points, .. } if points.is_empty())
                && lane.touch.is_empty()
        }) {
            return Err(Error::InvalidDescriptor);
        }
        Ok(())
    }
    /// Exactly one control-prepared transient target; authored constructors do
    /// not arm anything. This metadata is never a saved-project envelope.
    pub fn input_target(&self) -> Option<DeviceAutomationTarget> {
        self.input_target
    }
    /// Conservative temporary backing charge before arm_provider_input grows a
    /// missing provider slot. The final observed storage charge covers the new
    /// vectors; this adds the previous ID/metadata vectors during reallocation.
    /// Point backing is retained in place and is never duplicated by arming.
    pub fn provider_input_growth_overlap_bytes(&self, id: ParameterId) -> Option<usize> {
        if self.ids.binary_search(&id).is_ok() {
            return Some(0);
        }
        self.ids
            .capacity()
            .checked_mul(std::mem::size_of::<ParameterId>())?
            .checked_add(
                self.lanes
                    .capacity()
                    .checked_mul(std::mem::size_of::<PreparedParameterAutomationLane>())?,
            )
    }
    /// Prepare the sorted delivery union without adding authored points. The
    /// caller must admit the resulting IDs on the actual factory/runtime twins.
    /// New backing is visible through storage_vectors/storage_bytes; quotes use
    /// the resulting provider-slot count, including this base-only slot.
    pub fn arm_provider_input(
        &mut self,
        descriptor: ParameterDescriptor,
        semantics: ParameterSemantics,
    ) -> Result<(), Error> {
        let target = DeviceAutomationTarget::Parameter(descriptor.id);
        self.validate_input_target(target)?;
        self.prepare_external_provider(descriptor, semantics)?;
        self.input_target = Some(target);
        Ok(())
    }
    /// Add a shared provider reading slot without arming a live gesture.
    pub fn prepare_external_provider(
        &mut self,
        descriptor: ParameterDescriptor,
        semantics: ParameterSemantics,
    ) -> Result<(), Error> {
        if self.foreign.is_some() {
            return Err(Error::UnsupportedProcessingClass);
        }
        semantics.validate(descriptor)?;
        if semantics.access != ParameterAccess::ReadWrite
            || semantics.automation != ParameterAutomation::DeviceArrangementEnvelope
            || matches!(
                semantics.normalization,
                hex_lattice_contract::ParameterNormalization::Unspecified
            )
            || matches!(semantics.stepping, ParameterStepping::Unspecified)
        {
            return Err(Error::InvalidDescriptor);
        }
        let mapping = Mapping::prepare(descriptor, semantics)?;
        match self.ids.binary_search(&descriptor.id) {
            Ok(index) => {
                if self.lanes.get(index).is_none_or(|lane| {
                    lane.descriptor != Some(descriptor) || lane.mapping != mapping
                }) {
                    return Err(Error::InvalidDescriptor);
                }
            }
            Err(index) => {
                if self.lanes.len() >= MAX_AUTOMATION_LANES {
                    return Err(Error::RuntimeBudgetExceeded);
                }
                self.ids
                    .try_reserve_exact(1)
                    .map_err(|_| Error::RuntimeAllocation)?;
                self.lanes
                    .try_reserve_exact(1)
                    .map_err(|_| Error::RuntimeAllocation)?;
                self.ids.insert(index, descriptor.id);
                self.lanes.insert(
                    index,
                    PreparedParameterAutomationLane {
                        descriptor: Some(descriptor),
                        mapping,
                        touch: 0..0,
                        source: PreparedLaneSource::BaseFallback,
                    },
                );
                if let Some(slot) = &mut self.host_input {
                    *slot += 1;
                }
                if let Some(slot) = &mut self.host_output {
                    *slot += 1;
                }
            }
        }
        Ok(())
    }
    /// Host reading slots use the same normalization as authored host curves.
    pub fn prepare_external_host(&mut self, target: DeviceAutomationTarget) -> Result<(), Error> {
        let slot = match target {
            DeviceAutomationTarget::HostInputGain => self.host_input,
            DeviceAutomationTarget::HostOutputGain => self.host_output,
            DeviceAutomationTarget::Parameter(_) | DeviceAutomationTarget::Vst3 { .. } => {
                return Err(Error::InvalidDescriptor);
            }
        };
        if slot.is_some() {
            return Ok(());
        }
        if self.lanes.len() >= MAX_AUTOMATION_LANES {
            return Err(Error::RuntimeBudgetExceeded);
        }
        self.lanes
            .try_reserve_exact(1)
            .map_err(|_| Error::RuntimeAllocation)?;
        let index = self.lanes.len();
        self.lanes.push(PreparedParameterAutomationLane {
            descriptor: None,
            mapping: Mapping::HostGain,
            touch: 0..0,
            source: PreparedLaneSource::BaseFallback,
        });
        match target {
            DeviceAutomationTarget::HostInputGain => self.host_input = Some(index),
            DeviceAutomationTarget::HostOutputGain => self.host_output = Some(index),
            DeviceAutomationTarget::Parameter(_) | DeviceAutomationTarget::Vst3 { .. } => {
                return Err(Error::InvalidDescriptor);
            }
        }
        Ok(())
    }
    /// Arm host staging without inventing a provider ID or requiring a provider
    /// runtime. Existing input/output curves remain the clear fallback.
    pub fn arm_host_gain_input(&mut self, target: DeviceAutomationTarget) -> Result<(), Error> {
        if !target.is_host() {
            return Err(Error::InvalidDescriptor);
        }
        self.validate_input_target(target)?;
        self.input_target = Some(target);
        Ok(())
    }
    fn validate_input_target(&self, target: DeviceAutomationTarget) -> Result<(), Error> {
        if self.foreign.is_some() || self.input_target.is_some_and(|current| current != target) {
            return Err(Error::InvalidDescriptor);
        }
        Ok(())
    }
    pub(crate) fn validate_input(&self, input: NativeTransientValue) -> Result<(), Error> {
        if self.input_target != Some(input.target()) {
            return Err(Error::InvalidDescriptor);
        }
        if let NativeTransientValue::Parameter { id, value } = input {
            self.descriptor(id)
                .ok_or(Error::UnknownParameter)?
                .validate(value)?;
        }
        Ok(())
    }
    pub fn is_empty(&self) -> bool {
        self.lanes.is_empty() && self.input_target.is_none() && self.foreign.is_none()
    }
    /// The tail split can stage output-gain control while input gain stays
    /// static. Provider controls, authored curves and input-gain windows need
    /// their own verified combined-signal contracts.
    pub(crate) fn supports_tail_split_controls(&self) -> bool {
        self.foreign.is_none()
            && self.points.is_empty()
            && self.ids.is_empty()
            && self.host_input.is_none()
            && self.input_target != Some(DeviceAutomationTarget::HostInputGain)
    }
    pub fn has_host_target(&self, target: DeviceAutomationTarget) -> bool {
        match target {
            DeviceAutomationTarget::HostInputGain => self.host_input.is_some(),
            DeviceAutomationTarget::HostOutputGain => self.host_output.is_some(),
            DeviceAutomationTarget::Parameter(_) | DeviceAutomationTarget::Vst3 { .. } => false,
        }
    }
    pub fn has_host_targets(&self) -> bool {
        self.host_input.is_some()
            || self.host_output.is_some()
            || self.input_target.is_some_and(|target| target.is_host())
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
        &Vec<PreparedParameterAutomationPoint>,
    ) {
        (&self.ids, &self.lanes, &self.points)
    }
    pub fn touch_storage(&self) -> &Vec<ParameterAutomationTouchSpan> {
        &self.touch
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
            )?
            .checked_add(
                self.foreign()
                    .map_or(0, PreparedForeignAutomation::storage_bytes),
            )?
            .checked_add(Self::quoted_touch_storage_bytes(self.touch.capacity())?)
    }

    pub fn quoted_touch_storage_bytes(spans: usize) -> Option<usize> {
        spans.checked_mul(std::mem::size_of::<ParameterAutomationTouchSpan>())
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
            .checked_add(
                points.checked_mul(std::mem::size_of::<PreparedParameterAutomationPoint>())?,
            )
    }
    /// Raw provider/host metadata and normalized point staging coexist with the
    /// retained compiled vectors during control construction. The compiled
    /// point size includes its explicit canonical cache and is charged above.
    pub fn quoted_construction_overlap_bytes(lanes: usize, points: usize) -> Option<usize> {
        Self::quoted_construction_overlap_bytes_with_host(lanes, 0, points)
    }
    pub fn quoted_construction_overlap_bytes_with_host(
        provider: usize,
        host: usize,
        points: usize,
    ) -> Option<usize> {
        provider
            .checked_mul(std::mem::size_of::<ParameterAutomationLane>())?
            .checked_add(host.checked_mul(std::mem::size_of::<HostAutomationLane>())?)?
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
            input: None,
            external: None,
        }
    }
    /// Overlay one final canonical value. No clock is inferred: the engine must
    /// defer a requested input until its relevant stage origin is available.
    /// None restores the current curve, or explicit immutable-base fallback.
    pub fn block_with_input<'a>(
        &'a self,
        clock: Option<&'a dyn NativeInstrumentClock>,
        frames: usize,
        input: Option<NativeTransientValue>,
    ) -> Result<ParameterAutomationBlock<'a>, Error> {
        let mut block = self.block(clock, frames);
        if let Some(input) = input {
            self.validate_input(input)?;
            block.input = Some(match input {
                NativeTransientValue::Parameter { value, .. } => CanonicalValue::Provider(value),
                NativeTransientValue::HostInputGain(gain)
                | NativeTransientValue::HostOutputGain(gain) => {
                    CanonicalValue::HostGainLinear(gain.linear())
                }
            });
        }
        Ok(block)
    }
    pub(crate) fn block_with_optional_external<'a>(
        &'a self,
        clock: Option<&'a dyn NativeInstrumentClock>,
        frames: usize,
        input: Option<NativeTransientValue>,
        external: Option<&'a dyn ExternalParameterAutomation>,
    ) -> Result<ParameterAutomationBlock<'a>, Error> {
        match external {
            Some(external) => self.block_with_external(clock, frames, input, external),
            None => self.block_with_input(clock, frames, input),
        }
    }

    /// Compose available scalar samples through the existing normalization law.
    /// Page availability and stage origins are established by the render owner.
    pub fn block_with_external<'a>(
        &'a self,
        clock: Option<&'a dyn NativeInstrumentClock>,
        frames: usize,
        input: Option<NativeTransientValue>,
        external: &'a dyn ExternalParameterAutomation,
    ) -> Result<ParameterAutomationBlock<'a>, Error> {
        let mut block = self.block_with_input(clock, frames, input)?;
        if external.frames() != frames || external.targets().len() > MAX_AUTOMATION_LANES {
            return Err(Error::InvalidDescriptor);
        }
        let mut previous = None;
        for &target in external.targets() {
            if previous.is_some_and(|prior| prior >= target)
                || match target {
                    DeviceAutomationTarget::Parameter(id) => self.ids.binary_search(&id).is_err(),
                    _ => !self.has_host_target(target),
                }
            {
                return Err(Error::InvalidDescriptor);
            }
            previous = Some(target);
        }
        block.external = Some(external);
        Ok(block)
    }
}
pub struct ParameterAutomationBlock<'a> {
    program: &'a PreparedParameterAutomation,
    clock: Option<&'a dyn NativeInstrumentClock>,
    frames: usize,
    input: Option<CanonicalValue>,
    external: Option<&'a dyn ExternalParameterAutomation>,
}
impl ParameterAutomationBlock<'_> {
    /// Native host gain in linear amplitude units, from the same exact integer
    /// HostGain conversion used by base state. Missing origin uses base fallback.
    pub fn host_gain_at(&self, target: DeviceAutomationTarget, offset: usize) -> Option<f32> {
        if offset >= self.frames {
            return None;
        }
        if self.program.input_target == Some(target)
            && let Some(input) = self.input
        {
            return input.host_gain();
        }
        let index = match target {
            DeviceAutomationTarget::HostInputGain => self.program.host_input?,
            DeviceAutomationTarget::HostOutputGain => self.program.host_output?,
            DeviceAutomationTarget::Parameter(_) | DeviceAutomationTarget::Vst3 { .. } => {
                return None;
            }
        };
        self.external_value(index, target, offset)
            .or_else(|| self.value_at_beat(index, self.clock?.arrangement_beat_at_offset(offset)?))?
            .host_gain()
    }
    fn external_value(
        &self,
        index: usize,
        target: DeviceAutomationTarget,
        offset: usize,
    ) -> Option<CanonicalValue> {
        let external = self.external?;
        external.targets().binary_search(&target).ok()?;
        let normalized = external.value_at(target, offset)?;
        self.program
            .lanes
            .get(index)?
            .mapping
            .value(normalized.get())
            .ok()
    }
    fn value_at_beat(&self, index: usize, beat: u64) -> Option<CanonicalValue> {
        let lane = self.program.lanes.get(index)?;
        if !lane.touch.is_empty() {
            let spans = self.program.touch.get(lane.touch.clone())?;
            let next = spans.partition_point(|span| span.start_beat <= beat);
            if let Some(span) = next.checked_sub(1).and_then(|index| spans.get(index))
                && beat < span.end_beat
            {
                let steps = self.program.points.get(span.points.clone())?;
                let point = steps.partition_point(|point| point.point.beat_raw <= beat);
                return steps
                    .get(point.checked_sub(1)?)
                    .map(|point| point.canonical);
            }
        }
        let PreparedLaneSource::Curve { points, constant } = &lane.source else {
            return None;
        };
        if let Some(value) = constant {
            return Some(*value);
        }
        let points = self.program.points.get(points.clone())?;
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
                lane.mapping.value(normalized).ok()
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
        if self.program.input_target == Some(DeviceAutomationTarget::Parameter(parameter))
            && let Some(input) = self.input
        {
            return input.provider();
        }
        let index = self.program.ids.binary_search(&parameter).ok()?;
        self.external_value(index, DeviceAutomationTarget::Parameter(parameter), offset)
            .or_else(|| self.value_at_beat(index, self.clock?.arrangement_beat_at_offset(offset)?))?
            .provider()
    }
    fn values_at(&self, offset: usize, output: &mut [Option<ParameterValue>]) -> Result<(), Error> {
        if output.len() != self.program.ids.len() || offset >= self.frames {
            return Err(Error::InvalidParameterValue);
        }
        let beat = self
            .clock
            .and_then(|clock| clock.arrangement_beat_at_offset(offset));
        for (index, slot) in output.iter_mut().enumerate() {
            *slot = if self.program.input_target
                == Some(DeviceAutomationTarget::Parameter(self.program.ids[index]))
                && self.input.is_some()
            {
                self.input.and_then(CanonicalValue::provider)
            } else {
                self.external_value(
                    index,
                    DeviceAutomationTarget::Parameter(self.program.ids[index]),
                    offset,
                )
                .or_else(|| beat.and_then(|beat| self.value_at_beat(index, beat)))
                .and_then(CanonicalValue::provider)
            };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
