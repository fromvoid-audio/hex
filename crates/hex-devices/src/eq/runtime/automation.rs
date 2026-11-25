//! Exact shared grants resolve stable node identities on control. Borrowed typed
//! values are validated at every original frame offset; node targets refresh at
//! the existing 64-frame coefficient cadence and use its existing ramp/history law.
//! Cold held targets start canonically. Missing values select immutable Base.
//! Granted fields override historical opaque lanes, including Base; ungranted
//! historical fields still use their original clock. No authored lane is erased.
//! Mode uses the preallocated routing fade: rapid targets coalesce to the
//! latest reading until that one outgoing branch finishes. Cold/seek targets
//! select directly. This module owns no curves, clock, publication or resource
//! lifecycle. A mapped output gain applies on the same sample to both sides of
//! the mode fade. An ordinary authored lane continues through its existing gain
//! smoother from the last effective gain.
use super::*;
use hex_lattice::NativeParameterAutomation;
use hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE;

#[derive(Clone, Copy)]
pub(super) struct Grant {
    pub(super) descriptor: ParameterDescriptor,
    node: Option<(usize, NodeParameter)>,
}
impl Grant {
    pub(super) fn node_slot(&self) -> Option<(usize, NodeParameter)> {
        self.node
    }
    pub(super) fn prepare(
        settings: &EqState,
        ids: &[ParameterId],
    ) -> Result<Vec<Self>, DeviceContractError> {
        if ids.len() > MAX_PARAMETERS_PER_DEVICE || ids.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        ids.iter()
            .map(|&id| {
                if id == OUTPUT_GAIN || id == MODE {
                    return Ok(Self {
                        descriptor: DESCRIPTORS[if id == MODE { 200 } else { 201 }],
                        node: None,
                    });
                }
                let (identity, field) = NodeId::from_parameter(id)?;
                let index = settings
                    .nodes
                    .iter()
                    .position(|node| node.id == identity)
                    .ok_or(DeviceContractError::UnknownParameter)?;
                if settings.nodes[index].legacy_shelf
                    && matches!(
                        field,
                        NodeParameter::Kind | NodeParameter::Gain | NodeParameter::Q
                    )
                {
                    return Err(DeviceContractError::UnsupportedProcessingClass);
                }
                Ok(Self {
                    descriptor: descriptor(identity, field)?,
                    node: Some((index, field)),
                })
            })
            .collect()
    }
}
#[derive(Clone, Copy)]
struct Patch {
    node: Node,
    mask: u8,
}
pub(super) struct Nodes {
    patches: [Option<Patch>; MAX_NODES],
}
impl Nodes {
    fn new(config: &Configuration) -> Self {
        let mut nodes = Self {
            patches: [None; MAX_NODES],
        };
        // Preparation validates node indices against the bounded canonical nodes.
        for grant in &config.parameters {
            if let Some((index, field)) = grant.node {
                let patch = nodes.patches[index].get_or_insert(Patch {
                    node: config.nodes[index],
                    mask: 0,
                });
                patch.mask |= 1 << field as u8;
            }
        }
        nodes
    }
    pub(super) fn apply(&self, index: usize, node: &mut Node) {
        let Some(patch) = self.patches[index] else {
            return;
        };
        for field in NodeParameter::ALL {
            if patch.mask & (1 << field as u8) == 0 {
                continue;
            }
            match field {
                NodeParameter::Enabled => node.enabled = patch.node.enabled,
                NodeParameter::Kind => {
                    node.kind = patch.node.kind;
                    node.legacy_shelf = patch.node.legacy_shelf;
                }
                NodeParameter::Frequency => node.frequency_hz = patch.node.frequency_hz,
                NodeParameter::Gain => node.gain_db = patch.node.gain_db,
                NodeParameter::Q => {
                    node.q = patch.node.q;
                    node.legacy_shelf = patch.node.legacy_shelf;
                }
                NodeParameter::Slope => node.slope = patch.node.slope,
                NodeParameter::Bank => node.bank = patch.node.bank,
            }
        }
    }
}
impl PreparedEq {
    pub(in crate::eq8::flexible) fn automated(
        settings: EqState,
        rate: u32,
        parameters: &[ParameterId],
    ) -> Result<Self, DeviceContractError> {
        if parameters.is_empty() {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        Self::prepare(settings, rate, parameters)
    }
}
#[derive(Clone, Copy)]
pub(super) struct Targets {
    pub(super) gain: f64,
    pub(super) mode: u8,
    pub(super) mapped_gain: bool,
}
impl EqRuntime {
    fn frame_clock_state(
        clock: Option<&dyn NativeInstrumentClock>,
        offset: usize,
        last_frame: Option<(u64, u64)>,
        fallback_frame: u64,
    ) -> (Option<(u64, u64)>, u64, bool, bool) {
        let running = clock.is_some_and(|c| c.transport_is_running() == Some(true));
        let origin = if running {
            clock.and_then(|c| c.arrangement_frame_at_offset(offset))
        } else {
            None
        };
        let position = origin.unwrap_or(fallback_frame);
        let current_frame = origin.map(|arrangement| {
            let continuity = clock
                .and_then(|clock| clock.performance_frame_at_offset(offset))
                .unwrap_or(arrangement);
            (arrangement, continuity)
        });
        let arrangement_changed = origin.is_some_and(|position| {
            last_frame.is_none_or(|(last, _)| last.checked_add(1) != Some(position))
        });
        let discontinuity = current_frame.is_some_and(|(_, position)| {
            last_frame.is_none_or(|(_, last)| last.checked_add(1) != Some(position))
        });
        (current_frame, position, arrangement_changed, discontinuity)
    }

