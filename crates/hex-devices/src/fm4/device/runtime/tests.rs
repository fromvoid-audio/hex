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
