//! New-track keyboard intent uses canonical insertion, history and saved order.
use super::{
    EditorView,
    midi_canvas::creation_tests::{fixture, pump},
};
use gpui::{Entity, TestAppContext, VisualTestContext};
use hex_project::{Command, Snapshot, TrackId};
use hex_session::service::Request;

fn snapshot(editor: &Entity<EditorView>, visual: &VisualTestContext) -> Snapshot {
    editor.read_with(visual, |v, _| v.state.as_ref().unwrap().project.snapshot())
}

fn history_matches(actual: Snapshot, expected: &Snapshot) {
    // Undo deliberately preserves the high-water mark of allocated identities.
    assert!(actual.next_id >= expected.next_id);
    let mut expected = expected.clone();
    expected.next_id = actual.next_id;
    assert_eq!(actual, expected);
}

fn setup(
    cx: &mut TestAppContext,
) -> (
    VisualTestContext,
    Entity<EditorView>,
    std::sync::Arc<super::test_environment::Environment>,
    Vec<TrackId>,
) {
    let (mut visual, editor, environment, clip) = fixture(cx);
    let first = editor.read_with(&visual, |v, _| v.selected_track.unwrap());
    for command in [
        Command::AddTrack {
            name: "middle".into(),
        },
        Command::AddTrack {
            name: "last".into(),
        },
        Command::AddReturnTrack {
            name: "return".into(),
        },
        Command::InsertInstrument {
            track: first,
            state: hex_project::default_instrument_state().unwrap(),
        },
        Command::AddMidiNote {
            clip,
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        },
    ] {
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(command)))
        });
        pump(&editor, &mut visual);
    }
    let ids = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .tracks()
            .iter()
            .map(|t| t.id())
            .collect()
    });
    (visual, editor, environment, ids)
}

fn focus(editor: &Entity<EditorView>, visual: &mut VisualTestContext, selected: Option<TrackId>) {
    visual.update(|window, cx| {
        editor.update(cx, |v, cx| {
            v.selected_track = selected;
            v.selected_clip = None;
            v.focused_return = None;
            v.device_panel.main_selected = None;
            v.focus.focus(window);
            cx.notify();
        })
    });
    visual.run_until_parked();
}

#[gpui::test]
fn keyboard_creation_inserts_after_first_middle_last_with_one_history_step_and_reopen(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, environment, ids) = setup(cx);
    for key in ["cmd-t", "cmd-shift-t"] {
        for index in 0..ids.len() {
            focus(&editor, &mut visual, Some(ids[index]));
            let before = snapshot(&editor, &visual);
            visual.simulate_keystrokes(key);
            pump(&editor, &mut visual);
            let after = snapshot(&editor, &visual);
            editor.read_with(&visual, |v, _| {
                assert!(v.error.is_none(), "{:?}", v.error);
                let created = v.selected_track.unwrap();
                assert!(!ids.contains(&created));
                let mut expected = ids.clone();
                expected.insert(index + 1, created);
                let actual: Vec<_> = v
                    .state
                    .as_ref()
                    .unwrap()
                    .project
                    .tracks()
                    .iter()
                    .map(|t| t.id())
                    .collect();
                assert_eq!(actual, expected, "{key}, selected row {index}");
                assert_eq!(v.selected_track_ids(), vec![created]);
            });
            editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
            pump(&editor, &mut visual);
            history_matches(snapshot(&editor, &visual), &before);
            editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Redo)));
            pump(&editor, &mut visual);
            assert_eq!(snapshot(&editor, &visual), after);
            if key == "cmd-t" && index == 1 {
                let path = environment.root().join("inserted-track.hex");
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::SaveNew(path.clone())))
                });
                pump(&editor, &mut visual);
                editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Open(path))));
                pump(&editor, &mut visual);
                assert_eq!(snapshot(&editor, &visual), after);
                // Reopening deliberately resets history; remove the created row
                // through the same canonical owner before the next case.
                let created = editor.read_with(&visual, |v, _| {
                    v.state.as_ref().unwrap().project.tracks()[index + 1].id()
                });
                editor.update(&mut visual, |v, _| {
                    assert!(v.enqueue(Request::Edit(Command::RemoveTrack(created))))
                });
            } else {
                editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
            }
            pump(&editor, &mut visual);
        }
    }
}

#[gpui::test]
fn no_ordinary_selection_and_return_or_main_focus_append_before_pinned_buses(
    cx: &mut TestAppContext,
) {
    let (mut visual, editor, _environment, ids) = setup(cx);
    for owner in ["none", "return", "main"] {
        focus(&editor, &mut visual, (owner == "return").then_some(ids[0]));
        editor.update(&mut visual, |v, _| {
            if owner == "return" {
                v.focused_return = Some(v.state.as_ref().unwrap().project.return_tracks()[0].id());
            } else if owner == "main" {
                v.device_panel.main_selected = Some(v.project_generation);
            }
        });
        let before = snapshot(&editor, &visual);
        visual.simulate_keystrokes("cmd-t");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let created = v.selected_track.unwrap();
            let mut expected = ids.clone();
            expected.push(created);
            assert_eq!(
                state
                    .project
                    .tracks()
                    .iter()
                    .map(|t| t.id())
                    .collect::<Vec<_>>(),
                expected,
                "{owner}"
            );
            assert_eq!(v.selected_track_ids(), vec![created]);
            assert!(v.focused_return.is_none());
            assert!(v.device_panel.main_selected.is_none());
        });
        editor.update(&mut visual, |v, _| assert!(v.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        history_matches(snapshot(&editor, &visual), &before);
    }
}
