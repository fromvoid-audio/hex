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
    let deadline = crate::test_wait::hang_deadline();
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
struct DeliveredFixture {
    visual: VisualTestContext,
    editor: gpui::Entity<EditorView>,
    track: TrackId,
    output: Option<hex_audio::WorkerPreparedOutput>,
    backend: hex_audio::worker_fixture::HeadlessBackend,
}
impl DeliveredFixture {
    fn quiesce_and_poll(&mut self) {
        // Release the actual host owner and both real callback carriers. The
        // existing lifecycle reaper, rather than this UI fixture, drains receipts.
        drop(self.output.take());
        self.backend.close();
        let deadline = crate::test_wait::hang_deadline();
        loop {
            let pending = self.editor.update(&mut self.visual, |view, _| {
                view.tick_gain_recording();
                view.gain_recorder.retired.is_some()
            });
            if !pending {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "retired receipt transfer timed out"
            );
            std::thread::yield_now();
        }
    }
}
fn open_gain_output() -> (
    hex_audio::WorkerPreparedOutput,
    hex_audio::worker_fixture::HeadlessBackend,
) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(pair) => return pair,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("headless output: {error}"),
        }
    }
}

#[test]
fn competing_gain_fixture_waits_for_real_output_retirement() {
    use std::sync::mpsc;
    let guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let (output, mut backend) = open_gain_output();
    assert!(matches!(
        hex_audio::worker_fixture::open(48_000),
        Err(error) if error == "audio output is open or still shutting down"
    ));

    let (started_tx, started_rx) = mpsc::channel();
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let (opened_tx, opened_rx) = mpsc::channel();
    let waiter = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let _guard = crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
        acquired_tx.send(()).unwrap();
        let (output, mut backend) = open_gain_output();
        opened_tx.send(()).unwrap();
        backend.close();
        drop(output);
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        acquired_rx.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    backend.close();
    drop(output);
    drop(guard);
    opened_rx.recv_timeout(Duration::from_secs(7)).unwrap();
    waiter.join().unwrap();
}

fn delivered_fixture(cx: &mut TestAppContext, release: bool) -> DeliveredFixture {
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
    let (mut output, mut backend) = open_gain_output();
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
        if release {
            view.end_gain_touch();
        }
    });
    if release {
        // Finished follows the complete 5 ms rejoin, rather than the first
        // callback after release. Deliver that terminal before testing custody.
        let release_frames = 48_000_usize.div_ceil(200);
        for _ in 0..release_frames.div_ceil(frames.len()) {
            backend.render_direct(&mut frames, &mut host).unwrap();
        }
    }
    DeliveredFixture {
        visual,
        editor,
        track,
        output: Some(output),
        backend,
    }
}

