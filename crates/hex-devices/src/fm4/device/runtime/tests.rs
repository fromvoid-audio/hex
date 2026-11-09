mod retarget;
use super::*;
use hex_lattice::{
    BorrowedDeviceResources, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
};
const LIMITS: NativeInstrumentRuntimeLimits = NativeInstrumentRuntimeLimits {
    max_pair_bytes: 2 * 1024 * 1024,
    max_events_per_block: EVENTS,
    max_active_occurrences: VOICES,
    max_tail_frames: 192000 * 120,
};
fn on(id: u64, offset: usize, pitch: u8, velocity: u8) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOn {
        frame_offset: offset,
        occurrence_id: id,
        pitch,
        velocity,
    }
}
fn prepared(
    rate: u32,
    settings: Fm4Settings,
    tuning: &hex_lattice::TuningTable,
) -> (
    Box<dyn PreparedMutableInstrumentRuntime>,
    Box<dyn PreparedNativeInstrument>,
) {
    let state = settings.to_state().unwrap();
    let context = hex_lattice::DevicePreparationContext {
        sample_rate: rate,
        tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    (
        FM4_DEFINITION
            .prepare_carrier_runtime(&state, &context)
            .unwrap(),
        FM4_DEFINITION.prepare_instrument(&state, &context).unwrap(),
    )
}
#[test]
fn ordinary_voice_is_bit_exact_to_inline_source_across_rates_tuning_release_and_partitions() {
    let resources = BorrowedDeviceResources(&[]);
    let mut frequencies = *hex_lattice::TuningTable::default().frequencies();
    frequencies[60] *= 1.013;
    for tuning in [
        hex_lattice::TuningTable::default(),
        hex_lattice::TuningTable::new(frequencies).unwrap(),
    ] {
        for rate in [8000, 44100, 192000] {
            let mut settings = Fm4Settings::defaults();
            settings.envelope = [0.001, 0.002, 0.7, 0.001];
            for velocity in [1, 93, 127] {
                let (factory, source) = prepared(rate, settings, &tuning);
                let note = source.prepare_note(60, velocity).unwrap();
                let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
                let mut frames = vec![[0.0; 2]; 640];
                pair.begin_block();
                pair.process_block(&[on(1, 0, 60, velocity)], &mut frames[..97], &resources)
                    .unwrap();
                pair.process_block(&[], &mut frames[97..200], &resources)
                    .unwrap();
                pair.process_block(
                    &[NativeInstrumentEvent::NoteOff {
                        frame_offset: 0,
                        occurrence_id: 1,
                    }],
                    &mut frames[200..],
                    &resources,
                )
                .unwrap();
                pair.commit_block();
                for (age, actual) in frames.into_iter().enumerate() {
                    assert_eq!(
                        actual,
                        note.sample(age as u64, 200, &resources),
                        "rate {rate}, age {age}, velocity {velocity}"
                    );
                }
            }
        }
    }
}

struct Values {
    weights: [Option<f32>; 4],
    bad_at: Option<usize>,
}
impl NativeParameterAutomation for Values {
    fn parameter_ids(&self) -> &[ParameterId] {
        const IDS: [ParameterId; 4] = [
            parameter::carrier(1),
            parameter::carrier(2),
            parameter::carrier(3),
            parameter::carrier(4),
        ];
        &IDS
    }
    fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
        if self.bad_at == Some(offset) {
            return Some(ParameterValue::Float(f32::NAN));
        }
        id.value()
            .checked_sub(29)
            .and_then(|index| self.weights.get(index as usize))
            .copied()
            .flatten()
            .map(ParameterValue::Float)
    }
}
fn automated(
    pair: &mut NativeInstrumentRuntimePair,
    events: &[NativeInstrumentEvent],
    frames: &mut [StereoFrame],
    values: &Values,
) -> Result<(), Error> {
    pair.process_block_with_clock_and_automation(
        events,
        frames,
        None,
        &[],
        values,
        &BorrowedDeviceResources(&[]),
    )
}
#[test]
fn oscillator_phase_changes_at_sample_offsets_and_returns_to_authored_after_handoff() {
    struct Phases<'a>(&'a [f32]);
    impl NativeParameterAutomation for Phases<'_> {
        fn parameter_ids(&self) -> &[ParameterId] {
            const IDS: [ParameterId; 1] = [parameter::oscillator_phase(2)];
            &IDS
        }
        fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
            if id != parameter::oscillator_phase(2) {
                return None;
            }
            self.0.get(offset).copied().map(ParameterValue::Float)
        }
        fn has_sample_rate_modulation(&self, id: ParameterId) -> bool {
            id == parameter::oscillator_phase(2)
        }
    }
    let resources = BorrowedDeviceResources(&[]);
    let tuning = hex_lattice::TuningTable::default();
    let mut settings = Fm4Settings::defaults();
    settings.envelope = [0.0, 0.0, 1.0, 0.02];
    // These valid authored turns quantize to the same DSP phase as zero.
    let authored_turns = f32::MIN_POSITIVE;
    settings.oscillators[1][2] = authored_turns;
    settings.modulation[1][0] = 0.25;
    let (factory, instrument) = prepared(48_000, settings, &tuning);
    let authored = instrument.prepare_note(60, 127).unwrap();
    let expected = [0.0, 0.25, 0.5].map(|phase| {
        let mut settings = settings;
        settings.oscillators[1][2] = phase;
        prepared(48_000, settings, &tuning)
            .1
            .prepare_note(60, 127)
            .unwrap()
    });
    let phase_id = parameter::oscillator_phase(2);
    let phase = [0.0, 0.25, 0.5, 0.0, 0.25, 0.5, 0.0, 0.25, 0.5];
    let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    let mut output = [[0.0; 2]; 9];
    pair.begin_block();
    pair.process_block_with_clock_and_automation(
        &[on(1, 0, 60, 127)],
        &mut output[..4],
        None,
        &[],
        &Phases(&phase[..4]),
        &resources,
    )
    .unwrap();
    pair.process_block_with_clock_and_automation(
        &[],
        &mut output[4..],
        None,
        &[],
        &Phases(&phase[4..]),
        &resources,
    )
    .unwrap();
    pair.commit_block();
    for (age, frame) in output.into_iter().enumerate() {
        assert_eq!(
            frame,
            expected[age % 3].sample(age as u64, u64::MAX, &resources)
        );
    }
    let ids = TARGETS.map(|target| target.id);
    let phase_index = ids.iter().position(|&id| id == phase_id).unwrap();
    let mut values = [None; TARGETS.len()];
    pair.copy_committed_parameter_targets(&ids, &mut values)
        .unwrap();
    assert_eq!(values[phase_index], Some(ParameterValue::Float(0.5)));
    let mut successor = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    successor.inherit_from(&pair).unwrap();
    let mut restored = [[0.0; 2]; 5];
    successor.begin_block();
    successor
        .process_block(&[], &mut restored, &resources)
        .unwrap();
    successor.commit_block();
    for (offset, frame) in restored.into_iter().enumerate() {
        assert_eq!(
            frame,
            authored.sample((output.len() + offset) as u64, u64::MAX, &resources)
        );
    }
    successor
        .copy_committed_parameter_targets(&ids, &mut values)
        .unwrap();
    assert_eq!(
        values[phase_index],
        Some(ParameterValue::Float(authored_turns))
    );
}

