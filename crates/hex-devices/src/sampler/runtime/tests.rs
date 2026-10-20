use super::*;
use hex_lattice::{
    BorrowedDeviceResources, DeviceResourceIdentity, DeviceResourcePcm, DeviceResourceView,
    NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetSlot};

#[test]
fn compact_key_table_preserves_every_full_coefficient_and_stays_under_context_cap() {
    let pcm = [1.0_f32; 12];
    let view = view(&pcm);
    let mut frequencies = *hex_lattice::TuningTable::default().frequencies();
    frequencies[61] = frequencies[60] * 1.3;
    let tuning = hex_lattice::TuningTable::new(frequencies).unwrap();
    for mode in [
        Mode::Classic {
            loop_region: Some(Region { start: 3, end: 7 }),
        },
        Mode::OneShot {
            gate: GateMode::Gate,
        },
        Mode::OneShot {
            gate: GateMode::Trigger,
        },
        Mode::Slice {
            starts: &[1, 3, 7],
            base_note: 60,
            gate: GateMode::Trigger,
        },
    ] {
        for reverse in [false, true] {
            let mut settings = Settings::new(Region { start: 1, end: 10 });
            settings.mode = mode;
            settings.reverse = reverse;
            settings.pan = -0.25;
            settings.gain = 0.375;
            settings.envelope.release_seconds = 0.125;
            settings.one_shot_fades_seconds = [0.01, 0.02];
            let plan = PreparedSamplerPlan::new(
                SampleShape::new(12, 8_000, SampleChannels::Mono).unwrap(),
                44_100,
                settings,
            )
            .unwrap();
            let compact = plan
                .bind_source_on_control(view)
                .unwrap()
                .prepare_runtime_on_control(16, 259, &tuning)
                .unwrap();
            for pitch in 0..128 {
                assert_eq!(
                    compact.note(pitch),
                    plan.prepare_tuned_voice_coefficients(pitch, 127, &tuning)
                        .unwrap()
                );
            }
            assert!(
                std::mem::size_of_val(&compact) <= hex_lattice::MAX_SHARED_ANALYTIC_CONTEXT_BYTES
            );
            assert_eq!(
                compact.control_storage_declaration(),
                hex_lattice::PreparedStorageDeclaration::ExclusiveOnly {
                    bytes: std::mem::size_of_val(&compact)
                }
            );
        }
    }
}

fn view(pcm: &[f32]) -> DeviceResourceView<'_> {
    DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(1).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8_000,
        pcm: DeviceResourcePcm::Mono(pcm),
    }
}
fn prepare(view: DeviceResourceView<'_>, mode: Mode<'_>, voices: usize) -> PreparedSamplerRuntime {
    let mut settings = Settings::new(Region {
        start: 0,
        end: view.pcm.frames(),
    });
    settings.mode = mode;
    PreparedSamplerPlan::new(
        SampleShape::new(view.pcm.frames(), 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(view)
    .unwrap()
    .prepare_runtime_on_control(voices, 259, &hex_lattice::TuningTable::default())
    .unwrap()
}
fn pair(prepared: &PreparedSamplerRuntime) -> NativeInstrumentRuntimePair {
    NativeInstrumentRuntimePair::new(
        prepared,
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 259,
            max_active_occurrences: MAX_SAMPLER_RUNTIME_VOICES,
            max_tail_frames: 8_000 * 600,
        },
    )
    .unwrap()
}
fn on(frame_offset: usize, occurrence_id: u64, pitch: u8) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOn {
        frame_offset,
        occurrence_id,
        pitch,
        velocity: 127,
    }
}
fn off(frame_offset: usize, occurrence_id: u64) -> NativeInstrumentEvent {
    NativeInstrumentEvent::NoteOff {
        frame_offset,
        occurrence_id,
    }
}
fn process(
    pair: &mut NativeInstrumentRuntimePair,
    events: &[NativeInstrumentEvent],
    output: &mut [StereoFrame],
    resources: &dyn DeviceResources,
) {
    pair.begin_block();
    pair.process_block(events, output, resources).unwrap();
    pair.commit_block();
}