fn delivered_source_race(cx: &mut TestAppContext, install_source_first: bool) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let DeliveredFixture {
        mut visual,
        editor,
        track,
        output,
        mut backend,
    } = delivered_fixture(cx, true);
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
        view.audio.take().unwrap().control.close();
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
fn unexpected_gain_abort_remains_visible_and_keeps_its_custody_contract(cx: &mut TestAppContext) {
    use hex_audio::gain_recording::AbortReason;
    let (mut visual, editor, target, track) = draft_window(cx);
    editor.update(&mut visual, |view, _| {
        for (explicit_cancel, reason, expected) in [
            (false, AbortReason::Cancelled, "cancelled"),
            (true, AbortReason::OutputFailed, "the audio output failed"),
        ] {
            view.gain_recorder = Recorder::default();
            view.gain_recorder.track = Some(track);
            view.gain_recorder.target = Some(target);
            view.gain_recorder.mode = Mode::Finishing;
            view.gain_recorder.cancelled = explicit_cancel;
            view.error = None;
            view.gain_receipt(Receipt::Aborted { target, reason });
            assert!(
                view.error
                    .as_deref()
                    .is_some_and(|error| error.contains(expected))
            );
            if explicit_cancel {
                assert!(!view.gain_recorder.blocks_edits());
            } else {
                assert!(view.gain_recorder.blocks_edits());
                assert!(!view.gain_recorder.sealed.as_ref().unwrap().complete());
            }
        }
        view.gain_recorder = Recorder::default();
        view.gain_recorder.track = Some(track);
        view.gain_recorder.target = Some(target);
        view.gain_recorder.mode = Mode::Touching;
        view.gain_recorder.events = vec![
            AppliedGain {
                sequence: 1,
                performance_frame: 0,
                content_frame: 0,
                loop_pass: 0,
                gain: 0.5,
            };
            hex_audio::gain_recording::MAX_TAKE_EVENTS as usize
        ];
        view.error = None;
        view.gain_receipt(Receipt::Applied {
            target,
            sequence: 1023,
            performance_frame: 1,
            content_frame: 1,
            loop_pass: 0,
            gain: 0.5,
        });
        view.gain_receipt(Receipt::Aborted {
            target,
            reason: AbortReason::Cancelled,
        });
        assert!(
            view.error
                .as_deref()
                .is_some_and(|error| error.contains("event limit"))
        );
        assert!(!view.gain_recorder.blocks_edits());
    });
}

#[gpui::test]
fn sealed_gain_take_waits_through_repaints_and_exact_install_then_one_undo(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, target, track) = draft_window(cx);
    let terminal = editor.update(&mut visual, |view, _| {
        view.seal_gain_take(target, end());
        assert!(view.gain_recorder.has_retained_take());
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
            assert!(view.gain_recorder.has_retained_take());
            cx.notify();
        });
    }
    editor.update(&mut visual, |view, _| {
        view.receive(terminal);
        assert!(!view.gain_recorder.has_retained_take());
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
        assert!(view.gain_recorder.has_retained_take());
        // No queued take was admitted: clearing only this test's pressure lets
        // the explicit retry submit the unchanged captured take exactly once.
        view.pending_requests.clear();
        view.retry_gain_take();
        assert_eq!(view.worker_requests_pending, 1);
        let accepted = next(&view.service);
        assert!(accepted.error.is_none());
        view.receive(accepted);
        assert!(!view.gain_recorder.has_retained_take());
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
        assert!(view.gain_recorder.has_retained_take());
        let terminal = next(&view.service);
        assert!(terminal.error.is_none());
        view.receive(terminal);
        assert!(view.gain_recorder.events.is_empty());
        assert!(!view.gain_recorder.has_retained_take());
    });
}

