//! Real borrowed PCM through analytic/effect/mutable callbacks; no host lease admission.
use hex_lattice::{
    BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity, DeviceResourcePcm,
    DeviceResourceView, TuningTable, conformance::resource_pcm,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};
#[test]
fn native_callbacks_consume_borrowed_original_pcm_with_zero_allocations_or_frees() {
    let mono = vec![0.25_f32; 64];
    let stereo = vec![[0.5_f32, -0.75]; 64];
    let mut refs = Vec::new();
    let mut views = Vec::new();
    for (index, pcm) in [
        DeviceResourcePcm::Mono(&mono),
        DeviceResourcePcm::Stereo(&stereo),
    ]
    .into_iter()
    .enumerate()
    {
        let slot = DeviceAssetSlot::new(index as u32 + 1).unwrap();
        let asset = DeviceAssetKey::new(index as u64 + 7).unwrap();
        refs.push(DeviceAssetReference { slot, asset });
        views.push(DeviceResourceView {
            slot,
            identity: DeviceResourceIdentity {
                asset,
                content: [3; 32],
                derivation_version: 1,
                derivation: [5; 32],
            },
            sample_rate: 44_100,
            pcm,
        });
    }
    let state = NativeDeviceState::new(
        DeviceTypeId::from_static("test.borrowed-pcm-allocation"),
        1,
        vec![],
    )
    .unwrap()
    .with_asset_references(refs)
    .unwrap();
    let resources = BorrowedDeviceResources(&views);
    verify_callbacks(&state, &resources);
}

