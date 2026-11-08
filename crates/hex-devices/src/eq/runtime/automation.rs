//! Exact shared grants resolve stable node identities on control. Borrowed typed
//! values are validated at every original frame offset; node targets refresh at
//! the existing 64-frame coefficient cadence and use its existing ramp/history law.
//! Cold held targets start canonically. Missing values select immutable Base.
//! Granted fields override historical opaque lanes, including Base; ungranted
//! historical fields still use their original clock. No authored lane is erased.
//! This module owns no curves, clock, publication or resource lifecycle.
use super::*;
use hex_lattice::NativeParameterAutomation;
use hex_lattice_contract::MAX_PARAMETERS_PER_DEVICE;

#[derive(Clone, Copy)]
pub(super) struct Grant {
    pub(super) descriptor: ParameterDescriptor,
    node: Option<(usize, NodeParameter)>,
}
impl Grant {
    pub(super) fn prepare(
        settings: &EqState,
        ids: &[ParameterId],
    ) -> Result<Vec<Self>, DeviceContractError> {
        if ids.len() > MAX_PARAMETERS_PER_DEVICE || ids.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(DeviceContractError::InvalidParameterValue);
        }
        ids.iter()
            .map(|&id| {
                if id == OUTPUT_GAIN {
                    return Ok(Self {
                        descriptor: DESCRIPTORS[201],
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
impl EqRuntime {
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
        for (slot, id) in bindings.iter_mut().zip(ids) {
            *slot = self
                .configuration
                .parameters
                .binary_search_by_key(id, |grant| grant.descriptor.id)
                .map_err(|_| DeviceContractError::UnsupportedProcessingClass)?;
        }
        let mut nodes = Nodes::new(&self.configuration);
        let has_nodes = self
            .configuration
            .parameters
            .iter()
            .any(|grant| grant.node.is_some());
        let mut readings = [None; MAX_PARAMETERS_PER_DEVICE];
        for (offset, frame) in frames.iter_mut().enumerate() {
            values.values_at(offset, &mut readings[..ids.len()])?;
            let mut gain = self.configuration.gain;
            let mut authored_gain = false;
            for (&reading, &binding) in readings[..ids.len()].iter().zip(&bindings) {
                let grant = self.configuration.parameters[binding];
                if let Some(value) = reading {
                    grant.descriptor.validate(value)?;
                }
                if let Some((index, field)) = grant.node {
                    if let Some(patch) = nodes.patches[index].as_mut() {
                        patch.node.set(
                            field,
                            reading.unwrap_or_else(|| self.configuration.nodes[index].value(field)),
                        )?;
                    }
                } else if let Some(ParameterValue::Float(value)) = reading {
                    gain = f64::from(value);
                    authored_gain = true;
                }
            }
            if authored_gain && self.pristine {
                self.gain = gain;
            }
            self.process_at(
                std::slice::from_mut(frame),
                clock,
                bypassed,
                offset,
                gain,
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
        rt: &mut dyn NativeProcessorRuntime,
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
                let mut actual = f.create_runtime();
                actual
                    .process_block_with_automation(&mut [], None, &Values::held(Some(0.5)), &R)
                    .unwrap();
                s.output_gain = 0.5;
                let mut canonical = PreparedEq::new(s, rate).unwrap().create_runtime();
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
            let mut rt = f.create_runtime();
            let mut v = Values::held(None);
            v.moving = true;
            assert_eq!(render(rt.as_mut(), &input, v, width), expected);
        }
    }
    #[test]
    fn checkpoint_retry_and_live_base_handoff_preserve_gain_progress_and_bypass() {
        let f = PreparedEq::automated(EqState::default(), 48000, &[OUTPUT_GAIN]).unwrap();
        let mut primary = f.create_runtime();
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
            let mut scratch = f.create_runtime();
            scratch.copy_history_from(primary.as_ref());
            let mut values = Values::held(Some(0.75));
            values.bad = Some(bad);
            assert!(
                scratch
                    .process_block_with_automation(&mut [[0.5, -0.25]; 32], None, &values, &R)
                    .is_err()
            );
            scratch.copy_history_from(primary.as_ref());
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
            .create_runtime();
        candidate.copy_history_from(primary.as_ref());
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
        let mut clone = f.create_runtime();
        clone.copy_history_from(candidate.as_ref());
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
            &[MODE],
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
            !EQ_DEFINITION
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
                .supports_parameter_automation(OUTPUT_GAIN)
        );
        assert!(
            !PreparedEq::new(EqState::default(), 48000)
                .unwrap()
                .supports_parameter_automation(OUTPUT_GAIN)
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
            .create_runtime();
        let mut canonical = PreparedEq::new(s, 48000).unwrap().create_runtime();
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
            let mut rt = f.create_runtime();
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
}

#[cfg(test)]
mod node_tests;