#[gpui::test]
fn fault_disarms_gain_without_an_admitted_take(cx: &mut TestAppContext) {
    let _guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let mut project =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Track(track) = project
        .execute(hex_project::Command::AddTrack {
            name: "gain".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let original = project.project().clone();
    let (mut output, mut backend) = open_gain_output();
    let client = output.take_client().unwrap();
    let (handle, editor) =
        super::super::clip_drag_ui_test::editor_window(cx, original.clone(), Arc::default());
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        view.audio = Some(crate::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "headless".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
        view.gain_recorder.mode = Mode::Ready;
        view.gain_recorder.track = Some(track);
        view.gain_recorder.generation = view.project_generation;
        view.gain_recorder.revision = view.state.as_ref().unwrap().revision;
    });
    backend.report_device_error().unwrap();
    editor.update(&mut visual, |view, _| {
        assert!(view.tick_gain_recording());
        assert!(view.gain_recorder.mode == Mode::Idle);
        assert!(!view.gain_recorder.blocks_edits());
        assert_eq!(view.state.as_ref().unwrap().project.as_ref(), &original);
    });
    backend.close();
    drop(output);
}

#[gpui::test]
fn device_fault_keeps_gain_take_and_recovery_cursor(cx: &mut TestAppContext) {
    gain_fault_keeps_take_and_cursor(cx, false);
}

#[gpui::test]
fn render_fault_keeps_gain_take_and_recovery_cursor(cx: &mut TestAppContext) {
    gain_fault_keeps_take_and_cursor(cx, true);
}

fn gain_fault_keeps_take_and_cursor(cx: &mut TestAppContext, render_fault: bool) {
    let _guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let mut fixture = delivered_fixture(cx, false);
    let (before, cursor) = fixture.editor.read_with(&fixture.visual, |view, _| {
        let status = view.audio.as_ref().unwrap().control.status().unwrap();
        (
            view.state.as_ref().unwrap().project.snapshot(),
            status.recovery_position(),
        )
    });
    assert!(cursor > 0);
    if render_fault {
        fixture.backend.report_render_error().unwrap();
    } else {
        fixture.backend.report_device_error().unwrap();
    }
    fixture.editor.update(&mut fixture.visual, |view, _| {
        assert!(view.tick_gain_recording());
        assert_eq!(
            view.audio
                .as_ref()
                .unwrap()
                .control
                .status()
                .unwrap()
                .recovery_position(),
            cursor
        );
        assert_eq!(view.gain_recorder.events.len(), 1);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
    });
    let mut frames = [[1.0; 2]; 16];
    let mut host = [1.0; 32];
    fixture
        .backend
        .render_direct(&mut frames, &mut host)
        .unwrap();
    assert_eq!(host, [0.0; 32]);
    fixture.editor.update(&mut fixture.visual, |view, _| {
        view.tick_gain_recording();
        assert_eq!(
            view.audio
                .as_ref()
                .unwrap()
                .control
                .status()
                .unwrap()
                .recovery_position(),
            cursor
        );
        assert!(view.gain_recorder.has_retained_take());
        assert_eq!(view.gain_recorder.events.len(), 1);
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        view.discard_gain_take();
        assert!(!view.gain_recorder.blocks_edits());
    });
}

#[gpui::test]
fn finished_gain_receipts_survive_all_current_output_retirement_paths(cx: &mut TestAppContext) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    for retirement in ["recovery", "future generation", "project replacement"] {
        let mut fixture = delivered_fixture(cx, true);
        fixture.editor.update(&mut fixture.visual, |view, _| {
            // Finished is already in the real callback receipt queue, with no
            // intervening UI tick. Each production retirement entry must retain it.
            match retirement {
                "recovery" => view.recover_audio(),
                "future generation" => view.retire_audio_for_future_generation(),
                _ => {
                    view.service
                        .submit(Request::NewWithAudioMode {
                            mode: hex_session::DocumentAudioMode::Resident,
                            reservation: None,
                        })
                        .unwrap();
                    let replaced = next(&view.service);
                    assert!(replaced.project_replaced);
                    view.receive(replaced);
                }
            }
            assert!(view.audio.is_none());
            assert!(view.gain_recorder.retired.is_some());
            assert_eq!(view.gain_recorder.events.len(), 1);
            assert!(!view.gain_take_retry_available());
            assert!(view.gain_recorder.blocks_edits());
        });
        fixture.quiesce_and_poll();
        fixture.editor.update(&mut fixture.visual, |view, _| {
            assert_eq!(view.gain_recorder.events.len(), 1);
            assert!(view.gain_recorder.sealed.as_ref().unwrap().complete());
            if retirement == "project replacement" {
                assert!(!view.gain_take_retry_available());
                let pending = view.worker_requests_pending;
                view.retry_gain_take();
                assert_eq!(view.worker_requests_pending, pending);
                view.discard_gain_take();
            } else {
                // The canonical take is now admitted: explicit discard cannot
                // cancel it, even though the old audio endpoint is gone.
                view.discard_gain_take();
                assert_eq!(view.gain_recorder.events.len(), 1);
                while view.worker_requests_pending != 0 {
                    let event = next(&view.service);
                    view.receive(event);
                }
                assert_eq!(
                    view.state
                        .as_ref()
                        .unwrap()
                        .project
                        .track(fixture.track)
                        .unwrap()
                        .gain_lane()
                        .spans()
                        .len(),
                    1
                );
            }
            assert!(view.gain_recorder.events.is_empty());
            assert!(!view.gain_recorder.has_retained_take());
        });
    }
}

