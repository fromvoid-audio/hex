//! Submission is not application, and a delayed preview view cannot be replaced
//! with canonical UI state. Events/views here come from the actual Session actor.
use super::*;
use gpui::{TestAppContext, VisualTestContext};
use hex_session::{
    DocumentAudioMode,
    device_gesture::{GestureKey, GestureRequest, GestureTarget, ParameterValue},
    service::{Event, Service},
};
use std::time::Duration;
fn request(service: &Service, request: Request) -> Event {
    service.submit(request).unwrap();
    next(service)
}
fn next(service: &Service) -> Event {
    let until = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(
            std::time::Instant::now() < until,
            "next: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[gpui::test]
fn verified_preview_submission_waits_for_exact_view_without_canonical_promotion(
    cx: &mut TestAppContext,
) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let service = Service::new(48_000, 16, 0).unwrap();
    next(&service);
    request(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(hex_session::SourcePlanReservation {
                reader: hex_media::VerifiedWavReaderLimits::new(
                    hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
                    1024 * 1024,
                ),
                max_retained_sources: 1,
                max_path_bytes: 1024 * 1024,
                max_manifest_bytes: 1024 * 1024,
            }),
        },
    );
    let canonical = request(
        &service,
        Request::Edit(hex_project::Command::AddTrack {
            name: "gain".into(),
        }),
    )
    .state;
    let track = canonical.project.tracks()[0].id();
    let context = canonical.canonical_audio_source.as_ref().unwrap().context();
    let key = GestureKey {
        generation: context.project_generation,
        token: 1,
    };
    let began = request(
        &service,
        Request::device_gesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }),
    );
    let mut changed = request(
        &service,
        Request::device_gesture(GestureRequest::Change {
            key,
            sequence: 1,
            value: ParameterValue::Float(0.25),
        }),
    );
    let preview = changed.device_preview.take().unwrap();
    assert!(preview.state.canonical_audio_source.is_none());
    let identity = preview.identity;
    let until = crate::test_wait::hang_deadline();
    let (mut output, _backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(pair) => break pair,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < until =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{error}"),
        }
    };
    let client = output.take_client().unwrap();
    let output_identity = client.output_identity().unwrap();
    let (handle, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        canonical.project.as_ref().clone(),
        Arc::clone(&canonical.midi_preview),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        view.service = service;
        view.state = Some(Arc::clone(&canonical));
        view.project_generation = context.project_generation;
        view.audio_pub.document_audio_mode = DocumentAudioMode::VerifiedStreaming;
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "headless verified observation".into(),
            spectrum_error: None,
            mode: DocumentAudioMode::VerifiedStreaming,
        });
        view.audio_pub.pending_playback = None;
        view.audio_pub.playback_candidates.clear();
        view.audio_pub.applied_project_revision = None;
        assert_eq!(
            view.device_panel
                .gesture
                .begin(
                    context.project_generation,
                    GestureTarget::TrackGain { track },
                    ParameterValue::Float(1.0)
                )
                .unwrap(),
            key
        );
        view.device_panel.gesture.next_request().unwrap();
        assert!(
            view.device_panel
                .gesture
                .acknowledge(began.gesture_ack.as_ref().unwrap())
        );
        assert_eq!(
            view.device_panel
                .gesture
                .change(ParameterValue::Float(0.25))
                .unwrap(),
            1
        );
        view.device_panel.gesture.next_request().unwrap();
        assert!(
            view.device_panel
                .gesture
                .acknowledge(changed.gesture_ack.as_ref().unwrap())
        );
    });
    visual.run_until_parked();
    let admission = editor.read_with(&visual, |view, _| view.audio_pub.audio_admission);
    editor.update(&mut visual, |view, _| {
        view.retain_publication_observation(
            context,
            output_identity,
            crate::EditorPublicationState::VerifiedPreviewSubmitted {
                audio_revision: 7,
                identity,
            },
        );
        assert!(view.audio_pub.pending_publication_observation.is_some());
    });
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.audio_pub.pending_publication_observation.is_some());
            assert!(view.audio_pub.playback_candidates.is_empty());
            assert!(view.audio_pub.audible_playback.is_none());
            assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &canonical));
            assert_eq!(view.audio_pub.audio_admission, admission);
        });
    }
    editor.update(&mut visual, |view, _| {
        view.device_panel.resident_preview = Some(preview);
        assert!(view.reconcile_publication_observation());
        let candidate = view.audio_pub.playback_candidates.back().unwrap();
        assert_eq!(candidate.device_preview_identity, Some(identity));
        assert_eq!(candidate.state.project.track(track).unwrap().gain(), 0.25);
        assert!(candidate.state.canonical_audio_source.is_none());
        assert!(candidate.observation.is_none());
        assert!(view.audio_pub.applied_project_revision.is_none());
        assert!(view.audio_pub.audible_playback.is_none());
        assert_eq!(view.audio_pub.audio_admission, admission);
        let mut stale = identity;
        stale.sequence += 1;
        assert!(view.device_publication_view(context, stale).is_none());
        view.retain_publication_observation(
            context,
            output_identity + 1,
            crate::EditorPublicationState::VerifiedPreviewSubmitted {
                audio_revision: 8,
                identity: stale,
            },
        );
        assert!(view.audio_pub.pending_publication_observation.is_none());
        assert_eq!(view.audio_pub.playback_candidates.len(), 1);
        view.reset_output_publication_tracking(Some(output_identity + 1));
        assert!(view.audio_pub.playback_candidates.is_empty());
        assert!(view.audio_pub.pending_publication_observation.is_none());
        view.audio = None;
    });
}

