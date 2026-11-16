//! Actual ordinary resource book + retained verified file + native queue/return
//! owners. This is headless source-boundary evidence, not a physical device run.
use super::*;
use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
use std::collections::BTreeMap;
fn resource_limits() -> hex_engine::AudioStorageLedgerReservation {
    hex_engine::AudioStorageLedgerReservation {
        data_rows: 512,
        handle_rows: 256,
        root_rows: 128,
        max_retained_bytes: 20_000_000,
        max_construction_peak_bytes: 20_000_000,
    }
}
fn compile(
    project: &hex_project::Project,
    metadata: hex_engine::StreamedSourceMetadata,
    store: &ResidentDeviceAssets,
) -> Playback {
    compile_sources(project, vec![metadata], store)
}
fn compile_sources(
    project: &hex_project::Project,
    metadata: Vec<hex_engine::StreamedSourceMetadata>,
    store: &ResidentDeviceAssets,
) -> Playback {
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(store, project, 1_000_000, &cancel).unwrap();
    let generation = metadata[0].key().generation();
    let sources = metadata
        .into_iter()
        .map(|metadata| {
            (
                metadata.key().asset(),
                hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(metadata)),
            )
        })
        .collect();
    let revision =
        hex_engine::PreparedRevision::prepare_with_prepared_audio_sources_and_device_resources(
            project,
            &sources,
            generation,
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            100_000,
            &cancel,
        )
        .unwrap();
    Playback::try_from_prepared_with_device_resources(Arc::new(revision), &resources).unwrap()
}
fn measured_block(
    data: &mut crate::native_callback_owner::Guard<
        crate::native_callback_owner::NativeCallbackOwner,
    >,
) -> [Frame; 8] {
    let mut output = [[1.0; 2]; 8];
    measured_render(data, &mut output);
    output
}
fn measured_render(
    data: &mut crate::native_callback_owner::Guard<
        crate::native_callback_owner::NativeCallbackOwner,
    >,
    output: &mut [Frame],
) {
    let counted = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(output).unwrap();
        });
    });
    assert_eq!(
        (
            counted.count_total,
            counted.bytes_total,
            counted.count_current,
            counted.bytes_current
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn streamed_retained_restart_under_live_fence_keeps_audio_and_cursor() {
    use hex_project::{Command, Created, Editor, Project, TrackDeviceSlot};

    let _serial = serial();
    let frames = 12_288;
    let mut editor = Editor::new(super::readiness::project_with_frames(frames), 16).unwrap();
    let Created::Track(receiver) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("receiver track")
    };
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(peer) = editor
        .execute(Command::AddMidiTrack {
            name: "peer".into(),
        })
        .unwrap()
    else {
        panic!("peer track")
    };
    editor
        .execute(Command::BindTrackSource {
            track: peer,
            source: Some(source),
        })
        .unwrap();
    let original = editor.project().clone();
    let mut snapshot = original.snapshot();
    snapshot.retained_source_paths.push(receiver.value());
    let retained = Project::from_snapshot(snapshot).unwrap();
    let asset = original.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(frames as u32);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let store = ResidentDeviceAssets::default();
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            2,
            4096,
        )
        .unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair_with_pool(vec![definition], 3),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|_| panic!("source registration"));
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let revision = control
        .publish_with_admission_and_source_on_control(
            compile(&original, metadata.clone(), &store),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 32];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while control.status.position.load(Acquire) < 64 {
        measured_render(&mut data, &mut output);
        assert!(
            std::time::Instant::now() < deadline,
            "initial stream readiness"
        );
        std::thread::yield_now();
    }
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    let mut unfenced = compile(&retained, metadata.clone(), &store);
    assert!(unfenced.request_live_restart());
    let refused = control
        .publish_with_admission_and_source_on_control(
            unfenced,
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    while control
        .status
        .rejected_revision()
        .is_none_or(|(id, _)| id != refused)
    {
        measured_render(&mut data, &mut output);
        assert!(std::time::Instant::now() < deadline, "unfenced refusal");
        std::thread::yield_now();
    }
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    control.reclaim();

    let stale = control
        .request_live_arrangement_edit_fence(7, revision)
        .unwrap();
    measured_render(&mut data, &mut output);
    assert_eq!(control.arrangement_edit_playing(stale), Some(true));
    let mut cancelled = compile(&retained, metadata.clone(), &store);
    assert!(cancelled.request_live_restart());
    let cancelled_revision = control
        .publish_with_admission_and_source_on_control(
            cancelled,
            PublicationPolicy::InheritUnderArrangement(stale),
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    control.release_stopped_edit_fence(stale);
    measured_render(&mut data, &mut output);
    assert!(
        control
            .status
            .rejected_revision()
            .is_some_and(|(id, _)| id == cancelled_revision)
    );
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    control.reclaim();

    let fence = control
        .request_live_arrangement_edit_fence(7, revision)
        .unwrap();
    measured_render(&mut data, &mut output);
    assert_eq!(control.arrangement_edit_playing(fence), Some(true));
    let mut candidate = compile(&retained, metadata.clone(), &store);
    assert!(candidate.request_live_restart());
    let next = control
        .publish_with_admission_and_source_on_control(
            candidate,
            PublicationPolicy::InheritUnderArrangement(fence),
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    let install_position = loop {
        let before = control.status.position.load(Acquire);
        measured_render(&mut data, &mut output);
        assert!(
            control
                .status
                .rejected_revision()
                .is_none_or(|(id, _)| id != next),
            "fenced streamed restart refused: {:?}",
            control.status.rejected_revision()
        );
        if control.status.applied_revision.load(Acquire) == next {
            break before;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fenced restart readiness"
        );
        std::thread::yield_now();
    };
    let audio = hex_engine::Audio::new(48_000, vec![[0.25; 2]; frames as usize]).unwrap();
    let resources = PreparedDeviceResources::prepare_on_control(
        &store,
        &retained,
        1_000_000,
        &AtomicBool::new(false),
    )
    .unwrap();
    let resident = hex_engine::PreparedRevision::prepare_with_device_resources(
        &retained,
        &BTreeMap::from([(asset, audio)]),
        Arc::new(hex_engine::SineTable::new()),
        None,
        &hex_engine::audio_transform::AudioTransformPreparation::default(),
        &resources,
        100_000,
        &AtomicBool::new(false),
    )
    .unwrap();
    let mut oracle =
        Playback::try_from_prepared_with_device_resources(Arc::new(resident), &resources).unwrap();
    oracle.apply(PlaybackCommand::PlayFrom(install_position));
    let mut expected = [[0.0; 2]; 32];
    oracle.render(&mut expected).unwrap();
    assert_eq!(output, expected);
    assert_eq!(control.status.position.load(Acquire), install_position + 32);
    assert!(control.status.playing.load(Acquire));
    control.release_stopped_edit_fence(fence);
    control.reclaim();

    let stop_fence = control
        .request_live_arrangement_edit_fence(7, next)
        .unwrap();
    measured_render(&mut data, &mut output);
    assert_eq!(control.arrangement_edit_playing(stop_fence), Some(true));
    let mut stop_candidate = compile(&original, metadata, &store);
    assert!(stop_candidate.request_live_restart());
    let stop_revision = control
        .publish_with_admission_and_source_on_control(
            stop_candidate,
            PublicationPolicy::InheritUnderArrangement(stop_fence),
            hex_engine::AudioSourceContext {
                editor_revision: 5,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    control.stop();
    while control.status.applied_revision.load(Acquire) != stop_revision
        || control.status.playing.load(Acquire)
    {
        measured_render(&mut data, &mut output);
        assert!(
            control
                .status
                .rejected_revision()
                .is_none_or(|(id, _)| id != stop_revision)
        );
        assert!(
            std::time::Instant::now() < deadline,
            "priority stop and restart"
        );
        std::thread::yield_now();
    }
    assert_eq!(control.status.position.load(Acquire), 0);
    control.release_stopped_edit_fence(stop_fence);
    assert!(control.retire_audio_candidate_domain(domain));
    control.request_shutdown();
    while control.recover_audio_candidate_domain(domain).is_none() {
        measured_render(&mut data, &mut output);
        control.reclaim();
        assert!(std::time::Instant::now() < deadline, "source retirement");
        std::thread::yield_now();
    }
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}
#[test]
fn ordinary_streamed_actions_deliver_split_client_outcomes() {
    let _serial = serial();
    let project = super::readiness::project_with_frames(12_288);
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(12_288);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let store = ResidentDeviceAssets::default();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = control
        .enable_action_outcomes_on_control(&mut callback)
        .unwrap();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut publication) =
        crate::native_callback_owner::reserve_native_worker(control, callback, None);
    let mut retirement =
        crate::native_callback_owner::NativeRetirement::new(terminals, root.clone());
    let control = publication.control().unwrap();
    control.enable_native_custody().unwrap();
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            3,
            4096,
        )
        .unwrap();
    let _domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair(definition),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .ok()
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let revision = control
        .publish_with_admission_and_source_on_control(
            compile(&project, metadata.clone(), &store),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    client.activate();
    let play = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while client.status().unwrap().applied_revision.load(Acquire) != revision {
        measured_block(&mut data);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket: play,
            outcome: ActionOutcome::Applied
        })
    );
    data.owner_mut()
        .unwrap()
        .run(|callback, _| assert!(callback.streamed_transport_mode()));
    let track = project.tracks()[0].id();
    let mut take = client
        .start_sample_recording(7, &[hex_engine::OutputCaptureId::Track(track)], true)
        .unwrap();
    assert!(take.ready());
    let output = measured_block(&mut data);
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.finish_sample_recording(true));
    client.stop_sample_recording();
    measured_block(&mut data);
    let mut seen = false;
    assert_eq!(
        take.drain(|frames, _, pcm| {
            assert!(!seen);
            assert!(output.iter().flatten().any(|sample| *sample != 0.0));
            for layer in 0..2 {
                assert_eq!(
                    crate::sample_recording::Reader::source_frames(pcm, frames, layer),
                    Some(&output[..])
                );
            }
            seen = true;
        }),
        Some(crate::sample_recording::End::Stopped)
    );
    assert!(seen);
    client.reclaim_sample_recording();
    assert!(
        !client
            .status()
            .unwrap()
            .verified_streaming_mode
            .load(Acquire)
    );
    let seek = client
        .command_with_ticket(PlaybackCommand::Seek(4096))
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let pause = client.command_with_ticket(PlaybackCommand::Pause).unwrap();
    measured_block(&mut data);
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket: seek,
            outcome: ActionOutcome::Rejected(PlaybackActionRejection::StreamActionCancelled)
        })
    );
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket: pause,
            outcome: ActionOutcome::Applied
        })
    );
    let resume = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    let resume_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        measured_block(&mut data);
        if let Some(receipt) = reader.try_take() {
            assert_eq!(
                receipt,
                ActionOutcomeReceipt {
                    ticket: resume,
                    outcome: ActionOutcome::Applied
                }
            );
            break;
        }
        assert!(
            std::time::Instant::now() < resume_deadline,
            "resume current playback"
        );
        std::thread::yield_now();
    }
    let pending_seek = client
        .command_with_ticket(PlaybackCommand::Seek(8192))
        .unwrap();
    data.owner_mut().unwrap().run(|callback, _| {
        callback.begin_block();
        assert!(callback.stream_action_pending());
    });
    let successor = control
        .publish_with_admission_and_source_on_control(
            compile(&project, metadata.clone(), &store),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    let successor_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        measured_block(&mut data);
        if let Some(receipt) = reader.try_take() {
            assert_eq!(
                receipt,
                ActionOutcomeReceipt {
                    ticket: pending_seek,
                    outcome: ActionOutcome::Rejected(
                        PlaybackActionRejection::StreamActionCancelled
                    ),
                }
            );
            break;
        }
        assert!(
            std::time::Instant::now() < successor_deadline,
            "legacy action cancellation"
        );
        std::thread::yield_now();
    }
    while client.status().unwrap().applied_revision.load(Acquire) != successor {
        measured_block(&mut data);
        assert!(
            std::time::Instant::now() < successor_deadline,
            "legacy successor installation"
        );
        std::thread::yield_now();
    }
    control.reclaim();
    let stale = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    let stale_second = client
        .command_with_ticket(PlaybackCommand::PlayFrom(32))
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.report_render_fault(Some(&Invalid("injected failure")), 4096));
    let replacement = control
        .publish_with_admission_and_source_on_control(
            compile(&project, metadata, &store),
            PublicationPolicy::InstallFreshRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    measured_block(&mut data);
    assert_eq!(
        client.status().unwrap().applied_revision.load(Acquire),
        replacement
    );
    for ticket in [stale, stale_second] {
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::OutputFailed)
            })
        );
    }
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.report_render_fault(Some(&Invalid("second failure")), 0));
    let seek = client
        .command_with_ticket(PlaybackCommand::Seek(49))
        .unwrap();
    let play_from = client
        .command_with_ticket(PlaybackCommand::PlayFrom(64))
        .unwrap();
    assert_eq!(measured_block(&mut data), [[0.0; 2]; 8]);
    // Seek keeps its ordinary stopped-graph semantics; only the start is refused.
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket: seek,
            outcome: ActionOutcome::Applied
        })
    );
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket: play_from,
            outcome: ActionOutcome::Rejected(PlaybackActionRejection::OutputFailed)
        })
    );
    assert_eq!(client.status().unwrap().recovery_position(), 64);
    assert!(!client.status().unwrap().playing.load(Acquire));
    client.stop();
    assert_eq!(measured_block(&mut data), [[0.0; 2]; 8]);
    assert_eq!(client.status().unwrap().recovery_position(), 0);
    assert_eq!(reader.failure(), None);
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    crate::lifecycle::test_dispose_backend(
        &mut backend,
        &mut pending,
        &mut disposed,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    drop(client);
    drop(publication);
    finish(&mut retirement);
    while !root.test_eligible_complete() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
}
#[test]
fn ordinary_publication_reports_changed_source_after_catalog_admission() {
    let _serial = serial();
    let project = super::readiness::project_with_frames(32_768);
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(32_768);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let source_path = definition.path.clone();
    let store = ResidentDeviceAssets::default();
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            3,
            4096,
        )
        .unwrap();
    let audio = hex_engine::Audio::new(48_000, vec![[0.375; 2]; 32_768]).unwrap();
    let resources = PreparedDeviceResources::prepare_on_control(
        &store,
        &project,
        1_000_000,
        &AtomicBool::new(false),
    )
    .unwrap();
    let revision = hex_engine::PreparedRevision::prepare_with_device_resources(
        &project,
        &BTreeMap::from([(asset, audio)]),
        Arc::new(hex_engine::SineTable::new()),
        None,
        &hex_engine::audio_transform::AudioTransformPreparation::default(),
        &resources,
        100_000,
        &AtomicBool::new(false),
    )
    .unwrap();
    let donor =
        Playback::try_from_prepared_with_device_resources(Arc::new(revision), &resources).unwrap();
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    control.status.stream_source_changed.store(true, Release);
    control.status.stream_read_failed.store(true, Release);
    let donor_revision = control
        .publish_with_admission_and_source_on_control(
            donor,
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|error| panic!("donor publication: {error:?}"));
    control.command(PlaybackCommand::Play).unwrap();
    assert_eq!(measured_block(&mut data), [[0.375; 2]; 8]);
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        donor_revision
    );
    assert!(!control.status.stream_source_changed.load(Acquire));
    assert!(!control.status.take_stream_read_failure());
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair(definition),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .unwrap_or_else(|_| panic!("source registration"));
    std::fs::write(source_path, b"changed source bytes").unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let revision = control
        .publish_with_admission_and_source_on_control(
            compile(&project, metadata, &store),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("ordinary publication"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let rejection = loop {
        assert_eq!(measured_block(&mut data), [[0.375; 2]; 8]);
        if let Some(rejection) = control.status.rejected_revision() {
            break rejection;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    };
    assert_eq!(
        rejection,
        (revision, PlaybackHandoffRejection::StreamSourceChanged)
    );
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        donor_revision
    );
    assert!(control.retire_audio_candidate_domain(domain));
    control.request_shutdown();
    loop {
        measured_block(&mut data);
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}

#[test]
fn ordinary_resource_graph_requires_actual_catalog_and_protected_pages_and_returns_before_join() {
    ordinary_case(false, false, false, false);
}
#[test]
fn ordinary_sampler_and_streamed_clip_share_resource_and_page_custody_without_callback_allocations()
{
    ordinary_case(true, false, false, false);
}
#[test]
fn ordinary_paused_keyboard_monitor_and_first_play_keep_resident_pcm_and_clock_semantics() {
    ordinary_case(true, true, false, false);
}
#[test]
fn grouped_file_backed_native_playback_matches_resident_oracle_without_callback_allocations() {
    ordinary_case(false, false, true, false);
}
#[test]
fn grouped_sampler_and_file_backed_sources_keep_existing_resource_custody() {
    ordinary_case(true, false, true, false);
}
#[test]
fn stop_and_restart_with_streamed_audio_and_active_sampler_match_resident_pcm() {
    assert!(hex_engine::audio_storage_constructor_layout_supported());
    ordinary_case(true, false, false, true);
}
fn ordinary_case(sampler: bool, keyboard: bool, grouped: bool, stop_restart: bool) {
    let _serial = serial();
    let frames = if stop_restart { 8192 } else { 8 };
    let mut project = super::readiness::project_with_frames(frames as u64);
    let asset = project.assets().next().unwrap().id();
    let mut store = ResidentDeviceAssets::default();
    let mut session_clip = None;
    if sampler {
        use hex_project::{BeatTime, Command, Created, Editor};
        let mut editor = Editor::new(project, 16).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddMidiTrack {
                name: "sampler".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor
            .execute(Command::InsertInstrument {
                track,
                state:
                    hex_engine::native_device_resources::current_sampler_fixture::one_shot_state(
                        frames, asset,
                    )
                    .unwrap(),
            })
            .unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 127,
            })
            .unwrap();
        if keyboard {
            let audio_track = editor.project().tracks()[0].id();
            let Created::Scene(scene) = editor
                .execute(Command::AddScene {
                    name: "scene".into(),
                })
                .unwrap()
            else {
                panic!("scene")
            };
            let Created::SessionClip(clip) = editor
                .execute(Command::CreateSessionAudioClip {
                    track: audio_track,
                    scene,
                    name: "source".into(),
                    asset,
                    source_start: 0,
                    source_end: 8,
                })
                .unwrap()
            else {
                panic!("session clip")
            };
            session_clip = Some(clip);
        }
        project = editor.project().clone();
        store
            .insert_on_control(
                (
                    asset,
                    hex_engine::OwnedAudioSource::try_resident(48_000, vec![[0.25; 2]; frames])
                        .ok()
                        .unwrap(),
                ),
                0,
                resource_limits(),
                frames as u64,
                &AtomicBool::new(false),
                || Ok::<_, hex_engine::native_device_resources::ResourcePreparationError>(()),
            )
            .unwrap();
    }
    if grouped {
        let mut editor = hex_project::Editor::new(project, 16).unwrap();
        let ids: Vec<_> = editor
            .project()
            .tracks()
            .iter()
            .map(|track| track.id())
            .collect();
        let hex_project::Created::Track(group) = editor
            .execute(hex_project::Command::GroupTracks {
                tracks: hex_project::TrackSelectionIds::new(&ids).unwrap(),
                name: "group".into(),
            })
            .unwrap()
        else {
            panic!("group")
        };
        editor
            .execute(hex_project::Command::Gain {
                track: group,
                gain: 0.5,
            })
            .unwrap();
        editor
            .execute(hex_project::Command::SetAudioFeed {
                source: hex_project::RoutingOwner::Track(ids[0]),
                destination: group,
                gain: 0.5,
            })
            .unwrap();
        project = editor.project().clone();
    }
    let resources = PreparedDeviceResources::prepare_on_control(
        &store,
        &project,
        1_000_000,
        &AtomicBool::new(false),
    )
    .unwrap();
    let oracle_revision = hex_engine::PreparedRevision::prepare_with_device_resources(
        &project,
        &BTreeMap::from([(
            asset,
            hex_engine::Audio::new(48_000, vec![[0.25; 2]; frames]).unwrap(),
        )]),
        Arc::new(hex_engine::SineTable::new()),
        None,
        &hex_engine::audio_transform::AudioTransformPreparation::default(),
        &resources,
        100_000,
        &AtomicBool::new(false),
    )
    .unwrap();
    let oracle_revision = Arc::new(oracle_revision);
    let mut oracle =
        Playback::try_from_prepared_with_device_resources(Arc::clone(&oracle_revision), &resources)
            .unwrap();
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    if keyboard {
        input.set_key(0, Some((60, 127, 1))).unwrap();
        let target = project
            .tracks()
            .iter()
            .find(|track| track.instrument().is_some())
            .unwrap()
            .id();
        oracle
            .prepare_computer_keyboard(&project, target, Arc::clone(&input))
            .unwrap();
    } else {
        oracle
            .try_apply_action(hex_engine::PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
    }
    let mut expected = [[0.0; 2]; 8];
    oracle.render(&mut expected).unwrap();
    let compile = |metadata| {
        let mut runtime = compile(&project, metadata, &store);
        if keyboard {
            let target = project
                .tracks()
                .iter()
                .find(|track| track.instrument().is_some())
                .unwrap()
                .id();
            runtime
                .prepare_computer_keyboard(&project, target, Arc::clone(&input))
                .unwrap();
        }
        runtime
    };
    let wav = stream_fixture::Wav::with_frames(frames as u32);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    control.set_audio_diagnostics(AudioDiagnosticsFlags::TRANSPORT);
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    for (extents, metadata_bytes) in [(usize::MAX, 4096), (3, 1)] {
        let mut refused = false;
        let counted = allocation_counter::measure(|| {
            refused = control
                .configure_ordinary_streaming_on_control(
                    crate::AudioCandidateDomainReservation {
                        max_retained_data_bytes: 8 * 1024 * 1024,
                        max_construction_peak_data_bytes: 8 * 1024 * 1024,
                    },
                    extents,
                    metadata_bytes,
                )
                .is_err();
        });
        assert!(refused);
        assert_eq!(
            (
                counted.count_total,
                counted.bytes_total,
                counted.count_current,
                counted.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(
            !control.candidate_root.domains_configured(),
            "oversized refusal must precede allocation and Root mutation"
        );
    }
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            3,
            4096,
        )
        .unwrap();
    let refused = control
        .publish_with_admission_and_source_on_control(
            compile(metadata.clone()),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .err()
        .unwrap();
    assert!(matches!(refused, AdmittedPublishError::Publication(_)));
    assert_eq!(control.submitted_revision(), 0);
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair(definition),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .unwrap_or_else(|_| panic!("source registration"));
    assert!(control.audio_candidate_domain_is_sealed(domain));
    assert!(
        !control.audio_candidate_domain_endpoint_is_installed(domain),
        "catalog sealing must not stand in for callback delivery"
    );
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert!(control.audio_candidate_domain_endpoint_is_installed(domain));
    let mut revision = control
        .publish_with_admission_and_source_on_control(
            compile(metadata.clone()),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("ordinary publication"));
    let mut play = if keyboard {
        0
    } else {
        control.command_with_ticket(PlaybackCommand::Play).unwrap()
    };
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        0,
        "no graph installation before an exact render window"
    );
    assert_eq!(control.status.performance_frame.load(Acquire), 0);
    if !keyboard && !sampler {
        // The queue cut has passed; this start races the faulting callback.
        let stale = control
            .command_with_ticket(PlaybackCommand::PlayFrom(2))
            .unwrap();
        data.owner_mut().unwrap().run(|callback, _| {
            assert_eq!(callback.stream_pending_play, Some(play));
            callback.report_render_fault(Some(&Invalid("injected buffered failure")), 0);
        });
        assert_eq!(
            control.status.rejected_action(),
            Some((play, PlaybackActionRejection::OutputFailed))
        );
        assert_eq!(control.status.recovery_position(), 0);
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        assert_eq!(
            control.status.rejected_action(),
            Some((stale, PlaybackActionRejection::OutputFailed))
        );
        assert_eq!(control.status.recovery_position(), 0);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut output = loop {
        let output = measured_block(&mut data);
        if control.status.applied_revision.load(Acquire) == revision {
            break output;
        }
        assert_eq!(output, [[0.0; 2]; 8]);
        assert_eq!(control.status.performance_frame.load(Acquire), 0);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    };
    if !keyboard && !sampler {
        // The pre-fault start failed. A fresh user start is admitted only
        // after this replacement has actually cleared the render fault.
        assert!(!control.status.render_fault.load(Acquire));
        play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        loop {
            output = measured_block(&mut data);
            if control.status.playing.load(Acquire) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    assert_eq!(output, expected);
    if stop_restart {
        assert!(
            output.iter().any(|frame| frame[0] > 0.25),
            "sampler must contribute before Stop"
        );
        oracle
            .try_apply_action(hex_engine::PlaybackAction::Transport(PlaybackCommand::Stop))
            .unwrap();
        control.stop();
        assert_eq!(measured_block(&mut data), [[0.0; 2]; 8]);
        assert_eq!(control.status.position.load(Acquire), 0);
        assert_eq!(control.status.performance_frame.load(Acquire), 0);
        assert!(!control.status.playing.load(Acquire));
        oracle
            .try_apply_action(hex_engine::PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
        oracle.render(&mut expected).unwrap();
        let restarted = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        let output = measured_block(&mut data);
        assert_eq!(output, expected);
        assert_eq!(
            control.status.stream_applied_command_prefix.load(Acquire),
            restarted
        );
        play = restarted;
    }
    if !keyboard {
        assert!(std::iter::from_fn(|| control.take_audio_diagnostic()).any(|event|
            matches!(event.kind, AudioDiagnosticKind::CommandDequeued { ticket, action: PlaybackAction::Transport(PlaybackCommand::Play), .. } if ticket == play)));
    }

    if sampler {
        assert!(control.outstanding_device_resource_leases() > 0);
    }
    assert_eq!(
        control.status.position.load(Acquire),
        if keyboard { 0 } else { 8 }
    );
    assert_eq!(
        control.status.performance_frame.load(Acquire),
        if keyboard { 0 } else { 8 }
    );
    assert_eq!(
        control.status.stream_applied_command_prefix.load(Acquire),
        play
    );
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
    if keyboard {
        oracle
            .try_apply_action(hex_engine::PlaybackAction::Transport(PlaybackCommand::Play))
            .unwrap();
        oracle.render(&mut expected).unwrap();
        let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
        let actual = loop {
            let output = measured_block(&mut data);
            if control.status.performance_frame.load(Acquire) == 8 {
                break output;
            }
            assert_eq!(control.status.performance_frame.load(Acquire), 0);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(actual, expected);
        assert_eq!(
            control.status.stream_applied_command_prefix.load(Acquire),
            play
        );
    }
    if let Some(clip) = session_clip {
        let action = hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        };
        oracle
            .try_apply_action(hex_engine::PlaybackAction::Session(action))
            .unwrap();
        oracle.render(&mut expected).unwrap();
        let ticket = control.session_command_with_ticket(action).unwrap();
        assert_eq!(measured_block(&mut data), expected);
        assert_eq!(
            control.status.stream_applied_command_prefix.load(Acquire),
            ticket
        );
        let mut resident_successor = Playback::try_from_prepared_with_device_resources(
            Arc::clone(&oracle_revision),
            &resources,
        )
        .unwrap();
        let target = project
            .tracks()
            .iter()
            .find(|track| track.instrument().is_some())
            .unwrap()
            .id();
        resident_successor
            .prepare_computer_keyboard(&project, target, Arc::clone(&input))
            .unwrap();
        resident_successor
            .try_inherit_runtime_from(&mut oracle)
            .unwrap();
        resident_successor.render(&mut expected).unwrap();
        revision = control
            .publish_with_admission_and_source_on_control(
                compile(metadata.clone()),
                PublicationPolicy::InheritRuntime,
                context,
                |_| Ok::<_, ()>(()),
            )
            .unwrap_or_else(|_| panic!("keyboard source handoff"));
        let output = loop {
            let output = measured_block(&mut data);
            if control.status.applied_revision.load(Acquire) == revision {
                break output;
            }
            assert!(control.status.rejected_revision().is_none());
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(output, expected);
        assert_eq!(
            control.status.performance_frame.load(Acquire),
            resident_successor.performance_frame()
        );
        control.reclaim();
        assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
    }
    if !sampler {
        let seek = control
            .command_with_ticket(PlaybackCommand::PlayFrom(0))
            .unwrap();
        assert_eq!(
            measured_block(&mut data),
            [[if grouped { 0.1875 } else { 0.25 }; 2]; 8]
        );
        assert_eq!(
            control.status.stream_applied_command_prefix.load(Acquire),
            seek
        );
    }
    // A compatible scalar generation does not substitute for the actual catalog.
    assert!(
        control
            .publish_with_admission_and_source_on_control(
                compile(wav.definition(asset, 12).metadata),
                PublicationPolicy::InheritRuntime,
                context,
                |_| Ok::<_, ()>(())
            )
            .is_err()
    );
    assert_eq!(control.submitted_revision(), revision);
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let cancelled = control
        .publish_with_admission_and_source_on_control(
            compile(metadata.clone()),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            hex_engine::AudioSourceContext {
                editor_revision: context.editor_revision + 1,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("fresh publication"));
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(2));
    assert_eq!(
        control.cancel_fresh_runtime_publication(cancelled),
        FreshRuntimeCancellationResult::Cancelled
    );
    measured_block(&mut data);
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert_eq!(
        control.status.rejected_revision(),
        Some((cancelled, PlaybackHandoffRejection::FreshRuntimeCancelled))
    );
    control.reclaim();
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
    let refused = control.publish_with_admission_and_source_on_control(
        compile(metadata.clone()),
        PublicationPolicy::InheritRuntime,
        context,
        |_| Err::<(), _>("injected admission refusal"),
    );
    assert!(matches!(
        refused,
        Err(AdmittedPublishError::Admission { .. })
    ));
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
    let wrong_context = control
        .publish_with_admission_and_source_on_control(
            compile(metadata.clone()),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: context.project_generation + 1,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("cross-context queue"));
    measured_block(&mut data);
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert_eq!(
        control.status.rejected_revision(),
        Some((wrong_context, PlaybackHandoffRejection::RuntimeFenceChanged))
    );
    control.reclaim();
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(1));
    if !sampler {
        let last_position = control.status.position.load(Acquire);
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.report_render_fault(None, last_position));
        let refused = control
            .command_with_ticket(PlaybackCommand::PlayFrom(2))
            .unwrap();
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        assert_eq!(control.status.recovery_position(), 2);
        assert_eq!(
            control.status.rejected_action(),
            Some((refused, PlaybackActionRejection::OutputFailed))
        );
        control.stop();
        assert_eq!(control.status.recovery_position(), 0);
        measured_block(&mut data);
        assert_eq!(control.status.recovery_position(), 0);
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.report_render_fault(None, 0));
        let seek = control
            .command_with_ticket(PlaybackCommand::PlayFrom(2))
            .unwrap();
        loop {
            measured_block(&mut data);
            if control.status.stream_applied_command_prefix.load(Acquire) >= seek {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert_eq!(control.status.recovery_position(), 2);
        assert_eq!(
            control.status.rejected_action(),
            Some((seek, PlaybackActionRejection::OutputFailed))
        );
    }
    if stop_restart {
        let last_position = control.status.position.load(Acquire);
        assert!(last_position > 0);
        let allocations = allocation_counter::measure(|| {
            data.owner_mut()
                .unwrap()
                .run(|callback, _| callback.report_render_fault(None, last_position));
        });
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current,
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.position.load(Acquire), 0);
        assert_eq!(control.status.recovery_position(), last_position);
        assert!(!control.status.playing.load(Acquire));
        let seek = control
            .command_with_ticket(PlaybackCommand::Seek(12))
            .unwrap();
        measured_block(&mut data);
        assert_eq!(
            control.status.rejected_action().map(|(ticket, _)| ticket),
            Some(seek)
        );
        assert_eq!(control.status.recovery_position(), last_position);
        control.stop();
        assert_eq!(control.status.recovery_position(), 0);
        measured_block(&mut data);
        assert_eq!(control.status.recovery_position(), 0);
    }
    if !sampler {
        control.reclaim();
        let restored = control
            .publish_with_admission_and_source_on_control(
                compile(metadata.clone()),
                PublicationPolicy::InstallFreshRuntime,
                context,
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while control.status.applied_revision.load(Acquire) != restored {
            measured_block(&mut data);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let cursor = control.status.recovery_position();
        let pending = control
            .command_with_ticket(PlaybackCommand::PlayFrom(1))
            .unwrap();
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            assert!(callback.stream_action_pending());
        });
        let measured = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                callback.report_render_fault(Some(&Invalid("injected render failure")), cursor);
            });
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
        assert_eq!(control.status.recovery_position(), cursor);
        assert_eq!(
            control.status.rejected_action(),
            Some((pending, PlaybackActionRejection::OutputFailed))
        );
    }
    if !sampler {
        control.reclaim();
        let restored = control
            .publish_with_admission_and_source_on_control(
                compile(metadata.clone()),
                PublicationPolicy::InstallFreshRuntime,
                context,
                |_| Ok::<_, ()>(()),
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while control.status.applied_revision.load(Acquire) != restored {
            measured_block(&mut data);
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        let cursor = control.status.recovery_position();
        let pending = control
            .command_with_ticket(PlaybackCommand::PlayFrom(1))
            .unwrap();
        data.owner_mut()
            .unwrap()
            .run(|callback, _| callback.begin_block());
        control.status.device_fault.store(true, Release);
        assert_eq!(measured_block(&mut data), [[0.0; 2]; 8]);
        assert_eq!(control.status.recovery_position(), cursor);
        assert_eq!(
            control.status.rejected_action(),
            Some((pending, PlaybackActionRejection::OutputFailed))
        );
    }
    assert!(control.retire_audio_candidate_domain(domain));
    assert!(
        !control.audio_candidate_domain_endpoint_is_installed(domain),
        "retiring domains must not authorize a new association"
    );
    for _ in 0..3 {
        control.reclaim();
    }
    assert!(
        control.recover_audio_candidate_domain(domain).is_none(),
        "the active graph still holds the actual source association"
    );
    control.request_shutdown();
    loop {
        measured_block(&mut data);
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
    assert!(!control.audio_candidate_domain_endpoint_is_installed(domain));
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}

#[test]
fn callback_stream_projection_is_allocation_free_for_both_runtime_kinds() {
    let frames = 8192;
    let mut snapshot = super::readiness::project_with_frames(frames).snapshot();
    snapshot.loop_region = Some(hex_project::LoopRegion::new(3840, 4224, true).unwrap());
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(frames as u32);
    let definition = wav.definition(asset, 11);
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    let mut ordinary = compile(
        &project,
        definition.metadata.clone(),
        &ResidentDeviceAssets::default(),
    );
    ordinary.apply(PlaybackCommand::PlayFrom(4160));
    let mut owner =
        stream_fixture::owner_sources(&project, vec![definition.metadata], Some(context));
    let _limits = reservation(&mut owner);
    let (mut constructor, _anchors) = owner.split_constructor_on_control().ok().unwrap();
    constructor
        .apply_action(hex_engine::PlaybackAction::Transport(
            PlaybackCommand::PlayFrom(4160),
        ))
        .unwrap();
    let mut ordinary_extents = Vec::with_capacity(32);
    let mut ordinary_result = None;
    let ordinary_allocations = allocation_counter::measure(|| {
        ordinary_result = Some(
            ordinary
                .stream_playback(context)
                .visit_current_stream_quantum_at(128, 256, None, &mut |extent| {
                    ordinary_extents.push(extent);
                    Ok(())
                }),
        );
    });
    let mut constructor_extents = Vec::with_capacity(32);
    let mut constructor_result = None;
    let constructor_allocations = allocation_counter::measure(|| {
        constructor_result = Some(
            constructor
                .stream_playback()
                .visit_current_stream_quantum_at(128, 256, None, &mut |extent| {
                    constructor_extents.push(extent);
                    Ok(())
                }),
        );
    });
    for allocations in [ordinary_allocations, constructor_allocations] {
        assert_eq!(
            (
                allocations.count_total,
                allocations.bytes_total,
                allocations.count_current,
                allocations.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    assert_eq!(ordinary_result, Some(Ok(256)));
    assert_eq!(constructor_result, Some(Ok(256)));
    assert_eq!(ordinary_extents, constructor_extents);
    assert!(ordinary_extents.iter().any(|extent| extent.page_index == 0));
    assert!(ordinary_extents.iter().any(|extent| extent.page_index == 1));
    let mut ordinary_failure = None;
    let allocation = allocation_counter::measure(|| {
        ordinary_failure = Some(
            ordinary
                .stream_playback(context)
                .visit_current_stream_quantum_at(128, 256, None, &mut |_| {
                    Err(hex_engine::Invalid("visitor full"))
                }),
        );
    });
    assert_eq!(
        (
            allocation.count_total,
            allocation.bytes_total,
            allocation.count_current,
            allocation.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let mut constructor_failure = None;
    let allocation = allocation_counter::measure(|| {
        constructor_failure = Some(
            constructor
                .stream_playback()
                .visit_current_stream_quantum_at(128, 256, None, &mut |_| {
                    Err(hex_engine::Invalid("visitor full"))
                }),
        );
    });
    assert_eq!(
        (
            allocation.count_total,
            allocation.bytes_total,
            allocation.count_current,
            allocation.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let expected = Some(Err(
        hex_engine::playback::StreamQuantumProjectionError::Visitor(hex_engine::Invalid(
            "visitor full",
        )),
    ));
    assert_eq!(ordinary_failure, expected);
    assert_eq!(constructor_failure, expected);
}

#[test]
fn ordinary_pending_replacement_yields_small_pool_to_the_current_stream() {
    pending_replacement_case(true, false, None, (false, false), false, false, false);
}

#[test]
fn ordinary_short_one_shot_replacement_at_device_cadence() {
    pending_replacement_case(false, false, None, (false, false), false, false, false);
}

#[test]
fn fenced_retained_restart_waits_for_missing_pages_without_displacing_current_audio() {
    pending_replacement_case(false, true, None, (false, false), false, false, false);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn ordinary_loop_seek_keeps_resident_destination_while_prefetching() {
    pending_replacement_case(
        true,
        false,
        Some(false),
        (false, false),
        false,
        false,
        false,
    );
    pending_replacement_case(true, false, Some(true), (false, false), false, false, false);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn ordinary_pending_replacement_preserves_resident_loop_page_in_same_callback() {
    pending_replacement_case(true, false, None, (true, false), false, false, false);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn ordinary_pending_replacement_preserves_resident_loop_pages_in_long_callback() {
    crate::candidate_window_probe::begin();
    pending_replacement_case(true, false, None, (true, true), false, false, false);
    let (_, _, smallest) = crate::candidate_window_probe::finish().unwrap();
    assert_eq!(
        smallest, 1,
        "candidate must actually try a one-frame window"
    );
}

#[cfg(feature = "stream-test-support")]
#[test]
fn oversized_current_demand_refuses_successor_after_bounded_attempts() {
    pending_replacement_case(false, false, None, (false, false), true, false, false);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn quantized_streamed_handoff_releases_old_page_before_new_suffix() {
    pending_replacement_case(true, false, None, (false, false), false, true, false);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn ordinary_candidate_retries_smaller_window_at_two_page_boundary() {
    crate::candidate_window_probe::begin();
    pending_replacement_case(true, false, None, (false, false), false, false, true);
    let (failed, next, _) = crate::candidate_window_probe::finish().unwrap();
    assert_eq!((failed, next), (256, 128));
}

fn pending_replacement_case(
    looping: bool,
    retained_restart: bool,
    lookahead: Option<bool>,
    (_short_loop, _long_callback): (bool, bool),
    _oversized: bool,
    _quantized: bool,
    _cross_page: bool,
) {
    use hex_project::{Command, Created, Editor};

    let _serial = serial();
    let frames = (hex_engine::STREAM_PAGE_FRAMES
        * if _oversized {
            32
        } else if lookahead == Some(true) {
            3
        } else {
            2
        }) as u64;
    let mut snapshot = super::readiness::project_with_frames(frames).snapshot();
    if looping {
        snapshot.loop_region = Some(if _quantized {
            hex_project::LoopRegion::new(3840, 6004, true).unwrap()
        } else if lookahead.is_some() {
            hex_project::LoopRegion::new(3840, 6272, true).unwrap()
        } else if _short_loop {
            hex_project::LoopRegion::new(3840, 4224, true).unwrap()
        } else {
            hex_project::LoopRegion::new(0, frames, true).unwrap()
        });
    }
    let original = hex_project::Project::from_snapshot(snapshot).unwrap();
    let a_track = original.tracks()[0].id();
    let a_asset = original.assets().next().unwrap().id();
    let mut editor = Editor::new(original, 16).unwrap();
    let receiver = if retained_restart {
        let Created::Track(receiver) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "receiver".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("receiver")
        };
        let source = editor
            .project()
            .track(receiver)
            .unwrap()
            .source_id()
            .unwrap();
        let Created::Track(peer) = editor
            .execute(Command::AddMidiTrack {
                name: "peer".into(),
            })
            .unwrap()
        else {
            panic!("peer")
        };
        editor
            .execute(Command::BindTrackSource {
                track: peer,
                source: Some(source),
            })
            .unwrap();
        Some(receiver)
    } else {
        None
    };
    let Created::Track(b_track) = editor
        .execute(Command::AddTrack {
            name: "replacement".into(),
        })
        .unwrap()
    else {
        panic!("replacement track")
    };
    let Created::Asset(b_asset) = editor
        .execute(Command::AddAsset {
            path: "replacement.wav".into(),
            sample_rate: 48_000,
            frames: if _quantized { frames * 2 } else { frames },
        })
        .unwrap()
    else {
        panic!("replacement asset")
    };
    editor
        .execute(Command::PlaceClip {
            track: b_track,
            asset: b_asset,
            start: 0,
            duration: frames,
            source_offset: if _quantized { 2192 } else { 0 },
            source_length: frames,
            looping: false,
        })
        .unwrap();
    editor
        .execute(Command::Mute {
            track: b_track,
            muted: true,
        })
        .unwrap();
    let a_project = editor.project().clone();
    editor
        .execute(Command::Mute {
            track: a_track,
            muted: true,
        })
        .unwrap();
    editor
        .execute(Command::Mute {
            track: b_track,
            muted: false,
        })
        .unwrap();
    let b_project = if let Some(receiver) = receiver {
        let mut snapshot = editor.project().snapshot();
        snapshot.retained_source_paths.push(receiver.value());
        hex_project::Project::from_snapshot(snapshot).unwrap()
    } else {
        editor.project().clone()
    };
    let a_wav = stream_fixture::Wav::with_frames(frames.try_into().unwrap());
    let b_wav = stream_fixture::Wav::with_samples(
        (if _quantized { frames * 2 } else { frames })
            .try_into()
            .unwrap(),
        |_| 16_384,
    );
    let a_definition = a_wav.definition(a_asset, 11);
    let b_definition = b_wav.definition(b_asset, 11);
    #[cfg(feature = "stream-test-support")]
    let a_key = a_definition.metadata.key();
    #[cfg(feature = "stream-test-support")]
    let b_key = b_definition.metadata.key();
    let metadata = vec![a_definition.metadata.clone(), b_definition.metadata.clone()];
    let store = ResidentDeviceAssets::default();
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            2,
            4096,
        )
        .unwrap();
    #[cfg(feature = "stream-test-support")]
    let (pair, prefetch_gate) = if lookahead == Some(true) {
        let (gate, entered, release) =
            hex_stream::test_support::PageGate::new(hex_stream::PageKey {
                source: a_key,
                page_index: 2,
            });
        let mut runtime = hex_stream::StreamRuntime::new_with_page_gate(
            hex_stream::StreamConfig::with_pool_pages(3),
            vec![a_definition, b_definition],
            gate,
        )
        .unwrap();
        runtime
            .wait_until_ready(std::time::Duration::from_secs(2))
            .unwrap();
        (runtime.split().ok().unwrap(), Some((entered, release)))
    } else {
        (
            stream_fixture::pair_with_pool(
                vec![a_definition, b_definition],
                if _quantized { 4 } else { 3 },
            ),
            None,
        )
    };
    #[cfg(not(feature = "stream-test-support"))]
    let pair = stream_fixture::pair_with_pool(vec![a_definition, b_definition], 3);
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            pair,
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|_| panic!("source registration"));
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let a_revision = control
        .publish_with_admission_and_source_on_control(
            compile_sources(&a_project, metadata.clone(), &store),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("original publication"));
    control.command(PlaybackCommand::Play).unwrap();
    let mut warmup = [[0.0; 2]; 1024];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while control.status.position.load(Acquire) < hex_engine::STREAM_PAGE_FRAMES as u64 {
        measured_render(&mut data, &mut warmup);
        let committed = if control.status.stream_buffering.load(Acquire) {
            usize::try_from(control.status.stream_committed_frames.load(Acquire)).unwrap()
        } else {
            warmup.len()
        };
        assert!(committed <= warmup.len());
        for (index, frame) in warmup.iter().enumerate() {
            assert_eq!(*frame, [if index < committed { 0.25 } else { 0.0 }; 2]);
        }
        assert!(control.status.rejected_revision().is_none());
        assert!(
            std::time::Instant::now() < deadline,
            "initial page readiness"
        );
        std::thread::yield_now();
    }
    assert_eq!(control.status.applied_revision.load(Acquire), a_revision);
    assert_eq!(
        control.status.position.load(Acquire),
        hex_engine::STREAM_PAGE_FRAMES as u64
    );
    let mut output = [[0.0; 2]; 256];

    #[cfg(feature = "stream-test-support")]
    if _oversized {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while control.status.position.load(Acquire) == 4096 {
            measured_render(&mut data, &mut output[..64]);
            assert!(
                std::time::Instant::now() < deadline,
                "warm oversized boundary"
            );
            std::thread::yield_now();
        }
    }

    #[cfg(feature = "stream-test-support")]
    if _quantized {
        // Keep old A0/A1 and new B1 resident, with B2 already decoded but
        // still queued. Old A0/A1 pins must be released at the actual swap so
        // the new graph can consume B2 in the same callback.
        control.command(PlaybackCommand::Seek(5996)).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while control.status.position.load(Acquire) != 5997 {
            measured_render(&mut data, &mut output[..1]);
            assert!(
                std::time::Instant::now() < deadline,
                "warm quantized old page"
            );
            std::thread::yield_now();
        }
        let first = hex_engine::StreamResourceExtent {
            source: b_key.into(),
            page_index: 1,
            start: hex_engine::STREAM_PAGE_FRAMES - 2,
            end: hex_engine::STREAM_PAGE_FRAMES,
        };
        let mut stamp = None;
        data.owner_mut().unwrap().run(|c, _| {
            let Some(ActivePublication::Legacy(active)) = c.active.as_ref() else {
                panic!("active graph")
            };
            let (loan, epoch) = active
                .streams
                .as_ref()
                .unwrap()
                .test_loan_for(&mut c.domain_pages, first.source)
                .unwrap();
            *epoch += 1;
            let value = hex_stream::ReadinessStamp {
                request_epoch: *epoch,
                artifact: hex_stream::ReadinessArtifact::Ordinary {
                    publication: a_revision,
                },
                project_generation: context.project_generation,
                editor_revision: context.editor_revision,
                device_epoch: 1,
                device_rate: 48_000,
                predecessor: None,
                admitted_command_prefix: 0,
                window: hex_engine::StreamChunkWindow {
                    performance_frame: 0,
                    arrangement_frame: 0,
                    route_generation: 0,
                    frames: 2,
                },
            };
            loan.submit_resources(hex_stream::ProtectionRole::Candidate, value, &[first])
                .unwrap();
            stamp = Some(value);
        });
        let stamp = stamp.unwrap();
        loop {
            let mut ready = false;
            data.owner_mut().unwrap().run(|c, _| {
                let Some(ActivePublication::Legacy(active)) = c.active.as_ref() else {
                    panic!("active graph")
                };
                let (loan, _) = active
                    .streams
                    .as_ref()
                    .unwrap()
                    .test_loan_for(&mut c.domain_pages, first.source)
                    .unwrap();
                ready = loan
                    .service(hex_stream::ProtectionRole::Candidate, stamp, 1, 1)
                    .unwrap()
                    == hex_stream::ReadinessProgress::InstalledProtected;
            });
            if ready {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "warm new first page");
            std::thread::yield_now();
        }
        data.owner_mut().unwrap().run(|c, _| {
            let Some(ActivePublication::Legacy(active)) = c.active.as_ref() else {
                panic!("active graph")
            };
            let (loan, _) = active
                .streams
                .as_ref()
                .unwrap()
                .test_loan_for(&mut c.domain_pages, first.source)
                .unwrap();
            loan.test_prefetch_audio_page(b_key, 2);
        });
        loop {
            let mut ready = false;
            data.owner_mut().unwrap().run(|c, _| {
                let Some(ActivePublication::Legacy(active)) = c.active.as_ref() else {
                    panic!("active graph")
                };
                let (loan, _) = active
                    .streams
                    .as_ref()
                    .unwrap()
                    .test_loan_for(&mut c.domain_pages, first.source)
                    .unwrap();
                ready = loan.test_ready_page(hex_stream::ResourcePageKey {
                    source: b_key.into(),
                    page_index: 2,
                });
            });
            if ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "queue new second page"
            );
            std::thread::yield_now();
        }
        data.owner_mut().unwrap().run(|c, _| {
            let Some(ActivePublication::Legacy(active)) = c.active.as_ref() else {
                panic!("active graph")
            };
            let (loan, _) = active
                .streams
                .as_ref()
                .unwrap()
                .test_loan_for(&mut c.domain_pages, first.source)
                .unwrap();
            assert!(loan.cancel(hex_stream::ProtectionRole::Candidate, stamp.request_epoch));
        });
        data.owner_mut().unwrap().run(|c, _| {
            let Some(ActivePublication::Legacy(active)) = c.active.as_ref() else {
                panic!("active graph")
            };
            let streams = active.streams.as_ref().unwrap();
            for extent in [
                hex_engine::StreamResourceExtent {
                    source: a_key.into(),
                    page_index: 0,
                    start: 3840,
                    end: 3841,
                },
                hex_engine::StreamResourceExtent {
                    source: a_key.into(),
                    page_index: 1,
                    start: 1904,
                    end: 1905,
                },
                first,
            ] {
                assert!(
                    streams.test_has_extent(&mut c.domain_pages, extent),
                    "fixture page missing: {extent:?}"
                );
            }
        });
    }

    #[cfg(feature = "stream-test-support")]
    if let Some(long_source) = lookahead {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while control.status.position.load(Acquire) == 4096 {
            measured_render(&mut data, &mut output);
            assert!(std::time::Instant::now() < deadline, "warm A1");
            std::thread::yield_now();
        }
        assert_eq!(control.status.position.load(Acquire), 4352);
        control.command(PlaybackCommand::Seek(3712)).unwrap();
        while control.status.position.load(Acquire) != 3968 {
            measured_render(&mut data, &mut output[..128]);
            assert!(std::time::Instant::now() < deadline, "restore A0");
            std::thread::yield_now();
        }
        let mut cached = [false; 2];
        data.owner_mut().unwrap().run(|c, _| {
            if let Some(ActivePublication::Legacy(active)) = c.active.as_ref()
                && let Some(streams) = active.streams.as_ref()
            {
                for (page_index, result) in cached.iter_mut().enumerate() {
                    *result = streams.test_has_extent(
                        &mut c.domain_pages,
                        hex_engine::StreamResourceExtent {
                            source: a_key.into(),
                            page_index: page_index as u64,
                            start: 0,
                            end: 1,
                        },
                    );
                }
            }
        });
        assert_eq!(cached, [true; 2], "A0 and A1 must be resident before seek");
        control.command(PlaybackCommand::Seek(6208)).unwrap();
        measured_render(&mut data, &mut output[..32]);
        assert!(!control.status.stream_buffering.load(Acquire));
        assert_eq!(control.status.position.load(Acquire), 6240);
        if long_source {
            let (entered, release) = prefetch_gate.expect("prefetch gate");
            entered
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("prefetch entered");
            release.send(()).expect("release prefetch");
            let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                let mut ready = false;
                data.owner_mut().unwrap().run(|c, _| {
                    if let Some(ActivePublication::Legacy(active)) = c.active.as_ref()
                        && let Some(streams) = active.streams.as_ref()
                    {
                        ready = streams.test_ready_page(
                            &mut c.domain_pages,
                            hex_stream::ResourcePageKey {
                                source: a_key.into(),
                                page_index: 2,
                            },
                        );
                    }
                });
                if ready {
                    break;
                }
                assert!(
                    std::time::Instant::now() < ready_deadline,
                    "prefetch did not become ready"
                );
                std::thread::yield_now();
            }
        }
        measured_render(&mut data, &mut output);
        assert!(
            !control.status.stream_buffering.load(Acquire),
            "resident loop destination was lost, long_source={long_source}"
        );
        assert_eq!(control.status.position.load(Acquire), 4064);
        for (index, frame) in output.iter().enumerate() {
            let expected = if index < 32 {
                0.25 * ((31 - index) as f32 / 127.0)
            } else {
                0.25
            };
            assert_eq!(*frame, [expected; 2], "loop PCM at {index}");
        }
        for _ in 0..12 {
            let before = control.status.position.load(Acquire);
            measured_render(&mut data, &mut output);
            assert!(
                !control.status.stream_buffering.load(Acquire),
                "loop buffered after lookahead, long_source={long_source}, before={before}"
            );
            let expected_position = 3840 + ((before - 3840 + 256) % (6272 - 3840));
            assert_eq!(control.status.position.load(Acquire), expected_position);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(control.retire_audio_candidate_domain(domain));
        control.request_shutdown();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            measured_render(&mut data, &mut output);
            control.reclaim();
            if let Some(report) = control.recover_audio_candidate_domain(domain) {
                assert!(report.worker_joined && report.accounting_complete);
                break;
            }
            assert!(std::time::Instant::now() < deadline, "source reclamation");
            std::thread::yield_now();
        }
        drop(data);
        drop(errors);
        drop(control);
        finish(&mut retirement);
        return;
    }

    #[cfg(feature = "stream-test-support")]
    let loop_baseline = if _short_loop {
        // Warm A1 and leave its first 64 frames active; A0 remains resident.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while control.status.position.load(Acquire) == 4096 {
            measured_render(&mut data, &mut output[..64]);
            assert!(std::time::Instant::now() < deadline, "warm A1");
            std::thread::yield_now();
        }
        assert_eq!(control.status.position.load(Acquire), 4160);
        let a0 = hex_engine::StreamResourceExtent {
            source: a_key.into(),
            page_index: 0,
            start: 3840,
            end: 3841,
        };
        let mut cached = false;
        data.owner_mut().unwrap().run(|c, _| {
            if let Some(ActivePublication::Legacy(active)) = c.active.as_ref()
                && let Some(streams) = active.streams.as_ref()
            {
                cached = streams.test_has_extent(&mut c.domain_pages, a0);
            }
        });
        assert!(cached, "loop destination must be resident");
        measured_render(&mut data, &mut output);
        assert!(!control.status.stream_buffering.load(Acquire));
        assert_eq!(control.status.position.load(Acquire), 4032);
        let baseline = output;
        control.command(PlaybackCommand::Seek(4096)).unwrap();
        while control.status.position.load(Acquire) != 4160 {
            measured_render(&mut data, &mut output[..64]);
            assert!(std::time::Instant::now() < deadline, "restore A1 boundary");
            std::thread::yield_now();
        }
        let long_baseline = if _long_callback {
            let mut long = [[0.0; 2]; 512];
            measured_render(&mut data, &mut long);
            assert!(!control.status.stream_buffering.load(Acquire));
            assert_eq!(control.status.position.load(Acquire), 3904);
            control.command(PlaybackCommand::Seek(4096)).unwrap();
            while control.status.position.load(Acquire) != 4160 {
                measured_render(&mut data, &mut output[..64]);
                assert!(
                    std::time::Instant::now() < deadline,
                    "restore long callback boundary"
                );
                std::thread::yield_now();
            }
            Some(long)
        } else {
            None
        };
        cached = false;
        data.owner_mut().unwrap().run(|c, _| {
            if let Some(ActivePublication::Legacy(active)) = c.active.as_ref()
                && let Some(streams) = active.streams.as_ref()
            {
                cached = streams.test_has_extent(&mut c.domain_pages, a0);
            }
        });
        assert!(
            cached,
            "loop destination must remain resident before successor"
        );
        Some((baseline, long_baseline))
    } else {
        None
    };
    #[cfg(feature = "stream-test-support")]
    if looping && !retained_restart && !_short_loop && !_quantized {
        let boundary = if _cross_page { 3968 } else { 4096 };
        // Exercise A1 once so it can remain resident after returning to A0.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while control.status.position.load(Acquire) == 4096 {
            measured_render(&mut data, &mut output);
            assert!(
                std::time::Instant::now() < deadline,
                "baseline A1 readiness"
            );
            std::thread::yield_now();
        }
        assert_eq!(control.status.position.load(Acquire), 4352);
        assert_eq!(output, [[0.25; 2]; 256], "baseline A1 PCM");
        control
            .command(PlaybackCommand::Seek(boundary - 256))
            .unwrap();
        while control.status.position.load(Acquire) != boundary {
            measured_render(&mut data, &mut output[..128]);
            assert!(
                std::time::Instant::now() < deadline,
                "return to A0 boundary"
            );
            std::thread::yield_now();
        }
        let extent = hex_engine::StreamResourceExtent {
            source: a_key.into(),
            page_index: 1,
            start: 0,
            end: 1,
        };
        let mut cached = false;
        data.owner_mut().unwrap().run(|c, _| {
            if let Some(ActivePublication::Legacy(active)) = c.active.as_ref()
                && let Some(streams) = active.streams.as_ref()
            {
                cached = streams.test_has_extent(&mut c.domain_pages, extent);
            }
        });
        assert!(cached, "A1 must be resident before candidate publication");
        measured_render(&mut data, &mut output);
        assert!(!control.status.stream_buffering.load(Acquire));
        assert_eq!(control.status.position.load(Acquire), boundary + 256);
        assert_eq!(output, [[0.25; 2]; 256], "resident baseline PCM");
        control
            .command(PlaybackCommand::Seek(boundary - 256))
            .unwrap();
        while control.status.position.load(Acquire) != boundary {
            measured_render(&mut data, &mut output[..128]);
            assert!(std::time::Instant::now() < deadline, "restore A0 boundary");
            std::thread::yield_now();
        }
        cached = false;
        data.owner_mut().unwrap().run(|c, _| {
            if let Some(ActivePublication::Legacy(active)) = c.active.as_ref()
                && let Some(streams) = active.streams.as_ref()
            {
                cached = streams.test_has_extent(&mut c.domain_pages, extent);
            }
        });
        assert!(
            cached,
            "A1 must remain resident for the candidate comparison"
        );
    }

    // The last committed chunk protects A0. The next candidate needs B1, which
    // has never been requested: its first service must return Pending even if
    // the real worker finishes immediately afterward. The current graph then
    // needs A1. A0 + the speculative B1 intent + A1 cannot fit cache capacity 2.
    // Yielding the candidate's exact intent lets the old graph advance instead
    // of turning ordinary disk latency into a readiness refusal.
    let fence = if retained_restart {
        let fence = control
            .request_live_arrangement_edit_fence(7, a_revision)
            .unwrap();
        measured_render(&mut data, &mut output);
        assert_eq!(control.arrangement_edit_playing(fence), Some(true));
        Some(fence)
    } else {
        None
    };
    let mut b_playback = compile_sources(&b_project, metadata, &store);
    if retained_restart {
        assert!(b_playback.request_live_restart());
    }
    let mut policy = fence.map_or(
        PublicationPolicy::InheritRuntime,
        PublicationPolicy::InheritUnderArrangement,
    );
    let mut quantized_prefix = None;
    if _quantized {
        b_playback.apply(PlaybackCommand::PlayFrom(5998));
        // A real route boundary limits initial candidate demand to B1; the
        // next stream chunk needs the queued B2 without changing audible B.
        b_playback
            .try_apply_action(hex_engine::PlaybackAction::Session(
                hex_engine::SessionPlaybackCommand::StopTrack {
                    track: a_track,
                    quantization: hex_engine::LaunchQuantization::ThirtySecond,
                },
            ))
            .unwrap();
        assert_eq!(
            b_playback
                .session_route_status(0)
                .unwrap()
                .pending
                .unwrap()
                .performance_frame,
            6000,
            "candidate route boundary must split its first page from the next",
        );
        let mut prefix = None;
        data.owner_mut().unwrap().run(|c, _| {
            let Some(ActivePublication::Legacy(active)) = c.active.as_ref() else {
                panic!("active graph")
            };
            let due = active
                .playback
                .quantized_seek_frame(hex_engine::LaunchQuantization::ThirtySecond)
                .unwrap();
            prefix = Some(usize::try_from(due - active.playback.performance_frame()).unwrap());
        });
        let prefix = prefix.unwrap();
        assert!(
            prefix > 0 && prefix < 16,
            "handoff must be inside the callback"
        );
        quantized_prefix = Some(prefix);
        policy = PublicationPolicy::InstallQuantizedSeekWhenReady {
            fence: control.capture_fresh_runtime_fence().unwrap(),
            quantization: hex_engine::LaunchQuantization::ThirtySecond,
        };
    }
    let b_revision = control
        .publish_with_admission_and_source_on_control(
            b_playback,
            policy,
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("replacement publication"));
    #[cfg(feature = "stream-test-support")]
    if let Some(prefix) = quantized_prefix {
        let mut split = [[0.0; 2]; 16];
        measured_render(&mut data, &mut split);
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            b_revision,
            "pending={} rejection={:?} buffering={} position={} prefix={prefix}",
            control.status.publication_pending.load(Acquire),
            control.status.rejected_revision(),
            control.status.stream_buffering.load(Acquire),
            control.status.position.load(Acquire)
        );
        assert!(!control.status.stream_buffering.load(Acquire));
        for (index, frame) in split[..prefix].iter().enumerate() {
            let remaining = 6004 - (5997 + index);
            assert_eq!(*frame, [0.25 * (remaining - 1) as f32 / 127.0; 2]);
        }
        for (index, frame) in split[prefix..].iter().enumerate() {
            let position = 3840 + ((5998 - 3840 + index) % (6004 - 3840));
            let remaining = 6004 - position;
            let gain = if remaining <= 128 {
                (remaining - 1) as f32 / 127.0
            } else {
                1.0
            };
            assert_eq!(*frame, [0.5 * gain; 2]);
        }
        assert_eq!(
            control.status.position.load(Acquire),
            3840 + ((5998 - 3840 + split.len() - prefix) % (6004 - 3840)) as u64
        );
        assert!(control.retire_audio_candidate_domain(domain));
        control.request_shutdown();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            measured_render(&mut data, &mut output);
            control.reclaim();
            if let Some(report) = control.recover_audio_candidate_domain(domain) {
                assert!(report.worker_joined && report.accounting_complete);
                break;
            }
            assert!(std::time::Instant::now() < deadline, "source reclamation");
            std::thread::yield_now();
        }
        assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
        drop(data);
        drop(errors);
        drop(control);
        finish(&mut retirement);
        return;
    }
    #[cfg(not(feature = "stream-test-support"))]
    let _ = quantized_prefix;
    #[cfg(feature = "stream-test-support")]
    if _oversized {
        let mut large = vec![[0.0; 2]; hex_engine::STREAM_PAGE_FRAMES * 2 + 1];
        let mut total_committed = 0;
        for _ in 0..8 {
            let before = control.status.position.load(Acquire);
            measured_render(&mut data, &mut large);
            let committed = if control.status.stream_buffering.load(Acquire) {
                usize::try_from(control.status.stream_committed_frames.load(Acquire)).unwrap()
            } else {
                large.len()
            };
            assert!(committed <= large.len());
            assert_eq!(
                control.status.position.load(Acquire),
                before + committed as u64
            );
            for (index, frame) in large.iter().enumerate() {
                assert_eq!(*frame, [if index < committed { 0.25 } else { 0.0 }; 2]);
            }
            total_committed += committed;
            std::thread::yield_now();
        }
        assert!(total_committed > 0, "current playback must continue");
        assert_eq!(
            control.status.rejected_revision(),
            Some((b_revision, PlaybackHandoffRejection::StreamedPageCapacity))
        );
        assert!(!control.status.publication_pending.load(Acquire));
        assert!(control.retire_audio_candidate_domain(domain));
        control.request_shutdown();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            measured_render(&mut data, &mut output);
            control.reclaim();
            if let Some(report) = control.recover_audio_candidate_domain(domain) {
                assert!(report.worker_joined && report.accounting_complete);
                break;
            }
            assert!(std::time::Instant::now() < deadline, "source reclamation");
            std::thread::yield_now();
        }
        assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
        drop(data);
        drop(errors);
        drop(control);
        finish(&mut retirement);
        return;
    }
    let render_checked = |data: &mut _, output: &mut [Frame; 256]| {
        let before = control.status.position.load(Acquire);
        let performance_before = control.status.performance_frame.load(Acquire);
        measured_render(data, output);
        let after = control.status.position.load(Acquire);
        let performance_after = control.status.performance_frame.load(Acquire);
        let committed = if control.status.stream_buffering.load(Acquire) {
            usize::try_from(control.status.stream_committed_frames.load(Acquire)).unwrap()
        } else {
            output.len()
        };
        assert!(committed <= output.len());
        assert_eq!(
            performance_after,
            performance_before + committed as u64,
            "performance clock follows audible frames"
        );
        let expected_position = if looping {
            (before + committed as u64) % frames
        } else {
            before + committed as u64
        };
        assert_eq!(after, expected_position, "position follows audible frames");
        let revision = control.status.applied_revision.load(Acquire);
        let expected = if revision == a_revision {
            0.25
        } else {
            assert_eq!(revision, b_revision);
            0.5
        };
        for (index, frame) in output.iter().enumerate() {
            let position = (before + index as u64) % frames;
            let remaining = frames - position;
            let seam_gain = if looping && remaining <= 128 {
                (remaining - 1) as f32 / 127.0
            } else {
                1.0
            };
            assert_eq!(
                *frame,
                [if index < committed {
                    expected * seam_gain
                } else {
                    0.0
                }; 2],
                "before={before} after={after} committed={committed} index={index} revision={revision} buffering={} pending={}",
                control.status.stream_buffering.load(Acquire),
                control.status.publication_pending.load(Acquire),
            );
        }
    };
    #[cfg(feature = "stream-test-support")]
    if let Some((baseline, long_baseline)) = loop_baseline {
        let mut long_output = [[0.0; 2]; 512];
        if long_baseline.is_some() {
            measured_render(&mut data, &mut long_output);
        } else {
            measured_render(&mut data, &mut output);
        }
        assert!(
            !control.status.stream_buffering.load(Acquire),
            "successor displaced resident loop destination"
        );
        if let Some(long_baseline) = long_baseline {
            assert_eq!(control.status.position.load(Acquire), 3904);
            assert_eq!(
                long_output, long_baseline,
                "successor changed long callback PCM"
            );
            // The candidate gets one final one-frame admission attempt; the
            // protected predecessor must remain audible through that attempt.
            for _ in 0..8 {
                let before = control.status.position.load(Acquire);
                measured_render(&mut data, &mut long_output);
                assert!(!control.status.stream_buffering.load(Acquire));
                assert_eq!(
                    control.status.position.load(Acquire),
                    3840 + ((before - 3840 + 512) % (4224 - 3840))
                );
            }
            assert_eq!(
                control.status.rejected_revision(),
                Some((b_revision, PlaybackHandoffRejection::StreamedPageCapacity))
            );
            assert!(!control.status.publication_pending.load(Acquire));
        } else {
            assert_eq!(control.status.position.load(Acquire), 4032);
            assert_eq!(output, baseline, "successor changed current callback PCM");
        }
        assert!(control.retire_audio_candidate_domain(domain));
        control.request_shutdown();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            measured_render(&mut data, &mut output);
            control.reclaim();
            if let Some(report) = control.recover_audio_candidate_domain(domain) {
                assert!(report.worker_joined && report.accounting_complete);
                break;
            }
            assert!(std::time::Instant::now() < deadline, "source reclamation");
            std::thread::yield_now();
        }
        assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
        drop(data);
        drop(errors);
        drop(control);
        finish(&mut retirement);
        return;
    }
    render_checked(&mut data, &mut output);
    #[cfg(feature = "stream-test-support")]
    if looping && !retained_restart {
        assert!(
            !control.status.stream_buffering.load(Acquire),
            "a pending successor displaced the resident current page"
        );
    }
    let refused = control.status.stream_readiness_refused.load(Acquire);
    let rejected = control.status.rejected_revision();
    let pending = control.status.publication_pending.load(Acquire);
    if !refused && rejected.is_none() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut attempts = 0usize;
        while control.status.applied_revision.load(Acquire) != b_revision {
            render_checked(&mut data, &mut output);
            attempts += 1;
            assert!(control.status.rejected_revision().is_none());
            assert!(
                std::time::Instant::now() < deadline,
                "replacement readiness"
            );
            if looping && attempts <= 16 {
                std::thread::yield_now();
            } else {
                // The worker polls every 2 ms. After an unpaced burst, let it
                // run at a still-accelerated cadence; the one-shot uses the
                // actual 256-frame/48-kHz interval. Both delays count as test time.
                let micros = if looping { 1_000 } else { 5_333 };
                std::thread::sleep(std::time::Duration::from_micros(micros));
            }
        }
    }

    if let Some(fence) = fence {
        control.release_stopped_edit_fence(fence);
    }
    assert!(control.retire_audio_candidate_domain(domain));
    control.request_shutdown();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        measured_render(&mut data, &mut output);
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "source reclamation");
        std::thread::yield_now();
    }
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
    assert!(
        pending,
        "the replacement must remain owned while pages arrive"
    );
    assert!(rejected.is_none(), "replacement rejected: {rejected:?}");
    assert!(
        !refused,
        "pending candidate intent blocked the current page"
    );
}

#[cfg(feature = "stream-test-support")]
#[test]
fn prepared_audio_domain_holds_later_page_without_blocking_callback_or_custody() {
    prepared_audio_domain_gate(GatedPageAction::Resume);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn prepared_audio_domain_retires_while_later_page_is_held() {
    prepared_audio_domain_gate(GatedPageAction::Retire);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn prepared_audio_seek_does_not_render_a_late_old_page() {
    prepared_audio_domain_gate(GatedPageAction::Seek);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn seek_then_source_replacement_does_not_render_late_old_generation() {
    let _serial = serial();
    let frames = (hex_engine::STREAM_PAGE_FRAMES * 3) as u64;
    let project = super::readiness::project_with_frames(frames);
    let asset = project.assets().next().unwrap().id();
    let old_wav = stream_fixture::Wav::with_samples(frames.try_into().unwrap(), |_| 8192);
    let new_wav = stream_fixture::Wav::with_samples(frames.try_into().unwrap(), |_| 24_576);
    let old = old_wav.definition(asset, 11);
    let new = new_wav.definition(asset, 12);
    let old_key = old.metadata.key();
    let new_key = new.metadata.key();
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(hex_stream::PageKey {
        source: old_key,
        page_index: 1,
    });
    let store = ResidentDeviceAssets::default();
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            3,
            4096,
        )
        .unwrap();
    let domain = control
        .begin_audio_candidate_domain_preparation_with_gate(
            hex_stream::StreamConfig::with_pool_pages(8),
            vec![old.clone(), new.clone()],
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
            gate,
        )
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match control
            .poll_audio_candidate_domain_preparation(domain)
            .unwrap()
        {
            crate::AudioCandidateDomainPreparationProgress::Ready => break,
            crate::AudioCandidateDomainPreparationProgress::Verifying => {}
            other => panic!("unexpected preparation: {other:?}"),
        }
        assert!(std::time::Instant::now() < deadline, "catalog did not seal");
        std::thread::yield_now();
    }
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    let revision = control
        .publish_with_admission_and_source_on_control(
            compile(&project, old.metadata, &store),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("old source publication"));
    let mut output = [[0.0; 2]; 256];
    while control.status.applied_revision.load(Acquire) != revision {
        measured_render(&mut data, &mut output);
        assert!(
            std::time::Instant::now() < deadline,
            "old source not installed"
        );
        std::thread::yield_now();
    }
    // Warm the replacement position before holding the single worker. The
    // second seek targets a cold page with the same index as the delayed page.
    let extents =
        [(old_key, 2), (new_key, 2)].map(|(source, page_index)| hex_engine::StreamResourceExtent {
            source: source.into(),
            page_index,
            start: 0,
            end: hex_engine::STREAM_PAGE_FRAMES,
        });
    let mut stamp = None;
    data.owner_mut().unwrap().run(|callback, _| {
        let Some(ActivePublication::Legacy(active)) = callback.active.as_ref() else {
            panic!("active graph")
        };
        let (loan, epoch) = active
            .streams
            .as_ref()
            .unwrap()
            .test_loan_for(&mut callback.domain_pages, old_key.into())
            .unwrap();
        *epoch += 1;
        let value = hex_stream::ReadinessStamp {
            request_epoch: *epoch,
            artifact: hex_stream::ReadinessArtifact::Ordinary {
                publication: revision,
            },
            project_generation: context.project_generation,
            editor_revision: context.editor_revision,
            device_epoch: 1,
            device_rate: 48_000,
            predecessor: None,
            admitted_command_prefix: 0,
            window: hex_engine::StreamChunkWindow {
                performance_frame: 0,
                arrangement_frame: 0,
                route_generation: 0,
                frames: 256,
            },
        };
        loan.submit_resources(hex_stream::ProtectionRole::Candidate, value, &extents)
            .unwrap();
        stamp = Some(value);
    });
    let stamp = stamp.unwrap();
    loop {
        let mut ready = false;
        data.owner_mut().unwrap().run(|callback, _| {
            let Some(ActivePublication::Legacy(active)) = callback.active.as_ref() else {
                panic!("active graph")
            };
            let (loan, _) = active
                .streams
                .as_ref()
                .unwrap()
                .test_loan_for(&mut callback.domain_pages, old_key.into())
                .unwrap();
            ready = loan
                .service(hex_stream::ProtectionRole::Candidate, stamp, 2, 2)
                .unwrap()
                == hex_stream::ReadinessProgress::InstalledProtected;
        });
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "destination pages not prewarmed"
        );
        std::thread::yield_now();
    }
    data.owner_mut().unwrap().run(|callback, _| {
        let Some(ActivePublication::Legacy(active)) = callback.active.as_ref() else {
            panic!("active graph")
        };
        let (loan, _) = active
            .streams
            .as_ref()
            .unwrap()
            .test_loan_for(&mut callback.domain_pages, old_key.into())
            .unwrap();
        assert!(loan.cancel(hex_stream::ProtectionRole::Candidate, stamp.request_epoch));
    });
    control.command(PlaybackCommand::Play).unwrap();
    loop {
        measured_render(&mut data, &mut output);
        if entered.try_recv().is_ok() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "old page not held");
        std::thread::yield_now();
    }
    let destination = 2 * hex_engine::STREAM_PAGE_FRAMES as u64;
    control
        .command(PlaybackCommand::PlayFrom(destination))
        .unwrap();
    measured_render(&mut data, &mut output);
    assert_eq!(output, [[0.25; 2]; 256]);
    let replacement = control
        .publish_with_admission_and_source_on_control(
            compile(&project, new.metadata, &store),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("new source publication"));
    while control.status.applied_revision.load(Acquire) != replacement {
        measured_render(&mut data, &mut output);
        assert!(
            std::time::Instant::now() < deadline,
            "replacement not installed"
        );
        std::thread::yield_now();
    }
    assert_eq!(output, [[0.75; 2]; 256]);
    let replacement_destination = hex_engine::STREAM_PAGE_FRAMES as u64;
    let previous_position = control.status.position.load(Acquire);
    control
        .command(PlaybackCommand::PlayFrom(replacement_destination))
        .unwrap();
    measured_render(&mut data, &mut output);
    assert_eq!(control.status.position.load(Acquire), previous_position);
    assert_eq!(output, [[0.0; 2]; 256]);
    release.send(()).unwrap();
    loop {
        let mut ready = false;
        data.owner_mut().unwrap().run(|callback, _| {
            let Some(ActivePublication::Legacy(active)) = callback.active.as_ref() else {
                panic!("active graph")
            };
            let (loan, _) = active
                .streams
                .as_ref()
                .unwrap()
                .test_loan_for(&mut callback.domain_pages, new_key.into())
                .unwrap();
            ready = loan.test_ready_page(hex_stream::ResourcePageKey {
                source: old_key.into(),
                page_index: 1,
            });
        });
        if ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "old completion not delivered"
        );
        std::thread::yield_now();
    }
    loop {
        measured_render(&mut data, &mut output);
        assert!(
            output
                .iter()
                .all(|frame| *frame == [0.0; 2] || *frame == [0.75; 2]),
            "old generation reached the callback"
        );
        if output == [[0.75; 2]; 256] {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "new destination not rendered"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        control.status.position.load(Acquire),
        replacement_destination + 256
    );
    data.owner_mut().unwrap().run(|callback, _| {
        let Some(ActivePublication::Legacy(active)) = callback.active.as_ref() else {
            panic!("active graph")
        };
        let (loan, _) = active
            .streams
            .as_ref()
            .unwrap()
            .test_loan_for(&mut callback.domain_pages, new_key.into())
            .unwrap();
        assert!(
            !loan.test_ready_page(hex_stream::ResourcePageKey {
                source: old_key.into(),
                page_index: 1
            }),
            "callback did not consume the late completion"
        );
    });
    assert!(control.retire_audio_candidate_domain(domain));
    control.request_shutdown();
    loop {
        measured_render(&mut data, &mut output);
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "domain did not retire"
        );
        std::thread::yield_now();
    }
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn prepared_audio_cold_seek_waits_for_destination_after_old_page() {
    prepared_audio_domain_gate(GatedPageAction::SeekCold);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn stop_cancels_cold_seek_while_an_old_stream_page_is_held() {
    prepared_audio_domain_gate(GatedPageAction::SeekColdStop);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn candidate_capacity_in_second_domain_rolls_back_first_domain_and_installs() {
    candidate_two_domain_capacity_case(TwoDomainAfterPressure::Install);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn stop_remains_effective_after_candidate_window_shrinks() {
    candidate_two_domain_capacity_case(TwoDomainAfterPressure::Stop);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn changed_source_after_candidate_window_shrinks_refuses_without_losing_current_audio() {
    candidate_two_domain_capacity_case(TwoDomainAfterPressure::ChangeSource);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn seek_during_candidate_pressure_is_refused_and_can_be_retried() {
    candidate_two_domain_capacity_case(TwoDomainAfterPressure::Seek);
}

#[cfg(feature = "stream-test-support")]
enum TwoDomainAfterPressure {
    Install,
    Stop,
    ChangeSource,
    Seek,
}

#[cfg(feature = "stream-test-support")]
fn candidate_two_domain_capacity_case(action: TwoDomainAfterPressure) {
    use hex_project::{Command, Created, Editor};
    let _serial = serial();
    let frames = 8192_u64;
    let original = super::readiness::project_with_frames(frames);
    let first_asset = original.assets().next().unwrap().id();
    let mut editor = Editor::new(original, 16).unwrap();
    let mut tracks = Vec::new();
    let mut assets = Vec::new();
    for name in ["old second", "new second"] {
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: name.into() })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Asset(asset) = editor
            .execute(Command::AddAsset {
                path: format!("{name}.wav").into(),
                sample_rate: 48_000,
                frames,
            })
            .unwrap()
        else {
            panic!("asset")
        };
        editor
            .execute(Command::PlaceClip {
                track,
                asset,
                start: 0,
                duration: frames,
                source_offset: 0,
                source_length: frames,
                looping: false,
            })
            .unwrap();
        tracks.push(track);
        assets.push(asset);
    }
    editor
        .execute(Command::Mute {
            track: tracks[1],
            muted: true,
        })
        .unwrap();
    let old = editor.project().clone();
    editor
        .execute(Command::Mute {
            track: tracks[0],
            muted: true,
        })
        .unwrap();
    editor
        .execute(Command::Mute {
            track: tracks[1],
            muted: false,
        })
        .unwrap();
    let next = editor.project().clone();
    let first_wav = stream_fixture::Wav::with_frames(8192);
    let old_wav = stream_fixture::Wav::with_frames(8192);
    let new_wav = stream_fixture::Wav::with_samples(8192, |_| 16_384);
    let first = first_wav.definition(first_asset, 11);
    let first_key = first.metadata.key();
    let old_second = old_wav.definition(assets[0], 11);
    let new_second = new_wav.definition(assets[1], 11);
    let new_path = new_second.path.clone();
    let metadata = vec![
        first.metadata.clone(),
        old_second.metadata.clone(),
        new_second.metadata.clone(),
    ];
    let store = ResidentDeviceAssets::default();
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8_000_000,
                max_construction_peak_data_bytes: 8_000_000,
            },
            2,
            4096,
        )
        .unwrap();
    let first_domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair_with_pool(vec![first], 3),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|_| panic!("first domain"));
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let second_domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair_with_pool(vec![old_second, new_second], 3),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|_| panic!("second domain"));
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    let old_revision = control
        .publish_with_admission_and_source_on_control(
            compile_sources(&old, metadata.clone(), &store),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("old publication"));
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 256];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while control.status.applied_revision.load(Acquire) != old_revision {
        measured_render(&mut data, &mut output);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    control.command(PlaybackCommand::Seek(3840)).unwrap();
    while control.status.position.load(Acquire) != 3968 {
        measured_render(&mut data, &mut output[..128]);
        assert!(
            std::time::Instant::now() < deadline,
            "seek to candidate boundary"
        );
        std::thread::yield_now();
    }
    while control.status.position.load(Acquire) == 3968 {
        measured_render(&mut data, &mut output);
        assert!(
            std::time::Instant::now() < deadline,
            "warm both current domains"
        );
        std::thread::yield_now();
    }
    assert_eq!(control.status.position.load(Acquire), 4224);
    assert_eq!(output, [[0.5; 2]; 256]);
    control.command(PlaybackCommand::Seek(3840)).unwrap();
    while control.status.position.load(Acquire) != 3968 {
        measured_render(&mut data, &mut output[..128]);
        assert!(
            std::time::Instant::now() < deadline,
            "restore candidate boundary"
        );
        std::thread::yield_now();
    }
    crate::candidate_window_probe::begin();
    let new_revision = control
        .publish_with_admission_and_source_on_control(
            compile_sources(&next, metadata, &store),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("new publication"));
    measured_render(&mut data, &mut output);
    assert_eq!(control.status.applied_revision.load(Acquire), old_revision);
    assert!(!control.status.stream_buffering.load(Acquire));
    assert_eq!(output, [[0.5; 2]; 256], "old sources remain audible");
    data.owner_mut().unwrap().run(|c, _| {
        let deferred = c.deferred.as_ref().expect("candidate remains owned");
        assert!(
            !deferred.streams.as_ref().unwrap().has_install_pending(),
            "partial candidate intents must roll back"
        );
        assert!(
            deferred
                .streams
                .as_ref()
                .unwrap()
                .test_candidate_role_idle(&mut c.domain_pages, first_key.into()),
            "first domain's actual candidate role must be idle"
        );
    });
    assert_eq!(
        crate::candidate_window_probe::domains(),
        Some((1, 1)),
        "the first domain must admit its intent before the second refuses capacity"
    );
    match action {
        TwoDomainAfterPressure::Stop => {
            control.stop();
            measured_render(&mut data, &mut output);
            assert!(
                !control.status.playing.load(Acquire),
                "priority stop must not wait for candidate readiness"
            );
            measured_render(&mut data, &mut output);
            assert!(output.iter().all(|frame| *frame == [0.0; 2]));
            let (failed, _, _) = crate::candidate_window_probe::finish().unwrap();
            assert_eq!(failed, 256);
        }
        TwoDomainAfterPressure::ChangeSource => {
            std::fs::write(&new_path, b"changed source").unwrap();
            loop {
                measured_render(&mut data, &mut output);
                assert_eq!(control.status.applied_revision.load(Acquire), old_revision);
                assert!(!control.status.stream_buffering.load(Acquire));
                assert_eq!(output, [[0.5; 2]; 256], "old sources remain audible");
                if let Some(rejected) = control.status.rejected_revision() {
                    assert_eq!(
                        rejected,
                        (new_revision, PlaybackHandoffRejection::StreamSourceChanged)
                    );
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "changed source was not refused"
                );
                std::thread::yield_now();
            }
            let (failed, next_width, _) = crate::candidate_window_probe::finish().unwrap();
            assert_eq!((failed, next_width), (256, 128));
        }
        TwoDomainAfterPressure::Seek => {
            let seek = control
                .command_with_ticket(PlaybackCommand::Seek(128))
                .unwrap();
            measured_render(&mut data, &mut output);
            assert_eq!(
                control.status.rejected_action(),
                Some((seek, PlaybackActionRejection::StreamActionBusy))
            );
            while control.status.applied_revision.load(Acquire) != new_revision {
                measured_render(&mut data, &mut output);
                assert!(control.status.rejected_revision().is_none());
                assert!(
                    std::time::Instant::now() < deadline,
                    "candidate did not complete"
                );
                std::thread::yield_now();
            }
            let retry = control
                .command_with_ticket(PlaybackCommand::Seek(128))
                .unwrap();
            while control.status.stream_applied_command_prefix.load(Acquire) < retry {
                measured_render(&mut data, &mut output);
                assert!(
                    std::time::Instant::now() < deadline,
                    "seek retry did not complete"
                );
                std::thread::yield_now();
            }
            assert_ne!(
                control.status.rejected_action().map(|(ticket, _)| ticket),
                Some(retry)
            );
            assert_eq!(control.status.position.load(Acquire), 384);
            let (failed, _, _) = crate::candidate_window_probe::finish().unwrap();
            assert_eq!(failed, 256);
        }
        TwoDomainAfterPressure::Install => {
            while control.status.applied_revision.load(Acquire) != new_revision {
                measured_render(&mut data, &mut output);
                assert!(control.status.rejected_revision().is_none());
                assert!(!control.status.stream_buffering.load(Acquire));
                assert!(
                    std::time::Instant::now() < deadline,
                    "candidate never installed"
                );
                std::thread::yield_now();
            }
            let (failed, next_width, _) = crate::candidate_window_probe::finish().unwrap();
            assert_eq!((failed, next_width), (256, 128));
            assert_eq!(output, [[0.75; 2]; 256]);
        }
    }
    assert!(control.retire_audio_candidate_domain(first_domain));
    assert!(control.retire_audio_candidate_domain(second_domain));
    control.request_shutdown();
    let mut recovered = [false; 2];
    while !recovered.iter().all(|v| *v) {
        measured_render(&mut data, &mut output);
        control.reclaim();
        for (i, domain) in [first_domain, second_domain].into_iter().enumerate() {
            if !recovered[i]
                && let Some(report) = control.recover_audio_candidate_domain(domain)
            {
                assert!(report.worker_joined && report.accounting_complete);
                recovered[i] = true;
            }
        }
        assert!(std::time::Instant::now() < deadline, "domain retirement");
        std::thread::yield_now();
    }
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn ordinary_partial_domain_readiness_survives_buffering_and_stop() {
    partial_domain_readiness_case(false);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn fresh_replacement_can_escape_a_stalled_stream_window() {
    partial_domain_readiness_case(true);
}

#[cfg(feature = "stream-test-support")]
fn partial_domain_readiness_case(replace_while_stalled: bool) {
    use hex_project::{Command, Created, Editor};

    let _serial = serial();
    let frames = (hex_engine::STREAM_PAGE_FRAMES * 2) as u64;
    let original = super::readiness::project_with_frames(frames);
    let first_asset = original.assets().next().unwrap().id();
    let mut editor = Editor::new(original, 16).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddTrack {
            name: "second".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Asset(second_asset) = editor
        .execute(Command::AddAsset {
            path: "second.wav".into(),
            sample_rate: 48_000,
            frames,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    editor
        .execute(Command::PlaceClip {
            track,
            asset: second_asset,
            start: 0,
            duration: frames,
            source_offset: 0,
            source_length: frames,
            looping: false,
        })
        .unwrap();
    let project = editor.project().clone();
    editor
        .execute(Command::Mute { track, muted: true })
        .unwrap();
    let replacement_project = editor.project().clone();
    let first_wav = stream_fixture::Wav::with_frames(frames.try_into().unwrap());
    let second_wav = stream_fixture::Wav::with_samples(frames.try_into().unwrap(), |_| 16_384);
    let first = first_wav.definition(first_asset, 11);
    let second = second_wav.definition(second_asset, 11);
    let first_key = first.metadata.key();
    let second_key = second.metadata.key();
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(hex_stream::PageKey {
        source: second.metadata.key(),
        page_index: 1,
    });
    let store = ResidentDeviceAssets::default();
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            2,
            4096,
        )
        .unwrap();
    let first_domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair_with_pool(vec![first.clone()], 3),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|_| panic!("first domain"));
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    let second_domain = control
        .begin_audio_candidate_domain_preparation_with_gate(
            hex_stream::StreamConfig::with_pool_pages(3),
            vec![second.clone()],
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
            gate,
        )
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match control
            .poll_audio_candidate_domain_preparation(second_domain)
            .unwrap()
        {
            crate::AudioCandidateDomainPreparationProgress::Ready => break,
            crate::AudioCandidateDomainPreparationProgress::Verifying => {}
            other => panic!("unexpected preparation: {other:?}"),
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    control
        .publish_with_admission_and_source_on_control(
            compile_sources(
                &project,
                vec![first.metadata.clone(), second.metadata.clone()],
                &store,
            ),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("publication"));
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 256];
    let mut held = false;
    while control.status.position.load(Acquire) < hex_engine::STREAM_PAGE_FRAMES as u64 {
        measured_render(&mut data, &mut output);
        held |= entered.try_recv().is_ok();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    while !held {
        measured_render(&mut data, &mut output);
        held = entered.try_recv().is_ok();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let observed = |data: &mut crate::native_callback_owner::Guard<
        crate::native_callback_owner::NativeCallbackOwner,
    >| {
        let mut result = (false, false);
        data.owner_mut().unwrap().run(|c, _| {
            if let Some(ActivePublication::Legacy(active)) = c.active.as_ref()
                && let Some(streams) = active.streams.as_ref()
            {
                result.0 = streams.test_domain_cached(
                    &mut c.domain_pages,
                    hex_engine::StreamResourceKey::Audio(first_key),
                );
                result.1 = streams.test_domain_protected(
                    &mut c.domain_pages,
                    hex_engine::StreamResourceKey::Audio(first_key),
                );
            }
        });
        result
    };
    let mut result = (false, false);
    for _ in 0..256 {
        measured_render(&mut data, &mut output);
        assert!(control.status.stream_buffering.load(Acquire));
        assert_eq!(
            control.status.position.load(Acquire),
            hex_engine::STREAM_PAGE_FRAMES as u64
        );
        assert_eq!(output, [[0.0; 2]; 256]);
        result = observed(&mut data);
        if result.0 {
            break;
        }
        std::thread::yield_now();
    }
    let mut coalesced = false;
    data.owner_mut().unwrap().run(|c, _| {
        if let Some(ActivePublication::Legacy(active)) = c.active.as_ref()
            && let Some(streams) = active.streams.as_ref()
        {
            coalesced = streams.test_coalesced_prefetch(
                &mut c.domain_pages,
                hex_stream::ResourcePageKey {
                    source: second_key.into(),
                    page_index: 1,
                },
            );
        }
    });
    assert!(
        coalesced,
        "the gated page must be lookahead coalesced with actual readiness"
    );
    assert!(result.0, "first domain did not deliver the requested page");
    assert!(
        result.1,
        "ready domain lost its protection while the other domain was pending"
    );
    measured_render(&mut data, &mut output);
    assert!(
        observed(&mut data).1,
        "protection must survive the next callback"
    );
    if replace_while_stalled {
        let replacement = control
            .publish_with_admission_and_source_on_control(
                compile_sources(
                    &replacement_project,
                    vec![first.metadata, second.metadata],
                    &store,
                ),
                PublicationPolicy::InstallFreshRuntime,
                hex_engine::AudioSourceContext {
                    project_generation: 7,
                    editor_revision: 4,
                },
                |_| Ok::<_, ()>(()),
            )
            .unwrap_or_else(|_| panic!("fresh replacement"));
        for _ in 0..256 {
            measured_render(&mut data, &mut output);
            if control.status.applied_revision.load(Acquire) == replacement {
                break;
            }
            assert!(control.status.rejected_revision().is_none());
            std::thread::yield_now();
        }
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            replacement,
            "fresh replacement remained blocked by an unrelated held page"
        );
        release.send(()).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        loop {
            measured_render(&mut data, &mut output);
            if output.iter().all(|frame| *frame == [0.25; 2]) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "fresh replacement did not resume its resident source"
            );
            std::thread::yield_now();
        }
    } else {
        control.stop();
        measured_render(&mut data, &mut output);
        assert!(!observed(&mut data).1);
        assert!(!control.status.playing.load(Acquire));
        release.send(()).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        loop {
            measured_render(&mut data, &mut output);
            if output.iter().all(|frame| *frame == [0.75; 2]) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    assert!(control.retire_audio_candidate_domain(first_domain));
    assert!(control.retire_audio_candidate_domain(second_domain));
    control.request_shutdown();
    let mut recovered = [false; 2];
    while !recovered.iter().all(|value| *value) {
        measured_render(&mut data, &mut output);
        control.reclaim();
        for (index, domain) in [first_domain, second_domain].into_iter().enumerate() {
            if !recovered[index]
                && let Some(report) = control.recover_audio_candidate_domain(domain)
            {
                assert!(report.worker_joined && report.accounting_complete);
                recovered[index] = true;
            }
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}

#[cfg(feature = "stream-test-support")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum GatedPageAction {
    Resume,
    Retire,
    Seek,
    SeekCold,
    SeekColdStop,
}

#[cfg(feature = "stream-test-support")]
fn prepared_audio_domain_gate(action: GatedPageAction) {
    let _serial = serial();
    let frames = (hex_engine::STREAM_PAGE_FRAMES * 3) as u64;
    let project = super::readiness::project_with_frames(frames);
    let asset = project.assets().next().unwrap().id();
    let wav = if matches!(
        action,
        GatedPageAction::Seek | GatedPageAction::SeekCold | GatedPageAction::SeekColdStop
    ) {
        stream_fixture::Wav::with_samples(frames.try_into().unwrap(), |frame| {
            if frame < hex_engine::STREAM_PAGE_FRAMES as u32 {
                8192
            } else if frame < 2 * hex_engine::STREAM_PAGE_FRAMES as u32 {
                16_384
            } else {
                24_576
            }
        })
    } else {
        stream_fixture::Wav::with_frames(frames.try_into().unwrap())
    };
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let source_key = metadata.key();
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(hex_stream::PageKey {
        source: metadata.key(),
        page_index: 1,
    });
    let store = ResidentDeviceAssets::default();
    let (mut control, mut data, errors, mut retirement) = anchored();
    let (refused_gate, refused_entered, _refused_release) =
        hex_stream::test_support::PageGate::new(hex_stream::PageKey {
            source: metadata.key(),
            page_index: 1,
        });
    assert!(matches!(
        control.begin_audio_candidate_domain_preparation_with_gate(
            hex_stream::StreamConfig::with_pool_pages(4),
            vec![wav.definition(asset, 11)],
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3
            },
            refused_gate,
        ),
        Err(crate::AudioCandidateDomainStartError::Busy(_))
    ));
    assert!(matches!(
        refused_entered.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Disconnected)
    ));
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            3,
            4096,
        )
        .unwrap();
    let domain = control
        .begin_audio_candidate_domain_preparation_with_gate(
            hex_stream::StreamConfig::with_pool_pages(4),
            vec![definition],
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
            gate,
        )
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match control
            .poll_audio_candidate_domain_preparation(domain)
            .unwrap()
        {
            crate::AudioCandidateDomainPreparationProgress::Ready => break,
            crate::AudioCandidateDomainPreparationProgress::Verifying => {}
            other => panic!("unexpected preparation: {other:?}"),
        }
        assert!(std::time::Instant::now() < deadline, "catalog did not seal");
        std::thread::yield_now();
    }
    assert!(control.audio_candidate_domain_covers_source(domain, &metadata));
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let revision = control
        .publish_with_admission_and_source_on_control(
            compile(&project, metadata, &store),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("source publication"));
    let mut output = [[0.0; 2]; 256];
    if action == GatedPageAction::Seek {
        while control.status.applied_revision.load(Acquire) != revision {
            measured_render(&mut data, &mut output);
            assert!(std::time::Instant::now() < deadline, "source not installed");
            std::thread::yield_now();
        }
        data.owner_mut().unwrap().run(|callback, _| {
            let Some(ActivePublication::Legacy(active)) = callback.active.as_ref() else {
                panic!("active streamed graph")
            };
            let (loan, _) = active
                .streams
                .as_ref()
                .unwrap()
                .test_loan_for(&mut callback.domain_pages, source_key.into())
                .unwrap();
            loan.test_prefetch_audio_page(source_key, 2);
        });
        loop {
            measured_render(&mut data, &mut output);
            let mut destination_ready = false;
            data.owner_mut().unwrap().run(|callback, _| {
                let Some(ActivePublication::Legacy(active)) = callback.active.as_ref() else {
                    panic!("active streamed graph")
                };
                let (loan, _) = active
                    .streams
                    .as_ref()
                    .unwrap()
                    .test_loan_for(&mut callback.domain_pages, source_key.into())
                    .unwrap();
                destination_ready =
                    loan.test_has_resource_extent(hex_engine::StreamResourceExtent {
                        source: source_key.into(),
                        page_index: 2,
                        start: 0,
                        end: hex_engine::STREAM_PAGE_FRAMES,
                    });
            });
            if destination_ready {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "destination not prewarmed"
            );
            std::thread::yield_now();
        }
    }
    control.command(PlaybackCommand::Play).unwrap();
    let mut heard = false;
    loop {
        measured_render(&mut data, &mut output);
        heard |= output.contains(&[0.25; 2]);
        if entered.try_recv().is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "later page not requested"
        );
        std::thread::yield_now();
    }
    while control.status.position.load(Acquire) < hex_engine::STREAM_PAGE_FRAMES as u64 {
        measured_render(&mut data, &mut output);
        heard |= output.contains(&[0.25; 2]);
        assert!(
            std::time::Instant::now() < deadline,
            "first page did not finish"
        );
        std::thread::yield_now();
    }
    assert!(heard);
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    measured_render(&mut data, &mut output);
    assert!(control.status.stream_buffering.load(Acquire));
    assert_eq!(output, [[0.0; 2]; 256]);
    // The held page starved audible playback: the published count rises (#4074).
    let starved = control.status.stream_starved_callbacks.load(Relaxed);
    assert!(starved > 0, "mid-play page hold was not counted");
    measured_render(&mut data, &mut output);
    assert!(control.status.stream_starved_callbacks.load(Relaxed) > starved);
    if action == GatedPageAction::Retire {
        control.stop();
        measured_render(&mut data, &mut output);
        assert!(!control.status.playing.load(Acquire));
        assert_eq!(output, [[0.0; 2]; 256]);
        // Stopped holds are not audible starvation.
        let stopped = control.status.stream_starved_callbacks.load(Relaxed);
        measured_render(&mut data, &mut output);
        assert_eq!(
            control.status.stream_starved_callbacks.load(Relaxed),
            stopped
        );
    } else if matches!(
        action,
        GatedPageAction::Seek | GatedPageAction::SeekCold | GatedPageAction::SeekColdStop
    ) {
        let destination = 2 * hex_engine::STREAM_PAGE_FRAMES as u64;
        control
            .command(PlaybackCommand::PlayFrom(destination))
            .unwrap();
        measured_render(&mut data, &mut output);
        if action == GatedPageAction::Seek {
            assert_eq!(output, [[0.75; 2]; 256]);
            assert_eq!(control.status.position.load(Acquire), destination + 256);
        } else {
            assert_eq!(output, [[0.0; 2]; 256]);
            assert_eq!(
                control.status.position.load(Acquire),
                hex_engine::STREAM_PAGE_FRAMES as u64
            );
        }
        let stopped_position = if action == GatedPageAction::SeekColdStop {
            control.stop();
            measured_render(&mut data, &mut output);
            assert!(!control.status.playing.load(Acquire));
            assert_eq!(output, [[0.0; 2]; 256]);
            Some(control.status.position.load(Acquire))
        } else {
            None
        };
        release.send(()).unwrap();
        let old_page_is_ready = |data: &mut crate::native_callback_owner::Guard<
            crate::native_callback_owner::NativeCallbackOwner,
        >| {
            let mut ready = false;
            data.owner_mut().unwrap().run(|callback, _| {
                let Some(ActivePublication::Legacy(active)) = callback.active.as_ref() else {
                    panic!("active streamed graph")
                };
                let (loan, _) = active
                    .streams
                    .as_ref()
                    .unwrap()
                    .test_loan_for(&mut callback.domain_pages, source_key.into())
                    .unwrap();
                ready = loan.test_ready_page(hex_stream::ResourcePageKey {
                    source: source_key.into(),
                    page_index: 1,
                });
            });
            ready
        };
        while !old_page_is_ready(&mut data) {
            assert!(
                std::time::Instant::now() < deadline,
                "old page was not delivered"
            );
            std::thread::yield_now();
        }
        measured_render(&mut data, &mut output);
        assert!(
            !old_page_is_ready(&mut data),
            "callback did not consume the old completion"
        );
        if action == GatedPageAction::SeekColdStop {
            assert!(!control.status.playing.load(Acquire));
            assert_eq!(output, [[0.0; 2]; 256]);
            data.owner_mut().unwrap().run(|callback, _| {
                let Some(ActivePublication::Legacy(active)) = callback.active.as_ref() else {
                    panic!("active streamed graph")
                };
                let (loan, _) = active
                    .streams
                    .as_ref()
                    .unwrap()
                    .test_loan_for(&mut callback.domain_pages, source_key.into())
                    .unwrap();
                loan.test_prefetch_audio_page(source_key, 2);
            });
            loop {
                let mut available = false;
                data.owner_mut().unwrap().run(|callback, _| {
                    let Some(ActivePublication::Legacy(active)) = callback.active.as_ref() else {
                        panic!("active streamed graph")
                    };
                    let (loan, _) = active
                        .streams
                        .as_ref()
                        .unwrap()
                        .test_loan_for(&mut callback.domain_pages, source_key.into())
                        .unwrap();
                    available =
                        loan.test_ready_page(hex_stream::ResourcePageKey {
                            source: source_key.into(),
                            page_index: 2,
                        }) || loan.test_has_resource_extent(hex_engine::StreamResourceExtent {
                            source: source_key.into(),
                            page_index: 2,
                            start: 0,
                            end: hex_engine::STREAM_PAGE_FRAMES,
                        });
                });
                if available {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "destination page not delivered"
                );
                std::thread::yield_now();
            }
            for _ in 0..3 {
                measured_render(&mut data, &mut output);
                assert!(!control.status.playing.load(Acquire));
                assert_eq!(output, [[0.0; 2]; 256]);
                assert_eq!(
                    Some(control.status.position.load(Acquire)),
                    stopped_position
                );
            }
            control.command(PlaybackCommand::Play).unwrap();
            loop {
                measured_render(&mut data, &mut output);
                assert!(output.iter().all(|frame| *frame != [0.75; 2]));
                if output.iter().all(|frame| *frame == [0.5; 2]) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "stopped position did not resume"
                );
                std::thread::yield_now();
            }
        } else {
            if action == GatedPageAction::SeekCold {
                while control.status.position.load(Acquire) < destination {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "cold seek did not apply"
                    );
                    std::thread::yield_now();
                    measured_render(&mut data, &mut output);
                }
            }
            assert_eq!(
                output, [[0.75; 2]; 256],
                "old page must not play after the seek"
            );
            if action == GatedPageAction::Seek {
                assert_eq!(control.status.position.load(Acquire), destination + 512);
            } else {
                assert!(control.status.position.load(Acquire) >= destination + 256);
                for _ in 0..3 {
                    measured_render(&mut data, &mut output);
                    assert_eq!(output, [[0.75; 2]; 256]);
                }
            }
        }
    } else {
        release.send(()).unwrap();
        loop {
            measured_render(&mut data, &mut output);
            if output.iter().all(|frame| *frame == [0.25; 2]) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "released page did not render"
            );
            std::thread::yield_now();
        }
    }
    assert!(control.retire_audio_candidate_domain(domain));
    control.request_shutdown();
    loop {
        measured_render(&mut data, &mut output);
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "domain did not retire"
        );
        std::thread::yield_now();
    }
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}

