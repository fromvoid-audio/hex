use super::*;

pub struct ParameterAutomationBlock<'a> {
    pub(super) program: &'a PreparedParameterAutomation,
    pub(super) clock: Option<&'a dyn NativeInstrumentClock>,
    pub(super) frames: usize,
    pub(super) input: Option<CanonicalValue>,
    pub(super) external: Option<&'a dyn ExternalParameterAutomation>,
}
impl ParameterAutomationBlock<'_> {
    /// Native host gain in linear amplitude units, from the same exact integer
    /// HostGain conversion used by base state. Missing origin uses base fallback.
    pub fn host_gain_at(&self, target: DeviceAutomationTarget, offset: usize) -> Option<f32> {
        self.host_gain_frame(target, offset)
            .map(|(_, linear)| linear)
    }

    /// Host trim in normalized dB space before millidB rounding. This uses the
    /// same host target, precedence and clock as the linear amplitude read.
    pub fn host_gain_normalized_at(
        &self,
        target: DeviceAutomationTarget,
        offset: usize,
    ) -> Option<NormalizedAutomationValue> {
        self.host_gain_frame(target, offset)
            .map(|(normalized, _)| normalized)
    }

    fn host_gain_frame(
        &self,
        target: DeviceAutomationTarget,
        offset: usize,
    ) -> Option<(NormalizedAutomationValue, f32)> {
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
                let shaped = curve_shape(
                    elapsed as f64 / span as f64,
                    left.point.curve_tension_millis,
                );
                let normalized = (f64::from(left.point.normalized)
                    + (f64::from(right.point.normalized) - f64::from(left.point.normalized))
                        * shaped) as f32;
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
    fn guarantees_valid_values(&self, descriptors: &[ParameterDescriptor], frames: usize) -> bool {
        if frames > self.frames {
            return false;
        }
        if let (Some(DeviceAutomationTarget::Parameter(input_id)), Some(input)) =
            (self.program.input_target, self.input)
        {
            let Ok(index) = descriptors.binary_search_by_key(&input_id, |d| d.id) else {
                return false;
            };
            let Some(value) = input.provider() else {
                return false;
            };
            if descriptors[index].validate(value).is_err() {
                return false;
            }
        }
        self.program
            .ids
            .iter()
            .zip(&self.program.lanes)
            .all(|(id, lane)| {
                descriptors
                    .binary_search_by_key(id, |d| d.id)
                    .ok()
                    .is_some_and(|index| lane.mapping.matches_descriptor(descriptors[index]))
            })
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
