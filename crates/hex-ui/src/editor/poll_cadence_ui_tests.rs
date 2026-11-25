//! Real detached poll loop on the test clock: idle back-off and wake latency.
//!
//! Each wake case starts from a poll loop asleep in its 100 ms idle timer and
//! proves the producer's own call runs the next tick before any simulated time
//! passes, then that the 16 ms cadence continues.
use super::super::EditorView;
use super::super::midi_canvas::creation_tests::fixture;
use super::{ACTIVE_INTERVAL, IDLE_INTERVAL};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};
use hex_engine::playback::PlaybackCommand;
use hex_project::Command;
use hex_session::service::Request;
use std::time::Duration;

fn advance(visual: &mut VisualTestContext, total: Duration) {
    let step = Duration::from_millis(4);
    let mut elapsed = Duration::ZERO;
    while elapsed < total {
        visual.executor().advance_clock(step);
        visual.run_until_parked();
        elapsed += step;
    }
}

fn wakeups(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> u64 {
    editor.read_with(visual, |v, _| v.poll_cadence.wakeups())
}

fn wakeups_over(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    window: Duration,
) -> u64 {
    let before = wakeups(editor, visual);
    advance(visual, window);
    wakeups(editor, visual) - before
}

fn is_idle(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> bool {
    editor.read_with(visual, |v, _| v.poll_cadence.is_idle())
}

/// Settle into idle, then stop partway through an idle sleep.
fn sleep_idle(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    advance(visual, Duration::from_secs(2));
    assert!(is_idle(editor, visual), "a stopped, quiet editor idles");
    advance(visual, Duration::from_millis(40));
    let before = wakeups(editor, visual);
    visual.run_until_parked();
    assert_eq!(
        wakeups(editor, visual),
        before,
        "without a producer the sleeping idle timer does not tick"
    );
}

/// The producer's wake ran a tick at the current simulated instant, and the
/// next tick follows one active interval later.
fn assert_resumed_without_idle_wait(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    before: u64,
    what: &str,
) {
    visual.run_until_parked();
    assert!(
        wakeups(editor, visual) > before,
        "{what} must tick at once, not after the idle wait"
    );
    assert!(
        !is_idle(editor, visual),
        "{what} resumes the active cadence"
    );
    let next = wakeups_over(editor, visual, ACTIVE_INTERVAL);
    assert!(next >= 1, "{what}: the next tick follows within 16 ms");
}

#[gpui::test]
fn stopped_editor_idles_at_ten_wakeups_per_second(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    advance(&mut visual, Duration::from_secs(2));
    assert!(is_idle(&editor, &mut visual));
    let idle = wakeups_over(&editor, &mut visual, Duration::from_secs(10));
    // 10 s / 100 ms = 100; the former fixed 16 ms poll woke 625 times.
    let expected = (Duration::from_secs(10).as_millis() / IDLE_INTERVAL.as_millis()) as u64;
    assert!(
        idle.abs_diff(expected) <= 2,
        "idle poll must wake about {expected} times in 10 s, got {idle}"
    );
}

#[gpui::test]
fn pending_edit_wakes_a_sleeping_poll_and_is_acknowledged(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    sleep_idle(&editor, &mut visual);
    let before = wakeups(&editor, &mut visual);
    // The ordinary enqueue path, with no keyboard or pointer input.
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::RenameTrack {
            track: v.selected_track.unwrap(),
            name: "poll".into(),
        })));
    });
    assert_resumed_without_idle_wait(&editor, &mut visual, before, "a pending edit");
    let deadline = crate::test_wait::hang_deadline();
    while editor.read_with(&visual, |v, _| v.edit_pending()) {
        assert!(std::time::Instant::now() < deadline, "edit never settled");
        std::thread::yield_now();
        advance(&mut visual, ACTIVE_INTERVAL);
        assert!(
            !is_idle(&editor, &mut visual),
            "an unacknowledged edit never idles"
        );
    }
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        let track = state.project.track(v.selected_track.unwrap()).unwrap();
        assert_eq!(track.name(), "poll");
    });
    advance(&mut visual, Duration::from_secs(1));
    assert!(
        is_idle(&editor, &mut visual),
        "settled edits return to idle"
    );
}

