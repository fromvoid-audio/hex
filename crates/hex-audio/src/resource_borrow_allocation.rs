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
        NativeInstrumentEvent, NativeInstrumentOccurrenceGain, NativeInstrumentRuntimeError,
        NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
        PreparedMutableInstrumentRuntime,
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
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 16,
        max_active_occurrences: 4,
        max_tail_frames: 8_000 * 600,
    };
    let mut runtime = NativeInstrumentRuntimePair::new(&prepared, limits).unwrap();
    let mut inherited = NativeInstrumentRuntimePair::new(&prepared, limits).unwrap();
    assert!(
        prepared.max_release_tail_frames() > 32,
        "conservative factory bound outlives an individual source voice"
    );
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
            NativeInstrumentOccurrenceGain::new(1, 0.0, hex_lattice::INSTRUMENT_GAIN_UNITY, 7, 0)
                .unwrap(),
        ];
    let mut frames = [[0.0; 2]; 7];
    let mut tail = [[0.0; 2]; 25];
    let mut replay = tail;
    let mut survivor = [[0.0; 2]; 2];
    let mut copied = survivor;
    let mut refused = [[9.0; 2]; 2];
    let mut ids = [0; 4];
    let survivor_gain = [NativeInstrumentOccurrenceGain::new(1, 0.5, 0.5, 2, 0).unwrap()];
    let retired_gain = [NativeInstrumentOccurrenceGain::new(2, 0.5, 0.5, 2, 0).unwrap()];
    let mut failure = None;
    fn observe(
        pair: &NativeInstrumentRuntimePair,
        output: &mut [u64],
        expected: &[u64],
    ) -> Result<(), NativeInstrumentRuntimeError> {
        let count = pair.active_occurrence_ids(output)?;
        if &output[..count] != expected {
            return Err(NativeInstrumentRuntimeError::InvalidActiveOccurrenceIds);
        }
        Ok(())
    }
    // Construction, original resource binding, caller buffers, gain values,
    // oracles and destruction stay outside. Query both transactional histories,
    // short capacity, early per-voice retirement and unchanged gain refusals.
    let measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            let result = (|| {
                runtime.reset();
                observe(&runtime, &mut ids, &[])?;
                runtime.begin_block();
                frames.fill([0.0; 2]);
                runtime.process_block_with_clock_and_occurrence_gains(
                    std::hint::black_box(&events),
                    &mut frames,
                    None,
                    std::hint::black_box(&gains),
                    std::hint::black_box(&resources),
                )?;
                observe(&runtime, &mut ids, &[1, 2])?;
                if runtime.active_occurrence_ids(&mut ids[..1])
                    != Err(NativeInstrumentRuntimeError::ActiveOccurrenceCapacityExceeded)
                {
                    return Err(NativeInstrumentRuntimeError::InvalidActiveOccurrenceIds);
                }
                runtime.commit_block();
                observe(&runtime, &mut ids, &[1, 2])?;
                runtime.begin_block();
                tail.fill([0.0; 2]);
                runtime.process_block(&[], &mut tail, &resources)?;
                observe(&runtime, &mut ids, &[1])?;
                runtime.rollback_block();
                observe(&runtime, &mut ids, &[1, 2])?;
                runtime.begin_block();
                replay.fill([0.0; 2]);
                runtime.process_block(&[], &mut replay, &resources)?;
                runtime.commit_block();
                observe(&runtime, &mut ids, &[1])?;
                inherited.inherit_from(&runtime)?;
                observe(&inherited, &mut ids, &[1])?;
                runtime.begin_block();
                if runtime.process_block_with_clock_and_occurrence_gains(
                    &[],
                    &mut refused,
                    None,
                    &retired_gain,
                    &resources,
                ) != Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain)
                {
                    return Err(NativeInstrumentRuntimeError::InvalidOccurrenceGain);
                }
                observe(&runtime, &mut ids, &[1])?;
                survivor.fill([0.0; 2]);
                runtime.process_block_with_clock_and_occurrence_gains(
                    &[],
                    &mut survivor,
                    None,
                    &survivor_gain,
                    &resources,
                )?;
                runtime.commit_block();
                observe(&runtime, &mut ids, &[])?;
                inherited.begin_block();
                copied.fill([0.0; 2]);
                inherited.process_block_with_clock_and_occurrence_gains(
                    &[],
                    &mut copied,
                    None,
                    &survivor_gain,
                    &resources,
                )?;
                inherited.commit_block();
                observe(&inherited, &mut ids, &[])?;
                Ok(())
            })();
            if let Err(error) = result {
                failure = Some(error);
                break;
            }
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
    assert_eq!(tail, [[2.0; 2]; 25]);
    assert_eq!(
        replay, tail,
        "rollback and read-only observation retain sample history"
    );
    assert_eq!(refused, [[9.0; 2]; 2]);
    assert_eq!(survivor, [[0.5; 2]; 2]);
    assert_eq!(copied, survivor);
    for (index, actual) in frames.iter().enumerate() {
        let expected = if index < 2 {
            1.0
        } else {
            (1.0 + index as f64 / 7.0) as f32
        };
        assert_eq!(*actual, [expected; 2]);
    }
}

