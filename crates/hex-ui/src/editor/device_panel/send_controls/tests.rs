use crate::editor::{
    EditorView,
    device_panel::SendControlKey,
    track_gain::tests::{actor_fixture_in_mode, pump},
};
use gpui::{Entity, Modifiers, MouseButton, TestAppContext, point, px};
use hex_project::{Command, ReturnId, TrackId};
use hex_session::{DocumentAudioMode, service::Request};

fn gain(view: &EditorView, track: TrackId, return_track: ReturnId) -> f32 {
    super::canonical_gain(
        view,
        SendControlKey {
            track,
            return_track,
        },
    )
    .unwrap()
}

fn add_return(editor: &Entity<EditorView>, cx: &mut TestAppContext) -> ReturnId {
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddReturnTrack {
            name: "bus".into(),
        })))
    });
    pump(editor, cx, |view| {
        view.worker_requests_pending == 0
            && !view
                .state
                .as_ref()
                .unwrap()
                .project
                .return_tracks()
                .is_empty()
    });
    editor.read_with(cx, |view, _| {
        view.state.as_ref().unwrap().project.return_tracks()[0].id()
    })
}

#[gpui::test]
fn verified_send_entry_and_owner_finalization_keep_single_edit_history(cx: &mut TestAppContext) {
    let (_browser, mut visual, editor, [track, other]) = actor_fixture_in_mode(
        cx,
        crate::editor::test_environment::Environment::new().unwrap(),
        DocumentAudioMode::VerifiedStreaming,
    );
    let return_track = add_return(&editor, cx);
    editor.update(cx, |view, cx| {
        view.send_menu_track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    let selector = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let initial_revision = editor.read_with(cx, |view, _| view.state.as_ref().unwrap().revision);
    for (text, expected, finalize) in [("0.5", 0.5, true), ("0.75", 0.75, false)] {
        let field = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_click(field, Modifiers::default());
        visual.simulate_keystrokes("cmd-a");
        visual.simulate_input(text);
        if finalize {
            editor.update(cx, |view, cx| {
                assert!(crate::editor::device_panel::finalize_controls(view, cx))
            });
        } else {
            visual.simulate_keystrokes("enter");
        }
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0 && gain(view, track, return_track) == expected
        });
        visual.run_until_parked();
    }
    editor.read_with(cx, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().revision, initial_revision + 2);
        assert!(view.device_gesture.snapshot().is_none());
        assert!(
            view.audible_playback.is_none(),
            "no resident preview fallback"
        );
        assert_eq!(
            view.document_audio_mode,
            DocumentAudioMode::VerifiedStreaming
        );
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.master_gain(), 1.0);
        assert_eq!(project.return_tracks()[0].gain(), 1.0);
        assert_eq!(project.track(track).unwrap().gain(), 0.5);
        assert_eq!(project.track(other).unwrap().gain(), 1.5);
        assert_eq!(gain(view, other, return_track), 0.0);
    });
    for (request, expected) in [
        (Request::Undo, 0.5),
        (Request::Undo, 0.0),
        (Request::Redo, 0.5),
        (Request::Redo, 0.75),
    ] {
        editor.update(cx, |view, _| assert!(view.enqueue(request)));
        pump(&editor, cx, |view| {
            view.worker_requests_pending == 0 && gain(view, track, return_track) == expected
        });
    }
}

#[gpui::test]
fn verified_send_refuses_held_preview_and_busy_entry_restoring_canonical(cx: &mut TestAppContext) {
    let (_browser, mut visual, editor, [track, other]) = actor_fixture_in_mode(
        cx,
        crate::editor::test_environment::Environment::new().unwrap(),
        DocumentAudioMode::VerifiedStreaming,
    );
    let return_track = add_return(&editor, cx);
    editor.update(cx, |view, cx| {
        view.send_menu_track = Some(track);
        cx.notify();
    });
    visual.run_until_parked();
    let rail_id = Box::leak(
        format!(
            "return-send-{}-{}-slider",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let rail = visual.debug_bounds(rail_id).unwrap();
    let start = point(rail.left() + px(10.0), rail.center().y);
    let end = start - point(px(0.0), px(18.0));
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(cx, |view, _| {
        assert!(
            view.error
                .as_deref()
                .is_some_and(|s| s.contains("resident"))
        );
        assert!(view.device_gesture.snapshot().is_none());
        assert!(view.audible_playback.is_none());
        assert_eq!(gain(view, track, return_track), 0.0);
    });
    let field_id = Box::leak(
        format!(
            "return-send-{}-{}-value",
            track.value(),
            return_track.value()
        )
        .into_boxed_str(),
    );
    let field = visual.debug_bounds(field_id).unwrap().center();
    visual.simulate_click(field, Modifiers::default());
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("0.75");
    editor.update(cx, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::Gain {
            track: other,
            gain: 0.2
        })))
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    editor.read_with(cx, |view, cx| {
        assert_eq!(gain(view, track, return_track), 0.0);
        assert_eq!(
            view.device_panel.send_controls[&SendControlKey {
                track,
                return_track
            }]
                .entity
                .read(cx)
                .value(),
            0.0
        );
        assert!(
            view.error
                .as_deref()
                .is_some_and(|s| s.contains("current edit") && !s.contains("resident"))
        );
    });
}
