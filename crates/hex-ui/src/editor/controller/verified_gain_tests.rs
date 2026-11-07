//! Submission is not application, and a delayed preview view cannot be replaced
//! with canonical UI state. Events/views here come from the actual Session actor.
use super::*;
use gpui::{TestAppContext, VisualTestContext};
use hex_session::{
    DocumentAudioMode,
    device_gesture::{GestureRequest, GestureTarget, ParameterValue},
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
        Request::DeviceGesture(GestureRequest::Begin {
            key,
            target: GestureTarget::TrackGain { track },
        }),
    );
    let mut changed = request(
        &service,
        Request::DeviceGesture(GestureRequest::Change {
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
        view.document_audio_mode = DocumentAudioMode::VerifiedStreaming;
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "headless verified observation".into(),
            spectrum_error: None,
            mode: DocumentAudioMode::VerifiedStreaming,
        });
        view.pending_playback = None;
        view.playback_candidates.clear();
        view.applied_project_revision = None;
        assert_eq!(
            view.device_gesture
                .begin(
                    context.project_generation,
                    GestureTarget::TrackGain { track },
                    ParameterValue::Float(1.0)
                )
                .unwrap(),
            key
        );
        view.device_gesture.next_request().unwrap();
        assert!(
            view.device_gesture
                .acknowledge(began.gesture_ack.as_ref().unwrap())
        );
        assert_eq!(
            view.device_gesture
                .change(ParameterValue::Float(0.25))
                .unwrap(),
            1
        );
        view.device_gesture.next_request().unwrap();
        assert!(
            view.device_gesture
                .acknowledge(changed.gesture_ack.as_ref().unwrap())
        );
    });
    visual.run_until_parked();
    let admission = editor.read_with(&visual, |view, _| view.audio_admission);
    editor.update(&mut visual, |view, _| {
        view.retain_publication_observation(
            context,
            output_identity,
            crate::EditorPublicationState::VerifiedPreviewSubmitted {
                audio_revision: 7,
                identity,
            },
        );
        assert!(view.pending_publication_observation.is_some());
    });
    for _ in 0..3 {
        editor.update(&mut visual, |_, cx| cx.notify());
        visual.run_until_parked();
        editor.read_with(&visual, |view, _| {
            assert!(view.pending_publication_observation.is_some());
            assert!(view.playback_candidates.is_empty());
            assert!(view.audible_playback.is_none());
            assert!(Arc::ptr_eq(view.state.as_ref().unwrap(), &canonical));
            assert_eq!(view.audio_admission, admission);
        });
    }
    editor.update(&mut visual, |view, _| {
        view.resident_device_preview = Some(preview);
        assert!(view.reconcile_publication_observation());
        let candidate = view.playback_candidates.back().unwrap();
        assert_eq!(candidate.device_preview_identity, Some(identity));
        assert_eq!(candidate.state.project.track(track).unwrap().gain(), 0.25);
        assert!(candidate.state.canonical_audio_source.is_none());
        assert!(candidate.observation.is_none());
        assert!(view.applied_project_revision.is_none());
        assert!(view.audible_playback.is_none());
        assert_eq!(view.audio_admission, admission);
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
        assert!(view.pending_publication_observation.is_none());
        assert_eq!(view.playback_candidates.len(), 1);
        view.reset_output_publication_tracking(Some(output_identity + 1));
        assert!(view.playback_candidates.is_empty());
        assert!(view.pending_publication_observation.is_none());
        view.audio = None;
    });
}