#[test]
fn one_shot_fades_actual_mutable_pair_matches_pcm_oracle_across_partitions_and_rollback() {
    let pcm = [1.0_f32; 8];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region { start: 0, end: 8 });
    settings.mode = Mode::OneShot {
        gate: GateMode::Trigger,
    };
    settings.one_shot_fades_seconds = [2.0 / 8_000.0; 2];
    let prepared = PreparedSamplerPlan::new(
        SampleShape::new(8, 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(views[0])
    .unwrap()
    .prepare_runtime_on_control(4, 259, &hex_lattice::TuningTable::default())
    .unwrap();
    let expected = [0.0, 0.5, 1.0, 1.0, 1.0, 1.0, 0.5, 0.0].map(|v| [v; 2]);
    let mut runtime = pair(&prepared);
    let mut output = [[99.0; 2]; 8];
    runtime.begin_block();
    runtime
        .process_block(&[on(0, 1, 60)], &mut output, &resources)
        .unwrap();
    assert_eq!(
        output,
        expected.map(|frame| frame.map(|value| 99.0 + value))
    );
    runtime.rollback_block();
    // Instrument processing ADDS into the caller's mix buffer. Replay starts
    // from an independent silent bus, not the speculative mixed output.
    output.fill([0.0; 2]);
    process(&mut runtime, &[on(0, 1, 60)], &mut output[..3], &resources);
    process(&mut runtime, &[], &mut output[3..], &resources);
    assert_eq!(output, expected);
}

#[test]
fn actual_mutable_pair_keeps_trigger_audio_after_note_off_and_retires_at_source_end() {
    let pcm = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = prepare(
        views[0],
        Mode::OneShot {
            gate: GateMode::Trigger,
        },
        4,
    );
    assert!(prepared.declared_twin_bytes() * 2 < 2 * 1024 * 1024);
    assert_eq!(prepared.max_events_per_block(), 259);
    let mut whole = pair(&prepared);
    let mut output = [[0.0; 2]; 12];
    process(
        &mut whole,
        &[on(0, 1, 60), off(1, 1)],
        &mut output,
        &resources,
    );
    for (actual, expected) in output[..8].iter().zip(pcm) {
        assert_eq!(*actual, [expected; 2]);
    }
    assert_eq!(output[8..], [[0.0; 2]; 4]);
    // The actual twin is quiescent after source exhaustion, not MIDI note-off.
    let mut split = pair(&prepared);
    let mut partitioned = [[0.0; 2]; 12];
    process(
        &mut split,
        &[on(0, 1, 60), off(1, 1)],
        &mut partitioned[..2],
        &resources,
    );
    process(&mut split, &[], &mut partitioned[2..7], &resources);
    process(&mut split, &[], &mut partitioned[7..], &resources);
    assert_eq!(output, partitioned);
}

#[test]
fn capacity_refusal_and_speculative_rollback_leave_actual_pair_unchanged() {
    let pcm = [1.0, 3.0, 5.0, 7.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = prepare(views[0], Mode::Classic { loop_region: None }, 1);
    let mut runtime = pair(&prepared);
    let mut untouched = [[9.0; 2]; 2];
    runtime.begin_block();
    assert_eq!(
        runtime.process_block(&[on(0, 1, 60), on(0, 2, 60)], &mut untouched, &resources),
        Err(Error::EventCapacityExceeded)
    );
    assert_eq!(untouched, [[9.0; 2]; 2]);
    runtime.rollback_block();
    let mut speculative = [[0.0; 2]; 2];
    runtime.begin_block();
    runtime
        .process_block(&[on(0, 1, 60)], &mut speculative, &resources)
        .unwrap();
    runtime.rollback_block();
    let mut committed = [[0.0; 2]; 2];
    process(&mut runtime, &[on(0, 1, 60)], &mut committed, &resources);
    assert_eq!(speculative, committed);
    assert_eq!(committed, [[1.0; 2], [3.0; 2]]);
    // Equal-offset gate-off frees a zero-release slot before the next onset.
    committed.fill([0.0; 2]);
    process(
        &mut runtime,
        &[off(0, 1), on(0, 2, 60)],
        &mut committed,
        &resources,
    );
    assert_eq!(committed, [[1.0; 2], [3.0; 2]]);
}

#[test]
fn slice_selection_and_source_refusal_use_original_pcm_through_native_runtime() {
    let pcm = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let prepared = prepare(
        views[0],
        Mode::Slice {
            starts: &[0, 3, 5],
            base_note: 60,
            gate: GateMode::Trigger,
        },
        4,
    );
    let mut runtime = pair(&prepared);
    let mut wrong = views;
    wrong[0].identity.content[31] ^= 1;
    let mut output = [[9.0; 2]; 4];
    runtime.begin_block();
    assert_eq!(
        runtime.process_block(
            &[on(0, 1, 61)],
            &mut output,
            &BorrowedDeviceResources(&wrong)
        ),
        Err(Error::ResourceUnavailable)
    );
    runtime.rollback_block();
    assert_eq!(output, [[9.0; 2]; 4]);
    output.fill([0.0; 2]);
    process(
        &mut runtime,
        &[on(0, 1, 61), off(0, 1)],
        &mut output,
        &resources,
    );
    assert_eq!(output, [[7.0; 2], [9.0; 2], [0.0; 2], [0.0; 2]]);
}

#[test]
fn project_tuning_velocity_amount_and_gated_release_reach_actual_mutable_samples() {
    let pcm = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let mut frequencies = *hex_lattice::TuningTable::default().frequencies();
    frequencies[61] = frequencies[60] * 1.5;
    let tuning = hex_lattice::TuningTable::new(frequencies).unwrap();
    let mut settings = Settings::new(Region { start: 0, end: 6 });
    settings.velocity_amount = 0.0;
    settings.envelope.release_seconds = 2.0 / 8_000.0;
    let bound = PreparedSamplerPlan::new(
        SampleShape::new(6, 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(views[0])
    .unwrap();
    let prepared = bound.prepare_runtime_on_control(4, 259, &tuning).unwrap();
    assert_eq!(prepared.max_release_tail_frames(), 2);
    let mut runtime = pair(&prepared);
    let note = NativeInstrumentEvent::NoteOn {
        frame_offset: 0,
        occurrence_id: 1,
        pitch: 61,
        velocity: 1,
    };
    let mut output = [[0.0; 2]; 6];
    process(&mut runtime, &[note, off(2, 1)], &mut output, &resources);
    assert_eq!(
        output,
        [[1.0; 2], [4.0; 2], [7.0; 2], [5.0; 2], [0.0; 2], [0.0; 2]]
    );
    let original = prepared.create_runtime();
    let same = prepared.create_runtime();
    assert!(same.can_copy_history_from(original.as_ref()));
    let neutral = bound
        .prepare_runtime_on_control(4, 259, &hex_lattice::TuningTable::default())
        .unwrap()
        .create_runtime();
    assert!(!neutral.can_copy_history_from(original.as_ref()));
}

#[test]
fn concert_reference_retunes_assigned_root_instead_of_canceling_with_it() {
    let pcm = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let neutral = hex_lattice::TuningTable::default();
    let tuning = hex_lattice::TuningTable::new(neutral.frequencies().map(|hz| hz * 2.0)).unwrap();
    let settings = Settings::new(Region { start: 0, end: 6 });
    let plan = PreparedSamplerPlan::new(
        SampleShape::new(6, 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap();
    // Disabled tuning preserves the exact original scalar law, not a new
    // quotient/log approximation of equal temperament.
    for key in 0..128 {
        assert_eq!(
            plan.prepare_tuned_voice_coefficients(key, 127, &neutral)
                .unwrap(),
            plan.prepare_voice_coefficients(key, 127).unwrap()
        );
    }
    let prepared = plan
        .bind_source_on_control(views[0])
        .unwrap()
        .prepare_runtime_on_control(4, 259, &tuning)
        .unwrap();
    let mut runtime = pair(&prepared);
    let mut output = [[0.0; 2]; 5];
    process(&mut runtime, &[on(0, 1, 60)], &mut output, &resources);
    assert_eq!(output, [[1.0; 2], [5.0; 2], [9.0; 2], [0.0; 2], [0.0; 2]]);
}

#[test]
fn native_loaded_adapter_uses_mutable_original_pcm_and_keeps_registry_gate_closed() {
    use hex_lattice::{NativeInstrumentDefinition, PreparedNativeInstrument};
    use hex_lattice_contract::{DeviceAssetReference, DeviceContractError as ContractError};
    let mono = [0.25, 0.5, -0.25, 1.0];
    let stereo = mono.map(|sample| [sample, -sample]);
    for pcm in [
        DeviceResourcePcm::Mono(&mono),
        DeviceResourcePcm::Stereo(&stereo),
    ] {
        let mut source = view(&mono);
        source.pcm = pcm;
        let views = [source];
        let resources = BorrowedDeviceResources(&views);
        let state = super::super::state::SamplerState::for_source_frames(4)
            .unwrap()
            .to_native()
            .unwrap()
            .with_asset_references(vec![DeviceAssetReference {
                slot: source.slot,
                asset: source.identity.asset,
            }])
            .unwrap();
        let context = hex_lattice::DevicePreparationContext {
            sample_rate: 8_000,
            tuning: &hex_lattice::TuningTable::default(),
            resources: &resources,
        };
        let prepared = super::super::metadata::METADATA
            .prepare_instrument(&state, &context)
            .unwrap();
        assert_eq!(prepared.algorithmic_latency_frames(), Some(0));
        assert!(matches!(
            prepared.prepare_note(60, 127),
            Err(ContractError::UnsupportedProcessingClass)
        ));
        assert!(prepared.validate_event_note(128, 127).is_err());
        assert!(prepared.validate_event_note(60, 128).is_err());
        assert!(prepared.validate_event_note(0, 0).is_ok());
        let factory = prepared.prepare_mutable_runtime().unwrap().unwrap();
        assert_eq!(factory.max_events_per_block(), 259);
        let mut pair = NativeInstrumentRuntimePair::new(
            factory.as_ref(),
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: 259,
                max_active_occurrences: 32,
                max_tail_frames: 8_000 * 600,
            },
        )
        .unwrap();
        let mut output = [[0.0; 2]; 6];
        process(&mut pair, &[on(0, 1, 60)], &mut output, &resources);
        let expected = match pcm {
            DeviceResourcePcm::Mono(_) => mono.map(|sample| [sample; 2]),
            DeviceResourcePcm::Stereo(_) => stereo,
        };
        assert_eq!(&output[..4], &expected);
        assert_eq!(&output[4..], &[[0.0; 2]; 2]);
        // The fixed concrete factory route remains available to the actual
        // engine owner enum; trait-object preparation supplies no lease proof.
        let concrete =
            super::super::state::prepare_runtime_on_control(&state, &context, 259).unwrap();
        assert_eq!(
            std::mem::size_of_val(&concrete),
            std::mem::size_of::<PreparedSamplerRuntime>()
        );
        assert!(PreparedNativeInstrument::validate_event_note(&concrete, 127, 127).is_ok());
        let registry = crate::native_instrument_registry().unwrap();
        assert!(registry.definition(super::super::state::TYPE_ID).is_none());
        assert!(registry.prepare_instrument(&state, &context).is_err());
    }
}

#[test]
fn native_adapter_refuses_unexpected_empty_resources_and_missing_or_mismatched_loaded_sources() {
    use hex_lattice::NativeInstrumentDefinition;
    use hex_lattice_contract::{DeviceAssetReference, NativeDeviceMetadata};
    let pcm = [1.0; 4];
    let views = [view(&pcm)];
    let resources = BorrowedDeviceResources(&views);
    let context = hex_lattice::DevicePreparationContext {
        sample_rate: 8_000,
        tuning: &hex_lattice::TuningTable::default(),
        resources: &resources,
    };
    let empty = super::super::metadata::METADATA.default_state().unwrap();
    assert!(matches!(
        super::super::metadata::METADATA.prepare_instrument(&empty, &context),
        Err(hex_lattice_contract::DeviceContractError::AssetResourcesUnavailable)
    ));
    let unbound = super::super::state::SamplerState::for_source_frames(4)
        .unwrap()
        .to_native()
        .unwrap();
    assert!(
        super::super::metadata::METADATA
            .prepare_instrument(&unbound, &context)
            .is_err()
    );
    let wrong = unbound
        .with_asset_references(vec![DeviceAssetReference {
            slot: views[0].slot,
            asset: DeviceAssetKey::new(7).unwrap(),
        }])
        .unwrap();
    assert!(
        super::super::metadata::METADATA
            .prepare_instrument(&wrong, &context)
            .is_err()
    );
}

#[test]
fn empty_native_sampler_is_silent_without_source_or_invented_voice_region() {
    use hex_lattice::{NativeInstrumentDefinition, PreparedNativeInstrument};
    use hex_lattice_contract::NativeDeviceMetadata;
    let state = super::super::metadata::METADATA.default_state().unwrap();
    let tuning = hex_lattice::TuningTable::default();
    let context = hex_lattice::DevicePreparationContext {
        sample_rate: 44_100,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let prepared = super::super::state::prepare_runtime_on_control(&state, &context, 259).unwrap();
    assert!(prepared.source.is_none());
    assert!(prepared.common.is_none());
    assert_eq!(prepared.max_release_tail_frames(), 0);
    assert_eq!(
        prepared.control_storage_declaration(),
        hex_lattice::PreparedStorageDeclaration::ExclusiveOnly {
            bytes: std::mem::size_of::<PreparedSamplerRuntime>(),
        }
    );
    for pitch in 0..128 {
        assert!(prepared.note(pitch).is_none());
        assert!(PreparedNativeInstrument::validate_event_note(&prepared, pitch, 127).is_ok());
    }
    assert!(std::mem::size_of_val(&prepared) <= hex_lattice::MAX_SHARED_ANALYTIC_CONTEXT_BYTES);
    let definition = super::super::metadata::METADATA
        .prepare_instrument(&state, &context)
        .unwrap();
    assert!(definition.prepare_note(60, 127).is_err());
    assert!(definition.prepare_mutable_runtime().unwrap().is_some());
    let mut pair = pair(&prepared);
    let events = (0..128)
        .map(|pitch| on(0, u64::from(pitch) + 1, pitch))
        .chain((1..=128).map(|occurrence| off(1, occurrence)))
        .collect::<Vec<_>>();
    for commit in [false, true, false, true] {
        let mut output = [[0.25, -0.5]; 2];
        pair.begin_block();
        pair.process_block(&events, &mut output, context.resources)
            .unwrap();
        assert_eq!(output, [[0.25, -0.5]; 2]);
        if commit {
            pair.commit_block();
        } else {
            pair.rollback_block();
        }
    }
    pair.reset();
    assert!(
        crate::native_instrument_registry()
            .unwrap()
            .definition(super::super::state::TYPE_ID)
            .is_none()
    );
}

#[test]
fn empty_sampler_refuses_bad_context_and_never_inherits_loaded_voice_history() {
    use hex_lattice_contract::NativeDeviceMetadata;
    let state = super::super::metadata::METADATA.default_state().unwrap();
    let tuning = hex_lattice::TuningTable::default();
    let mut context = hex_lattice::DevicePreparationContext {
        sample_rate: 44_100,
        tuning: &tuning,
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    for invalid_events in [0, MAX_EVENTS + 1] {
        assert!(
            super::super::state::prepare_runtime_on_control(&state, &context, invalid_events)
                .is_err()
        );
    }
    let prepared = super::super::state::prepare_runtime_on_control(&state, &context, 259).unwrap();
    let mut empty_pair = pair(&prepared);
    context.sample_rate = 0;
    assert!(super::super::state::prepare_runtime_on_control(&state, &context, 259).is_err());
    let pcm = [1.0; 4];
    let view = view(&pcm);
    let loaded = prepare(
        view,
        Mode::OneShot {
            gate: GateMode::Trigger,
        },
        16,
    );
    let loaded_pair = pair(&loaded);
    assert!(!empty_pair.can_inherit_from(&loaded_pair));
    assert!(!loaded_pair.can_inherit_from(&empty_pair));
    empty_pair.begin_block();
    let mut unchanged = [[0.125, -0.25]; 2];
    let resources = BorrowedDeviceResources(&[view]);
    assert_eq!(
        empty_pair.process_block(&[], &mut unchanged, &resources),
        Err(Error::ResourceUnavailable)
    );
    assert_eq!(unchanged, [[0.125, -0.25]; 2]);
    empty_pair.rollback_block();
    empty_pair.begin_block();
    assert!(
        empty_pair
            .process_block(
                &[on(0, 1, 128)],
                &mut unchanged,
                &hex_lattice::EMPTY_DEVICE_RESOURCES
            )
            .is_err()
    );
    assert_eq!(unchanged, [[0.125, -0.25]; 2]);
    empty_pair.rollback_block();
}
