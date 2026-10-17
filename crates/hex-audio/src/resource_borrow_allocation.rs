//! Real borrowed PCM through analytic/effect/mutable callbacks; no host lease admission.
use hex_lattice::{
    BorrowedDeviceResources, DevicePreparationContext, DeviceResourceIdentity, DeviceResourcePcm,
    DeviceResourceView, TuningTable, conformance::resource_pcm,
};
use hex_lattice_contract::{
    DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot, DeviceTypeId, NativeDeviceState,
};

#[test]
fn sampler_pair_event_history_and_original_pcm_queries_allocate_and_free_nothing() {
    use hex_lattice::{
        NativeInstrumentEvent, NativeInstrumentRuntimeLimits, NativeInstrumentRuntimePair,
    };
    let mono = [0.5, 1.0, -0.25, 0.75];
    let stereo = mono.map(|value| [value, -value]);
    for pcm in [
        DeviceResourcePcm::Mono(&mono),
        DeviceResourcePcm::Stereo(&stereo),
    ] {
        let views = [DeviceResourceView {
            slot: DeviceAssetSlot::new(1).unwrap(),
            identity: DeviceResourceIdentity {
                asset: DeviceAssetKey::new(1).unwrap(),
                content: [2; 32],
                derivation_version: 1,
                derivation: [3; 32],
            },
            sample_rate: 8_000,
            pcm,
        }];
        let resources = BorrowedDeviceResources(&views);
        let prepared = hex_engine::sampler_runtime_for_allocation_probe(views[0], 8_000).unwrap();
        let mut runtime = NativeInstrumentRuntimePair::new(
            prepared.as_ref(),
            NativeInstrumentRuntimeLimits {
                max_pair_bytes: 2 * 1024 * 1024,
                max_events_per_block: 259,
                max_active_occurrences: 32,
                max_tail_frames: 8_000 * 600,
            },
        )
        .unwrap();
        let mut checksum = [0.0_f64; 2];
        let measurement = allocation_counter::measure(|| {
            for occurrence_id in 1..=128 {
                let events = [
                    NativeInstrumentEvent::NoteOn {
                        frame_offset: 0,
                        occurrence_id,
                        pitch: 60,
                        velocity: 127,
                    },
                    NativeInstrumentEvent::NoteOff {
                        frame_offset: 8,
                        occurrence_id,
                    },
                ];
                let mut speculative = [[0.0; 2]; 16];
                runtime.begin_block();
                runtime
                    .process_block(&events, &mut speculative, &resources)
                    .unwrap();
                runtime.rollback_block();
                let mut committed = [[0.0; 2]; 16];
                runtime.begin_block();
                runtime
                    .process_block(&events, &mut committed, &resources)
                    .unwrap();
                runtime.commit_block();
                assert_eq!(speculative, committed);
                for frame in committed {
                    for channel in 0..2 {
                        checksum[channel] += f64::from(frame[channel]);
                    }
                }
            }
        });
        let expected_right = match pcm {
            DeviceResourcePcm::Mono(_) => 512.0,
            DeviceResourcePcm::Stereo(_) => -512.0,
        };
        assert_eq!(checksum, [512.0, expected_right]);
        assert_eq!(
            (
                measurement.count_total,
                measurement.bytes_total,
                measurement.count_current,
                measurement.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
}
#[test]
fn canonical_midi_control_inventory_has_zero_allocations_and_unchanged_owner_counts() {
    use hex_engine::{AudioRootStorageLedger, AudioStorageLedgerReservation};
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    if !hex_engine::canonical_midi_storage_layout_supported() {
        return;
    }
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "inventory".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(4).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::from_quarters(1).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor
        .execute(Command::RenameTrack {
            track,
            name: "retained".into(),
        })
        .unwrap();
    let identity = editor.project().midi_clip(clip).unwrap().source_identity();
    let before = identity.source_storage_on_control().owning_counts();
    let mut ledger = AudioRootStorageLedger::try_new_on_control(AudioStorageLedgerReservation {
        data_rows: 64,
        handle_rows: 64,
        root_rows: 4,
        max_retained_bytes: 100_000,
        max_construction_peak_bytes: 100_000,
    })
    .unwrap();
    let measurement = allocation_counter::measure(|| {
        ledger
            .borrow_editor_midi_sources_on_control(&editor)
            .unwrap();
        ledger
            .borrow_midi_source_identity_on_control(&identity)
            .unwrap();
        ledger
            .validate_selected_shared_handles_on_control()
            .unwrap();
        ledger.summary().unwrap().require_known_storage().unwrap();
        assert_eq!(identity.source_storage_on_control().owning_counts(), before);
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
    assert_eq!(identity.source_storage_on_control().owning_counts(), before);
    // Actual owners/ledger are destroyed on control outside the measured view.
}
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
    let prepared_effect = |id| {
        let state = state
            .clone()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(id).unwrap(),
            }])
            .unwrap();
        let resources = candidate
            .prepare_configuration_on_control(&state, 4096)
            .unwrap();
        let prepared = resource_pcm::prepare_effect(
            &state,
            &DevicePreparationContext {
                sample_rate: 44_100,
                tuning: &TuningTable::default(),
                resources: &resources,
            },
            0,
        )
        .unwrap();
        hex_lattice::PreparedScopedRuntimeInstance {
            resources,
            instance: hex_lattice::PreparedRuntimeInstance {
                meter: std::sync::Arc::new(hex_lattice::DeviceMeter::default()),
                owner: hex_lattice::ProcessorChainOwner::Track(1),
                device_id: 1,
                device_type: state.type_id(),
                matrix: hex_lattice::StereoMatrix::identity(),
                bypassed: false,
                sample_rate: 44_100,
                configuration: Some(std::sync::Arc::new(state)),
                definition: Some(hex_lattice::PreparedProcessorFactory::Legacy(
                    std::sync::Arc::new(ScopedPcmEffect(prepared)),
                )),
            },
        }
    };
    let effect =
        |id| hex_lattice::NativeRuntimeChain::new_prepared(vec![prepared_effect(id)]).unwrap();
    let mut effects = [effect(ids[0].value()), effect(ids[1].value())];
    let renderer_project = |id| {
        let prepared = prepared_effect(id);
        let track = hex_engine::Track::new(
            vec![
                hex_engine::Clip::new(
                    hex_engine::Audio::new(44_100, vec![[1.0; 2]; 64]).unwrap(),
                    0,
                    64,
                    false,
                )
                .unwrap(),
            ],
            1.0,
            false,
        )
        .unwrap()
        .with_test_effect_scopes_on_control(vec![hex_engine::PreparedEffectInstance {
            instance: prepared.instance,
            resources: Some(prepared.resources),
        }])
        .unwrap();
        hex_engine::Project::new(44_100, vec![track]).unwrap()
    };
    let renderer_projects = [
        renderer_project(ids[0].value()),
        renderer_project(ids[1].value()),
    ];
    let mut renderers = renderer_projects.each_ref().map(hex_engine::Renderer::new);
    for renderer in &mut renderers {
        renderer.play();
    }

    let mutable = |id| {
        let state = state
            .clone()
            .with_asset_references(vec![DeviceAssetReference {
                slot: DeviceAssetSlot::new(1).unwrap(),
                asset: DeviceAssetKey::new(id).unwrap(),
            }])
            .unwrap();
        let resources = candidate
            .prepare_configuration_on_control(&state, 4096)
            .unwrap();
        hex_engine::MutableResourceHostFixture::prepare_on_control(&state, 44_100, resources)
            .unwrap()
    };
    let mut mutable_hosts = [mutable(ids[0].value()), mutable(ids[1].value())];
    drop(candidate_lease);
    drop(candidate); // only actual current/retained configuration owners remain
    let mut mutable_sum = [0.0_f32; 2];
    let mut mutable_output = [[0.0_f32; 2]; 64];
    let mut speculative = [[0.0_f32; 2]; 17];
    let mutable_measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            for (index, host) in mutable_hosts.iter_mut().enumerate() {
                host.reset();
                speculative.fill([0.0; 2]);
                host.process_segment(0, &mut speculative, false).unwrap();
                mutable_output.fill([0.0; 2]);
                for (partition, block) in mutable_output.chunks_mut(17).enumerate() {
                    host.process_segment((partition * 17) as u64, block, true)
                        .unwrap();
                }
                assert_eq!(&mutable_output[..17], &speculative);
                for actual in mutable_output {
                    assert_eq!(actual, if index == 0 { [0.25; 2] } else { [0.5, -0.75] });
                    mutable_sum[0] += actual[0];
                    mutable_sum[1] += actual[1];
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
    let mut effect_sum = [0.0_f32; 2];
    let mut effect_output = [[1.0_f32; 2]; 64];
    let effect_measurement = allocation_counter::measure(|| {
        for _ in 0..32 {
            for (index, effect) in effects.iter_mut().enumerate() {
                effect.reset();
                effect.begin_block();
                effect_output.fill([1.0; 2]);
                for block in effect_output.chunks_mut(17) {
                    effect.process_block(block);
                }
                effect.commit_block();
                for actual in effect_output {
                    assert_eq!(actual, if index == 0 { [0.25; 2] } else { [0.5, -0.75] });
                    effect_sum[0] += actual[0];
                    effect_sum[1] += actual[1];
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
    // Actual prepared Track -> RendererRuntime -> paired entry scope path.
    // Source bank is already retired; each original is still held by its real
    // prepared source and runtime fields. This unregistered fixture grants no
    // native graph admission and does not copy original resource PCM.
    let mut renderer_sum = [0.0_f32; 2];
    let mut rendered = [[0.0_f32; 2]; 64];
    let measured = allocation_counter::measure(|| {
        for _ in 0..32 {
            for (index, renderer) in renderers.iter_mut().enumerate() {
                renderer.seek(0);
                for block in rendered.chunks_mut(17) {
                    renderer.render(block).unwrap();
                }
                for frame in rendered {
                    assert_eq!(frame, if index == 0 { [0.25; 2] } else { [0.5, -0.75] });
                    renderer_sum[0] += frame[0];
                    renderer_sum[1] += frame[1];
                }
            }
        }
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(renderer_sum, [1536.0, -1024.0]);
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

// Scalar-only factory; actual host entries own the resource configuration.
struct ScopedPcmEffect(resource_pcm::BorrowedPcmEffect);
impl hex_lattice::PreparedNativeProcessor for ScopedPcmEffect {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<resource_pcm::BorrowedPcmEffect>()
    }
    fn create_runtime(&self) -> Box<dyn hex_lattice::NativeProcessorRuntime> {
        Box::new(self.0.clone())
    }
}

#[test]
fn actual_mutable_host_replays_nonconstant_pcm_after_speculation_without_callback_allocation() {
    use hex_engine::native_device_resources::{
        DeviceResourceBank, DeviceResourceCatalog, OriginalResourceLayout, OriginalResourceSource,
        ResourceCatalogReservation,
    };
    use hex_engine::{AudioStorageReservation, OwnedAudioInputs, OwnedAudioSource};
    use std::sync::atomic::AtomicBool;
    if !hex_engine::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported constructor; no mutable host ownership proof");
        return;
    }
    let expected: [[f32; 2]; 64] =
        std::array::from_fn(|i| [i as f32 / 64.0, (63 - i) as f32 / -128.0]);
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(44_100).unwrap(), 8).unwrap();
    let hex_project::Created::Asset(asset) = editor
        .execute(hex_project::Command::AddAsset {
            path: "mutable-ramp.wav".into(),
            sample_rate: 44_100,
            frames: 64,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let source = OwnedAudioSource::try_resident(44_100, expected.to_vec())
        .ok()
        .unwrap();
    let inputs = OwnedAudioInputs::try_new(
        vec![(asset, source)],
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
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &inputs,
        &[OriginalResourceSource {
            asset,
            sample_rate: 44_100,
            frames: 64,
            layout: OriginalResourceLayout::Stereo,
        }],
        ResourceCatalogReservation {
            metadata_bytes: 65_536,
            hash_frames: 64,
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    let bank = DeviceResourceBank::try_new_on_control(
        inputs,
        catalog,
        65_536,
        65_536,
        &AtomicBool::new(false),
    )
    .ok()
    .unwrap();
    let state = NativeDeviceState::new(DeviceTypeId::from_static("test.mutable-ramp"), 1, vec![])
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    let configuration = bank.prepare_configuration_on_control(&state, 4096).unwrap();
    let mut host =
        hex_engine::MutableResourceHostFixture::prepare_on_control(&state, 44_100, configuration)
            .unwrap();
    drop(bank); // Only the actual host's retained configuration now owns PCM.
    let mut output = [[0.0_f32; 2]; 64];
    let mut speculative = [[0.0_f32; 2]; 17];
    let mut silence = [[0.0_f32; 2]; 3];
    let measurement = allocation_counter::measure(|| {
        for _ in 0..16 {
            host.reset();
            speculative.fill([0.0; 2]);
            host.process_segment(0, &mut speculative, false).unwrap();
            assert_eq!(speculative, expected[..17]);
            output.fill([0.0; 2]);
            for (index, block) in output.chunks_mut(17).enumerate() {
                host.process_segment((index * 17) as u64, block, true)
                    .unwrap();
            }
            assert_eq!(output, expected);
            silence.fill([0.0; 2]);
            host.process_segment(64, &mut silence, true).unwrap();
            assert_eq!(silence, [[0.0; 2]; 3]);
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
}
