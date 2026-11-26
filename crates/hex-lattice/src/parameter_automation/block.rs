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
    fn external_normalized(&self, target: DeviceAutomationTarget, offset: usize) -> Option<f32> {
        let external = self.external?;
        external.targets().binary_search(&target).ok()?;
        Some(external.value_at(target, offset)?.get())
    }
    fn normalized_at_beat(&self, index: usize, beat: u64) -> Option<f32> {
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
                    .map(|point| point.point.normalized);
            }
        }
        let PreparedLaneSource::Curve { points, constant } = &lane.source else {
            return None;
        };
        let points = self.program.points.get(points.clone())?;
        if constant.is_some() {
            return points.first().map(|point| point.point.normalized);
        }
        let next = points.partition_point(|point| point.point.beat_raw <= beat);
        if next == 0 {
            return points.first().map(|point| point.point.normalized);
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
                Some(
                    (f64::from(left.point.normalized)
                        + (f64::from(right.point.normalized) - f64::from(left.point.normalized))
                            * shaped) as f32,
                )
            }
            _ => Some(left.point.normalized),
        }
    }
    fn external_value(
        &self,
        index: usize,
        target: DeviceAutomationTarget,
        offset: usize,
    ) -> Option<CanonicalValue> {
        let normalized = self.external_normalized(target, offset)?;
        self.program
            .lanes
            .get(index)?
            .mapping
            .value(normalized)
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
    fn normalized_value_at(&self, parameter: ParameterId, offset: usize) -> Option<f32> {
        if offset >= self.frames {
            return None;
        }
        let target = DeviceAutomationTarget::Parameter(parameter);
        if self.program.input_target == Some(target) && self.input.is_some() {
            return None;
        }
        let index = self.program.ids.binary_search(&parameter).ok()?;
        self.external_normalized(target, offset).or_else(|| {
            self.normalized_at_beat(index, self.clock?.arrangement_beat_at_offset(offset)?)
        })
    }
    fn fill_normalized_slice(&self, parameter: ParameterId, output: &mut [Option<f32>]) -> bool {
        let target = DeviceAutomationTarget::Parameter(parameter);
        if self.program.input_target == Some(target) && self.input.is_some() {
            return false;
        }
        let Ok(index) = self.program.ids.binary_search(&parameter) else {
            output.fill(None);
            return true;
        };
        let Some(lane) = self.program.lanes.get(index) else {
            output.fill(None);
            return true;
        };
        let valid_len = output.len().min(self.frames);
        if valid_len < output.len() {
            output[valid_len..].fill(None);
        }
        let has_external = self
            .external
            .is_some_and(|ext| ext.targets().binary_search(&target).is_ok());
        let has_touch = !lane.touch.is_empty();
        let curve_points = match &lane.source {
            PreparedLaneSource::Curve { points, constant } => {
                let slice = self.program.points.get(points.clone());
                Some((slice, constant.is_some()))
            }
            PreparedLaneSource::BaseFallback => None,
        };
        // Fast cursor-tracked beat walk avoids O(log P) binary search per sample.
        let mut cursor = 0usize;
        let mut last_beat: Option<u64> = None;
        let discrete = lane.mapping.discrete();
        for (offset, slot) in output[..valid_len].iter_mut().enumerate() {
            if has_external
                && let Some(ext) = self.external
                && let Some(val) = ext.value_at(target, offset)
            {
                *slot = Some(val.get());
                continue;
            }
            let Some(beat) = self
                .clock
                .and_then(|c| c.arrangement_beat_at_offset(offset))
            else {
                *slot = None;
                continue;
            };
            if has_touch {
                *slot = self.normalized_at_beat(index, beat);
                continue;
            }
            let Some((Some(points), is_constant)) = curve_points else {
                *slot = None;
                continue;
            };
            if is_constant {
                *slot = points.first().map(|p| p.point.normalized);
                continue;
            }
            let next = if last_beat.is_some_and(|prev| beat >= prev) {
                while cursor < points.len() && points[cursor].point.beat_raw <= beat {
                    cursor += 1;
                }
                cursor
            } else {
                cursor = points.partition_point(|p| p.point.beat_raw <= beat);
                cursor
            };
            last_beat = Some(beat);
            if next == 0 {
                *slot = points.first().map(|p| p.point.normalized);
                continue;
            }
            let left = &points[next - 1];
            *slot = match points.get(next) {
                Some(right) if !left.point.step && !discrete && beat != left.point.beat_raw => {
                    let span = right.point.beat_raw.saturating_sub(left.point.beat_raw);
                    if span == 0 {
                        Some(left.point.normalized)
                    } else {
                        let elapsed = beat.saturating_sub(left.point.beat_raw);
                        let shaped = curve_shape(
                            elapsed as f64 / span as f64,
                            left.point.curve_tension_millis,
                        );
                        Some(
                            (f64::from(left.point.normalized)
                                + (f64::from(right.point.normalized)
                                    - f64::from(left.point.normalized))
                                    * shaped) as f32,
                        )
                    }
                }
                _ => Some(left.point.normalized),
            };
        }
        true
    }
    fn fill_values(
        &self,
        parameter: ParameterId,
        output: &mut [Option<ParameterValue>],
    ) -> Result<(), Error> {
        let valid_len = output.len().min(self.frames);
        if valid_len < output.len() {
            output[valid_len..].fill(None);
        }
        let target = DeviceAutomationTarget::Parameter(parameter);
        if self.program.input_target == Some(target)
            && let Some(input) = self.input
        {
            output[..valid_len].fill(input.provider());
            return Ok(());
        }
        let Ok(index) = self.program.ids.binary_search(&parameter) else {
            output[..valid_len].fill(None);
            return Ok(());
        };
        let Some(lane) = self.program.lanes.get(index) else {
            output[..valid_len].fill(None);
            return Ok(());
        };
        // Fast path: constant curve without external/touch maps zero times per block.
        let has_external = self
            .external
            .is_some_and(|ext| ext.targets().binary_search(&target).is_ok());
        if !has_external
            && lane.touch.is_empty()
            && let PreparedLaneSource::Curve {
                constant: Some(constant),
                ..
            } = lane.source
        {
            let value = constant.provider();
            for (offset, slot) in output[..valid_len].iter_mut().enumerate() {
                *slot = if self
                    .clock
                    .and_then(|c| c.arrangement_beat_at_offset(offset))
                    .is_some()
                {
                    value
                } else {
                    None
                };
            }
            return Ok(());
        }
        let mut normalized = [None; 64];
        let mut start = 0;
        while start < valid_len {
            let chunk_len = (valid_len - start).min(normalized.len());
            if start == 0 && valid_len <= normalized.len() {
                self.fill_normalized_slice(parameter, &mut normalized[..chunk_len]);
                for (offset, slot) in output[..chunk_len].iter_mut().enumerate() {
                    *slot = normalized[offset]
                        .and_then(|n| lane.mapping.value(n).ok())
                        .and_then(CanonicalValue::provider);
                }
            } else {
                for (offset, slot) in output.iter_mut().enumerate().skip(start).take(chunk_len) {
                    *slot = self
                        .external_value(index, target, offset)
                        .or_else(|| {
                            self.value_at_beat(
                                index,
                                self.clock?.arrangement_beat_at_offset(offset)?,
                            )
                        })
                        .and_then(CanonicalValue::provider);
                }
            }
            start += chunk_len;
        }
        Ok(())
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