#[test]
fn resident_sampler_voice_steal_declick_allocates_and_frees_nothing_in_the_actual_pair() {
    use hex_devices::sampler::{Mode, PreparedSamplerPlan, Region, SampleChannels, SampleShape};
    use hex_lattice::{
        NativeInstrumentEvent, NativeInstrumentRuntimeError, NativeInstrumentRuntimeLimits,
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
    let mut settings = hex_devices::sampler::Settings::new(Region { start: 0, end: 32 });
    settings.mode = Mode::Classic {
        loop_region: Some(Region { start: 0, end: 32 }),
    };
    settings.envelope.release_seconds = 64.0 / 8_000.0;
    let prepared = PreparedSamplerPlan::new(
        SampleShape::new(32, 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(views[0])
    .unwrap()
    .prepare_runtime_on_control(1, 128, &TuningTable::default())
    .unwrap();
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 128,
        max_active_occurrences: 1,
        max_tail_frames: 8_000 * 600,
    };
    let mut runtime = NativeInstrumentRuntimePair::new(&prepared, limits).unwrap();
    assert!(runtime.steals_releasing_voices());
    let note = |frame_offset, occurrence_id| NativeInstrumentEvent::NoteOn {
        frame_offset,
        occurrence_id,
        pitch: 60,
        velocity: 127,
    };
    // A one-voice runtime: note 2 steals releasing note 1 into the fade pool.
    let steal = [
        note(0, 1),
        NativeInstrumentEvent::NoteOff {
            frame_offset: 4,
            occurrence_id: 1,
        },
        note(8, 2),
    ];
    let held = [note(0, 1), note(2, 2)];
    // `steals` same-frame steals; one more than the fade pool drops its onset.
    let burst = |steals: u64| {
        let mut events = vec![note(0, 1)];
        for id in 1..=steals {
            events.push(NativeInstrumentEvent::NoteOff {
                frame_offset: 0,
                occurrence_id: id,
            });
            events.push(note(0, id + 1));
        }
        events
    };
    let pool = hex_devices::sampler::MAX_SAMPLER_RUNTIME_VOICES as u64;
    let full_burst = burst(pool);
    let over_burst = burst(pool + 1);
    let mut burst_output = [[0.0; 2]; 24];
    let mut over_output = burst_output;
    let mut speculative = [[0.0; 2]; 24];
    let mut committed = speculative;
    let mut refused = [[9.0; 2]; 4];
    let mut ids = [0; 1];
    let mut failure = None;
    // Construction, resource binding, buffers and oracles stay outside. Measure
    // preflight, the steal, the declick, rollback, the held-capacity refusal and
    // the fade-pool burst and its dropped onset.
    let measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            let result = (|| {
                runtime.reset();
                runtime.begin_block();
                if runtime.process_block(&held, &mut refused, &resources)
                    != Err(NativeInstrumentRuntimeError::EventCapacityExceeded)
                {
                    return Err(NativeInstrumentRuntimeError::InvalidEventBatch);
                }
                runtime.rollback_block();
                runtime.begin_block();
                over_output.fill([0.0; 2]);
                runtime.process_block(&over_burst, &mut over_output, &resources)?;
                runtime.rollback_block();
                runtime.begin_block();
                burst_output.fill([0.0; 2]);
                runtime.process_block(&full_burst, &mut burst_output, &resources)?;
                runtime.commit_block();
                runtime.reset();
                runtime.begin_block();
                speculative.fill([0.0; 2]);
                runtime.process_block(
                    std::hint::black_box(&steal),
                    &mut speculative,
                    &resources,
                )?;
                runtime.rollback_block();
                runtime.begin_block();
                committed.fill([0.0; 2]);
                runtime.process_block(std::hint::black_box(&steal), &mut committed, &resources)?;
                runtime.commit_block();
                if runtime.active_occurrence_ids(&mut ids)? != 1 || ids[0] != 2 {
                    return Err(NativeInstrumentRuntimeError::InvalidActiveOccurrenceIds);
                }
                Ok(())
            })();
            if let Err(error) = result {
                failure = Some(error);
                break;
            }
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
    assert_eq!(refused, [[9.0; 2]; 4]);
    assert!((burst_output[0][0] - (1.0 + pool as f32)).abs() < 1e-4);
    assert!(
        (burst_output[23][0] - 1.0).abs() < 1e-6,
        "burst declick complete"
    );
    // The dropped onset leaves the releasing slot voice and every fade intact.
    assert!((over_output[0][0] - (1.0 + pool as f32)).abs() < 1e-4);
    assert!((over_output[23][0] - (1.0 - 23.0 / 64.0)).abs() < 1e-6);
    assert_eq!(speculative, committed);
    // Frame8 carries both the new onset and the stolen voice at its release level.
    assert!((committed[8][0] - (1.0 + (1.0 - 4.0 / 64.0))).abs() < 1e-6);
    assert!((committed[23][0] - 1.0).abs() < 1e-6, "declick complete");
}

#[test]
fn sampler_loop_crossfade_renders_through_repeated_seams_without_allocations_or_frees() {
    use hex_devices::sampler::{Mode, PreparedSamplerPlan, Region, SampleChannels, SampleShape};
    use hex_lattice::{
        NativeInstrumentEvent, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
    };
    // A ramp makes the 8..32 loop seam a large step that the crossfade hides.
    let pcm: [f32; 32] = std::array::from_fn(|n| n as f32 / 32.0);
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
    let mut settings = hex_devices::sampler::Settings::new(Region { start: 0, end: 32 });
    settings.mode = Mode::Classic {
        loop_region: Some(Region { start: 8, end: 32 }),
    };
    // 8 source frames at 8 kHz: the whole pre-roll.
    settings.loop_crossfade_ms = 1.0;
    let prepared = PreparedSamplerPlan::new(
        SampleShape::new(32, 8_000, SampleChannels::Mono).unwrap(),
        8_000,
        settings,
    )
    .unwrap()
    .bind_source_on_control(views[0])
    .unwrap()
    .prepare_runtime_on_control(1, 128, &TuningTable::default())
    .unwrap();
    let limits = NativeInstrumentRuntimeLimits {
        max_pair_bytes: 2 * 1024 * 1024,
        max_events_per_block: 128,
        max_active_occurrences: 1,
        max_tail_frames: 8_000 * 600,
    };
    // 60 renders at unity (linear interpolation); 72 at 2x (band-limited).
    for (pitch, bound) in [(60, 0.2), (72, 0.35)] {
        let mut runtime = NativeInstrumentRuntimePair::new(&prepared, limits).unwrap();
        let onset = [NativeInstrumentEvent::NoteOn {
            frame_offset: 0,
            occurrence_id: 1,
            pitch,
            velocity: 127,
        }];
        // 16 blocks of 24 frames cross the 24-frame loop seam 15 times.
        let mut output = [[[0.0; 2]; 24]; 16];
        let mut failure = None;
        // Construction, binding and buffers stay outside; measure the rendering.
        let measurement = allocation_counter::measure(|| {
            runtime.reset();
            for (index, block) in output.iter_mut().enumerate() {
                runtime.begin_block();
                let events: &[NativeInstrumentEvent] = if index == 0 { &onset } else { &[] };
                if let Err(error) =
                    runtime.process_block(std::hint::black_box(events), block, &resources)
                {
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
        // The ramp's slope is 1/32 per source frame; a hard seam would jump by
        // 24/32. At 2x the 8-frame fade spans 4 output frames.
        let frames: Vec<f32> = output.iter().flatten().map(|frame| frame[0]).collect();
        let step = frames
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max);
        assert!(step < bound, "{pitch}: {step}");
        assert!(frames[380] > 0.0, "the loop keeps sounding");
    }
}