    fn frame_refreshes_nodes(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        offset: usize,
        automated: bool,
    ) -> bool {
        let (_, position, arrangement_changed, discontinuity) =
            Self::frame_clock_state(clock, offset, self.last_frame, self.fallback_frame);
        self.refresh_target
            || self.pristine
            || arrangement_changed
            || discontinuity
            || (automated && position % CADENCE == 0)
    }

    /// Returns the exclusive end offset `> start` of the contiguous slice
    /// starting at `start` during which no subsequent frame refreshes node
    /// targets (`process_at` reads `node_overrides` at `start` only).
    fn next_node_refresh_offset(
        &self,
        clock: Option<&dyn NativeInstrumentClock>,
        start: usize,
        end: usize,
        automated: bool,
    ) -> usize {
        let (mut sim_last_frame, _, _, _) =
            Self::frame_clock_state(clock, start, self.last_frame, self.fallback_frame);
        let mut sim_fallback = self.fallback_frame.wrapping_add(1);
        for offset in (start + 1)..end {
            let (current_frame, position, arrangement_changed, discontinuity) =
                Self::frame_clock_state(clock, offset, sim_last_frame, sim_fallback);
            if arrangement_changed || discontinuity || (automated && position % CADENCE == 0) {
                return offset;
            }
            sim_last_frame = current_frame;
            sim_fallback = sim_fallback.wrapping_add(1);
        }
        end
    }