#[test]
fn oscillator_level_reveals_an_fm_ancestor_even_with_a_pressure_revealed_carrier() {
    use std::num::NonZeroU64;

    struct Levels;
    impl NativeParameterAutomation for Levels {
        fn parameter_ids(&self) -> &[ParameterId] {
            const IDS: [ParameterId; 1] = [parameter::oscillator_level(2)];
            &IDS
        }

        fn value_at(&self, id: ParameterId, offset: usize) -> Option<ParameterValue> {
            (id == parameter::oscillator_level(2)).then_some(ParameterValue::Float(
                if offset.is_multiple_of(2) { 0.0 } else { 0.5 },
            ))
        }

        fn has_sample_rate_modulation(&self, id: ParameterId) -> bool {
            id == parameter::oscillator_level(2)
        }
    }

    let tuning = hex_lattice::TuningTable::default();
    let resources = BorrowedDeviceResources(&[]);
    let mut settings = Fm4Settings::defaults();
    settings.envelope = [0.0, 0.0, 1.0, 0.02];
    settings.oscillators[1][1] = 0.0;
    settings.modulation[1][0] = 0.25;
    let oracle = [0.0, 0.5].map(|level| {
        let mut authored = settings;
        authored.oscillators[1][1] = level;
        let (_, instrument) = prepared(48_000, authored, &tuning);
        instrument.prepare_note(60, 127).unwrap()
    });
    for use_pressure in [false, true] {
        let mut authored = settings;
        if use_pressure {
            authored.carriers = [0.0; 4];
        }
        let (mut factory, instrument) = prepared(48_000, authored, &tuning);
        let mut events = vec![on(1, 0, 60, 127)];
        if use_pressure {
            let carrier = CARRIERS[0];
            let route = PreparedPressureRoute::new(
                carrier,
                modulation_semantics(carrier.id).unwrap(),
                0.5,
                [0.0, 1.0],
            )
            .unwrap();
            factory = instrument
                .prepare_mutable_note_pressure(
                    hex_lattice_contract::ModulationSourceId::new(1).unwrap(),
                    Some(route),
                )
                .unwrap();
            events.push(NativeInstrumentEvent::NotePressure {
                frame_offset: 0,
                occurrence_id: NonZeroU64::new(1).unwrap(),
                value: 127,
            });
        }
        let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
        let mut frames = [[0.0; 2]; 138];
        pair.begin_block();
        pair.process_block_with_clock_and_automation(
            &events,
            &mut frames,
            None,
            &[],
            &Levels,
            &resources,
        )
        .unwrap();
        pair.commit_block();
        for (index, actual) in frames.into_iter().enumerate() {
            assert_eq!(
                actual,
                oracle[index % 2].sample(index as u64, u64::MAX, &resources),
                "pressure={use_pressure}, frame {index}",
            );
        }
        let mut effective = [None; TARGETS.len()];
        pair.copy_committed_parameter_targets(&TARGETS.map(|target| target.id), &mut effective)
            .unwrap();
        assert_eq!(effective[2], Some(ParameterValue::Float(0.5)));
    }
}