#[gpui::test]
fn verified_streaming_held_clip_move_submits_tracks_callback_ack_and_handles_refusal(
    cx: &mut TestAppContext,
) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let service = Service::new(48_000, 16, 0).unwrap();
    next(&service);
    request(
        &service,
        Request::NewWithAudioMode {
            mode: DocumentAudioMode::VerifiedStreaming,
            reservation: Some(hex_session::SourcePlanReservation {
                reader: hex_media::VerifiedWavReaderLimits::new(
                    hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
                    1024 * 1024,
                ),
                max_retained_sources: 1,
                max_path_bytes: 1024 * 1024,
                max_manifest_bytes: 1024 * 1024,
            }),
        },
    );
    let added = request(
        &service,
        Request::Edit(hex_project::Command::AddTrack {
            name: "clip".into(),
        }),
    );
    let track = added.state.project.tracks()[0].id();
    let placed = request(
        &service,
        Request::Edit(hex_project::Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::ZERO,
            length: hex_project::BeatTime::from_quarters(2).unwrap(),
        }),
    );
    let canonical = placed.state;
    let Some(hex_project::Created::Clip(clip)) = placed.created else {
        panic!("midi clip");
    };
    let context = canonical.canonical_audio_source.as_ref().unwrap().context();
    let until = crate::test_wait::hang_deadline();
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(pair) => break pair,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < until =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{error}"),
        }
    };
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare(
            canonical.project.as_ref(),
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let graph = hex_engine::playback::Playback::try_from_prepared_with_session(prepared).unwrap();
    let client = output.take_client().unwrap();
    let output_identity = client.output_identity().unwrap();
    let (handle, editor) = crate::editor::clip_drag_ui_test::editor_window(
        cx,
        canonical.project.as_ref().clone(),
        Arc::clone(&canonical.midi_preview),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    let audio_revision = output
        .publication
        .control()
        .unwrap()
        .publish(graph)
        .unwrap();
    editor.update(&mut visual, |view, _| {
        view.service = service;
        view.state = Some(Arc::clone(&canonical));
        view.project_generation = context.project_generation;
        view.audio_pub.document_audio_mode = DocumentAudioMode::VerifiedStreaming;
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "headless verified clip move".into(),
            spectrum_error: None,
            mode: DocumentAudioMode::VerifiedStreaming,
        });
        view.audio
            .as_mut()
            .unwrap()
            .control
            .command(hex_engine::PlaybackCommand::Play)
            .unwrap();
    });
    backend.render(&mut [[0.0; 2]; 64]).unwrap();
    let prepared_preview = Arc::new(
        hex_engine::PreparedRevision::prepare(
            canonical.project.as_ref(),
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let preview_graph =
        hex_engine::playback::Playback::try_from_prepared_with_session(prepared_preview).unwrap();
    let preview_revision = output
        .publication
        .control()
        .unwrap()
        .publish(preview_graph)
        .unwrap();
    assert!(preview_revision > audio_revision);
    editor.update(&mut visual, |view, _| {
        view.audio_pub.pending_playback = None;
        view.audio_pub.playback_candidates.clear();
        view.audio_pub.applied_project_revision = None;
        let viewport = view.visible_viewport(canonical.timeline_frames);
        let moved_pos = crate::editor::drag::ClipPosition::Midi {
            start: hex_project::BeatTime::ONE,
            length: hex_project::BeatTime::from_quarters(2).unwrap(),
        };
        let drag = crate::editor::drag::ClipDrag {
            project_generation: context.project_generation,
            revision: canonical.revision,
            source_track: track,
            clip,
            group: Default::default(),
            position: crate::editor::drag::ClipPosition::Midi {
                start: hex_project::BeatTime::ZERO,
                length: hex_project::BeatTime::from_quarters(2).unwrap(),
            },
            grab_offset: std::rc::Rc::new(std::cell::Cell::new(Some(0))),
            snap_edge: std::rc::Rc::new(std::cell::Cell::new(None)),
            source_span: Default::default(),
            capture: std::rc::Rc::new(std::cell::Cell::new(None)),
            viewport_start: viewport.start,
            viewport_length: viewport.length,
            scroll: Default::default(),
            snap_grid: view.snap_grid,
            snap_modifier: std::rc::Rc::new(std::cell::Cell::new(false)),
            copy_modifier: std::rc::Rc::new(std::cell::Cell::new(false)),
            preview: std::rc::Rc::new(std::cell::Cell::new(Some(
                crate::editor::drag::DropPreview {
                    target_track: track,
                    start_frame: 24_000,
                    end_frame: 72_000,
                    position: moved_pos,
                },
            ))),
            destination: std::rc::Rc::new(std::cell::Cell::new(Some(
                crate::editor::drag::ClipDropDestination::ExistingTrack(track),
            ))),
            insertion_line_y: std::rc::Rc::new(std::cell::Cell::new(None)),
            ghost_label: "midi".into(),
            ghost_waveform: None,
            lane_hovered: std::rc::Rc::new(std::cell::Cell::new(true)),
            last_pointer: std::rc::Rc::new(std::cell::Cell::new(None)),
        };
        view.begin_clip_drag(&drag);
        assert!(view.drag_start_is_current(&drag));
        assert!(
            view.audio_status()
                .unwrap()
                .playing
                .load(std::sync::atomic::Ordering::Acquire)
        );
        view.sync_held_clip_move_audition();
        let identity = view
            .expected_clip_preview
            .expect("VerifiedStreaming held clip move must submit audition target");
        assert!(view.applied_clip_preview.is_none());
        view.apply_publication_observation(
            context,
            crate::EditorPublicationState::VerifiedClipMoveSubmitted {
                audio_revision: preview_revision,
                identity,
            },
        );
        assert_eq!(view.audio_pub.playback_candidates.len(), 1);
        assert_eq!(
            view.audio_pub
                .playback_candidates
                .back()
                .unwrap()
                .clip_preview_identity,
            Some(identity)
        );
        assert!(
            view.applied_clip_preview.is_none(),
            "submission must not claim callback application before render ACK"
        );
        assert_eq!(view.status, "publishing clip preview");
        backend.render(&mut [[0.0; 2]; 64]).unwrap();
    });
    editor.update(&mut visual, |view, cx| {
        let identity = view.expected_clip_preview.unwrap();
        view.tick(cx);
        assert_eq!(
            view.applied_clip_preview.map(|(_, actual)| actual),
            Some(identity)
        );
        assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &canonical));
        view.apply_publication_observation(
            context,
            crate::EditorPublicationState::VerifiedClipMoveRefused("busy".into()),
        );
        assert_eq!(view.status, "clip preview unavailable · busy");
        assert_eq!(
            view.applied_clip_preview.map(|(_, actual)| actual),
            Some(identity),
            "refusing a later move must not erase the callback-confirmed audible preview"
        );
        assert!(view.retire_active_clip_drag());
        assert!(view.expected_clip_preview.is_none());
        assert!(view.applied_clip_preview.is_none());
        assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &canonical));
        view.audio = None;
    });
    let _ = output_identity;
}