    fn apply_node_readings(
        &self,
        readings: &[Option<ParameterValue>],
        bindings: &[usize],
        nodes: &mut Nodes,
    ) -> Result<(), DeviceContractError> {
        for (&reading, &binding) in readings.iter().zip(bindings) {
            let grant = self.configuration.parameters[binding];
            if let Some((index, field)) = grant.node
                && let Some(patch) = nodes.patches[index].as_mut()
            {
                patch.node.set(
                    field,
                    reading.unwrap_or_else(|| self.configuration.nodes[index].value(field)),
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn process_automation(
        &mut self,
        frames: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        values: &dyn NativeParameterAutomation,
        bypassed: bool,
    ) -> Result<(), DeviceContractError> {
        let ids = values.parameter_ids();
        if self.configuration.parameters.is_empty()
            || ids.len() > self.configuration.parameters.len()
            || ids.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(DeviceContractError::UnsupportedProcessingClass);
        }
        let mut bindings = [0; MAX_PARAMETERS_PER_DEVICE];
        let mut has_gain_or_mode = false;
        for (slot, id) in bindings.iter_mut().zip(ids) {
            let binding = self
                .configuration
                .parameters
                .binary_search_by_key(id, |grant| grant.descriptor.id)
                .map_err(|_| DeviceContractError::UnsupportedProcessingClass)?;
            *slot = binding;
            if self.configuration.parameters[binding].node.is_none() {
                has_gain_or_mode = true;
            }
        }
        let active_bindings = &bindings[..ids.len()];
        let mut preflight_nodes = Nodes::new(&self.configuration);
        let mut readings = [None; MAX_PARAMETERS_PER_DEVICE];
        // Preflight every frame offset on the stack before mutating filter
        // histories, gain state, or the caller's audio buffer.
        for offset in 0..frames.len() {
            values.values_at(offset, &mut readings[..ids.len()])?;
            for (&reading, &binding) in readings[..ids.len()].iter().zip(active_bindings) {
                let grant = self.configuration.parameters[binding];
                if let Some(value) = reading {
                    grant.descriptor.validate(value)?;
                    if let Some((index, field)) = grant.node
                        && let Some(patch) = preflight_nodes.patches[index].as_mut()
                    {
                        patch.node.set(field, value)?;
                    }
                }
            }
        }
        let mut nodes = Nodes::new(&self.configuration);
        let has_nodes = self.configuration.granted_nodes != 0;
        let automated = !self.configuration.lanes.is_empty() || has_nodes;
        let mapped_gain = values.has_sample_rate_modulation(OUTPUT_GAIN);
        if !has_gain_or_mode {
            let targets = Targets {
                gain: self.configuration.gain,
                mode: self.configuration.mode,
                mapped_gain,
            };
            let mut chunk_start = 0;
            while chunk_start < frames.len() {
                if has_nodes && self.frame_refreshes_nodes(clock, chunk_start, automated) {
                    values.values_at(chunk_start, &mut readings[..ids.len()])?;
                    self.apply_node_readings(&readings[..ids.len()], active_bindings, &mut nodes)?;
                }
                let chunk_end =
                    self.next_node_refresh_offset(clock, chunk_start, frames.len(), automated);
                self.process_at(
                    &mut frames[chunk_start..chunk_end],
                    clock,
                    bypassed,
                    chunk_start,
                    targets,
                    has_nodes.then_some(&nodes),
                );
                chunk_start = chunk_end;
            }
            return Ok(());
        }
        for (offset, frame) in frames.iter_mut().enumerate() {
            values.values_at(offset, &mut readings[..ids.len()])?;
            let mut targets = Targets {
                gain: self.configuration.gain,
                mode: self.configuration.mode,
                mapped_gain,
            };
            let mut authored_gain = false;
            let update_nodes = has_nodes && self.frame_refreshes_nodes(clock, offset, automated);
            for (&reading, &binding) in readings[..ids.len()].iter().zip(active_bindings) {
                let grant = self.configuration.parameters[binding];
                if let Some((index, field)) = grant.node {
                    if update_nodes && let Some(patch) = nodes.patches[index].as_mut() {
                        patch.node.set(
                            field,
                            reading.unwrap_or_else(|| self.configuration.nodes[index].value(field)),
                        )?;
                    }
                } else if grant.descriptor.id == MODE {
                    if let Some(ParameterValue::Choice(value)) = reading {
                        targets.mode = value as u8;
                    }
                } else if let Some(ParameterValue::Float(value)) = reading {
                    targets.gain = f64::from(value);
                    authored_gain = true;
                }
            }
            if authored_gain && self.pristine {
                self.gain = targets.gain;
            }
            self.process_at(
                std::slice::from_mut(frame),
                clock,
                bypassed,
                offset,
                targets,
                has_nodes.then_some(&nodes),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_lattice::{EMPTY_DEVICE_RESOURCES as R, NativeDeviceDefinition};
    struct Values {
        start: usize,
        held: Option<f32>,
        moving: bool,
        bad: Option<ParameterValue>,
    }
    impl Values {
        fn held(value: Option<f32>) -> Self {
            Self {
                start: 0,
                held: value,
                moving: false,
                bad: None,
            }
        }
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[OUTPUT_GAIN]
        }
        fn value_at(&self, _: ParameterId, offset: usize) -> Option<ParameterValue> {
            if offset == 17
                && let Some(bad) = self.bad
            {
                return Some(bad);
            }
            let i = self.start + offset;
            if self.moving {
                if (250..320).contains(&i) {
                    None
                } else {
                    Some(ParameterValue::Float(0.25 + (i % 101) as f32 * 0.01))
                }
            } else {
                self.held.map(ParameterValue::Float)
            }
        }
    }
    fn render(
        rt: &mut dyn OwnedProcessorRuntime,
        input: &[[f32; 2]],
        mut values: Values,
        width: usize,
    ) -> Vec<[f32; 2]> {
        let mut out = input.to_vec();
        for (i, block) in out.chunks_mut(width).enumerate() {
            values.start = i * width;
            rt.process_block_with_automation(block, None, &values, &R)
                .unwrap();
        }
        out
    }
    #[test]
    fn cold_held_gain_matches_canonical_across_nodes_modes_and_rate_endpoints() {
        for rate in [8000, 192000] {
            for mode in 0..3 {
                let mut s = EqState {
                    mode,
                    ..EqState::default()
                };
                s.add(
                    Node::new(
                        NodeId::new(7).unwrap(),
                        1000.0,
                        6.0,
                        if mode == 2 { 3 } else { 0 },
                    )
                    .unwrap(),
                )
                .unwrap();
                let f = PreparedEq::automated(s.clone(), rate, &[OUTPUT_GAIN]).unwrap();
                let mut actual = f.create_runtime().unwrap();
                actual
                    .process_block_with_automation(&mut [], None, &Values::held(Some(0.5)), &R)
                    .unwrap();
                s.output_gain = 0.5;
                let mut canonical = PreparedEq::new(s, rate).unwrap().create_runtime().unwrap();
                let input: Vec<_> = (0..257)
                    .map(|i| [(i as f32 * 0.17).sin(), (i as f32 * 0.31).sin() * 0.4])
                    .collect();
                let mut expected = input.clone();
                canonical.process_block(&mut expected, &R);
                assert_eq!(
                    render(actual.as_mut(), &input, Values::held(Some(0.5)), 17),
                    expected
                );
            }
        }
    }
    #[test]
    fn moving_gain_and_base_follow_existing_gain_law_independent_of_partition() {
        let f = PreparedEq::automated(EqState::default(), 48000, &[OUTPUT_GAIN]).unwrap();
        let input = vec![[0.5, -0.25]; 513];
        let mut expected = Vec::new();
        let mut gain = 0.25_f64;
        for i in 0..input.len() {
            let target = if (250..320).contains(&i) {
                1.0
            } else {
                f64::from(0.25 + (i % 101) as f32 * 0.01)
            };
            gain += (target - gain) * 0.015625;
            expected.push([(0.5 * gain) as f32, (-0.25 * gain) as f32]);
        }
        for width in [1, 17, 513] {
            let mut rt = f.create_runtime().unwrap();
            let mut v = Values::held(None);
            v.moving = true;
            assert_eq!(render(rt.as_mut(), &input, v, width), expected);
        }
    }
    #[test]
    fn checkpoint_retry_and_live_base_handoff_preserve_gain_progress_and_bypass() {
        let f = PreparedEq::automated(EqState::default(), 48000, &[OUTPUT_GAIN]).unwrap();
        let mut primary = f.create_runtime().unwrap();
        render(
            primary.as_mut(),
            &[[0.5, -0.25]; 32],
            Values::held(Some(0.25)),
            32,
        );
        for bad in [
            ParameterValue::Float(f32::NAN),
            ParameterValue::Float(f32::INFINITY),
            ParameterValue::Float(-0.01),
            ParameterValue::Float(16.1),
            ParameterValue::Bool(true),
        ] {
            let mut scratch = f.create_runtime().unwrap();
            scratch.inherit_from(primary.as_ref());
            let mut values = Values::held(Some(0.75));
            values.bad = Some(bad);
            assert!(
                scratch
                    .process_block_with_automation(&mut [[0.5, -0.25]; 32], None, &values, &R)
                    .is_err()
            );
            scratch.inherit_from(primary.as_ref());
            assert_eq!(
                render(
                    scratch.as_mut(),
                    &[[0.5, -0.25]; 32],
                    Values::held(Some(0.75)),
                    17
                ),
                render(
                    primary.as_mut(),
                    &[[0.5, -0.25]; 32],
                    Values::held(Some(0.75)),
                    17
                )
            );
        }
        let settings = EqState {
            output_gain: 0.5,
            ..Default::default()
        };
        let mut candidate = PreparedEq::automated(settings, 48000, &[OUTPUT_GAIN])
            .unwrap()
            .create_runtime()
            .unwrap();
        candidate.inherit_from(primary.as_ref());
        let prior = primary.as_any().downcast_ref::<EqRuntime>().unwrap().gain;
        let actual = render(candidate.as_mut(), &[[0.5, -0.25]], Values::held(None), 1);
        assert_eq!(
            actual[0][0],
            (0.5 * (prior + (0.5 - prior) * 0.015625)) as f32
        );
        let mut dry = [[0.5, -0.25]; 37];
        candidate
            .process_bypassed_block_with_automation(&mut dry, None, &Values::held(Some(0.1)), &R)
            .unwrap();
        assert_eq!(dry, [[0.5, -0.25]; 37]);
        let mut clone = f.create_runtime().unwrap();
        clone.inherit_from(candidate.as_ref());
        assert_eq!(
            render(
                candidate.as_mut(),
                &[[0.5, -0.25]; 19],
                Values::held(Some(0.1)),
                19
            ),
            render(
                clone.as_mut(),
                &[[0.5, -0.25]; 19],
                Values::held(Some(0.1)),
                1
            )
        );
        candidate.reset();
        assert_eq!(
            render(
                candidate.as_mut(),
                &[[0.5, -0.25]],
                Values::held(Some(0.25)),
                1
            ),
            [[0.125, -0.0625]]
        );
    }
    #[test]
    fn exact_grant_and_typed_descriptor_agree_with_shared_metadata() {
        let s = EqState::default();
        let state = s.encode().unwrap();
        for ids in [
            &[][..],
            &[ParameterId::from_static(13)],
            &[OUTPUT_GAIN, OUTPUT_GAIN],
            &[OUTPUT_GAIN, MODE],
        ] {
            assert!(PreparedEq::automated(s.clone(), 48000, ids).is_err());
        }
        assert!(
            EQ_DEFINITION
                .supports_parameter_automation(&state, OUTPUT_GAIN)
                .unwrap()
        );
        assert!(
            EQ_DEFINITION
                .supports_parameter_automation(&state, MODE)
                .unwrap()
        );
        let f = PreparedEq::automated(s, 48000, &[OUTPUT_GAIN]).unwrap();
        assert!(f.supports_parameter_automation(OUTPUT_GAIN));
        assert_eq!(
            f.parameter_automation_descriptor(OUTPUT_GAIN),
            Some(DESCRIPTORS[201])
        );
        assert!(
            f.create_runtime()
                .unwrap()
                .supports_parameter_automation(OUTPUT_GAIN)
        );
        assert!(
            !PreparedEq::new(EqState::default(), 48000)
                .unwrap()
                .supports_parameter_automation(OUTPUT_GAIN)
        );
    }

    #[test]
    fn mid_block_invalid_automation_leaves_frames_and_dsp_state_unmutated() {
        let mut settings = EqState::default();
        settings
            .add(Node::new(NodeId::new(1).unwrap(), 1_000.0, 6.0, 0).unwrap())
            .unwrap();
        let prepared = PreparedEq::automated(settings, 48_000, &[OUTPUT_GAIN]).unwrap();
        let mut candidate = prepared.create_runtime().unwrap();
        let mut reference = prepared.create_runtime().unwrap();
        let input: Vec<[f32; 2]> = (0..64)
            .map(|i| [(i as f32 * 0.13).sin() * 0.5, (i as f32 * 0.29).cos() * 0.5])
            .collect();
        let mut failed_block = input.clone();
        let bad_values = Values {
            start: 0,
            held: Some(0.5),
            moving: false,
            bad: Some(ParameterValue::Float(f32::NAN)),
        };
        let err = candidate
            .process_block_with_automation(&mut failed_block, None, &bad_values, &R)
            .expect_err("mid-block NaN automation must fail");
        assert_eq!(err, DeviceContractError::InvalidParameterValue);
        assert_eq!(
            failed_block, input,
            "failed block must not partially overwrite caller frames"
        );
        // Subsequent valid processing must match an untouched runtime bit-for-bit.
        let mut after_failure = input.clone();
        let mut expected = input.clone();
        let valid_values = Values::held(Some(0.75));
        candidate
            .process_block_with_automation(&mut after_failure, None, &valid_values, &R)
            .unwrap();
        reference
            .process_block_with_automation(&mut expected, None, &valid_values, &R)
            .unwrap();
        assert_eq!(
            after_failure, expected,
            "DSP state must remain unmutated after mid-block automation error"
        );
    }
}

#[cfg(test)]
mod clock_tests {
    use super::*;
    use hex_lattice::EMPTY_DEVICE_RESOURCES as R;
    struct Clock(usize);
    impl NativeInstrumentClock for Clock {
        fn beat_at_offset(&self, offset: usize) -> Option<u64> {
            Some(((self.0 + offset) as u64) << 24)
        }
        fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
            self.beat_at_offset(offset)
        }
        fn transport_is_running(&self) -> Option<bool> {
            Some(true)
        }
        fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
            Some((self.0 + offset) as u64)
        }
    }
    struct Held;
    impl NativeParameterAutomation for Held {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[OUTPUT_GAIN]
        }
        fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
            Some(ParameterValue::Float(0.5))
        }
    }
    #[test]
    fn reset_returns_historical_lane_coefficients_to_authored_settings() {
        let mut state = EqState::default();
        let id = NodeId::new(7).unwrap();
        state.add(Node::new(id, 1_000.0, 12.0, 0).unwrap()).unwrap();
        let mut authored = PreparedEq::new(state.clone(), 48_000)
            .unwrap()
            .create_runtime()
            .unwrap();
        state
            .set_lane(AutomationLane {
                node: id,
                parameter: NodeParameter::Frequency,
                points: vec![AutomationPoint {
                    beat: 0,
                    normalized: 0.8,
                }],
            })
            .unwrap();
        let mut warmed = PreparedEq::new(state, 48_000)
            .unwrap()
            .create_runtime()
            .unwrap();
        let input = [[0.25, -0.25]; 64];
        let mut expected = input;
        authored.process_block(&mut expected, &R);
        let mut running = input;
        warmed.process_block_with_clock(&mut running, Some(&Clock(0)), &R);
        assert_ne!(running, expected, "historical lane must affect the filter");

        warmed.reset();
        let mut actual = input;
        warmed.process_block(&mut actual, &R);
        assert_eq!(
            actual, expected,
            "reset retained old automated coefficients"
        );
    }
    #[test]
    fn shared_gain_preserves_historical_lane_clock_offsets_and_discontinuities() {
        let mut s = EqState::default();
        let id = NodeId::new(7).unwrap();
        s.add(Node::new(id, 1000.0, 6.0, 0).unwrap()).unwrap();
        s.set_lane(AutomationLane {
            node: id,
            parameter: NodeParameter::Frequency,
            points: vec![
                AutomationPoint {
                    beat: 0,
                    normalized: 0.2,
                },
                AutomationPoint {
                    beat: 4 << 32,
                    normalized: 0.8,
                },
            ],
        })
        .unwrap();
        let f = PreparedEq::automated(s.clone(), 48000, &[OUTPUT_GAIN]).unwrap();
        s.output_gain = 0.5;
        let mut static_settings = s.clone();
        static_settings.lanes.clear();
        let mut without_lane = PreparedEq::new(static_settings, 48000)
            .unwrap()
            .create_runtime()
            .unwrap();
        let mut canonical = PreparedEq::new(s, 48000).unwrap().create_runtime().unwrap();
        let input: Vec<_> = (0..513)
            .map(|i| [(i as f32 * 0.17).sin(), (i as f32 * 0.31).sin() * 0.4])
            .collect();
        let mut expected = input.clone();
        canonical.process_block_with_clock(&mut expected, Some(&Clock(0)), &R);
        let mut unautomated = input.clone();
        without_lane.process_block_with_clock(&mut unautomated, Some(&Clock(0)), &R);
        assert_ne!(
            expected, unautomated,
            "historical lane must actually move the filter"
        );
        for width in [1, 17, 513] {
            let mut actual = input.clone();
            let mut rt = f.create_runtime().unwrap();
            for (i, block) in actual.chunks_mut(width).enumerate() {
                rt.process_block_with_automation(block, Some(&Clock(i * width)), &Held, &R)
                    .unwrap();
            }
            assert_eq!(actual, expected);
            let mut seek = input.clone();
            rt.process_block_with_automation(&mut seek, Some(&Clock(0)), &Held, &R)
                .unwrap();
            assert_eq!(seek, expected);
        }
    }

    #[test]
    fn indexed_lanes_partition_point_descriptor_lookup_and_cadence_chunking_stay_exact() {
        let mut s = EqState::default();
        let n1 = NodeId::new(1).unwrap();
        let n2 = NodeId::new(2).unwrap();
        let n3 = NodeId::new(3).unwrap();
        s.add(Node::new(n1, 180.0, 4.0, 0).unwrap()).unwrap();
        s.add(Node::new(n2, 1200.0, -3.0, 0).unwrap()).unwrap();
        s.add(Node::new(n3, 6000.0, 2.5, 0).unwrap()).unwrap();
        // Add lanes out of node order (n3 before n1) to verify prepare-time sorting/indexing.
        s.set_lane(AutomationLane {
            node: n3,
            parameter: NodeParameter::Enabled,
            points: vec![
                AutomationPoint {
                    beat: 2 << 32,
                    normalized: 0.0,
                },
                AutomationPoint {
                    beat: 4 << 32,
                    normalized: 1.0,
                },
            ],
        })
        .unwrap();
        s.set_lane(AutomationLane {
            node: n1,
            parameter: NodeParameter::Frequency,
            points: vec![
                AutomationPoint {
                    beat: 1 << 32,
                    normalized: 0.2,
                },
                AutomationPoint {
                    beat: 3 << 32,
                    normalized: 0.6,
                },
                AutomationPoint {
                    beat: 5 << 32,
                    normalized: 0.8,
                },
            ],
        })
        .unwrap();

        let n2_freq = n2.parameter(NodeParameter::Frequency).unwrap();
        let prepared = PreparedEq::automated(s, 48_000, &[n2_freq]).unwrap();
        assert!(prepared.parameter_automation_descriptor(n2_freq).is_some());
        assert!(
            prepared
                .parameter_automation_descriptor(OUTPUT_GAIN)
                .is_none()
        );

        // Verify partition_point boundaries: before-first, exact, mid-segment, after-last.
        let before = prepared.configuration.node_at(0, Some(0));
        let at_first = prepared.configuration.node_at(0, Some(1 << 32));
        let mid = prepared.configuration.node_at(0, Some(2 << 32));
        let after = prepared.configuration.node_at(0, Some(9 << 32));
        assert_eq!(before.frequency_hz, at_first.frequency_hz);
        assert!(mid.frequency_hz > at_first.frequency_hz);
        assert!(after.frequency_hz > mid.frequency_hz);
        assert!(!prepared.configuration.node_at(2, Some(3 << 32)).enabled);
        assert!(prepared.configuration.node_at(2, Some(4 << 32)).enabled);

        // Verify chunked node-only automation matches 1-sample delivery bit-for-bit.
        struct Sweep {
            ids: [ParameterId; 1],
            start: usize,
        }
        impl NativeParameterAutomation for Sweep {
            fn parameter_ids(&self) -> &[ParameterId] {
                &self.ids
            }
            fn value_at(&self, _: ParameterId, offset: usize) -> Option<ParameterValue> {
                let step = (self.start + offset) as f32;
                Some(ParameterValue::Float(400.0 + (step % 256.0) * 8.0))
            }
        }
        let input: Vec<_> = (0..260)
            .map(|i| [(i as f32 * 0.13).sin() * 0.5, (i as f32 * 0.29).cos() * 0.5])
            .collect();
        let mut expected = input.clone();
        let mut rt_single = prepared.create_runtime().unwrap();
        for (i, sample) in expected.iter_mut().enumerate() {
            let v = Sweep {
                ids: [n2_freq],
                start: i,
            };
            rt_single
                .process_block_with_automation(
                    std::slice::from_mut(sample),
                    Some(&Clock(i)),
                    &v,
                    &R,
                )
                .unwrap();
        }
        for width in [17_usize, 128, 260] {
            let mut actual = input.clone();
            let mut rt = prepared.create_runtime().unwrap();
            let mut offset = 0;
            for chunk in actual.chunks_mut(width) {
                let v = Sweep {
                    ids: [n2_freq],
                    start: offset,
                };
                rt.process_block_with_automation(chunk, Some(&Clock(offset)), &v, &R)
                    .unwrap();
                offset += chunk.len();
            }
            assert_eq!(actual, expected, "mismatch at chunk width {width}");
        }
    }
}

#[cfg(test)]
mod node_tests;

#[cfg(test)]
mod mode_tests;
