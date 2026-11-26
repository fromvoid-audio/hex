use super::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreparedParameterAutomation {
    pub(super) ids: Vec<ParameterId>,
    pub(super) lanes: Vec<PreparedParameterAutomationLane>,
    pub(super) points: Vec<PreparedParameterAutomationPoint>,
    pub(super) host_input: Option<usize>,
    pub(super) host_output: Option<usize>,
    pub(super) input_target: Option<DeviceAutomationTarget>,
    pub(super) foreign: Option<PreparedForeignAutomation>,
    pub(super) touch: Vec<ParameterAutomationTouchSpan>,
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

    /// Label the current unpublished effect program with its frame clock.
    pub fn bind_foreign_effect_clock_on_control(
        &mut self,
        clock_epoch: std::num::NonZeroU64,
        clock_splices: bool,
    ) {
        if let Some(program) = self.foreign.as_mut() {
            program.bind_effect_clock_on_control(clock_epoch, clock_splices);
        }
    }

    pub(crate) fn shares_foreign_program_history_with(
        &self,
        other: &Self,
        binding: hex_lattice_contract::ForeignProcessorBinding,
    ) -> bool {
        let Self {
            ids,
            lanes,
            points,
            host_input,
            host_output,
            input_target,
            foreign,
            touch,
        } = self;
        ids == &other.ids
            && lanes == &other.lanes
            && points == &other.points
            && host_input == &other.host_input
            && host_output == &other.host_output
            && input_target == &other.input_target
            && touch == &other.touch
            && match (foreign, &other.foreign) {
                (Some(a), Some(b)) => {
                    a.borrow().binding == binding
                        && b.borrow().binding == binding
                        && a.shares_current_program_history_with(b)
                }
                _ => false,
            }
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

    /// Select foreign history while the caller holds the exact stopped source
    /// fence and knows no omitted program can be used by any callback.
    /// This control-side operation does not establish that fence or authorize
    /// publication; failures leave the existing program owners unchanged.
    pub fn retain_selected_foreign_history_on_control(
        &mut self,
        previous: &Self,
        required: &[std::num::NonZeroU64],
    ) -> Result<(), Error> {
        let current = self
            .foreign
            .as_mut()
            .ok_or(Error::ForeignProcessingFailed)?;
        let previous = previous
            .foreign
            .as_ref()
            .ok_or(Error::ForeignProcessingFailed)?;
        current.retain_selected_on_control(previous, required)
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
            let constant = compile_points(mapping, values, &mut compiled_points, true)?;
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
            let constant = compile_points(Mapping::HostGain, values, &mut compiled_points, true)?;
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
        let mut lane_count = 0usize;
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
            let PreparedLaneSource::Curve { .. } = &lane.source else {
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
                lane_count = 0;
            }
            lane_count = lane_count
                .checked_add(span.points.len())
                .ok_or(Error::RuntimeBudgetExceeded)?;
            if lane_count > MAX_AUTOMATION_TOUCH_STEPS_PER_LANE || lane.touch.len() == 128 {
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
            compile_points(lane.mapping, values, &mut self.points, false)?;
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
                | NativeTransientValue::HostOutputGain(gain) => CanonicalValue::HostGain {
                    normalized: NormalizedAutomationValue(gain.normalized()),
                    linear: gain.linear(),
                },
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