#[test]
fn carrier_automation_reveals_inaudible_operator_on_held_note_then_restores_base() {
    let rate = 48000;
    let tuning = hex_lattice::TuningTable::default();
    let settings = Fm4Settings::defaults();
    let (factory, original) = prepared(rate, settings, &tuning);
    let base = original.prepare_note(60, 97).unwrap();
    let mut target = settings;
    target.carriers[1] = 1.0;
    let (_, target) = prepared(rate, target, &tuning);
    let target = target.prepare_note(60, 97).unwrap();
    let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    pair.begin_block();
    let mut initial = [[0.0; 2]; 123];
    pair.process_block(
        &[on(7, 0, 60, 97)],
        &mut initial,
        &BorrowedDeviceResources(&[]),
    )
    .unwrap();
    pair.commit_block();
    for (age, actual) in initial.iter().enumerate() {
        assert_eq!(
            *actual,
            base.sample(age as u64, u64::MAX, &BorrowedDeviceResources(&[]))
        );
    }
    pair.begin_block();
    let mut changed = [[0.0; 2]; 300];
    automated(
        &mut pair,
        &[],
        &mut changed,
        &Values {
            weights: [None, Some(1.0), None, None],
            bad_at: None,
        },
    )
    .unwrap();
    pair.commit_block();
    for (offset, actual) in changed.iter().enumerate().skip(239) {
        assert_eq!(
            *actual,
            target.sample(123 + offset as u64, u64::MAX, &BorrowedDeviceResources(&[]))
        );
    }
    assert_ne!(
        changed[299],
        base.sample(422, u64::MAX, &BorrowedDeviceResources(&[]))
    );
    pair.begin_block();
    let mut restored = [[0.0; 2]; 300];
    automated(
        &mut pair,
        &[],
        &mut restored,
        &Values {
            weights: [None; 4],
            bad_at: None,
        },
    )
    .unwrap();
    pair.commit_block();
    for (offset, actual) in restored.iter().enumerate().skip(239) {
        assert_eq!(
            *actual,
            base.sample(423 + offset as u64, u64::MAX, &BorrowedDeviceResources(&[]))
        );
    }
}
#[test]
fn cold_values_are_direct_and_rollback_partition_and_history_copy_are_exact() {
    let (factory, _) = prepared(
        8000,
        Fm4Settings::defaults(),
        &hex_lattice::TuningTable::default(),
    );
    let mut whole = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    let mut split = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    let values = Values {
        weights: [Some(0.0), Some(1.0), None, None],
        bad_at: None,
    };
    whole.begin_block();
    let mut expected = [[0.0; 2]; 180];
    automated(&mut whole, &[on(1, 0, 60, 127)], &mut expected, &values).unwrap();
    let mut target_settings = Fm4Settings::defaults();
    target_settings.carriers = [0.0, 1.0, 0.0, 0.0];
    let (_, target) = prepared(8000, target_settings, &hex_lattice::TuningTable::default());
    let target = target.prepare_note(60, 127).unwrap();
    for (age, actual) in expected.iter().enumerate() {
        assert_eq!(
            *actual,
            target.sample(age as u64, u64::MAX, &BorrowedDeviceResources(&[]))
        );
    }
    whole.rollback_block();
    whole.begin_block();
    let mut retry = [[0.0; 2]; 180];
    automated(&mut whole, &[on(1, 0, 60, 127)], &mut retry, &values).unwrap();
    assert_eq!(retry, expected);
    whole.commit_block();
    split.begin_block();
    let mut actual = [[0.0; 2]; 180];
    automated(&mut split, &[on(1, 0, 60, 127)], &mut actual[..31], &values).unwrap();
    automated(&mut split, &[], &mut actual[31..], &values).unwrap();
    split.commit_block();
    assert_eq!(actual, expected);
    assert!(split.can_inherit_from(&whole));
    split.inherit_from(&whole).unwrap();
    whole.begin_block();
    split.begin_block();
    let mut a = [[0.0; 2]; 51];
    let mut b = a;
    automated(&mut whole, &[], &mut a, &values).unwrap();
    automated(&mut split, &[], &mut b, &values).unwrap();
    assert_eq!(a, b);
    whole.reset();
    let mut ids = [0; VOICES];
    assert_eq!(whole.active_occurrence_ids(&mut ids).unwrap(), 0);
}
#[test]
fn capacity_and_late_malformed_values_refuse_without_pcm_or_voice_changes() {
    let (factory, _) = prepared(
        48000,
        Fm4Settings::defaults(),
        &hex_lattice::TuningTable::default(),
    );
    let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    pair.begin_block();
    let events = (1..=64).map(|id| on(id, 0, 60, 100)).collect::<Vec<_>>();
    let mut frames = [[0.0; 2]; 2];
    pair.process_block(&events, &mut frames, &BorrowedDeviceResources(&[]))
        .unwrap();
    pair.commit_block();
    pair.begin_block();
    let mut ids = [0; VOICES];
    assert_eq!(pair.active_occurrence_ids(&mut ids).unwrap(), VOICES);
    assert_eq!(ids, std::array::from_fn(|index| index as u64 + 1));
    let mut refused = [[7.0; 2]; 32];
    assert_eq!(
        pair.process_block(
            &[on(65, 1, 60, 100)],
            &mut refused,
            &BorrowedDeviceResources(&[])
        ),
        Err(Error::EventCapacityExceeded)
    );
    assert_eq!(
        automated(
            &mut pair,
            &[],
            &mut refused,
            &Values {
                weights: [Some(0.0); 4],
                bad_at: Some(31)
            }
        ),
        Err(Error::InvalidParameterAutomation)
    );
    assert_eq!(refused, [[7.0; 2]; 32]);
    let mut after = [0; VOICES];
    pair.active_occurrence_ids(&mut after).unwrap();
    assert_eq!(ids, after);
}
#[test]
fn occurrence_gain_is_applied_after_original_velocity_law_and_released_ids_retire() {
    let mut settings = Fm4Settings::defaults();
    settings.envelope = [0.0, 0.0, 1.0, 0.001];
    let (factory, source) = prepared(8000, settings, &hex_lattice::TuningTable::default());
    let note = source.prepare_note(60, 23).unwrap();
    let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    let resources = BorrowedDeviceResources(&[]);
    let gain = NativeInstrumentOccurrenceGain::new(8, 0.25, 0.75, 32, 0).unwrap();
    pair.begin_block();
    let mut frames = [[0.0; 2]; 32];
    pair.process_block_with_clock_and_occurrence_gains(
        &[
            on(8, 0, 60, 23),
            NativeInstrumentEvent::NoteOff {
                frame_offset: 12,
                occurrence_id: 8,
            },
        ],
        &mut frames,
        None,
        &[gain],
        &resources,
    )
    .unwrap();
    pair.commit_block();
    for (age, actual) in frames.iter().enumerate() {
        let expected = (f64::from(note.sample(age as u64, 12, &resources)[0])
            * gain.value_at_offset(age as u64)) as f32;
        assert_eq!(*actual, [expected; 2]);
    }
    assert!(pair.committed_is_quiescent());
    let mut ids = [0; VOICES];
    assert_eq!(pair.active_occurrence_ids(&mut ids).unwrap(), 0);
}

