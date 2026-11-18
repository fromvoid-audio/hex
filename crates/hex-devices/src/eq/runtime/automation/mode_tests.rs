use super::*;
use hex_lattice::EMPTY_DEVICE_RESOURCES as R;
struct Held(Option<u32>);
impl NativeParameterAutomation for Held {
    fn parameter_ids(&self) -> &[ParameterId] {
        &[MODE]
    }
    fn value_at(&self, _: ParameterId, _: usize) -> Option<ParameterValue> {
        self.0.map(ParameterValue::Choice)
    }
}
fn state(mode: u8) -> EqState {
    let mut state = EqState {
        mode,
        ..EqState::default()
    };
    for bank in 0..5 {
        state
            .add(
                Node::new(
                    NodeId::new(bank as u64 + 1).unwrap(),
                    700.0,
                    bank as f32 * 3.0 - 6.0,
                    bank,
                )
                .unwrap(),
            )
            .unwrap();
    }
    state
}
fn input(frames: usize) -> Vec<StereoFrame> {
    (0..frames)
        .map(|i| {
            [
                (i as f32 * 0.17).sin() * 0.25,
                (i as f32 * 0.29).cos() * 0.13,
            ]
        })
        .collect()
}
#[test]
fn cold_held_modes_match_canonical_at_rate_endpoints_without_consuming_empty_blocks() {
    for rate in [8000, 192000] {
        for mode in 0..3 {
            let mut actual = PreparedEq::automated(state(0), rate, &[MODE])
                .unwrap()
                .create_runtime();
            actual
                .process_block_with_automation(&mut [], None, &Held(Some(2)), &R)
                .unwrap();
            let mut canonical = PreparedEq::new(state(mode), rate).unwrap().create_runtime();
            let mut expected = input(257);
            let mut output = expected.clone();
            canonical.process_block(&mut expected, &R);
            for block in output.chunks_mut(17) {
                actual
                    .process_block_with_automation(block, None, &Held(Some(mode as u32)), &R)
                    .unwrap();
            }
            assert_eq!(output, expected, "rate {rate} mode {mode}");
        }
    }
}
fn render(
    runtime: &mut dyn NativeProcessorRuntime,
    frames: &mut [StereoFrame],
    value: Option<u32>,
    width: usize,
) {
    for block in frames.chunks_mut(width) {
        runtime
            .process_block_with_automation(block, None, &Held(value), &R)
            .unwrap();
    }
}
#[test]
fn every_live_mode_direction_matches_existing_manual_routing_fade() {
    for rate in [8000, 192000] {
        for before in 0..3 {
            for after in 0..3 {
                if before == after {
                    continue;
                }
                let warm = 127;
                let fade = mode_fade_frames(rate) as usize;
                let tone = input(warm + fade + 129);
                let mut actual = PreparedEq::automated(state(before), rate, &[MODE])
                    .unwrap()
                    .create_runtime();
                let mut old = PreparedEq::new(state(before), rate)
                    .unwrap()
                    .create_runtime();
                let mut prefix = tone[..warm].to_vec();
                let mut expected_prefix = prefix.clone();
                render(actual.as_mut(), &mut prefix, Some(before as u32), 17);
                old.process_block(&mut expected_prefix, &R);
                assert_eq!(prefix, expected_prefix);
                let mut expected = PreparedEq::new(state(after), rate)
                    .unwrap()
                    .create_runtime();
                expected.copy_history_from(old.as_ref());
                let mut out = tone[warm..].to_vec();
                let mut reference = out.clone();
                expected.process_block(&mut reference, &R);
                render(actual.as_mut(), &mut out, Some(after as u32), 17);
                assert_eq!(out, reference, "{rate} {before}->{after}");
            }
        }
    }
}
struct Rapid {
    start: usize,
}
impl NativeParameterAutomation for Rapid {
    fn parameter_ids(&self) -> &[ParameterId] {
        &[MODE]
    }
    fn value_at(&self, _: ParameterId, offset: usize) -> Option<ParameterValue> {
        Some(ParameterValue::Choice(if self.start + offset == 0 {
            1
        } else {
            2
        }))
    }
}
#[test]
fn rapid_changes_take_latest_target_after_the_single_outgoing_fade() {
    let rate = 48000;
    let fade = mode_fade_frames(rate) as usize;
    let tone = input(32 + fade * 2 + 73);
    let mut actual = PreparedEq::automated(state(0), rate, &[MODE])
        .unwrap()
        .create_runtime();
    let mut initial = PreparedEq::new(state(0), rate).unwrap().create_runtime();
    let mut a = tone[..32].to_vec();
    let mut b = a.clone();
    render(actual.as_mut(), &mut a, Some(0), 32);
    initial.process_block(&mut b, &R);
    let mut first = PreparedEq::new(state(1), rate).unwrap().create_runtime();
    first.copy_history_from(initial.as_ref());
    let mut expected = tone[32..].to_vec();
    first.process_block(&mut expected[..fade], &R);
    let mut second = PreparedEq::new(state(2), rate).unwrap().create_runtime();
    assert!(!second.transition_busy_from(first.as_ref()));
    second.copy_history_from(first.as_ref());
    second.process_block(&mut expected[fade..], &R);
    let mut out = tone[32..].to_vec();
    for (i, block) in out.chunks_mut(17).enumerate() {
        actual
            .process_block_with_automation(block, None, &Rapid { start: i * 17 }, &R)
            .unwrap();
    }
    assert_eq!(out, expected);
}
#[test]
fn mode_base_handoff_shadowed_base_and_row_removal_keep_effective_routing() {
    let rate = 48000;
    let factory = PreparedEq::automated(state(0), rate, &[MODE]).unwrap();
    let mut source = factory.create_runtime();
    let mut warm = input(127);
    render(source.as_mut(), &mut warm, Some(2), 17);
    for base in [0, 1] {
        let mut candidate = PreparedEq::automated(state(base), rate, &[MODE])
            .unwrap()
            .create_runtime();
        candidate.copy_history_from(source.as_ref());
        let mut reference = factory.create_runtime();
        reference.copy_history_from(source.as_ref());
        let mut a = input(79);
        let mut b = a.clone();
        render(candidate.as_mut(), &mut a, Some(2), 17);
        render(reference.as_mut(), &mut b, Some(2), 17);
        assert_eq!(a, b);
    }
    let mut actual = factory.create_runtime();
    actual.copy_history_from(source.as_ref());
    let mut reference = PreparedEq::new(state(0), rate).unwrap().create_runtime();
    reference.copy_history_from(source.as_ref());
    let mut a = input(513);
    let mut b = a.clone();
    render(actual.as_mut(), &mut a, None, 17);
    reference.process_block(&mut b, &R);
    assert_eq!(a, b);
    let mut removed = PreparedEq::new(state(0), rate).unwrap().create_runtime();
    assert!(!removed.transition_busy_from(source.as_ref()));
    removed.copy_history_from(source.as_ref());
    let mut a = input(513);
    let mut b = a.clone();
    removed.process_block(&mut a, &R);
    let mut base = factory.create_runtime();
    base.copy_history_from(source.as_ref());
    render(base.as_mut(), &mut b, None, 17);
    assert_eq!(a, b);
}
struct Clock(usize);
impl NativeInstrumentClock for Clock {
    fn beat_at_offset(&self, _: usize) -> Option<u64> {
        None
    }
    fn transport_is_running(&self) -> Option<bool> {
        Some(true)
    }
    fn arrangement_frame_at_offset(&self, offset: usize) -> Option<u64> {
        Some((self.0 + offset) as u64)
    }
}
struct Bad {
    wrong: ParameterValue,
}
impl NativeParameterAutomation for Bad {
    fn parameter_ids(&self) -> &[ParameterId] {
        &[MODE]
    }
    fn value_at(&self, _: ParameterId, offset: usize) -> Option<ParameterValue> {
        Some(if offset == 17 {
            self.wrong
        } else {
            ParameterValue::Choice(1)
        })
    }
}
#[test]
fn seek_retry_bypass_reset_and_effective_audition_are_defined() {
    use hex_lattice::ProcessorAuditionSelection;
    let factory = PreparedEq::automated(state(0), 48000, &[MODE]).unwrap();
    let mut primary = factory.create_runtime();
    let mut warm = input(32);
    render(primary.as_mut(), &mut warm, Some(2), 17);
    assert!(
        primary
            .transient_audition()
            .unwrap()
            .can_select(ProcessorAuditionSelection { lane: 0, index: 4 })
    );
    assert!(
        !primary
            .transient_audition()
            .unwrap()
            .can_select(ProcessorAuditionSelection { lane: 0, index: 1 })
    );
    for wrong in [
        ParameterValue::Choice(3),
        ParameterValue::Float(f32::NAN),
        ParameterValue::Bool(true),
    ] {
        let mut scratch = factory.create_runtime();
        scratch.copy_history_from(primary.as_ref());
        let mut rejected = input(32);
        assert!(
            scratch
                .process_block_with_automation(&mut rejected, None, &Bad { wrong }, &R)
                .is_err()
        );
        scratch.copy_history_from(primary.as_ref());
        let mut a = input(64);
        let mut b = a.clone();
        render(scratch.as_mut(), &mut a, Some(1), 17);
        render(primary.as_mut(), &mut b, Some(1), 17);
        assert_eq!(a, b);
    }
    let mut actual = factory.create_runtime();
    actual.copy_history_from(primary.as_ref());
    let mut dry = input(37);
    let original = dry.clone();
    let mut ignored = dry.clone();
    actual
        .process_bypassed_block_with_automation(&mut dry, None, &Held(Some(0)), &R)
        .unwrap();
    render(primary.as_mut(), &mut ignored, Some(0), 37);
    assert_eq!(dry, original);
    let mut a = input(64);
    let mut b = a.clone();
    render(actual.as_mut(), &mut a, Some(0), 17);
    render(primary.as_mut(), &mut b, Some(0), 17);
    assert_eq!(a, b);
    let mut a = input(64);
    actual
        .process_block_with_automation(&mut a, Some(&Clock(1000)), &Held(Some(1)), &R)
        .unwrap();
    let mut expected = PreparedEq::new(state(1), 48000).unwrap().create_runtime();
    let mut b = input(64);
    expected.process_block_with_clock(&mut b, Some(&Clock(1000)), &R);
    assert_eq!(a, b);
    actual.reset();
    let mut a = input(64);
    let mut b = a.clone();
    render(actual.as_mut(), &mut a, Some(2), 17);
    let mut fresh = PreparedEq::new(state(2), 48000).unwrap().create_runtime();
    fresh.process_block(&mut b, &R);
    assert_eq!(a, b);
}

