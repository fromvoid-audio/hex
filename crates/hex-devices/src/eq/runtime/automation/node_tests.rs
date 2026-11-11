use super::*;
use hex_lattice::EMPTY_DEVICE_RESOURCES as R;
struct Held {
    id: [ParameterId; 1],
    value: Option<ParameterValue>,
}
impl NativeParameterAutomation for Held {
    fn parameter_ids(&self) -> &[ParameterId] {
        &self.id
    }
    fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
        self.value
    }
}
#[test]
fn cold_held_each_node_field_matches_canonical_at_rate_and_mode_endpoints() {
    for rate in [8000, 192000] {
        for mode in 0..3 {
            for field in NodeParameter::ALL {
                let mut settings = EqState {
                    mode,
                    ..EqState::default()
                };
                let id = NodeId::new(7).unwrap();
                let mut node = Node::new(
                    id,
                    700.0,
                    6.0,
                    if mode == 0 {
                        0
                    } else if mode == 1 {
                        1
                    } else {
                        3
                    },
                )
                .unwrap();
                if field == NodeParameter::Slope {
                    node.kind = NodeKind::LowCut;
                }
                settings.add(node).unwrap();
                let target = match field {
                    NodeParameter::Enabled => ParameterValue::Bool(false),
                    NodeParameter::Kind => ParameterValue::Choice(5),
                    NodeParameter::Frequency => ParameterValue::Float(1700.0),
                    NodeParameter::Gain => ParameterValue::Float(-3.0),
                    NodeParameter::Q => ParameterValue::Float(1.2),
                    NodeParameter::Slope => ParameterValue::Choice(3),
                    NodeParameter::Bank => ParameterValue::Choice(4),
                };
                let parameter = id.parameter(field).unwrap();
                let mut actual = PreparedEq::automated(settings.clone(), rate, &[parameter])
                    .unwrap()
                    .create_runtime();
                settings.node_mut(id).unwrap().set(field, target).unwrap();
                let mut reference = PreparedEq::new(settings, rate).unwrap().create_runtime();
                let input: Vec<_> = (0..257)
                    .map(|i| [(i as f32 * 0.17).sin(), (i as f32 * 0.31).sin() * 0.4])
                    .collect();
                let mut expected = input.clone();
                reference.process_block(&mut expected, &R);
                let mut output = input;
                for block in output.chunks_mut(17) {
                    actual
                        .process_block_with_automation(
                            block,
                            None,
                            &Held {
                                id: [parameter],
                                value: Some(target),
                            },
                            &R,
                        )
                        .unwrap();
                }
                assert_eq!(output, expected, "rate {rate} mode {mode} field {field:?}");
            }
        }
    }
}
fn fixture() -> (EqState, ParameterId) {
    let mut state = EqState::default();
    let id = NodeId::new(7).unwrap();
    state.add(Node::new(id, 700.0, 6.0, 0).unwrap()).unwrap();
    (state, id.parameter(NodeParameter::Gain).unwrap())
}
struct Moving {
    id: [ParameterId; 1],
    start: usize,
    bad: Option<ParameterValue>,
}
impl NativeParameterAutomation for Moving {
    fn parameter_ids(&self) -> &[ParameterId] {
        &self.id
    }
    fn value_at(&self, _: ParameterId, offset: usize) -> Option<ParameterValue> {
        if offset == 17 && self.bad.is_some() {
            return self.bad;
        }
        match self.start + offset {
            0..128 => Some(ParameterValue::Float(9.0)),
            128..320 => None,
            _ => Some(ParameterValue::Float(-3.0)),
        }
    }
}
#[test]
fn moving_nodes_and_base_match_canonical_handoffs_independent_of_partition() {
    let (state, id) = fixture();
    let input = vec![[0.5, -0.25]; 513];
    let mut expected = input.clone();
    let mut previous: Option<Box<dyn NativeProcessorRuntime>> = None;
    for (i, block) in expected.chunks_mut(64).enumerate() {
        let mut next = state.clone();
        next.nodes[0].gain_db = match i {
            0 | 1 => 9.0,
            2..5 => 6.0,
            _ => -3.0,
        };
        let mut runtime = PreparedEq::new(next, 48000).unwrap().create_runtime();
        if let Some(old) = previous.as_ref() {
            runtime.copy_history_from(old.as_ref());
        }
        runtime.process_block(block, &R);
        previous = Some(runtime);
    }
    let factory = PreparedEq::automated(state, 48000, &[id]).unwrap();
    for width in [1, 17, 513] {
        let mut runtime = factory.create_runtime();
        let mut actual = input.clone();
        for (i, block) in actual.chunks_mut(width).enumerate() {
            runtime
                .process_block_with_automation(
                    block,
                    None,
                    &Moving {
                        id: [id],
                        start: i * width,
                        bad: None,
                    },
                    &R,
                )
                .unwrap();
        }
        assert_eq!(actual, expected, "partition {width}");
    }
}
#[test]
fn late_invalid_values_retry_history_bypass_and_reset_preserve_observable_state() {
    let (state, id) = fixture();
    let factory = PreparedEq::automated(state, 48000, &[id]).unwrap();
    let mut primary = factory.create_runtime();
    primary
        .process_block_with_automation(
            &mut [[0.5, -0.25]; 64],
            None,
            &Held {
                id: [id],
                value: Some(ParameterValue::Float(-3.0)),
            },
            &R,
        )
        .unwrap();
    for bad in [
        ParameterValue::Float(f32::NAN),
        ParameterValue::Float(f32::INFINITY),
        ParameterValue::Float(24.1),
        ParameterValue::Bool(true),
    ] {
        let mut scratch = factory.create_runtime();
        scratch.copy_history_from(primary.as_ref());
        assert!(
            scratch
                .process_block_with_automation(
                    &mut [[0.5, -0.25]; 32],
                    None,
                    &Moving {
                        id: [id],
                        start: 64,
                        bad: Some(bad)
                    },
                    &R
                )
                .is_err()
        );
        scratch.copy_history_from(primary.as_ref());
        let mut actual = [[0.5, -0.25]; 32];
        let mut expected = actual;
        let values = Moving {
            id: [id],
            start: 64,
            bad: None,
        };
        scratch
            .process_block_with_automation(&mut actual, None, &values, &R)
            .unwrap();
        primary
            .process_block_with_automation(&mut expected, None, &values, &R)
            .unwrap();
        assert_eq!(actual, expected);
    }
    let mut bypass = factory.create_runtime();
    bypass.copy_history_from(primary.as_ref());
    let mut dry = [[0.5, -0.25]; 37];
    let mut ignored = dry;
    let base = Held {
        id: [id],
        value: None,
    };
    bypass
        .process_bypassed_block_with_automation(&mut dry, None, &base, &R)
        .unwrap();
    assert_eq!(dry, [[0.5, -0.25]; 37]);
    primary
        .process_block_with_automation(&mut ignored, None, &base, &R)
        .unwrap();
    let mut a = [[0.5, -0.25]; 64];
    let mut b = a;
    bypass
        .process_block_with_automation(&mut a, None, &base, &R)
        .unwrap();
    primary
        .process_block_with_automation(&mut b, None, &base, &R)
        .unwrap();
    assert_eq!(a, b);
    bypass.reset();
    let mut fresh = factory.create_runtime();
    bypass
        .process_block_with_automation(&mut a, None, &base, &R)
        .unwrap();
    fresh
        .process_block_with_automation(&mut b, None, &base, &R)
        .unwrap();
    assert_eq!(a, b);
}
#[test]
fn exact_node_grants_refuse_unknown_reserved_legacy_and_ungranted_values() {
    let (mut state, id) = fixture();
    for ids in [
        vec![id, id],
        vec![id, OUTPUT_GAIN],
        vec![ParameterId::from_static(1063)],
        vec![
            NodeId::new(99)
                .unwrap()
                .parameter(NodeParameter::Gain)
                .unwrap(),
        ],
    ] {
        assert!(PreparedEq::automated(state.clone(), 48000, &ids).is_err());
    }
    let factory = PreparedEq::automated(state.clone(), 48000, &[id]).unwrap();
    assert_eq!(
        factory.parameter_automation_descriptor(id),
        Some(descriptor(NodeId::new(7).unwrap(), NodeParameter::Gain).unwrap())
    );
    assert!(!factory.supports_parameter_automation(OUTPUT_GAIN));
    let mut runtime = factory.create_runtime();
    let mut input = [[0.5, -0.25]; 32];
    assert!(
        runtime
            .process_block_with_automation(
                &mut input,
                None,
                &Held {
                    id: [OUTPUT_GAIN],
                    value: Some(ParameterValue::Float(1.0))
                },
                &R
            )
            .is_err()
    );
    assert_eq!(input, [[0.5, -0.25]; 32]);
    state.nodes[0].kind = NodeKind::LowShelf;
    state.nodes[0].legacy_shelf = true;
    for field in [NodeParameter::Kind, NodeParameter::Gain, NodeParameter::Q] {
        assert!(
            PreparedEq::automated(
                state.clone(),
                48000,
                &[state.nodes[0].id.parameter(field).unwrap()]
            )
            .is_err()
        );
    }
    assert!(
        PreparedEq::automated(
            state,
            48000,
            &[NodeId::new(7)
                .unwrap()
                .parameter(NodeParameter::Frequency)
                .unwrap()]
        )
        .is_ok()
    );
}
struct Clock(usize);
impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, offset: usize) -> Option<u64> {
        Some(((self.0 + offset) as u64) << 24)
    }
    fn arrangement_beat_at_offset(&self, offset: usize) -> Option<u64> {
        self.beat_at_offset(offset)
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        Some((self.0 + offset) as u64)
    }
    fn transport_is_running(&self) -> Option<bool> {
        Some(true)
    }
}
#[test]
fn shared_fields_override_legacy_lanes_while_ungranted_lanes_keep_clock_and_seek() {
    let (mut state, id) = fixture();
    let node = state.nodes[0].id;
    for (field, a, b) in [
        (NodeParameter::Gain, 0.75, 0.25),
        (NodeParameter::Frequency, 0.3, 0.7),
    ] {
        state
            .set_lane(AutomationLane {
                node,
                parameter: field,
                points: vec![
                    AutomationPoint {
                        beat: 0,
                        normalized: a,
                    },
                    AutomationPoint {
                        beat: 8 << 32,
                        normalized: b,
                    },
                ],
            })
            .unwrap();
    }
    for value in [None, Some(ParameterValue::Float(-3.0))] {
        let mut canonical = state.clone();
        canonical
            .lanes
            .retain(|lane| lane.parameter != NodeParameter::Gain);
        if let Some(value) = value {
            canonical.nodes[0].set(NodeParameter::Gain, value).unwrap();
        }
        let mut expected = PreparedEq::new(canonical, 48000).unwrap().create_runtime();
        let mut actual = PreparedEq::automated(state.clone(), 48000, &[id])
            .unwrap()
            .create_runtime();
        for start in [0, 17, 34, 1000, 1017] {
            let mut a = [[0.5, -0.25]; 17];
            let mut b = a;
            actual
                .process_block_with_automation(
                    &mut a,
                    Some(&Clock(start)),
                    &Held { id: [id], value },
                    &R,
                )
                .unwrap();
            expected.process_block_with_clock(&mut b, Some(&Clock(start)), &R);
            assert_eq!(a, b, "start {start} value {value:?}");
        }
        assert_eq!(EqState::decode(&state.encode().unwrap()).unwrap(), state);
    }
}