#[test]
fn sparse_high_slot_and_reused_low_slot_match_source_and_partition_history() {
    let mut settings = Fm4Settings::defaults();
    settings.envelope = [0.0, 0.0, 1.0, 0.0];
    let tuning = hex_lattice::TuningTable::default();
    let (factory, inline) = prepared(48000, settings, &tuning);
    let mut whole = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    let mut split = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    let resources = BorrowedDeviceResources(&[]);
    let events = (1..=64).map(|id| on(id, 0, 60, 100)).collect::<Vec<_>>();
    for pair in [&mut whole, &mut split] {
        pair.begin_block();
        pair.process_block(&events, &mut [[0.0; 2]; 1], &resources)
            .unwrap();
        pair.commit_block();
    }
    let mut events = (1..64)
        .map(|id| NativeInstrumentEvent::NoteOff {
            frame_offset: 0,
            occurrence_id: id,
        })
        .collect::<Vec<_>>();
    events.push(on(65, 17, 60, 100));
    events.push(NativeInstrumentEvent::NoteOff {
        frame_offset: 29,
        occurrence_id: 64,
    });
    whole.begin_block();
    split.begin_block();
    let mut expected = [[0.0; 2]; 64];
    let mut actual = expected;
    whole
        .process_block(&events, &mut expected, &resources)
        .unwrap();
    split
        .process_block(&events[..63], &mut actual[..17], &resources)
        .unwrap();
    let remaining = [
        on(65, 0, 60, 100),
        NativeInstrumentEvent::NoteOff {
            frame_offset: 12,
            occurrence_id: 64,
        },
    ];
    split
        .process_block(&remaining, &mut actual[17..], &resources)
        .unwrap();
    assert_eq!(actual, expected);
    let note = inline.prepare_note(60, 100).unwrap();
    for (offset, actual) in actual.iter().enumerate() {
        let old = if offset < 29 {
            note.sample(offset as u64 + 1, u64::MAX, &resources)[0]
        } else {
            0.0
        };
        let new = if offset >= 17 {
            note.sample((offset - 17) as u64, u64::MAX, &resources)[0]
        } else {
            0.0
        };
        assert_eq!(*actual, [(f64::from(old) + f64::from(new)) as f32; 2]);
    }
    whole.commit_block();
    split.rollback_block();
    let mut ids = [0; VOICES];
    assert_eq!(whole.active_occurrence_ids(&mut ids).unwrap(), 1);
    assert_eq!(ids[0], 65);
}

