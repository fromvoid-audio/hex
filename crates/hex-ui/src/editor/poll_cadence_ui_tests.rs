//! Real detached poll loop on the test clock: idle back-off and fast resume.
use super::super::EditorView;
use super::super::midi_canvas::creation_tests::fixture;
use gpui::{Entity, TestAppContext, VisualTestContext};
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

fn wakeups_over(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    window: Duration,
) -> u64 {
    let before = editor.read_with(visual, |v, _| v.poll_cadence.wakeups());
    advance(visual, window);
    editor.read_with(visual, |v, _| v.poll_cadence.wakeups()) - before
}

fn is_idle(editor: &Entity<EditorView>, visual: &mut VisualTestContext) -> bool {
    editor.read_with(visual, |v, _| v.poll_cadence.is_idle())
}

#[gpui::test]
fn stopped_editor_backs_off_and_resumes_on_play_edit_and_input(cx: &mut TestAppContext) {
    let (mut visual, editor, _environment, _clip) = fixture(cx);
    advance(&mut visual, Duration::from_secs(2));
    assert!(
        is_idle(&editor, &mut visual),
        "a stopped, quiet editor idles"
    );
    let idle = wakeups_over(&editor, &mut visual, Duration::from_secs(1));
    assert!(
        (8..=11).contains(&idle),
        "idle poll must wake about 10 times per second, got {idle}"
    );

    // Requested playback keeps the 16 ms cadence for as long as it lasts.
    editor.update(&mut visual, |v, _| {
        let token = v.play_intent.reserve_token().unwrap();
        v.play_intent.admit(token, true);
    });
    advance(&mut visual, Duration::from_millis(120));
    assert!(
        !is_idle(&editor, &mut visual),
        "play resumes the active cadence"
    );
    let playing = wakeups_over(&editor, &mut visual, Duration::from_secs(1));
    assert!(playing >= 60, "playback polls every 16 ms, got {playing}");
    editor.update(&mut visual, |v, _| {
        let token = v.play_intent.reserve_token().unwrap();
        v.play_intent.admit(token, false);
    });
    advance(&mut visual, Duration::from_secs(1));
    assert!(is_idle(&editor, &mut visual), "stop returns to idle");

    // A pending edit resumes the active cadence and the ACK is still received.
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::RenameTrack {
            track: v.selected_track.unwrap(),
            name: "poll".into(),
        })));
    });
    advance(&mut visual, Duration::from_millis(120));
    assert!(
        !is_idle(&editor, &mut visual),
        "a pending edit resumes 16 ms"
    );
    let deadline = crate::test_wait::hang_deadline();
    while editor.read_with(&visual, |v, _| v.edit_pending()) {
        assert!(std::time::Instant::now() < deadline, "edit never settled");
        std::thread::yield_now();
        advance(&mut visual, Duration::from_millis(16));
    }
    editor.read_with(&visual, |v, _| {
        let state = v.state.as_ref().unwrap();
        let track = state.project.track(v.selected_track.unwrap()).unwrap();
        assert_eq!(track.name(), "poll");
    });
    advance(&mut visual, Duration::from_secs(1));
    assert!(is_idle(&editor, &mut visual));

    // Keyboard input leaves idle immediately, before the next idle wake.
    visual.simulate_keystrokes("escape");
    assert!(!is_idle(&editor, &mut visual), "a key wakes the poll");
    let woken = wakeups_over(&editor, &mut visual, Duration::from_millis(160));
    assert!(woken >= 8, "input resumes 16 ms polling, got {woken}");
}