#[test]
fn borrowed_bulk_values_are_used_and_bulk_failure_is_reported() {
    struct Bulk {
        id: [ParameterId; 1],
        refuse: bool,
    }
    impl NativeParameterAutomation for Bulk {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.id
        }
        fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
            panic!("bulk override must be used")
        }
        fn values_at(
            &self,
            offset: usize,
            out: &mut [Option<ParameterValue>],
        ) -> Result<(), DeviceContractError> {
            if out.len() != 1 || (self.refuse && offset == 17) {
                return Err(DeviceContractError::InvalidParameterValue);
            }
            out[0] = Some(ParameterValue::Float(9.0));
            Ok(())
        }
    }
    let (state, id) = fixture();
    let factory = PreparedEq::automated(state, 48000, &[id]).unwrap();
    let mut actual = factory.create_runtime();
    let mut reference = factory.create_runtime();
    let mut a = [[0.5, -0.25]; 32];
    let mut b = a;
    actual
        .process_block_with_automation(
            &mut a,
            None,
            &Bulk {
                id: [id],
                refuse: false,
            },
            &R,
        )
        .unwrap();
    reference
        .process_block_with_automation(
            &mut b,
            None,
            &Held {
                id: [id],
                value: Some(ParameterValue::Float(9.0)),
            },
            &R,
        )
        .unwrap();
    assert_eq!(a, b);
    assert!(
        actual
            .process_block_with_automation(
                &mut a,
                None,
                &Bulk {
                    id: [id],
                    refuse: true
                },
                &R
            )
            .is_err()
    );
}