#[test]
fn committed_target_snapshot_finishes_existing_ramp_and_preserves_resolved_base_on_retarget() {
    let mut settings = Fm4Settings::defaults();
    settings.carriers = [0.75, -0.25, 0.5, 0.0];
    settings.envelope = [0.0, 0.0, 1.0, 0.02];
    let tuning = hex_lattice::TuningTable::default();
    let (factory, _) = prepared(48000, settings, &tuning);
    let mut original = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    let mut frozen = NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap();
    let read_carriers = |pair: &NativeInstrumentRuntimePair| {
        let ids = TARGETS.map(|descriptor| descriptor.id);
        let mut values = [None; TARGETS.len()];
        pair.copy_committed_parameter_targets(&ids, &mut values)
            .unwrap();
        <[Option<ParameterValue>; 4]>::try_from(&values[values.len() - CARRIERS.len()..]).unwrap()
    };
    let base = settings
        .carriers
        .map(|value| Some(ParameterValue::Float(value)));
    let targets = Values {
        weights: [Some(-0.25), Some(0.5), None, None],
        bad_at: None,
    };
    for pair in [&mut original, &mut frozen] {
        assert!(pair.supports_committed_parameter_targets());
        let copied = read_carriers(pair);
        assert_eq!(
            copied, base,
            "authored Base differs from descriptor defaults"
        );
        pair.begin_block();
        automated(
            pair,
            &[on(1, 0, 60, 100)],
            &mut [[0.0; 2]; 64],
            &Values {
                weights: [None; 4],
                bad_at: None,
            },
        )
        .unwrap();
        pair.commit_block();
        pair.begin_block();
        automated(
            pair,
            &[NativeInstrumentEvent::NoteOff {
                frame_offset: 8,
                occurrence_id: 1,
            }],
            &mut [[0.0; 2]; 16],
            &targets,
        )
        .unwrap();
        assert_eq!(
            read_carriers(pair),
            base,
            "open speculative ramp is not committed"
        );
        pair.commit_block();
    }
    let copied = read_carriers(&frozen);
    assert_eq!(
        copied,
        [-0.25, 0.5, 0.5, 0.0].map(|value| Some(ParameterValue::Float(value)))
    );
    let frozen_values = Values {
        weights: copied.map(|value| match value {
            Some(ParameterValue::Float(value)) => Some(value),
            _ => panic!("complete float target"),
        }),
        bad_at: None,
    };
    let mut next_settings = settings;
    next_settings.carriers = [0.125; 4];
    let (next_factory, _) = prepared(48000, next_settings, &tuning);
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        next_factory.prepare_history_retarget_target().unwrap()
    else {
        panic!("retarget proof")
    };
    let mut next = NativeInstrumentRuntimePair::new(next_factory.as_ref(), LIMITS).unwrap();
    next.retarget_from(&frozen, proof.as_ref()).unwrap();
    let after_retarget = read_carriers(&next);
    assert_eq!(
        after_retarget, copied,
        "observation must not resolve a new prepared Base"
    );
    next.reset();
    assert_eq!(
        read_carriers(&next),
        next_settings
            .carriers
            .map(|value| Some(ParameterValue::Float(value)))
    );
    let mut expected = [[0.0; 2]; 1100];
    let mut actual = expected;
    let mut offset = 0;
    for &requested in [7, 31, 128].iter().cycle() {
        if offset == actual.len() {
            break;
        }
        let count = requested.min(actual.len() - offset);
        original.begin_block();
        automated(
            &mut original,
            &[],
            &mut expected[offset..offset + count],
            &targets,
        )
        .unwrap();
        original.commit_block();
        frozen.begin_block();
        automated(
            &mut frozen,
            &[],
            &mut actual[offset..offset + count],
            &frozen_values,
        )
        .unwrap();
        frozen.commit_block();
        offset += count;
    }
    assert_eq!(
        actual, expected,
        "freezing targets does not restart or cancel ramps"
    );
    assert!(actual[..64].iter().any(|frame| frame[0] != 0.0));
    assert_eq!(&actual[actual.len() - 16..], &[[0.0; 2]; 16]);
    frozen.reset();
    assert_eq!(read_carriers(&frozen), base);
}