#[gpui::test]
fn redo_cancels_delivered_gain_take_and_preserves_project(cx: &mut TestAppContext) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let mut fixture = delivered_fixture(cx, false);
    let (before, mut next_target) = fixture.editor.update(&mut fixture.visual, |view, cx| {
        let before = view.state.as_ref().unwrap().project.snapshot();
        let target = view.gain_recorder.target.unwrap();
        view.history_action(Request::Redo, cx);
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.gain_recorder.blocks_edits());
        assert_eq!(view.gain_recorder.events.len(), 1);
        (before, target)
    });
    let mut frames = [[0.0; 2]; 16];
    let mut host = [0.0; 32];
    fixture
        .backend
        .render_direct(&mut frames, &mut host)
        .unwrap();
    fixture.editor.update(&mut fixture.visual, |view, _| {
        assert!(view.tick_gain_recording());
        assert!(!view.gain_recorder.blocks_edits());
        assert!(view.gain_recorder.events.is_empty());
        assert!(view.error.is_none(), "successful cancel: {:?}", view.error);
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.pending_requests.is_empty());
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);

        next_target.gesture_serial += 1;
        view.audio
            .as_mut()
            .unwrap()
            .control
            .gain_begin(next_target, 0.75)
            .unwrap();
    });
    fixture
        .backend
        .render_direct(&mut frames, &mut host)
        .unwrap();
    fixture.editor.update(&mut fixture.visual, |view, _| {
        let control = &mut view.audio.as_mut().unwrap().control;
        assert!(matches!(control.gain_receipt(), Some(Receipt::Applied { target, gain, .. }) if target == next_target && gain == 0.75));
        control.gain_cancel().unwrap();
    });
    fixture
        .backend
        .render_direct(&mut frames, &mut host)
        .unwrap();
    fixture.editor.update(&mut fixture.visual, |view, _| {
        let control = &mut view.audio.as_mut().unwrap().control;
        assert!(matches!(control.gain_receipt(), Some(Receipt::Aborted { target, .. }) if target == next_target));
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        view.audio.take().unwrap().control.close();
    });
    fixture.backend.close();
    drop(fixture.output.take());
}

#[gpui::test]
fn interrupted_gain_capture_retains_incomplete_events_without_inventing_end_clock(
    cx: &mut TestAppContext,
) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let mut fixture = delivered_fixture(cx, false);
    fixture.editor.update(&mut fixture.visual, |view, _| {
        view.retire_audio_preserving_gain_take();
        assert!(view.gain_recorder.retired.is_some());
        assert_eq!(view.gain_recorder.events.len(), 1);
    });
    fixture.quiesce_and_poll();
    fixture.editor.update(&mut fixture.visual, |view, _| {
        assert_eq!(view.gain_recorder.events.len(), 1);
        assert!(!view.gain_recorder.sealed.as_ref().unwrap().complete());
        assert!(view.gain_recorder.blocks_edits());
        let pending = view.worker_requests_pending;
        view.retry_gain_take();
        assert_eq!(view.worker_requests_pending, pending);
        assert!(!view.gain_take_retry_available());
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(fixture.track)
                .unwrap()
                .gain_lane()
                .spans()
                .is_empty()
        );
        view.discard_gain_take();
        assert!(!view.gain_recorder.blocks_edits());
        assert!(view.gain_recorder.events.is_empty());
    });
}