#[test]
fn shared_enabled_grant_refuses_invalid_disabled_legacy_coefficients_on_control() {
    let (mut state, _) = fixture();
    state.nodes[0].enabled = false;
    state.nodes[0].kind = NodeKind::LowShelf;
    state.nodes[0].legacy_shelf = true;
    state.nodes[0].gain_db = 24.0;
    state.nodes[0].q = 18.0;
    let encoded = state.encode().unwrap();
    assert!(PreparedEq::new(state.clone(), 48000).is_ok());
    let enabled = state.nodes[0].id.parameter(NodeParameter::Enabled).unwrap();
    assert!(EqDefinition::prepare_automation_component(&encoded, 48000, &[enabled]).is_err());
    assert_eq!(state.encode().unwrap(), encoded);
}

#[test]
fn effective_kind_and_bank_history_survive_equivalent_and_shadowed_base_retargets() {
    for (mode, bank) in [(1, 1), (1, 2), (2, 3), (2, 4)] {
        let (mut state, _) = fixture();
        state.mode = mode;
        let id = state.nodes[0].id.parameter(NodeParameter::Bank).unwrap();
        let factory = PreparedEq::automated(state.clone(), 48000, &[id]).unwrap();
        let values = Held {
            id: [id],
            value: Some(ParameterValue::Choice(bank)),
        };
        let mut source = factory.create_runtime();
        source
            .process_block_with_automation(&mut [[0.5, -0.25]; 32], None, &values, &R)
            .unwrap();
        for canonical_bank in [0, if bank == 1 { 2 } else { 1 }] {
            let mut next = state.clone();
            next.nodes[0].bank = canonical_bank;
            let mut candidate = PreparedEq::automated(next, 48000, &[id])
                .unwrap()
                .create_runtime();
            candidate.copy_history_from(source.as_ref());
            let mut uninterrupted = factory.create_runtime();
            uninterrupted.copy_history_from(source.as_ref());
            let mut actual = [[0.5, -0.25]; 64];
            let mut expected = actual;
            candidate
                .process_block_with_automation(&mut actual, None, &values, &R)
                .unwrap();
            uninterrupted
                .process_block_with_automation(&mut expected, None, &values, &R)
                .unwrap();
            assert!(
                actual == expected,
                "mode {mode} effective bank {bank} base bank {canonical_bank}: {:?} != {:?}",
                actual[0],
                expected[0]
            );
        }
    }
    let (state, _) = fixture();
    let id = state.nodes[0].id.parameter(NodeParameter::Kind).unwrap();
    let factory = PreparedEq::automated(state.clone(), 48000, &[id]).unwrap();
    let values = Held {
        id: [id],
        value: Some(ParameterValue::Choice(5)),
    };
    let mut source = factory.create_runtime();
    source
        .process_block_with_automation(&mut [[0.5, -0.25]; 32], None, &values, &R)
        .unwrap();
    let mut next = state;
    next.nodes[0].kind = NodeKind::LowCut;
    let mut candidate = PreparedEq::automated(next, 48000, &[id])
        .unwrap()
        .create_runtime();
    candidate.copy_history_from(source.as_ref());
    let mut uninterrupted = factory.create_runtime();
    uninterrupted.copy_history_from(source.as_ref());
    let mut actual = [[0.5, -0.25]; 64];
    let mut expected = actual;
    candidate
        .process_block_with_automation(&mut actual, None, &values, &R)
        .unwrap();
    uninterrupted
        .process_block_with_automation(&mut expected, None, &values, &R)
        .unwrap();
    assert!(
        actual == expected,
        "effective kind history was reset: {:?} != {:?}",
        actual[0],
        expected[0]
    );
}