#[test]
fn an_unmapped_pressure_slot_retains_held_readings_until_its_identity_changes() {
    use hex_lattice_contract::ModulationSourceId;
    use std::num::NonZeroU64;

    let settings = Fm4Settings::defaults();
    let resources = BorrowedDeviceResources(&[]);
    let tuning = hex_lattice::TuningTable::default();
    let (_, instrument) = prepared(48_000, settings, &tuning);
    let target = CARRIERS[0];
    let route = PreparedPressureRoute::new(
        target,
        modulation_semantics(target.id).unwrap(),
        -0.25,
        [0.0, 1.0],
    )
    .unwrap();
    let first = ModulationSourceId::new(7).unwrap();
    let replacement = ModulationSourceId::new(8).unwrap();
    let pair = |id, route| {
        let factory = instrument.prepare_mutable_note_pressure(id, route).unwrap();
        NativeInstrumentRuntimePair::new(factory.as_ref(), LIMITS).unwrap()
    };
    let mut listening = pair(first, None);
    let mut mapped = pair(first, Some(route));
    let mut replaced = pair(replacement, Some(route));
    let mut initial = [[0.0; 2]; 11];
    listening.begin_block();
    listening
        .process_block(
            &[
                on(1, 0, 60, 127),
                NativeInstrumentEvent::NotePressure {
                    frame_offset: 2,
                    occurrence_id: NonZeroU64::new(1).unwrap(),
                    value: 127,
                },
            ],
            &mut initial,
            &resources,
        )
        .unwrap();
    listening.commit_block();

    let mut modified = settings;
    modified.carriers[0] = 0.5;
    let (_, expected) = prepared(48_000, modified, &tuning);
    let expected = expected.prepare_note(60, 127).unwrap();
    let original = instrument.prepare_note(60, 127).unwrap();
    for (index, frame) in initial.into_iter().enumerate() {
        assert_eq!(frame, original.sample(index as u64, u64::MAX, &resources));
    }

    let mut pcm = [[0.0; 2]; 19];
    mapped.inherit_from(&listening).unwrap();
    mapped.begin_block();
    mapped.process_block(&[], &mut pcm, &resources).unwrap();
    mapped.commit_block();
    for (index, frame) in pcm.into_iter().enumerate() {
        assert_eq!(
            frame,
            expected.sample(11 + index as u64, u64::MAX, &resources)
        );
    }

    let mut pcm = [[0.0; 2]; 13];
    replaced.inherit_from(&mapped).unwrap();
    replaced.begin_block();
    replaced.process_block(&[], &mut pcm, &resources).unwrap();
    replaced.commit_block();
    for (index, frame) in pcm.into_iter().enumerate() {
        assert_eq!(
            frame,
            original.sample(30 + index as u64, u64::MAX, &resources)
        );
    }
}
