//! Real detached poll loop on the test clock: idle back-off and wake latency.
//!
//! Each wake case starts from a poll loop asleep in its 100 ms idle timer and
//! proves the producer's own call runs the next tick before any simulated time
//! passes, then that the 16 ms cadence continues.
use super::super::EditorView;
use super::super::midi_canvas::creation_tests::fixture;
use super::{ACTIVE_INTERVAL, IDLE_INTERVAL};
use gpui::{Entity, TestAppContext, VisualTestContext};
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
