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
    ordinary_case(false, false, false);
}
#[test]
fn ordinary_sampler_and_streamed_clip_share_resource_and_page_custody_without_callback_allocations()
{
    ordinary_case(true, false, false);
}
#[test]
fn ordinary_paused_keyboard_monitor_and_first_play_keep_resident_pcm_and_clock_semantics() {
    ordinary_case(true, true, false);
}
#[test]
fn grouped_file_backed_native_playback_matches_resident_oracle_without_callback_allocations() {
    ordinary_case(false, false, true);
}
#[test]
fn grouped_sampler_and_file_backed_sources_keep_existing_resource_custody() {
    ordinary_case(true, false, true);
}
fn ordinary_case(sampler: bool, keyboard: bool, grouped: bool) {
    let _serial = serial();
    let mut project = super::readiness::project_with_frames(8);
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
                        8, asset,
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
                    hex_engine::OwnedAudioSource::try_resident(48_000, vec![[0.25; 2]; 8])
                        .ok()
                        .unwrap(),
                ),
                0,
                resource_limits(),
                8,
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
            hex_engine::Audio::new(48_000, vec![[0.25; 2]; 8]).unwrap(),
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
    let wav = stream_fixture::Wav::new();
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
    let play = if keyboard {
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let output = loop {
        let output = measured_block(&mut data);
        if control.status.applied_revision.load(Acquire) == revision {
            break output;
        }
        assert_eq!(output, [[0.0; 2]; 8]);
        assert_eq!(control.status.performance_frame.load(Acquire), 0);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    };
    assert_eq!(output, expected);
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
fn ordinary_pending_replacement_yields_small_pool_to_the_current_stream() {
    use hex_project::{Command, Created, Editor};

    let _serial = serial();
    let frames = (hex_engine::STREAM_PAGE_FRAMES * 2) as u64;
    let original = super::readiness::project_with_frames(frames);
    let a_track = original.tracks()[0].id();
    let a_asset = original.assets().next().unwrap().id();
    let mut editor = Editor::new(original, 16).unwrap();
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
            frames,
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
            source_offset: 0,
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
    let b_project = editor.project().clone();
    let a_wav = stream_fixture::Wav::with_frames(frames.try_into().unwrap());
    let b_wav = stream_fixture::Wav::with_samples(frames.try_into().unwrap(), |_| 16_384);
    let a_definition = a_wav.definition(a_asset, 11);
    let b_definition = b_wav.definition(b_asset, 11);
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
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            stream_fixture::pair_with_pool(vec![a_definition, b_definition], 3),
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
    let mut output = [[0.0; 2]; 256];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while control.status.position.load(Acquire) < hex_engine::STREAM_PAGE_FRAMES as u64 {
        measured_render(&mut data, &mut output);
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
    assert_eq!(output, [[0.25; 2]; 256]);

    // The last committed chunk protects A0. The next candidate needs B1, which
    // has never been requested: its first service must return Pending even if
    // the real worker finishes immediately afterward. The current graph then
    // needs A1. A0 + the speculative B1 intent + A1 cannot fit cache capacity 2.
    // Yielding the candidate's exact intent lets the old graph advance instead
    // of turning ordinary disk latency into a readiness refusal.
    let b_revision = control
        .publish_with_admission_and_source_on_control(
            compile_sources(&b_project, metadata, &store),
            PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                editor_revision: 4,
                ..context
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap_or_else(|_| panic!("replacement publication"));
    measured_render(&mut data, &mut output);
    let refused = control.status.stream_readiness_refused.load(Acquire);
    let rejected = control.status.rejected_revision();
    let pending = control.status.publication_pending.load(Acquire);
    if !refused && rejected.is_none() {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while control.status.applied_revision.load(Acquire) != b_revision {
            measured_render(&mut data, &mut output);
            assert!(control.status.rejected_revision().is_none());
            assert!(
                std::time::Instant::now() < deadline,
                "replacement readiness"
            );
            std::thread::yield_now();
        }
        assert_eq!(output, [[0.5; 2]; 256]);
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