fn verify_callbacks(state: &NativeDeviceState, resources: &dyn hex_lattice::DeviceResources) {
    let tuning = TuningTable::default();
    let context = DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &tuning,
        resources,
    };
    let notes = [
        resource_pcm::prepare_note(state, &context, 0).unwrap(),
        resource_pcm::prepare_note(state, &context, 1).unwrap(),
    ];
    let mut sum = [0.0_f32; 2];
    let measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            for age in 0..64 {
                for note in &notes {
                    let frame =
                        std::hint::black_box(note).sample(age, 64, std::hint::black_box(resources));
                    sum[0] += frame[0];
                    sum[1] += frame[1];
                }
            }
        }
    });
    assert_eq!(
        (
            measurement.count_total,
            measurement.bytes_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(sum, [1536.0, -1024.0]);

    use hex_lattice::NativeProcessorRuntime;
    let mut effects = [
        resource_pcm::prepare_effect(state, &context, 0).unwrap(),
        resource_pcm::prepare_effect(state, &context, 1).unwrap(),
    ];
    let mut output = [[[1.0_f32; 2]; 64]; 2];
    let mut effect_sum = [0.0_f32; 2];
    let effect_measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            for (index, effect) in effects.iter_mut().enumerate() {
                effect.reset();
                output[index].fill([1.0; 2]);
                let callback: &mut dyn NativeProcessorRuntime = std::hint::black_box(effect);
                callback.process_block(&mut output[index], std::hint::black_box(resources));
                for frame in &output[index] {
                    effect_sum[0] += frame[0];
                    effect_sum[1] += frame[1];
                }
            }
        }
    });
    assert_eq!(
        (
            effect_measurement.count_total,
            effect_measurement.bytes_total,
            effect_measurement.count_current,
            effect_measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(effect_sum, [1536.0, -1024.0]);
    use hex_lattice::{NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair};
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 4096,
        max_events_per_block: 1,
        max_active_occurrences: 1,
        max_tail_frames: 64,
    };
    let factories = [
        resource_pcm::mutable::prepare(state, &context, 0).unwrap(),
        resource_pcm::mutable::prepare(state, &context, 1).unwrap(),
    ];
    let mut pairs = [
        NativeInstrumentRuntimePair::new(&factories[0], limits).unwrap(),
        NativeInstrumentRuntimePair::new(&factories[1], limits).unwrap(),
    ];
    let mut mutable_sum = [0.0_f32; 2];
    let mutable_measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            for (index, pair) in pairs.iter_mut().enumerate() {
                pair.reset();
                pair.begin_block();
                output[index].fill([0.0; 2]);
                pair.preflight_event_batch(&[], 64, std::hint::black_box(resources))
                    .unwrap();
                pair.process_block(&[], &mut output[index], std::hint::black_box(resources))
                    .unwrap();
                pair.commit_block();
                for frame in &output[index] {
                    mutable_sum[0] += frame[0];
                    mutable_sum[1] += frame[1];
                }
            }
        }
    });
    assert_eq!(
        (
            mutable_measurement.count_total,
            mutable_measurement.bytes_total,
            mutable_measurement.count_current,
            mutable_measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(mutable_sum, [1536.0, -1024.0]);
}

#[test]
fn callbacks_borrow_actual_native_bank_scopes_without_allocations_or_frees() {
    use hex_engine::native_device_resources::{
        DeviceResourceCatalog, OriginalResourceSource, ResourceCatalogReservation,
    };
    use hex_engine::{AudioStorageReservation, OwnedAudioInputs, OwnedAudioSource};
    use std::sync::atomic::AtomicBool;
    let first = match OwnedAudioSource::try_resident(44_100, vec![[0.25; 2]; 64]) {
        Ok(source) => source,
        Err(refusal) => panic!(
            "required actual-bank allocation probe needs supported native constructor: {:?}",
            refusal.reason
        ),
    };
    let second = OwnedAudioSource::try_resident(44_100, vec![[0.5, -0.75]; 64])
        .ok()
        .unwrap();
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(44_100).unwrap(), 8).unwrap();
    let ids: Vec<_> = (0..2)
        .map(|index| {
            let hex_project::Created::Asset(id) = editor
                .execute(hex_project::Command::AddAsset {
                    path: format!("callback-source-{index}.wav").into(),
                    sample_rate: 44_100,
                    frames: 64,
                })
                .unwrap()
            else {
                panic!("asset")
            };
            id
        })
        .collect();
    let bank = OwnedAudioInputs::try_new(
        vec![(ids[0], first), (ids[1], second)],
        AudioStorageReservation {
            allocation_items: 8,
            handle_obligations: 8,
            max_prepared_data_bytes: 65_536,
            max_construction_data_bytes: 65_536,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap()
    .prepare_input_storage_on_control()
    .ok()
    .unwrap();
    let sources: Vec<_> = ids
        .iter()
        .map(|&asset| OriginalResourceSource {
            asset,
            sample_rate: 44_100,
            frames: 64,
        })
        .collect();
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &bank,
        &sources,
        ResourceCatalogReservation {
            metadata_bytes: 65_536,
            hash_frames: 128,
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    let references = ids
        .iter()
        .enumerate()
        .map(|(index, id)| DeviceAssetReference {
            slot: DeviceAssetSlot::new(index as u32 + 1).unwrap(),
            asset: DeviceAssetKey::new(id.value()).unwrap(),
        })
        .collect();
    let state = NativeDeviceState::new(
        DeviceTypeId::from_static("test.actual-bank-allocation"),
        1,
        vec![],
    )
    .unwrap()
    .with_asset_references(references)
    .unwrap();
    let scope = catalog.prepare_scope_on_control(&state, 4096).unwrap();
    let before = bank.summary();
    let resources = catalog.resources(&bank, &scope).unwrap();
    verify_callbacks(&state, &resources);
    assert_eq!(bank.summary(), before);
    bank.validate_unresolved_on_control().unwrap();
    eprintln!("native bank callback probe: supported-controls=completed");
}

#[test]
fn resident_sampler_occurrence_gains_allocate_and_free_nothing_in_the_actual_pair() {
    use hex_devices::sampler::{
        GateMode, Mode, PreparedSamplerPlan, Region, SampleChannels, SampleShape, Settings,
    };
    use hex_lattice::{
        NativeInstrumentEvent, NativeInstrumentOccurrenceGain, NativeInstrumentRuntimeLimits,
        NativeInstrumentRuntimePair,
    };
    let pcm = [1.0; 32];
    let views = [DeviceResourceView {
        slot: DeviceAssetSlot::new(1).unwrap(),
        identity: DeviceResourceIdentity {
            asset: DeviceAssetKey::new(1).unwrap(),
            content: [2; 32],
            derivation_version: 1,
            derivation: [3; 32],
        },
        sample_rate: 8_000,
        pcm: DeviceResourcePcm::Mono(&pcm),
    }];
    let resources = BorrowedDeviceResources(&views);
    let mut settings = Settings::new(Region { start: 0, end: 32 });
    settings.mode = Mode::OneShot {
        gate: GateMode::Trigger,
    };
    let prepared = PreparedSamplerPlan::new(
        SampleShape::new(32, 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(views[0])
    .unwrap()
    .prepare_runtime_on_control(4, 16, &TuningTable::default())
    .unwrap();
    let mut runtime = NativeInstrumentRuntimePair::new(
        &prepared,
        NativeInstrumentRuntimeLimits {
            max_pair_bytes: 2 * 1024 * 1024,
            max_events_per_block: 16,
            max_active_occurrences: 4,
            max_tail_frames: 8_000 * 600,
        },
    )
    .unwrap();
    let events = [
        NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 2,
            pitch: 60,
            velocity: 127,
        },
        NativeInstrumentEvent::NoteOn {
            frame_offset: 2,
            occurrence_id: 1,
            pitch: 60,
            velocity: 127,
        },
    ];
    let gains =
        [
            NativeInstrumentOccurrenceGain::new(1, 0, hex_lattice::INSTRUMENT_GAIN_UNITY, 7, 0)
                .unwrap(),
        ];
    let mut frames = [[0.0; 2]; 7];
    let mut failure = None;
    // Construction, resource binding, output oracle and destruction stay out.
    let measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            runtime.reset();
            runtime.begin_block();
            frames.fill([0.0; 2]);
            if let Err(error) = runtime.process_block_with_clock_and_occurrence_gains(
                std::hint::black_box(&events),
                &mut frames,
                None,
                std::hint::black_box(&gains),
                std::hint::black_box(&resources),
            ) {
                failure = Some(error);
                break;
            }
            runtime.commit_block();
        }
    });
    assert_eq!(failure, None);
    assert_eq!(
        (
            measurement.count_total,
            measurement.bytes_total,
            measurement.count_current,
            measurement.bytes_current
        ),
        (0, 0, 0, 0)
    );
    for (index, actual) in frames.iter().enumerate() {
        let expected = if index < 2 {
            1.0
        } else {
            (1.0 + index as f64 / 7.0) as f32
        };
        assert_eq!(*actual, [expected; 2]);
    }
}
