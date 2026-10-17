//! Real borrowed PCM through analytic/effect/mutable callbacks; no host lease admission.
use hex_lattice::{
    BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity, DeviceResourcePcm,
    DeviceResourceView, TuningTable, conformance::resource_pcm,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};
#[test]
fn freshly_prepared_clip_pcm_renders_and_seeks_without_callback_allocation_or_free() {
    use hex_engine::{
        AudioRootStorageLedger, AudioStorageLedgerReservation, AudioStorageRole, OwnedAudioSource,
    };
    use hex_project::{Command, Created, Editor, Project};
    use std::{collections::BTreeMap, sync::Arc};
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported constructor; no native prepared clip proof");
        return;
    }
    let mut editor = Editor::new(Project::new(16_000).unwrap(), 8).unwrap();
    let Created::Imported { asset, clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "prepared pcm".into(),
            path: "prepared.wav".into(),
            sample_rate: 16_000,
            frames: 512,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::SetAudioClipSourceGain {
            clip,
            points: vec![
                hex_project::SourceGainPoint {
                    frame: 0,
                    gain_q16: 32768,
                },
                hex_project::SourceGainPoint {
                    frame: 512,
                    gain_q16: 32768,
                },
            ],
        })
        .unwrap();
    let original = OwnedAudioSource::try_resident(16_000, vec![[0.5, -0.25]; 512])
        .ok()
        .unwrap()
        .try_into_native_audio_on_control()
        .ok()
        .unwrap();
    let media = BTreeMap::from([(asset, original)]);
    let transforms = hex_engine::audio_transform::AudioTransformPreparation::default();
    let revision = hex_engine::PreparedRevision::prepare_with_audio_transforms(
        editor.project(),
        &media,
        Arc::new(hex_engine::SineTable::new()),
        None,
        &transforms,
    )
    .unwrap();
    let prepared = &revision.arrangement().audio_clip_views()[0].audio;
    assert!(matches!(
        OwnedAudioSource::alias_native_audio_on_control(prepared, 4096),
        Err(hex_engine::AudioStorageError::UnknownStorage)
    ));
    let mut ledger = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        data_rows: 16,
        handle_rows: 8,
        root_rows: 8,
        max_retained_bytes: 65_536,
        max_construction_peak_bytes: 65_536,
    })
    .unwrap();
    ledger
        .borrow_native_audio_on_control(prepared, AudioStorageRole::Current)
        .unwrap();
    ledger.summary().unwrap().require_known_storage().unwrap();
    assert!(
        ledger
            .validate_selected_shared_handles_on_control()
            .is_err()
    ); // live graph/cache owners remain
    drop(ledger);
    let mut renderer = hex_engine::Renderer::new(revision.arrangement());
    renderer.play();
    let mut output = [[0.0; 2]; 512];
    renderer.render(&mut output).unwrap(); // warm exact renderer before measurement
    let mut sums = [0.0_f32; 2];
    let measurement = allocation_counter::measure(|| {
        for _ in 0..64 {
            renderer.seek(0);
            for block in output.chunks_mut(17) {
                renderer.render(block).unwrap();
            }
            for actual in &output {
                let expected = [0.25, -0.125];
                assert_eq!(*actual, expected);
                sums[0] += actual[0];
                sums[1] += actual[1];
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
    assert_eq!(sums, [8192.0, -4096.0]);
    // Actual renderer/cache/revision destruction stays on this control thread.
}
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
    let first = match OwnedAudioSource::try_resident_mono(44_100, vec![0.25; 64]) {
        Ok(source) => source,
        Err(refusal) => {
            assert_eq!(
                refusal.reason,
                hex_engine::AudioStorageError::UnsupportedConstructor
            );
            eprintln!("native bank callback probe: unsupported constructor; no ownership proof");
            return;
        }
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
        .enumerate()
        .map(|(index, &asset)| OriginalResourceSource {
            layout: if index == 0 {
                hex_engine::native_device_resources::OriginalResourceLayout::Mono
            } else {
                hex_engine::native_device_resources::OriginalResourceLayout::Stereo
            },
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
    let owner = hex_engine::native_device_resources::DeviceResourceBank::try_new_on_control(
        bank,
        catalog,
        65_536,
        65_536,
        &AtomicBool::new(false),
    )
    .ok()
    .unwrap();
    let lease = owner.prepare_lease_on_control(&state, 4096).unwrap();
    verify_callbacks(&state, &lease);
    assert_eq!(owner.input_storage_summary(), before);
    // Actual media/source-plan bridge: clone only the closed source handles
    // into a candidate bank, then retire all old root/lease custody on control.
    let originals: Vec<_> = ids
        .iter()
        .map(|&id| owner.borrow_original_source_on_control(id, 4096).unwrap())
        .collect();
    let candidate_sources: Vec<_> = ids
        .iter()
        .zip(&originals)
        .map(|(&id, source)| (id, source.alias_for_resource_plan_on_control(4096).unwrap()))
        .collect();
    let candidate_bank = OwnedAudioInputs::try_new(
        candidate_sources,
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
    let candidate_catalog = DeviceResourceCatalog::prepare_on_control(
        &candidate_bank,
        &sources,
        ResourceCatalogReservation {
            metadata_bytes: 65_536,
            hash_frames: 128,
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    let candidate = hex_engine::native_device_resources::DeviceResourceBank::try_new_on_control(
        candidate_bank,
        candidate_catalog,
        65_536,
        65_536,
        &AtomicBool::new(false),
    )
    .ok()
    .unwrap();
    let candidate_lease = candidate.prepare_lease_on_control(&state, 4096).unwrap();
    drop(originals);
    drop(lease);
    drop(owner);
    verify_callbacks(&state, &candidate_lease);
    // Real bank -> scoped inline bank copy -> retained/current source owners.
    // Both configurations reuse slot1 with different typed original sources.
    let configuration = |id| {
        let state = state
            .clone()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(id).unwrap(),
            }])
            .unwrap();
        let resources = candidate.prepare_lease_on_control(&state, 4096).unwrap();
        let note = resource_pcm::prepare_note(
            &state,
            &DevicePreparationContext {
                sample_rate: 44_100,
                tuning: &TuningTable::default(),
                resources: &resources,
            },
            0,
        )
        .unwrap();
        hex_engine::InlineResourceFixtureConfiguration {
            state,
            resources,
            note,
        }
    };
    let fixture = hex_engine::InlineResourceHostFixture::prepare_on_control(
        44_100,
        64,
        configuration(ids[1].value()),
        configuration(ids[0].value()),
    )
    .unwrap();
    drop(candidate_lease);
    drop(candidate); // only actual current/retained configuration owners remain
    let mut sum = [0.0_f32; 2];
    let measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            for age in 0..64 {
                let current = fixture.sample_current(age, 64).unwrap();
                let carried = fixture.sample_carried(age, 64).unwrap();
                assert_eq!(current, [0.5, -0.75]);
                assert_eq!(carried, [0.25; 2]);
                sum[0] += current[0] + carried[0];
                sum[1] += current[1] + carried[1];
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
    drop(fixture); // actual configuration/root teardown on control
    eprintln!("native bank callback probe: supported-controls=completed");
}