#[gpui::test]
fn play_command_wakes_a_sleeping_poll_and_keeps_sixteen_ms(cx: &mut TestAppContext) {
    let _output_fixture_guard =
        crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    let (mut output, _backend) = open_output();
    let client = output.take_client().unwrap();
    editor.update(&mut visual, |v, _| {
        v.audio = Some(crate::editor_backend::EditorAudio {
            control: client,
            sample_rate: 48_000,
            label: "headless".into(),
            spectrum_error: None,
            mode: hex_session::DocumentAudioMode::Resident,
        });
    });
    sleep_idle(&editor, &mut visual);
    let before = wakeups(&editor, &mut visual);
    // The transport entry point, as a menu action reaches it.
    editor.update(&mut visual, |v, _| {
        v.queue_audio_command(PlaybackCommand::Play);
        assert!(v.error.is_none(), "{:?}", v.error);
        assert!(v.play_intent.desired_playing(false), "play was admitted");
    });
    assert_resumed_without_idle_wait(&editor, &mut visual, before, "play");
    let playing = wakeups_over(&editor, &mut visual, Duration::from_secs(1));
    assert!(
        playing >= 60,
        "requested play polls every 16 ms, got {playing}"
    );
    editor.update(&mut visual, |v, _| v.audio = None);
    drop(output);
}

#[gpui::test]
fn key_and_render_wake_a_sleeping_poll(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    sleep_idle(&editor, &mut visual);
    let before = wakeups(&editor, &mut visual);
    visual.simulate_keystrokes("escape");
    assert_resumed_without_idle_wait(&editor, &mut visual, before, "a key");

    // A producer that only notifies is caught by the render-time check.
    sleep_idle(&editor, &mut visual);
    let before = wakeups(&editor, &mut visual);
    editor.update(&mut visual, |v, cx| {
        let token = v.play_intent.reserve_token().unwrap();
        v.play_intent.admit(token, true);
        cx.notify();
    });
    assert_resumed_without_idle_wait(&editor, &mut visual, before, "a notified play intent");
}

#[gpui::test]
fn worker_response_wakes_active_poll_before_sixteen_ms_timer(cx: &mut TestAppContext) {
    // Construct EditorView through the production `new_with_backend` entrypoint so
    // this test proves production automatically attaches the worker wake callback.
    let service = hex_session::service::Service::new(48_000, 32, 0).unwrap();
    let (_audio_tx, audio_events) = std::sync::mpsc::channel();
    let backend = crate::EditorBackend {
        service,
        audio_events,
        source_reservation: hex_session::SourcePlanReservation {
            reader: hex_media::VerifiedWavReaderLimits::new(
                hex_media::WavReaderLimits::new(64 * 1024 * 1024, 8_000_000, 32, 4096),
                1024 * 1024,
            ),
            max_retained_sources: 1,
            max_path_bytes: 1024 * 1024,
            max_manifest_bytes: 1024 * 1024,
        },
    };
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            window.replace_root(cx, |window, cx| {
                EditorView::new_with_backend(window, cx, backend)
            })
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    // Wait for initial state to arrive via the worker wake hook without advancing the clock.
    let deadline = crate::test_wait::hang_deadline();
    while editor.read_with(&visual, |v, _| v.state.is_none()) {
        assert!(
            std::time::Instant::now() < deadline,
            "initial state did not arrive via production wake hook"
        );
        visual.run_until_parked();
        std::thread::yield_now();
    }
    // Stay in the active 16 ms cadence (not idle) and advance partway into a frame.
    assert!(!is_idle(&editor, &mut visual));
    visual.executor().advance_clock(Duration::from_millis(4));
    visual.run_until_parked();
    let before_wakeups = wakeups(&editor, &mut visual);

    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "woken-active".into(),
        })));
    });
    // Do NOT advance the simulated clock at all: the worker wake hook must
    // interrupt the active 16 ms timer and deliver the edit ACK immediately.
    let deadline = crate::test_wait::hang_deadline();
    while editor.read_with(&visual, |v, _| v.edit_pending()) {
        assert!(
            std::time::Instant::now() < deadline,
            "worker wake hook did not wake active poll before 16 ms timer"
        );
        visual.run_until_parked();
        std::thread::yield_now();
    }
    assert!(
        wakeups(&editor, &mut visual) > before_wakeups,
        "worker response must run a poll tick immediately"
    );
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        assert_eq!(state.project.tracks()[0].name(), "woken-active");
    });
}

