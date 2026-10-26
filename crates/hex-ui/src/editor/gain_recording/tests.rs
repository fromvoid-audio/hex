//! Delivered takes remain owned after the real canonical worker refuses a stale commit.
use super::*;
use gpui::{TestAppContext, VisualTestContext};
use hex_session::service::{Event, Service};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

fn next(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match service.try_event() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < deadline => {
                std::thread::yield_now()
            }
            Err(error) => panic!("worker event: {error}"),
        }
    }
}

#[gpui::test]
fn delivered_gain_take_survives_finished_and_real_worker_source_refusal(cx: &mut TestAppContext) {
    delivered_source_race(cx, false);
}
#[gpui::test]
fn source_install_before_finished_poll_retains_already_delivered_gain_take(
    cx: &mut TestAppContext,
) {
    delivered_source_race(cx, true);
}
fn delivered_source_race(cx: &mut TestAppContext, install_source_first: bool) {
    let service = Service::new(48_000, 16, 0).unwrap();
    next(&service);
    service
        .submit(Request::Edit(hex_project::Command::AddTrack {
            name: "recorded gain".into(),
        }))
        .unwrap();
    let canonical = next(&service).state;
    let track = canonical.project.tracks()[0].id();
    let prepared = Arc::new(
        hex_engine::PreparedRevision::prepare_for_gain_recording(
            &canonical.project,
            &BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
            track,
        )
        .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut output, mut backend) = loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(pair) => break pair,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("headless output: {error}"),
        }
    };
    let control = output.publication.control().unwrap();
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 32_000_000,
                max_construction_peak_bytes: 32_000_000,
            },
        )
        .unwrap();
    let audio_revision = control
        .publish_with_admission_and_source_on_control(
            hex_engine::playback::Playback::try_from_prepared_with_session(prepared).unwrap(),
            hex_audio::prepared::PublicationPolicy::InheritRuntime,
            hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: canonical.revision,
            },
            |_| Ok::<_, ()>(()),
        )
        .unwrap();
    let mut client = output.take_client().unwrap();
    client.command(hex_engine::PlaybackCommand::Play).unwrap();
    let mut frames = [[0.0; 2]; 16];
    let mut host = [0.0; 32];
    backend.render_direct(&mut frames, &mut host).unwrap();
    let target = Target {
        track_id: track.value(),
        project_generation: 0,
        canonical_revision: canonical.revision,
        audio_revision,
        recording_epoch: 1,
        transport_epoch: client.status().unwrap().transport_epoch.load(Acquire),
        gesture_serial: 1,
        sample_rate: 48_000,
    };
    client.gain_begin(target, 0.5).unwrap();
    backend.render_direct(&mut frames, &mut host).unwrap();
    let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        canonical.project.as_ref().clone(),
        Arc::clone(&canonical.midi_preview),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        view.service = service;
        view.state = Some(Arc::clone(&canonical));
        view.worker_requests_pending = 0;
        view.pending_requests.clear();
        view.project_generation = 0;
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "headless gain take".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.pending_playback = None;
        view.playback_candidates.clear();
        view.gain_recorder.mode = Mode::Touching;
        view.gain_recorder.track = Some(track);
        view.gain_recorder.generation = 0;
        view.gain_recorder.revision = canonical.revision;
        view.gain_recorder.audio_revision = Some(audio_revision);
        view.gain_recorder.target = Some(target);
        assert!(view.tick_gain_recording());
        assert_eq!(view.gain_recorder.events.len(), 1);
        view.end_gain_touch();
    });
    backend.render_direct(&mut frames, &mut host).unwrap();
    editor.update(&mut visual, |view, _| {
        // Another producer changed the canonical worker before this delivered
        // take was submitted. Keep its event pending so the UI still owns the
        // actual source view from which recording began.
        view.service
            .submit(Request::Edit(hex_project::Command::Gain {
                track,
                gain: 0.75,
            }))
            .unwrap();
        let changed = next(&view.service);
        assert!(changed.error.is_none());
        if install_source_first {
            view.receive(changed);
        }
        assert!(view.tick_gain_recording());
        assert_eq!(
            view.gain_recorder.events.len(),
            1,
            "Finished must transfer custody, never erase delivered events before commit acceptance"
        );
        if !install_source_first {
            let refused = next(&view.service);
            assert!(
                refused
                    .error
                    .as_deref()
                    .is_some_and(|reason| reason.contains("project changed"))
            );
            view.receive(refused);
        }
        assert_eq!(
            view.gain_recorder.events.len(),
            1,
            "worker refusal must retain the sealed take for recovery"
        );
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .gain_lane()
                .spans()
                .is_empty()
        );
        let pending = view.worker_requests_pending;
        view.retry_gain_take();
        assert_eq!(
            view.worker_requests_pending, pending,
            "source changes must not restamp or resubmit a retained take"
        );
        view.gain_recorder.audio_lost();
        assert_eq!(view.gain_recorder.events.len(), 1);
        view.discard_gain_take();
        assert!(view.gain_recorder.events.is_empty());
    });
    backend.close();
    drop(output);
}

