//! Rendered G grouping dispatches one canonical MIDI clip edit.
use super::super::drag::group_drag_ui_test::{click, fixture, pump};
use gpui::{TestAppContext, VisualTestContext};
use hex_project::{BeatTime, Command};
use hex_session::service::Request;

#[gpui::test]
fn g_groups_selected_midi_clips_and_selects_one_created_clip(cx: &mut TestAppContext) {
    let (_files, handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddMidiTrack {
            name: "notes".into(),
        })));
    });
    pump(&editor, &mut visual);
    let track = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[1].id()
    });
    for (start, pitch) in [
        (BeatTime::ZERO, 60),
        (BeatTime::from_raw(3 << 32), 67),
        (BeatTime::from_raw(6 << 32), 72),
    ] {
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start,
                length: BeatTime::ONE,
            },)));
        });
        pump(&editor, &mut visual);
        let clip = editor.read_with(&visual, |view, _| {
            view.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .midi_clips()
                .last()
                .unwrap()
                .id()
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(1 << 31),
                pitch,
                channel: 0,
                velocity: 100,
            },)));
        });
        pump(&editor, &mut visual);
    }
    let clips = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .iter()
            .map(|clip| clip.id())
            .collect::<Vec<_>>()
    });
    click(&mut visual, clips[0], false);
    click(&mut visual, clips[1], true);
    let before = editor.read_with(&visual, |view, _| {
        assert_eq!(view.selected_clip_ids(), clips[..2]);
        view.state.as_ref().unwrap().project.snapshot()
    });
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.simulate_keystrokes("g");
    pump(&editor, &mut visual);
    let (grouped_id, after) = editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        let selected = view.selected_clip.expect("group result selected");
        let grouped = project.midi_clip(selected).expect("grouped midi clip");
        assert_eq!(project.track(track).unwrap().midi_clips().len(), 2);
        assert_eq!(view.selected_clip_ids(), vec![selected]);
        assert_eq!(grouped.start(), BeatTime::ZERO);
        assert_eq!(grouped.notes().len(), 2);
        assert_eq!(grouped.notes()[0].start(), BeatTime::ZERO);
        assert_eq!(grouped.notes()[1].start(), BeatTime::from_raw(3 << 32));
        (selected, project.snapshot())
    });
    let remaining = editor.read_with(&visual, |view, _| {
        view.state
            .as_ref()
            .unwrap()
            .project
            .track(track)
            .unwrap()
            .midi_clips()
            .iter()
            .find(|clip| clip.id() != grouped_id)
            .unwrap()
            .id()
    });
    click(&mut visual, remaining, true);
    editor.read_with(&visual, |view, _| {
        let selected = view.selected_clip_ids();
        assert_eq!(
            selected.len(),
            2,
            "selected {selected:?}, primary {:?}",
            view.selected_clip
        );
        assert!(selected.contains(&remaining));
        assert!(selected.contains(&grouped_id));
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let restored = view.state.as_ref().unwrap().project.snapshot();
        let mut expected = before;
        expected.next_id = restored.next_id;
        assert_eq!(restored, expected);
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
    });
}

#[gpui::test]
fn g_respects_text_midi_and_computer_keyboard_focus(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, clips[0], false);
    click(&mut visual, clips[1], true);

    visual.update(|window, cx| {
        editor.update(cx, |view, cx| {
            view.begin_clip_name(window, cx);
        });
    });
    visual.run_until_parked();
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    visual.simulate_keystrokes("g");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(view.worker_requests_pending, 0);
    });

    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            view.focus.focus(window);
            view.computer_keyboard.enabled = true;
        });
    });
    visual.simulate_keystrokes("g");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(!matches!(
            view.last_enqueued_edit,
            Some(Command::GroupClips(_))
        ));
    });

    visual.update(|window, cx| editor.update(cx, |view, _| view.midi_focus.focus(window)));
    visual.simulate_keystrokes("g");
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert!(!matches!(
            view.last_enqueued_edit,
            Some(Command::GroupClips(_))
        ));
    });
}

#[gpui::test]
fn g_refuses_to_enqueue_while_a_canonical_edit_is_pending(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, clips[0], false);
    click(&mut visual, clips[1], true);
    visual.update(|window, cx| {
        editor.update(cx, |view, _| {
            view.focus.focus(window);
            assert!(view.enqueue(Request::Edit(Command::RenameClip {
                clip: clips[0],
                name: "pending rename".into(),
            })));
            assert!(view.worker_requests_pending > 0);
        });
        window.dispatch_input(
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: gpui::Keystroke::parse("g").unwrap(),
                is_held: false,
            }),
            cx,
        );
        editor.read_with(cx, |view, _| {
            assert_eq!(view.worker_requests_pending, 1);
            assert_eq!(view.selected_clip_ids(), clips[..2]);
        });
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert_eq!(project.tracks()[0].clips().len(), 3);
        assert_eq!(project.clip(clips[0]).unwrap().name(), "pending rename");
    });
}

#[gpui::test]
fn g_does_not_group_arrangement_selection_under_another_focus_owner(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, clips[0], false);
    click(&mut visual, clips[1], true);
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    visual.update(|window, cx| {
        let control_focus = cx.focus_handle();
        control_focus.focus(window);
        editor.update(cx, |view, _| view.group_selected_clips(window));
    });
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(view.worker_requests_pending, 0);
    });
}
