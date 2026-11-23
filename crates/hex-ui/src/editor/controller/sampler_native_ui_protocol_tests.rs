use super::*;
#[cfg(feature = "ui-test-support")]
#[gpui::test]
fn worker_preview_ack_waits_for_exact_submission_metadata(cx: &mut gpui::TestAppContext) {
    worker_preview_ack_case(cx, false);
    worker_preview_ack_case(cx, true);
}

#[cfg(feature = "ui-test-support")]
fn worker_preview_ack_case(cx: &mut gpui::TestAppContext, loaded_sampler: bool) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    use hex_audio::prepared::PublicationPolicy;
    use hex_engine::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    let service = hex_session::service::Service::with_native_reservation(
        48_000,
        8,
        8_000_000,
        hex_session::ResidentNativeReservation {
            storage: hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
            max_scan_frames: 100_000,
        },
    )
    .unwrap();
    let receive = || {
        let deadline = crate::test_wait::hang_deadline();
        loop {
            match service.try_event() {
                Ok(event) => break event,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "worker_preview_ack_case: hang guard expired"
                    );
                    std::thread::yield_now();
                }
                Err(error) => panic!("{error}"),
            }
        }
    };
    receive();
    let command = if loaded_sampler {
        hex_project::Command::AddMidiTrack {
            name: "ack sampler".into(),
        }
    } else {
        hex_project::Command::InsertTrackWithDevice {
            name: "ack fixture".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        }
    };
    service.submit(Request::Edit(command)).unwrap();
    let mut event = receive();
    let limits = hex_engine::AudioStorageLedgerReservation {
        data_rows: 512,
        handle_rows: 256,
        root_rows: 128,
        max_retained_bytes: 20_000_000,
        max_construction_peak_bytes: 20_000_000,
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut sources = ResidentDeviceAssets::default();
    let sample_path = loaded_sampler.then(|| {
        std::env::temp_dir().join(format!(
            "hex-preview-ack-{}-{}.wav",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ))
    });
    if let Some(path) = &sample_path {
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&52_u32.to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&48_000_u32.to_le_bytes());
        wav.extend_from_slice(&192_000_u32.to_le_bytes());
        wav.extend_from_slice(&4_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        for _ in 0..8 {
            wav.extend_from_slice(&8192_i16.to_le_bytes());
        }
        std::fs::write(path, wav).unwrap();
        let track = event.state.project.tracks()[0].id();
        service
            .submit(Request::LoadFirstSampler {
                request_id: 1,
                project_generation: 0,
                path: path.clone(),
                revision: event.state.revision,
                target: hex_session::service::SamplerLoadTarget::Track {
                    track,
                    slot: hex_session::service::SamplerLoadSlot::New(
                        hex_project::InstrumentLoadPlacement::First,
                    ),
                },
                cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            })
            .unwrap();
        event = receive();
        assert!(event.error.is_none(), "{:?}", event.error);
        let hex_project::Created::Asset(asset) = event.created.unwrap() else {
            panic!("asset")
        };
        sources
            .insert_on_control(
                (
                    asset,
                    hex_engine::OwnedAudioSource::try_resident(48_000, vec![[0.25; 2]; 4])
                        .ok()
                        .unwrap(),
                ),
                0,
                limits,
                4,
                &cancel,
                || Ok::<_, hex_engine::native_device_resources::ResourcePreparationError>(()),
            )
            .unwrap();
    }
    let resources = PreparedDeviceResources::prepare_on_control(
        &sources,
        &event.state.project,
        1_000_000,
        &cancel,
    )
    .unwrap();
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare_with_device_resources(
            &event.state.project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            100_000,
            &cancel,
        )
        .unwrap(),
    );
    let observation =
        Arc::new(hex_engine::PreparedPlaybackObservation::prepare_on_control(&prepared).unwrap());
    let canonical = event.state;
    let context = canonical.canonical_audio_source.as_ref().unwrap().context();
    let deadline = crate::test_wait::hang_deadline();
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(output) => break output,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{error}"),
        }
    };
    output
        .publication
        .control()
        .unwrap()
        .configure_device_resource_publication_on_control(limits)
        .unwrap();
    let client = output.take_client().unwrap();
    assert!(client.worker_owns_graph_publication());
    let publish = |output: &mut hex_audio::WorkerPreparedOutput| {
        let control = output.publication.control().unwrap();
        control.reclaim();
        control
            .publish_with_policy_and_context_generation(
                hex_engine::playback::Playback::try_from_prepared_with_device_resources(
                    Arc::clone(&prepared),
                    &resources,
                )
                .unwrap(),
                PublicationPolicy::InheritRuntime,
                context.project_generation,
            )
            .unwrap()
    };
    let preview_revision = publish(&mut output);
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    assert_eq!(
        backend.status().unwrap().applied_revision.load(Acquire),
        preview_revision
    );
    assert!(
        backend.status().unwrap().applied_audio_source().is_none(),
        "ordinary publication uses protected metadata, not eligible-constructor source ACKs"
    );
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        canonical.project.as_ref().clone(),
        Arc::clone(&canonical.midi_preview),
    );
    editor.update(cx, |view, cx| {
        view.service = service;
        view.state = Some(Arc::clone(&canonical));
        view.project_generation = context.project_generation;
        view.document_audio_mode = hex_session::DocumentAudioMode::Resident;
        view.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "private control".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.pending_playback = None;
        view.playback_candidates.clear();
        view.applied_project_revision = None;
        let key = view
            .device_gesture
            .begin(
                context.project_generation,
                hex_session::device_gesture::GestureTarget::TrackGain {
                    track: canonical.project.tracks()[0].id(),
                },
                hex_session::device_gesture::ParameterValue::Float(1.0),
            )
            .unwrap();
        view.device_gesture.next_request().unwrap();
        assert!(
            view.device_gesture
                .acknowledge(&hex_session::device_gesture::GestureAck {
                    preview_retained: false,
                    source_lost: false,
                    key,
                    phase: hex_session::device_gesture::GesturePhase::Begin,
                    sequence: 0,
                    accepted: true,
                    reason: None,
                })
        );
        let identity = hex_session::service::DevicePreviewIdentity {
            project_generation: context.project_generation,
            key,
            sequence: 0,
            canonical_revision: canonical.revision,
        };
        view.resident_device_preview = Some(hex_session::service::DevicePreview {
            identity,
            state: Arc::clone(&canonical),
        });
        let before = view.audio_admission;
        view.tick(cx);
        assert!(
            view.audible_playback.is_none(),
            "context-only preview ACK fabricated canonical audio"
        );
        assert!(view.applied_project_revision.is_none());
        assert_eq!(view.audio_admission, before);
        let mut metadata = hex_session::service::ResidentPublicationMetadata {
            source_origin: hex_session::service::ResidentSourceOrigin::DevicePreview,
            clip_preview: None,
            context,
            device_preview: Some(identity),
            tuning_preview: None,
            seek_request: None,
            stopped_restore: false,
            computer_keyboard_request: None,
            gain_recording_track: None,
            requires_preroll: false,
        };
        let output_identity = view
            .audio
            .as_ref()
            .unwrap()
            .control
            .output_identity()
            .unwrap();
        view.retain_publication_observation(
            context,
            output_identity,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: preview_revision,
                observation: Arc::clone(&observation),
                metadata,
            },
        );
        view.tick(cx);
        let audible = view.audible_playback.as_ref().unwrap();
        assert_eq!(audible.device_preview_identity, Some(identity));
        assert!(Arc::ptr_eq(
            audible.observation.as_ref().unwrap(),
            &observation
        ));
        // A subsequent canonical graph still waits for its own protected metadata.
        let canonical_revision = publish(&mut output);
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        view.tick(cx);
        assert_eq!(
            view.audible_playback.as_ref().unwrap().audio_revision,
            preview_revision
        );
        metadata.device_preview = None;
        view.retain_publication_observation(
            context,
            output_identity,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: canonical_revision,
                observation: Arc::clone(&observation),
                metadata,
            },
        );
        view.tick(cx);
        let audible = view.audible_playback.as_ref().unwrap();
        assert_eq!(audible.audio_revision, canonical_revision);
        assert!(audible.device_preview_identity.is_none());

        let clip_identity = hex_session::service::ClipMovePreviewIdentity {
            context,
            gesture: 13,
            sequence: 2,
            destination: hex_session::service::ClipMovePreviewDestination::Existing(
                canonical.project.tracks()[0].id(),
            ),
            position: hex_project::ClipGroupPosition::Audio(1_000),
            mode: hex_session::service::ArrangementMoveMode::Move,
        };
        let canonical_project = super::super::audio_admission::ProjectRevision {
            generation: context.project_generation,
            revision: canonical.revision,
        };
        view.audio_admission.canonical_revision(canonical_project);
        view.audio_admission
            .published(canonical_project, canonical_revision);
        view.audio_admission
            .applied(canonical_project, canonical_revision);
        view.expected_clip_preview = Some(clip_identity);
        view.clip_preview_may_apply = true;
        metadata.clip_preview = Some(clip_identity);
        let clip_revision = publish(&mut output);
        view.retain_publication_observation(
            context,
            output_identity.wrapping_add(1),
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: clip_revision,
                observation: Arc::clone(&observation),
                metadata,
            },
        );
        assert!(
            !view
                .playback_candidates
                .iter()
                .any(|candidate| candidate.audio_revision == clip_revision)
        );
        view.tick(cx);
        assert_eq!(
            view.audible_playback.as_ref().unwrap().audio_revision,
            canonical_revision
        );
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        view.tick(cx);
        assert_eq!(
            backend.status().unwrap().applied_revision.load(Acquire),
            clip_revision
        );
        assert!(view.audible_playback.is_none());
        assert!(view.applied_project_revision.is_none());
        view.retain_publication_observation(
            context,
            output_identity,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: clip_revision,
                observation: Arc::clone(&observation),
                metadata,
            },
        );
        view.tick(cx);
        assert!(view.audible_playback.is_none());
        assert_eq!(
            view.applied_clip_preview,
            Some((clip_revision, clip_identity))
        );
        assert!(view.applied_project_revision.is_none());
        assert_ne!(
            view.audio_admission.current().unwrap().state,
            super::super::audio_admission::AdmissionState::Applied
        );

        let stale_revision = publish(&mut output);
        let next_identity = hex_session::service::ClipMovePreviewIdentity {
            sequence: clip_identity.sequence + 1,
            position: hex_project::ClipGroupPosition::Midi(hex_project::BeatTime::ONE),
            mode: hex_session::service::ArrangementMoveMode::Copy,
            ..clip_identity
        };
        view.expected_clip_preview = Some(next_identity);
        assert_eq!(
            view.applied_clip_preview,
            Some((clip_revision, clip_identity))
        );
        for _ in 0..128 {
            backend.render(&mut [[0.0; 2]; 64]).unwrap();
            view.tick(cx);
            if backend.status().unwrap().applied_revision.load(Acquire) == stale_revision {
                break;
            }
        }
        assert_eq!(
            backend.status().unwrap().applied_revision.load(Acquire),
            stale_revision,
            "rejected: {:?}",
            backend.status().unwrap().rejected_revision()
        );
        assert!(view.audible_playback.is_none());
        assert!(view.applied_clip_preview.is_none());
        assert!(view.applied_project_revision.is_none());
        metadata.clip_preview = Some(next_identity);
        view.retain_publication_observation(
            context,
            output_identity,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: stale_revision,
                observation: Arc::clone(&observation),
                metadata,
            },
        );
        view.tick(cx);
        assert_eq!(
            view.applied_clip_preview,
            Some((stale_revision, next_identity))
        );
        metadata.clip_preview = Some(clip_identity);
        view.retain_publication_observation(
            context,
            output_identity,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: clip_revision,
                observation: Arc::clone(&observation),
                metadata,
            },
        );
        view.tick(cx);
        assert_eq!(
            view.applied_clip_preview,
            Some((stale_revision, next_identity))
        );
        view.expected_clip_preview = None;
        metadata.clip_preview = None;
        let restored_revision = publish(&mut output);
        view.retain_publication_observation(
            context,
            output_identity,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: restored_revision,
                observation: Arc::clone(&observation),
                metadata,
            },
        );
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        view.tick(cx);
        assert_eq!(
            view.audible_playback.as_ref().unwrap().audio_revision,
            restored_revision
        );
        assert!(view.applied_clip_preview.is_none());
        if loaded_sampler {
            view.audio = None;
            return;
        }

        // A real metadata-only comparison retains the old source context. A subsequent
        // runtime publication uses the current authored revision and must not wait forever.
        view.device_gesture.reset();
        view.resident_device_preview = None;
        let device = canonical.project.tracks()[0].instrument().unwrap();
        view.service
            .submit(Request::CompareDevice {
                owner: hex_session::service::preset::Owner {
                    project_generation: context.project_generation,
                    revision: canonical.revision,
                    device: device.id(),
                    class: hex_engine::NativeDeviceClass::Instrument,
                    type_id: device.state().type_id(),
                },
                expected: hex_project::DeviceVariant::A,
                action: hex_project::DeviceComparisonAction::Switch,
            })
            .unwrap();
        let deadline = crate::test_wait::hang_deadline();
        let event = loop {
            match view.service.try_event() {
                Ok(event) => break event,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "worker_preview_ack_case: hang guard expired"
                    );
                    std::thread::yield_now();
                }
                Err(error) => panic!("{error}"),
            }
        };
        assert!(event.error.is_none(), "{:?}", event.error);
        view.receive(event);
        let authored = Arc::clone(view.state.as_ref().unwrap());
        assert!(authored.comparison_audio.is_some());
        assert!(authored.revision > context.editor_revision);
        assert_eq!(
            authored.canonical_audio_source.as_ref().unwrap().context(),
            context
        );
        let runtime_context = hex_engine::AudioSourceContext {
            editor_revision: authored.revision,
            ..context
        };
        let runtime_prepared = Arc::new(
            hex_engine::PreparedRevision::prepare(
                &authored.project,
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let runtime_observation = Arc::new(
            hex_engine::PreparedPlaybackObservation::prepare_on_control(&runtime_prepared).unwrap(),
        );
        let runtime_revision = output
            .publication
            .control()
            .unwrap()
            .publish_with_policy_and_context_generation(
                hex_engine::playback::Playback::try_from_prepared_with_session(Arc::clone(
                    &runtime_prepared,
                ))
                .unwrap(),
                PublicationPolicy::InheritRuntime,
                context.project_generation,
            )
            .unwrap();
        metadata.context = runtime_context;
        view.retain_publication_observation(
            runtime_context,
            output_identity,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: runtime_revision,
                observation: Arc::clone(&runtime_observation),
                metadata,
            },
        );
        assert!(
            view.pending_publication_observation.is_none(),
            "current authored publication waited on retained source context"
        );
        assert_eq!(
            view.playback_candidates.back().unwrap().audio_revision,
            runtime_revision
        );
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        view.tick(cx);
        assert_eq!(
            view.audible_playback.as_ref().unwrap().audio_revision,
            runtime_revision
        );
        assert_eq!(view.applied_project_revision, Some(authored.revision));

        // Deliver a callback refusal before its protected worker metadata.
        let control = output.publication.control().unwrap();
        control.reclaim();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let rejected = control
            .publish_with_policy_and_context_generation(
                hex_engine::playback::Playback::try_from_prepared_with_session(runtime_prepared)
                    .unwrap(),
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                context.project_generation,
            )
            .unwrap();
        assert_eq!(
            control.cancel_fresh_runtime_publication(rejected),
            hex_audio::prepared::FreshRuntimeCancellationResult::Cancelled
        );
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
        view.tick(cx);
        assert_eq!(view.observed_rejected_revision, rejected);
        view.retain_publication_observation(
            runtime_context,
            output_identity,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: rejected,
                observation: runtime_observation,
                metadata,
            },
        );
        view.tick(cx);
        assert!(matches!(
            view.audio_admission.current().unwrap().state,
            super::super::audio_admission::AdmissionState::Rejected(_)
        ));
        assert!(view.playback_candidates.is_empty());
        assert_eq!(
            view.audible_playback.as_ref().unwrap().audio_revision,
            runtime_revision
        );

        view.audio = None;
    });
    backend.close();
    drop(output);
    if let Some(path) = sample_path {
        std::fs::remove_file(path).unwrap();
    }
}
#[cfg(feature = "ui-test-support")]
#[gpui::test]
fn resident_publication_correlates_preview_restart_and_stopped_restore(
    cx: &mut gpui::TestAppContext,
) {
    let service = hex_session::service::Service::new(16_000, 8, 0).unwrap();
    let deadline = crate::test_wait::hang_deadline();
    let event = loop {
        match service.try_event() {
            Ok(event) => break event,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "resident_publication_correlates_preview_restart_and_stopped_restore: hang guard expired"
                );
                std::thread::yield_now();
            }
            Err(error) => panic!("{error}"),
        }
    };
    let observation = Arc::new(
        hex_engine::PreparedPlaybackObservation::prepare_on_control(
            event
                .playback
                .as_ref()
                .unwrap()
                .prepared_revision()
                .unwrap(),
        )
        .unwrap(),
    );
    let canonical = event.state;
    let context = canonical.canonical_audio_source.as_ref().unwrap().context();
    let preview = Arc::new(ViewState {
        canonical_audio_source: None,
        comparison_audio: None,
        project: Arc::clone(&canonical.project),
        waveforms: Default::default(),
        source_overviews: Default::default(),
        audio_clip_waveforms: Default::default(),
        midi_preview: Arc::clone(&canonical.midi_preview),
        gain_preview: canonical.gain_preview.clone(),
        timeline_frames: 1234,
        can_undo: false,
        can_redo: false,
        revision: canonical.revision,
        unsaved: false,
        document_path: None,
    });
    let (_, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        canonical.project.as_ref().clone(),
        Arc::clone(&canonical.midi_preview),
    );
    editor.update(cx, |view, _| {
        view.state = Some(Arc::clone(&canonical));
        view.project_generation = context.project_generation;
        let identity = hex_session::service::DevicePreviewIdentity {
            project_generation: context.project_generation,
            key: hex_session::device_gesture::GestureKey {
                generation: context.project_generation,
                token: 1,
            },
            sequence: 2,
            canonical_revision: canonical.revision,
        };
        let metadata = hex_session::service::ResidentPublicationMetadata {
            source_origin: hex_session::service::ResidentSourceOrigin::DevicePreview,
            clip_preview: None,
            context,
            device_preview: Some(identity),
            tuning_preview: None,
            seek_request: None,
            stopped_restore: false,
            computer_keyboard_request: None,
            gain_recording_track: None,
            requires_preroll: true,
        };
        let before = view.audio_admission;
        view.apply_publication_observation(
            context,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: 7,
                observation: Arc::clone(&observation),
                metadata,
            },
        );
        assert!(view.playback_candidates.is_empty());
        assert_eq!(view.audio_admission, before);
        view.resident_device_preview = Some(hex_session::service::DevicePreview {
            identity,
            state: Arc::clone(&preview),
        });
        let mut stale = metadata;
        stale.device_preview.as_mut().unwrap().sequence -= 1;
        assert!(view.resident_publication_view(stale).is_none());
        view.apply_publication_observation(
            context,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: 7,
                observation: Arc::clone(&observation),
                metadata,
            },
        );
        let candidate = view.playback_candidates.back().unwrap();
        assert!(Arc::ptr_eq(&candidate.state, &preview));
        assert!(candidate.prepared_revision.is_none());
        assert!(Arc::ptr_eq(
            candidate.observation.as_ref().unwrap(),
            &observation,
        ));
        assert!(
            view.audible_playback.is_none(),
            "submission is not application"
        );
        assert_eq!(candidate.worker_requires_preroll, Some(true));
        assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &canonical));
        assert_eq!(view.audio_admission, before);
        view.applied_project_revision = Some(canonical.revision);
        view.observed_rejected_revision = 70;
        view.pending_publication_observation = Some(PendingEditorPublicationObservation {
            context,
            output_identity: 41,
            state: crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: 7,
                observation: Arc::clone(&observation),
                metadata,
            },
        });
        view.reset_output_publication_tracking(Some(42));
        assert!(view.playback_candidates.is_empty());
        assert!(view.audible_playback.is_none());
        assert!(view.applied_project_revision.is_none());
        assert_eq!(view.observed_rejected_revision, 0);
        assert!(view.pending_publication_observation.is_none());
        view.pending_publication_observation = Some(PendingEditorPublicationObservation {
            context,
            output_identity: 42,
            state: crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: 7,
                observation: Arc::clone(&observation),
                metadata,
            },
        });
        view.reset_output_publication_tracking(Some(42));
        assert!(view.pending_publication_observation.is_some());
        view.apply_publication_observation(
            context,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: 7,
                observation: Arc::clone(&observation),
                metadata,
            },
        );
        assert_eq!(view.playback_candidates.len(), 1);
        view.stopped_playback_restore =
            Some(super::super::audio_admission::StoppedPlaybackRestore::Preparing);
        let mut canonical_metadata = metadata;
        canonical_metadata.device_preview = None;
        view.apply_publication_observation(
            context,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: 8,
                observation: Arc::clone(&observation),
                metadata: canonical_metadata,
            },
        );
        assert_eq!(
            view.stopped_playback_restore,
            Some(super::super::audio_admission::StoppedPlaybackRestore::Preparing),
            "an ordinary canonical submission cannot complete stopped restoration"
        );
        canonical_metadata.stopped_restore = true;
        view.apply_publication_observation(
            context,
            crate::EditorPublicationState::ResidentSubmitted {
                audio_revision: 9,
                observation: Arc::clone(&observation),
                metadata: canonical_metadata,
            },
        );
        assert_eq!(
            view.stopped_playback_restore,
            Some(super::super::audio_admission::StoppedPlaybackRestore::Published(9)),
            "only the tagged submission binds restoration to its actual audio revision"
        );
    });
}