#[test]
fn compensated_demand_retries_a_smaller_window_at_page_capacity() {
    let _serial = serial();
    let mut snapshot = super::readiness::project_with_frames(8192).snapshot();
    let clips = match &mut snapshot.tracks[0].content {
        hex_project::TrackContentSnapshot::Audio(clips) => clips,
        hex_project::TrackContentSnapshot::Hybrid { audio_clips, .. } => audio_clips,
        _ => panic!("audio fixture"),
    };
    clips[0].source_offset = 4095;
    clips[0].source_length = 4097;
    clips[0].duration = 4097;
    clips[0].fade = hex_project::ClipFade::neutral(4097).unwrap();
    snapshot.tracks[0].devices.clear();
    snapshot.tracks[0]
        .devices
        .push(hex_project::DeviceSnapshot {
            modulation: Default::default(),
            parent: None,
            automation: Vec::new(),
            comparison: None,
            id: snapshot.next_id,
            bypassed: false,
            state: hex_engine::default_native_device_state(
                hex_lattice_contract::DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap(),
        });
    snapshot.next_id += 1;
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(8192);
    let definition = wav.definition(asset, 11);
    let store = ResidentDeviceAssets::default();
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8_000_000,
                max_construction_peak_data_bytes: 8_000_000,
            },
            1,
            4096,
        )
        .unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair(definition.clone()),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 1,
            },
        )
        .unwrap_or_else(|_| panic!("source registration"));
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let revision = control
        .publish_with_admission_and_source_on_control(
            compile(&project, definition.metadata, &store),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("publication"));
    control.command(PlaybackCommand::Play).unwrap();
    let mut collected = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while collected.len() < 512 {
        let mut output = [[0.0; 2]; 256];
        let before = control.status.performance_frame.load(Acquire);
        measured_render(&mut data, &mut output);
        assert!(!control.status.stream_readiness_refused.load(Acquire));
        assert!(control.status.rejected_revision().is_none());
        let after = control.status.performance_frame.load(Acquire);
        let count = usize::try_from(after - before).unwrap();
        collected.extend_from_slice(&output[..count]);
        assert!(
            std::time::Instant::now() < deadline,
            "stream made no progress"
        );
        std::thread::yield_now();
    }
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert!(collected[..72].iter().all(|frame| *frame == [0.0; 2]));
    assert!(collected[72..512].iter().all(|frame| *frame == [0.25; 2]));
    assert!(control.retire_audio_candidate_domain(domain));
    control.request_shutdown();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        measured_render(&mut data, &mut [[0.0; 2]; 256]);
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "source reclamation");
        std::thread::yield_now();
    }
    assert_eq!(control.unreturned_resident_graphs_on_control(), Some(0));
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn refused_parameter_successor_reenables_predecessor_launch_prewarm() {
    let _serial = serial();
    let mut snapshot = super::readiness::project_with_frames(12288).snapshot();
    let scene = snapshot.next_id;
    let clip = scene + 1;
    snapshot.next_id += 2;
    snapshot.session.scenes.push(hex_project::SceneSnapshot {
        id: scene,
        name: "launch".into(),
    });
    snapshot
        .session
        .cells
        .push(hex_project::SessionCellSnapshot {
            track: snapshot.tracks[0].id,
            scene,
            content: hex_project::SessionCellContentSnapshot::Audio(
                hex_project::SessionAudioClipSnapshot {
                    loop_range: None,
                    active: true,
                    flavor: hex_project::ClipFlavor::default(),
                    gain: hex_project::ClipGain::UNITY,
                    id: clip,
                    name: "future".into(),
                    asset: snapshot.assets[0].id,
                    source_start: 8192,
                    source_end: 12288,
                },
            ),
        });
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    let hex_project::SessionCellContent::Audio(clip) = project.session_cells()[0].content() else {
        panic!("audio clip")
    };
    let clip = clip.id();
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(12288);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let store = ResidentDeviceAssets::default();
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            2,
            4096,
        )
        .unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair_with_pool(vec![definition], 3),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|_| panic!("source registration"));
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let revision = control
        .publish_with_admission_and_source_on_control(
            compile(&project, metadata.clone(), &store),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut output = [[0.0; 2]; 32];
    while control.status.position.load(Acquire) < 64 {
        measured_render(&mut data, &mut output);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let ticket = control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::FourQuarter,
        })
        .unwrap();
    measured_render(&mut data, &mut output);
    assert_eq!(
        control.status.stream_scheduled_command.load(Acquire),
        ticket
    );
    let pinned = |data: &mut crate::native_callback_owner::Guard<
        crate::native_callback_owner::NativeCallbackOwner,
    >| {
        let mut pinned = false;
        let counted = allocation_counter::measure(|| {
            data.owner_mut().unwrap().run(|callback, _| {
                if let Some(mut active) = callback
                    .active
                    .as_mut()
                    .and_then(ActivePublication::stream_view)
                {
                    pinned = active.test_launch_prewarm_protected(
                        &mut callback.domain_pages,
                        callback.stream_prefix,
                    );
                }
            });
        });
        assert_eq!(
            (
                counted.count_total,
                counted.bytes_total,
                counted.count_current,
                counted.bytes_current
            ),
            (0, 0, 0, 0)
        );
        pinned
    };
    while !pinned(&mut data) {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    // Simulate the earlier preflight of this exact successor. The callback's
    // rejection below must restore the retained graph's pending schedule.
    data.owner_mut().unwrap().run(|callback, _| {
        callback
            .active
            .as_mut()
            .unwrap()
            .stream_view()
            .unwrap()
            .publication_replaces_prewarm(&mut callback.domain_pages);
    });
    control
        .status
        .ordinary_native_revision
        .store(revision, Release);
    let (source, epoch) = control.status.native_parameter_source(true).unwrap();
    let candidate = control
        .publish_with_admission_and_source_on_control(
            compile(&project, metadata.clone(), &store),
            PublicationPolicy::InheritUnderNativeParameter {
                source,
                transport_epoch: epoch + 1,
                playing: true,
            },
            hex_engine::AudioSourceContext {
                editor_revision: context.editor_revision + 1,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    while control
        .status
        .rejected_revision()
        .is_none_or(|(id, _)| id != candidate)
    {
        measured_render(&mut data, &mut output);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    while !pinned(&mut data) {
        assert!(
            std::time::Instant::now() < deadline,
            "retained launch never regained prewarm"
        );
        std::thread::yield_now();
    }
    assert_eq!(
        control.status.stream_scheduled_command.load(Acquire),
        ticket
    );
    assert_eq!(control.status.stream_activated_command.load(Acquire), 0);
    control.reclaim();
    let candidate = control
        .publish_with_admission_and_source_on_control(
            compile(&project, metadata, &store),
            PublicationPolicy::InheritUnderNativeParameter {
                source,
                transport_epoch: epoch,
                playing: true,
            },
            hex_engine::AudioSourceContext {
                editor_revision: context.editor_revision + 1,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    // Receive the graph without stream preflight, then model an earlier
    // revocation before a command is admitted on the predecessor.
    data.owner_mut().unwrap().run(|callback, _| {
        callback.begin_block();
        assert!(callback.deferred.is_some());
        callback
            .active
            .as_mut()
            .unwrap()
            .stream_view()
            .unwrap()
            .publication_replaces_prewarm(&mut callback.domain_pages);
    });
    control
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    while !pinned(&mut data) {
        assert!(
            std::time::Instant::now() < deadline,
            "queued command stranded launch prewarm"
        );
        std::thread::yield_now();
    }
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert_eq!(
        control.status.stream_scheduled_command.load(Acquire),
        ticket
    );
    control.stop();
    while control
        .status
        .rejected_revision()
        .is_none_or(|(id, _)| id != candidate)
    {
        measured_render(&mut data, &mut output);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(control.retire_audio_candidate_domain(domain));
    control.request_shutdown();
    while control.recover_audio_candidate_domain(domain).is_none() {
        measured_render(&mut data, &mut output);
        control.reclaim();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn cancelled_quantized_stream_releases_pending_candidate_pages() {
    rejected_quantized_stream_releases_pending_candidate_pages(false);
}

#[cfg(feature = "stream-test-support")]
#[test]
fn stale_quantized_stream_releases_pending_candidate_pages() {
    rejected_quantized_stream_releases_pending_candidate_pages(true);
}

#[cfg(feature = "stream-test-support")]
fn rejected_quantized_stream_releases_pending_candidate_pages(stale: bool) {
    let _serial = serial();
    let frames = (hex_engine::STREAM_PAGE_FRAMES * 2) as u64;
    let project = super::readiness::project_with_frames(frames);
    let asset = project.assets().next().unwrap().id();
    let wav = stream_fixture::Wav::with_frames(frames.try_into().unwrap());
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let key = metadata.key();
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(hex_stream::PageKey {
        source: key,
        page_index: 1,
    });
    let mut runtime = hex_stream::StreamRuntime::new_with_page_gate(
        hex_stream::StreamConfig::with_pool_pages(3),
        vec![definition],
        gate,
    )
    .unwrap();
    runtime
        .wait_until_ready(std::time::Duration::from_secs(2))
        .unwrap();
    let pair = runtime.split().ok().unwrap();
    let store = ResidentDeviceAssets::default();
    let context = hex_engine::AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    struct ReleaseGate(Option<std::sync::mpsc::SyncSender<()>>);
    impl Drop for ReleaseGate {
        fn drop(&mut self) {
            if let Some(release) = self.0.take() {
                let _ = release.try_send(());
            }
        }
    }
    let mut release = ReleaseGate(Some(release));
    control
        .configure_device_resource_publication_on_control(resource_limits())
        .unwrap();
    control
        .configure_ordinary_streaming_on_control(
            crate::AudioCandidateDomainReservation {
                max_retained_data_bytes: 8 * 1024 * 1024,
                max_construction_peak_data_bytes: 8 * 1024 * 1024,
            },
            2,
            4096,
        )
        .unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            pair,
            hex_stream::StreamReadinessReservation {
                extents_per_role: 2,
            },
        )
        .unwrap_or_else(|error| panic!("domain registration: {}", error.reason));
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let old = control
        .publish_with_admission_and_source_on_control(
            compile(&project, metadata.clone(), &store),
            PublicationPolicy::InheritRuntime,
            context,
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("old publication"));
    control.command(PlaybackCommand::Play).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    let mut output = [[0.0; 2]; 8];
    loop {
        measured_render(&mut data, &mut output);
        if output.contains(&[0.25; 2]) {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "initial page");
        std::thread::yield_now();
    }
    let mut next = compile(&project, metadata.clone(), &store);
    next.apply(PlaybackCommand::PlayFrom(
        hex_engine::STREAM_PAGE_FRAMES as u64,
    ));
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_admission_and_source_on_control(
            next,
            PublicationPolicy::InstallQuantizedSeekWhenReady {
                fence,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("candidate publication"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    let mut pending = false;
    while !pending {
        measured_render(&mut data, &mut output);
        data.owner_mut().unwrap().run(|callback, _| {
            pending = callback
                .deferred
                .as_ref()
                .and_then(|p| p.streams.as_ref())
                .is_some_and(|s| s.has_install_pending());
        });
        assert!(
            std::time::Instant::now() < deadline,
            "candidate reservation"
        );
        std::thread::yield_now();
    }
    assert_eq!(control.status.applied_revision.load(Acquire), old);
    assert!(
        entered
            .recv_timeout(std::time::Duration::from_millis(500))
            .is_ok()
    );
    let reason = if stale {
        control.command(PlaybackCommand::Play).unwrap();
        PlaybackHandoffRejection::RuntimeFenceChanged
    } else {
        assert_eq!(
            control.cancel_fresh_runtime_publication(revision),
            FreshRuntimeCancellationResult::Cancelled
        );
        PlaybackHandoffRejection::FreshRuntimeCancelled
    };
    measured_render(&mut data, &mut output);
    assert_eq!(control.status.rejected_revision(), Some((revision, reason)));
    assert_eq!(control.status.applied_revision.load(Acquire), old);
    assert!(output.contains(&[0.25; 2]), "old stream continues");
    let returned = control.publication.retired.as_mut().unwrap().pop().unwrap();
    let ReturnedPublication::Legacy(ref publication) = returned else {
        panic!("returned candidate")
    };
    let pending_after_return = publication.streams.as_ref().unwrap().has_install_pending();
    release.0.take().unwrap().send(()).unwrap();
    assert!(
        !pending_after_return,
        "cancelled candidate must release its page reservation"
    );
    control.publication.held_control_return = Some(returned);
    control.reclaim();
    let mut retry = compile(&project, metadata, &store);
    retry.apply(PlaybackCommand::PlayFrom(
        hex_engine::STREAM_PAGE_FRAMES as u64,
    ));
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let retry_revision = control
        .publish_with_admission_and_source_on_control(
            retry,
            PublicationPolicy::InstallQuantizedSeekWhenReady {
                fence,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
            hex_engine::AudioSourceContext {
                editor_revision: 5,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("retry publication"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while control.status.applied_revision.load(Acquire) != retry_revision {
        measured_render(&mut data, &mut output);
        assert!(
            std::time::Instant::now() < deadline,
            "released page must admit the next candidate"
        );
        std::thread::yield_now();
    }
    assert!(output.contains(&[0.25; 2]));
    control.reclaim();
    assert!(control.retire_audio_candidate_domain(domain));
    control.request_shutdown();
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    loop {
        measured_render(&mut data, &mut output);
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "source reclamation");
        std::thread::yield_now();
    }
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}
