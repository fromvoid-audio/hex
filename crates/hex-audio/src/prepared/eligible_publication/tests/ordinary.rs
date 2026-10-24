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
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(store, project, 1_000_000, &cancel).unwrap();
    let generation = metadata.key().generation();
    let sources = BTreeMap::from([(
        metadata.key().asset(),
        hex_engine::compile::PreparedAudioSource::Streamed(Arc::new(metadata)),
    )]);
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
    let counted = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
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
    output
}
#[test]
fn ordinary_resource_graph_requires_actual_catalog_and_protected_pages_and_returns_before_join() {
    ordinary_case(false, false);
}
#[test]
fn ordinary_sampler_and_streamed_clip_share_resource_and_page_custody_without_callback_allocations()
{
    ordinary_case(true, false);
}
#[test]
fn ordinary_paused_keyboard_monitor_and_first_play_keep_resident_pcm_and_clock_semantics() {
    ordinary_case(true, true);
}
fn ordinary_case(sampler: bool, keyboard: bool) {
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
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
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
        assert_eq!(measured_block(&mut data), [[0.25; 2]; 8]);
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
    drop(data);
    drop(errors);
    drop(control);
    finish(&mut retirement);
}