#[test]
fn native_sampler_applied_source_recovers_lost_submitted_metadata_without_legacy_promotion() {
    // UI protocol metadata only; actual native proof/PCM is covered by the
    // Service/Desktop eligible publication and callback integration probes.
    let service = hex_session::service::Service::new(16000, 8, 0).unwrap();
    let end = crate::test_wait::hang_deadline();
    let event = loop {
        match service.try_event() {
            Ok(e) => break e,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < end,
                    "native_sampler_applied_source_recovers_lost_submitted_metadata_without_legacy_promotion: hang guard expired"
                );
                std::thread::yield_now();
            }
            Err(e) => panic!("{e}"),
        }
    };
    let context = event
        .state
        .canonical_audio_source
        .as_ref()
        .unwrap()
        .context();
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(16000).unwrap(), 4).unwrap();
    let hex_project::Created::Track(track) = editor
        .execute(hex_project::Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(hex_project::Command::InsertInstrument {
            track,
            state: hex_lattice_contract::NativeDeviceState::new(
                hex_lattice_contract::DeviceTypeId::new("hex.instrument.sampler").unwrap(),
                1,
                vec![],
            )
            .unwrap(),
        })
        .unwrap();
    let state = ViewState {
        canonical_audio_source: event.state.canonical_audio_source.clone(),
        comparison_audio: None,
        project: Arc::new(editor.project().clone()),
        waveforms: Default::default(),
        source_overviews: Default::default(),
        audio_clip_waveforms: Default::default(),
        midi_preview: event.state.midi_preview.clone(),
        gain_preview: event.state.gain_preview.clone(),
        timeline_frames: 16000,
        can_undo: false,
        can_redo: false,
        revision: context.editor_revision,
        unsaved: false,
        document_path: None,
    };
    let mut admission = super::super::audio_admission::AudioAdmissionStatus::default();
    admission.canonical_revision(super::super::audio_admission::ProjectRevision {
        generation: context.project_generation,
        revision: state.revision,
    });
    let source = hex_audio::prepared::AppliedAudioSource {
        context,
        output_identity: 12,
        audio_revision: 3,
    };
    assert_eq!(
        reconcile_verified_source_binding(
            &mut admission,
            hex_session::DocumentAudioMode::Resident,
            Some(13),
            context.project_generation,
            Some(&state),
            Some(source)
        ),
        (false, None)
    );
    assert_eq!(
        reconcile_verified_source_binding(
            &mut admission,
            hex_session::DocumentAudioMode::Resident,
            Some(12),
            context.project_generation,
            Some(&state),
            Some(source)
        )
        .1,
        Some(source)
    );
    let stale = hex_audio::prepared::AppliedAudioSource {
        context: hex_engine::AudioSourceContext {
            editor_revision: context.editor_revision + 1,
            ..context
        },
        ..source
    };
    assert_eq!(
        reconcile_verified_source_binding(
            &mut admission,
            hex_session::DocumentAudioMode::Resident,
            Some(12),
            context.project_generation,
            Some(&state),
            Some(stale)
        )
        .1,
        None
    );
}