#[test]
fn plain_handoff_freezes_active_audition_and_carries_an_inflight_mode_fade() {
    use hex_lattice::ProcessorAuditionSelection;
    let rate = 48000;
    let mut old = PreparedEq::new(state(0), rate).unwrap().create_runtime();
    old.transient_audition()
        .unwrap()
        .select(Some(ProcessorAuditionSelection { lane: 0, index: 1 }));
    let mut warm = input(319);
    old.process_block(&mut warm, &R);
    let factory = PreparedEq::automated(state(0), rate, &[MODE]).unwrap();
    let mut actual = factory.create_runtime();
    actual.copy_history_from(old.as_ref());
    let mut reference = PreparedEq::new(state(2), rate).unwrap().create_runtime();
    reference.copy_history_from(old.as_ref());
    let mut first = input(17);
    let mut expected = first.clone();
    render(actual.as_mut(), &mut first, Some(2), 17);
    reference.process_block(&mut expected, &R);
    assert_eq!(first, expected);
    assert_eq!(actual.transient_audition_requested_selection(), None);
    let mut checkpoint = factory.create_runtime();
    checkpoint.copy_history_from(actual.as_ref());
    let mut out = input(513);
    let mut expected = out.clone();
    render(checkpoint.as_mut(), &mut out, Some(2), 17);
    reference.process_block(&mut expected, &R);
    assert_eq!(out, expected);
}
#[test]
fn audition_queries_follow_effective_node_fields_and_reset_restores_base_routing() {
    use hex_lattice::ProcessorAuditionSelection;
    struct Values {
        ids: [ParameterId; 3],
        enabled: bool,
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, id: ParameterId, _: usize) -> Option<ParameterValue> {
            Some(if id == MODE {
                ParameterValue::Choice(2)
            } else if id == self.ids[1] {
                ParameterValue::Bool(self.enabled)
            } else {
                ParameterValue::Choice(3)
            })
        }
    }
    let node = NodeId::new(1).unwrap();
    let mut values = Values {
        ids: [
            MODE,
            node.parameter(NodeParameter::Enabled).unwrap(),
            node.parameter(NodeParameter::Bank).unwrap(),
        ],
        enabled: false,
    };
    let mut runtime = PreparedEq::automated(state(0), 48000, &values.ids)
        .unwrap()
        .create_runtime();
    let selection = ProcessorAuditionSelection { lane: 0, index: 1 };
    runtime
        .process_block_with_automation(&mut input(1), None, &values, &R)
        .unwrap();
    assert!(!runtime.transient_audition().unwrap().can_select(selection));
    runtime.reset();
    assert!(runtime.transient_audition().unwrap().can_select(selection));
    values.enabled = true;
    runtime
        .process_block_with_automation(&mut input(64), None, &values, &R)
        .unwrap();
    assert!(runtime.transient_audition().unwrap().can_select(selection));
    runtime.reset();
    assert!(
        runtime.transient_audition().unwrap().can_select(selection),
        "reset must expose canonical stereo bank before the next render"
    );
}
#[test]
fn routing_changes_preserve_the_existing_node_target_cadence() {
    struct Values {
        ids: [ParameterId; 2],
        start: usize,
        gain_frame: usize,
    }
    impl NativeParameterAutomation for Values {
        fn parameter_ids(&self) -> &[ParameterId] {
            &self.ids
        }
        fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
            let frame = self.start + offset;
            Some(if id == MODE {
                ParameterValue::Choice(u32::from(frame >= 33))
            } else {
                ParameterValue::Float(if frame >= self.gain_frame { 12.0 } else { 0.0 })
            })
        }
    }
    let mut settings = EqState::default();
    let node = NodeId::new(1).unwrap();
    settings
        .add(Node::new(node, 700.0, 0.0, 1).unwrap())
        .unwrap();
    let mut values = Values {
        ids: [MODE, node.parameter(NodeParameter::Gain).unwrap()],
        start: 0,
        gain_frame: 32,
    };
    let factory = PreparedEq::automated(settings, 48000, &values.ids).unwrap();
    let mut first = factory.create_runtime();
    let mut second = factory.create_runtime();
    let mut actual = input(257);
    let mut expected = actual.clone();
    for (index, (a, b)) in actual
        .chunks_mut(17)
        .zip(expected.chunks_mut(17))
        .enumerate()
    {
        values.start = index * 17;
        values.gain_frame = 32;
        first
            .process_block_with_automation(a, None, &values, &R)
            .unwrap();
        values.gain_frame = 64;
        second
            .process_block_with_automation(b, None, &values, &R)
            .unwrap();
    }
    assert!(
        actual.iter().zip(&expected).all(|(a, b)| a == b),
        "a mode change must not sample a node target before the existing 64-frame cadence"
    );
}