fn draft_window(
    cx: &mut TestAppContext,
) -> (VisualTestContext, gpui::Entity<EditorView>, Target, TrackId) {
    let service = Service::new(48_000, 16, 0).unwrap();
    next(&service);
    service
        .submit(Request::Edit(hex_project::Command::AddTrack {
            name: "sealed gain draft".into(),
        }))
        .unwrap();
    let canonical = next(&service).state;
    let track = canonical.project.tracks()[0].id();
    let target = Target {
        track_id: track.value(),
        project_generation: 0,
        canonical_revision: canonical.revision,
        audio_revision: 1,
        recording_epoch: 1,
        transport_epoch: 1,
        gesture_serial: 1,
        sample_rate: 48_000,
    };
    let (handle, editor) = super::super::clip_drag_ui_test::editor_window(
        cx,
        canonical.project.as_ref().clone(),
        Arc::clone(&canonical.midi_preview),
    );
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        view.service = service;
        view.state = Some(canonical);
        view.worker_requests_pending = 0;
        view.pending_requests.clear();
        view.pending_playback = None;
        view.project_generation = 0;
        view.gain_recorder.track = Some(track);
        view.gain_recorder.generation = 0;
        view.gain_recorder.revision = target.canonical_revision;
        view.gain_recorder.mode = Mode::Finishing;
        view.gain_recorder.events.push(AppliedGain {
            sequence: 1,
            performance_frame: 0,
            content_frame: 0,
            loop_pass: 0,
            gain: 0.5,
        });
    });
    (visual, editor, target, track)
}
fn end() -> GainTakeEnd {
    GainTakeEnd {
        performance_frame: 24_000,
        content_frame: 24_000,
        loop_pass: 0,
    }
}

#[gpui::test]
fn sealed_gain_take_waits_through_repaints_and_exact_install_then_one_undo(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, target, track) = draft_window(cx);
    let terminal = editor.update(&mut visual, |view, _| {
        view.seal_gain_take(target, end());
        assert!(view.gain_recorder.has_sealed_take());
        assert_eq!(view.worker_requests_pending, 1);
        let terminal = next(&view.service);
        assert!(terminal.error.is_none());
        let pending = view.worker_requests_pending;
        view.retry_gain_take();
        view.discard_gain_take();
        assert_eq!(view.worker_requests_pending, pending);
        assert_eq!(view.gain_recorder.events.len(), 1);
        terminal
    });
    for _ in 0..2 {
        visual.run_until_parked();
        editor.update(&mut visual, |view, cx| {
            assert_eq!(view.gain_recorder.events.len(), 1);
            assert!(view.gain_recorder.has_sealed_take());
            cx.notify();
        });
    }
    editor.update(&mut visual, |view, _| {
        view.receive(terminal);
        assert!(!view.gain_recorder.has_sealed_take());
        assert!(view.gain_recorder.events.is_empty());
        assert_eq!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .gain_lane()
                .spans()
                .len(),
            1
        );
        assert!(view.enqueue(Request::Undo));
        let undone = next(&view.service);
        assert!(undone.error.is_none());
        assert!(
            undone
                .state
                .project
                .track(track)
                .unwrap()
                .gain_lane()
                .spans()
                .is_empty()
        );
        view.receive(undone);
    });
}

#[gpui::test]
fn worker_refusal_keeps_same_source_take_retryable_and_explicit_discard_is_required(
    cx: &mut TestAppContext,
) {
    use hex_session::device_gesture::{GestureKey, GestureRequest, GestureTarget};
    let (mut visual, editor, target, track) = draft_window(cx);
    editor.update(&mut visual, |view, _| {
        let key = GestureKey {
            generation: 0,
            token: 1,
        };
        view.service
            .submit(Request::DeviceGesture(GestureRequest::Begin {
                key,
                target: GestureTarget::TrackGain { track },
            }))
            .unwrap();
        assert!(next(&view.service).gesture_ack.unwrap().accepted);
        view.seal_gain_take(target, end());
        let refused = next(&view.service);
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|reason| reason.contains("device edit"))
        );
        view.receive(refused);
        assert_eq!(view.gain_recorder.events.len(), 1);
        assert!(view.gain_recorder.blocks_edits());
        view.service
            .submit(Request::DeviceGesture(GestureRequest::Cancel { key }))
            .unwrap();
        let cancelled = next(&view.service);
        assert!(cancelled.gesture_ack.as_ref().unwrap().accepted);
        view.receive(cancelled);
        view.retry_gain_take();
        let accepted = next(&view.service);
        assert!(accepted.error.is_none());
        view.receive(accepted);
        assert!(view.gain_recorder.events.is_empty());
        assert!(view.enqueue(Request::Undo));
        let undone = next(&view.service);
        assert!(undone.error.is_none());
        assert!(
            undone
                .state
                .project
                .track(track)
                .unwrap()
                .gain_lane()
                .spans()
                .is_empty()
        );
        view.receive(undone);
    });
}