#[gpui::test]
fn explicit_discard_can_release_unresolved_capture_receipts_without_committing(
    cx: &mut TestAppContext,
) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let mut fixture = delivered_fixture(cx, true);
    fixture.editor.update(&mut fixture.visual, |view, _| {
        view.retire_audio_preserving_gain_take();
        assert!(view.gain_recorder.retired.is_some());
        assert_eq!(view.worker_requests_pending, 0);
        view.discard_gain_take();
        assert!(view.gain_recorder.retired.is_none());
        assert!(view.gain_recorder.events.is_empty());
    });
    fixture.quiesce_and_poll();
    fixture.editor.update(&mut fixture.visual, |view, _| {
        assert_eq!(view.worker_requests_pending, 0);
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(fixture.track)
                .unwrap()
                .gain_lane()
                .spans()
                .is_empty()
        );
    });
}

#[gpui::test]
fn os_quit_during_active_gain_recording_is_cancelled_with_the_error(cx: &mut TestAppContext) {
    let (mut visual, editor, _, _) = draft_window(cx);
    visual.update(|_, cx| super::super::close::install_quit_gate(cx));
    editor.update(&mut visual, |view, _| {
        view.gain_recorder.mode = Mode::Touching
    });
    assert!(visual.simulate_should_quit());
    visual.run_until_parked();
    assert!(!visual.has_pending_prompt());
    assert!(!visual.quit_request_pending());
    assert_eq!(visual.quit_replies(), [false]);
    editor.update(&mut visual, |view, _| {
        assert_eq!(
            view.error.as_deref(),
            Some("finish or cancel gain recording before quitting")
        );
        view.gain_recorder.mode = Mode::Idle;
    });
}

#[gpui::test]
fn arming_gain_recording_preserves_active_sampler_text(cx: &mut TestAppContext) {
    use crate::editor::device_automation::host_tests::{Slot, TestWave, window_with_gain};
    use gpui::{Modifiers, px, size};

    let source = TestWave::new();
    let (mut visual, editor, _, track, _) =
        window_with_gain(cx, "hex.utility", Slot::Track, false, Some(&source.0));
    let sampler = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id()
    });
    visual.simulate_resize(size(px(1600.0), px(800.0)));
    visual.run_until_parked();
    let selector = Box::leak(format!("sampler-attack-{}-value", sampler.value()).into_boxed_str());
    let at = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(at, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    editor.update(&mut visual, |view, cx| {
        assert!(view.sampler_parameter.numeric_draft_active());
        view.toggle_gain_recording(track, cx);
        assert_eq!(
            view.error.as_deref(),
            Some("finish local drafts before arming gain recording")
        );
        assert!(view.sampler_parameter.numeric_draft_active());
        assert_eq!(
            crate::editor::sampler_parameter::tests::sampler_text(view, cx),
            "invalid"
        );
        assert!(view.gain_recorder.mode == Mode::Idle);
        assert_eq!(view.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn arming_gain_recording_preserves_active_gain_text(cx: &mut TestAppContext) {
    use crate::editor::device_automation::host_tests::{Slot, window_with_gain};
    use gpui::Modifiers;

    let (mut visual, editor, _, track, _) =
        window_with_gain(cx, "hex.utility", Slot::Track, true, None);
    editor.update(&mut visual, |view, cx| {
        view.clip_view_open = false;
        view.gain_lane.track = Some(track);
        view.gain_lane.selected = Some(hex_project::BeatTime::from_raw(2));
        cx.notify();
    });
    visual.run_until_parked();
    let at = visual
        .debug_bounds("gain-point-value-value")
        .unwrap()
        .center();
    visual.simulate_click(at, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("invalid");
    editor.update(&mut visual, |view, cx| {
        assert!(view.gain_numeric_editing(cx));
        view.toggle_gain_recording(track, cx);
        assert_eq!(
            view.error.as_deref(),
            Some("finish local drafts before arming gain recording")
        );
        assert!(view.gain_numeric_editing(cx));
        assert!(view.gain_recorder.mode == Mode::Idle);
        assert_eq!(view.worker_requests_pending, 0);
    });
}