#[test]
fn genuine_bank_changes_reset_history_and_kind_changes_keep_canonical_ramps() {
    let (mut state, _) = fixture();
    state.mode = 1;
    state.nodes[0].bank = 1;
    let factory = PreparedEq::new(state.clone(), 48000).unwrap();
    let mut source = factory.create_runtime();
    source.process_block(&mut [[0.5, -0.25]; 32], &R);
    state.nodes[0].bank = 2;
    let factory = PreparedEq::new(state, 48000).unwrap();
    let mut candidate = factory.create_runtime();
    candidate.copy_history_from(source.as_ref());
    let mut fresh = factory.create_runtime();
    let mut actual = [[0.5, -0.25]; 64];
    let mut expected = actual;
    candidate.process_block(&mut actual, &R);
    fresh.process_block(&mut expected, &R);
    assert_eq!(actual, expected);

    let (state, _) = fixture();
    let id = state.nodes[0].id.parameter(NodeParameter::Kind).unwrap();
    let mut source = PreparedEq::new(state.clone(), 48000)
        .unwrap()
        .create_runtime();
    source.process_block(&mut [[0.5, -0.25]; 32], &R);
    let mut candidate = PreparedEq::automated(state.clone(), 48000, &[id])
        .unwrap()
        .create_runtime();
    candidate.copy_history_from(source.as_ref());
    let mut canonical = state;
    canonical.nodes[0].kind = NodeKind::Notch;
    let mut reference = PreparedEq::new(canonical, 48000).unwrap().create_runtime();
    reference.copy_history_from(source.as_ref());
    let mut actual = [[0.5, -0.25]; 64];
    let mut expected = actual;
    candidate
        .process_block_with_automation(
            &mut actual,
            None,
            &Held {
                id: [id],
                value: Some(ParameterValue::Choice(5)),
            },
            &R,
        )
        .unwrap();
    reference.process_block(&mut expected, &R);
    assert_eq!(actual, expected);
}