#[gpui::test]
fn rapid_burst_edits_and_inline_device_gesture_settle_without_timer_quantization(
    cx: &mut TestAppContext,
) {
    use hex_session::device_gesture::{GestureTarget, ParameterValue};
    use hex_session::service::diagnostics::{DiagnosticObserver, DiagnosticStage};
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };

    let (mut visual, editor, _environment, _clip) = fixture(cx);
    let backpressure = Arc::new(AtomicU64::new(0));
    let bp_counter = Arc::clone(&backpressure);
    let track = editor.read_with(&visual, |v, _| {
        v.attach_service_wake();
        v.service
            .set_diagnostic_observer(Arc::new(DiagnosticObserver::new(move |event| {
                if event.stage == DiagnosticStage::OutcomeBackpressure {
                    bp_counter.fetch_add(1, Ordering::Relaxed);
                }
            })))
            .unwrap();
        v.selected_track.unwrap()
    });

    // 1. Rapid burst of 20 edits completes via worker wake hook without advancing the 16 ms timer
    // and without hitting channel backpressure.
    editor.update(&mut visual, |v, _| {
        for index in 0..20 {
            assert!(v.enqueue(Request::Edit(Command::RenameTrack {
                track,
                name: format!("burst-{index}"),
            })));
        }
    });
    let deadline = crate::test_wait::hang_deadline();
    while editor.read_with(&visual, |v, _| v.edit_pending()) {
        assert!(
            std::time::Instant::now() < deadline,
            "burst edits did not settle via worker wake hook"
        );
        visual.run_until_parked();
        std::thread::yield_now();
    }
    assert_eq!(
        backpressure.load(Ordering::Relaxed),
        0,
        "burst edits must not stall on EVENT_CAPACITY backpressure"
    );
    editor.read_with(&visual, |v, _| {
        assert_eq!(
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .name(),
            "burst-19"
        );
    });

    // 2. Quick volume gesture (Begin -> Change -> Commit) pumps Begin inline immediately
    // and settles the full round-trip via worker wake callbacks with 0 ms simulated clock advance.
    editor.update(&mut visual, |v, _| {
        v.begin_device_gesture(
            GestureTarget::TrackGain { track },
            ParameterValue::Float(1.0),
        )
        .unwrap();
        assert_eq!(
            v.worker_requests_pending, 1,
            "Begin must be pumped inline on pointer down instead of waiting for tick()"
        );
        v.change_device_gesture(ParameterValue::Float(0.5)).unwrap();
        v.finish_device_gesture(ParameterValue::Float(0.5)).unwrap();
    });
    let deadline = crate::test_wait::hang_deadline();
    while editor.read_with(&visual, |v, _| {
        v.device_gesture_pending() || v.edit_pending()
    }) {
        assert!(
            std::time::Instant::now() < deadline,
            "inline device gesture did not settle via worker wake hook"
        );
        visual.run_until_parked();
        std::thread::yield_now();
    }
    editor.read_with(&visual, |v, _| {
        let gain = v
            .state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .gain();
        assert!(
            (gain - 0.5).abs() < 1e-5,
            "expected committed track gain 0.5, got {gain}"
        );
    });
}

fn open_output() -> (
    hex_audio::WorkerPreparedOutput,
    hex_audio::worker_fixture::HeadlessBackend,
) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        match hex_audio::worker_fixture::open(48_000) {
            Ok(opened) => return opened,
            Err(error)
                if error == "audio output is open or still shutting down"
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{error}"),
        }
    }
}