#[gpui::test]
fn failed_conversion_and_audio_source_loss_keep_clean_document_close_guarded(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, mut target, _) = draft_window(cx);
    editor.update(&mut visual, |view, _| {
        // A rejected lane epoch is a conversion refusal, before any submission.
        target.recording_epoch = 2;
        view.seal_gain_take(target, end());
        assert_eq!(view.worker_requests_pending, 0);
        assert_eq!(view.gain_recorder.events.len(), 1);
        view.gain_recorder.audio_lost();
        view.project_generation += 1;
        view.retry_gain_take();
        assert_eq!(view.worker_requests_pending, 0);
        assert_eq!(view.gain_recorder.events.len(), 1);
    });
    let state = editor.read_with(&visual, |view, _| Arc::clone(view.state.as_ref().unwrap()));
    let clean = hex_session::service::ViewState {
        canonical_audio_source: state.canonical_audio_source.clone(),
        comparison_audio: state.comparison_audio.clone(),
        project: Arc::clone(&state.project),
        waveforms: state.waveforms.clone(),
        source_overviews: state.source_overviews.clone(),
        audio_clip_waveforms: state.audio_clip_waveforms.clone(),
        midi_preview: Arc::clone(&state.midi_preview),
        gain_preview: state.gain_preview.clone(),
        timeline_frames: state.timeline_frames,
        can_undo: state.can_undo,
        can_redo: state.can_redo,
        revision: state.revision,
        unsaved: false,
        document_path: state.document_path.clone(),
    };
    editor.update(&mut visual, |view, _| view.state = Some(Arc::new(clean)));
    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            assert!(!view.request_window_close(window, cx));
            view.request_quit(window, cx);
            assert_eq!(view.gain_recorder.events.len(), 1);
            view.discard_gain_take();
            assert!(!view.gain_recorder.blocks_edits());
            assert!(view.gain_recorder.events.is_empty());
        });
    });
}

#[gpui::test]
fn local_queue_refusal_keeps_gain_take_until_explicit_retry(cx: &mut TestAppContext) {
    let (mut visual, editor, target, track) = draft_window(cx);
    editor.update(&mut visual, |view, _| {
        for _ in 0..super::super::UI_REQUEST_CAPACITY {
            view.pending_requests
                .push_back(Request::Edit(hex_project::Command::Gain {
                    track,
                    gain: 1.0,
                }));
        }
        view.seal_gain_take(target, end());
        assert_eq!(view.worker_requests_pending, 0);
        assert_eq!(view.gain_recorder.events.len(), 1);
        assert!(view.gain_recorder.has_sealed_take());
        // No queued take was admitted: clearing only this test's pressure lets
        // the explicit retry submit the unchanged captured take exactly once.
        view.pending_requests.clear();
        view.retry_gain_take();
        assert_eq!(view.worker_requests_pending, 1);
        let accepted = next(&view.service);
        assert!(accepted.error.is_none());
        view.receive(accepted);
        assert!(!view.gain_recorder.has_sealed_take());
        assert!(view.gain_recorder.events.is_empty());
    });
}

#[gpui::test]
fn queued_gain_take_ignores_earlier_terminal_before_its_own_install(cx: &mut TestAppContext) {
    let (mut visual, editor, target, track) = draft_window(cx);
    editor.update(&mut visual, |view, cx| {
        view.pending_requests
            .push_back(Request::Edit(hex_project::Command::Gain {
                track,
                gain: 1.0,
            }));
        view.seal_gain_take(target, end());
        assert_eq!(view.worker_requests_pending, 0);
        assert_eq!(view.pending_requests.len(), 2);
        view.retry_gain_take();
        view.discard_gain_take();
        assert_eq!(view.pending_requests.len(), 2);
        view.tick(cx);
        assert!(view.pending_requests.is_empty());
        assert_eq!(view.worker_requests_pending, 2);
        let earlier = next(&view.service);
        assert!(earlier.error.is_none());
        view.receive(earlier);
        assert_eq!(view.gain_recorder.events.len(), 1);
        assert!(view.gain_recorder.has_sealed_take());
        let terminal = next(&view.service);
        assert!(terminal.error.is_none());
        view.receive(terminal);
        assert!(view.gain_recorder.events.is_empty());
        assert!(!view.gain_recorder.has_sealed_take());
    });
}
