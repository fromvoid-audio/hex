//! Real FM4 component transaction; host/native admission is separate.
#![cfg(feature = "allocation-probe")]
use hex_devices::fm4::device::{FM4_DEFINITION, parameter};
use hex_lattice::{
    DevicePreparationContext, EMPTY_DEVICE_RESOURCES, NativeInstrumentEvent,
    NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair, NativeParameterAutomation,
    TuningTable,
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
struct Values;
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
    fn value_at(&self, _: ParameterId, offset: usize) -> Option<ParameterValue> {
        Some(ParameterValue::Float(if offset.is_multiple_of(7) {
            0.0
        } else {
            0.5
        }))
    }
}
#[test]
fn real_fm4_carrier_processing_rollbacks_and_reset_allocate_and_free_nothing() {
    let tuning = TuningTable::default();
    let context = DevicePreparationContext {
        sample_rate: 48000,
        tuning: &tuning,
        resources: &EMPTY_DEVICE_RESOURCES,
    };
    let state = FM4_DEFINITION.default_state().unwrap();
    let factory = FM4_DEFINITION
        .prepare_carrier_runtime(&state, &context)
        .unwrap();
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 259,
        max_active_occurrences: 64,
        max_tail_frames: 48000 * 120,
    };
    let mut pair = NativeInstrumentRuntimePair::new(factory.as_ref(), limits).unwrap();
    let mut frames = [[0.0; 2]; 512];
    let events = std::array::from_fn::<_, 64, _>(|index| NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: index as u64 + 1,
        pitch: 48,
        velocity: 100,
    });
    let mut results = [Ok(()); 3];
    let mut ids = [0; 64];
    let mut observed = Ok(0);
    let counts = allocation_counter::measure(|| {
        pair.begin_block();
        results[0] = pair.process_block_with_clock_and_automation(
            &events,
            &mut frames,
            None,
            &[],
            &Values,
            &EMPTY_DEVICE_RESOURCES,
        );
        pair.rollback_block();
        pair.begin_block();
        results[1] = pair.process_block_with_clock_and_automation(
            &events,
            &mut frames,
            None,
            &[],
            &Values,
            &EMPTY_DEVICE_RESOURCES,
        );
        pair.commit_block();
        observed = pair.active_occurrence_ids(&mut ids);
        pair.begin_block();
        results[2] = pair.process_block_with_clock_and_automation(
            &[],
            &mut frames,
            None,
            &[],
            &Values,
            &EMPTY_DEVICE_RESOURCES,
        );
        pair.commit_block();
        pair.reset();
    });
    assert_eq!(results, [Ok(()); 3]);
    assert_eq!(observed, Ok(64));
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(frames.iter().flatten().any(|sample| *sample != 0.0));
}

#[test]
fn real_fm4_retune_retargets_actual_voice_history_without_allocations_or_frees() {
    use hex_lattice::InstrumentHistoryRetargetPreparation;
    let tuning = TuningTable::default();
    let retuned = TuningTable::new(tuning.frequencies().map(|frequency| frequency * 1.01)).unwrap();
    let state = FM4_DEFINITION.default_state().unwrap();
    let prepare = |tuning| {
        FM4_DEFINITION
            .prepare_carrier_runtime(
                &state,
                &DevicePreparationContext {
                    sample_rate: 48000,
                    tuning,
                    resources: &EMPTY_DEVICE_RESOURCES,
                },
            )
            .unwrap()
    };
    let source = prepare(&tuning);
    let target = prepare(&retuned);
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 259,
        max_active_occurrences: 64,
        max_tail_frames: 48000 * 120,
    };
    let mut source_pair = NativeInstrumentRuntimePair::new(source.as_ref(), limits).unwrap();
    let mut target_pair = NativeInstrumentRuntimePair::new(target.as_ref(), limits).unwrap();
    let InstrumentHistoryRetargetPreparation::Prepared(proof) =
        target.prepare_history_retarget_target().unwrap()
    else {
        panic!("fm4 target proof");
    };
    let events = std::array::from_fn::<_, 64, _>(|index| NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: index as u64 + 1,
        pitch: 48,
        velocity: 100,
    });
    let mut frames = [[0.0; 2]; 512];
    source_pair.begin_block();
    source_pair
        .process_block_with_clock_and_automation(
            &events,
            &mut frames,
            None,
            &[],
            &Values,
            &EMPTY_DEVICE_RESOURCES,
        )
        .unwrap();
    source_pair.commit_block();
    let mut results = [Ok(()); 3];
    let counts = allocation_counter::measure(|| {
        results[0] = target_pair.retarget_from(&source_pair, proof.as_ref());
        target_pair.begin_block();
        results[1] = target_pair.process_block_with_clock_and_automation(
            &[],
            &mut frames,
            None,
            &[],
            &Values,
            &EMPTY_DEVICE_RESOURCES,
        );
        target_pair.rollback_block();
        target_pair.begin_block();
        results[2] = target_pair.process_block_with_clock_and_automation(
            &[],
            &mut frames,
            None,
            &[],
            &Values,
            &EMPTY_DEVICE_RESOURCES,
        );
        target_pair.commit_block();
        target_pair.reset();
    });
    assert_eq!(results, [Ok(()); 3]);
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(frames.iter().flatten().any(|sample| *sample != 0.0));
}
