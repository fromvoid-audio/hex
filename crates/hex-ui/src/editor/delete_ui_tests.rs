//! Whole-selection deletion through rendered headers and actual Service history.
use super::*;
use crate::editor::drag::group_drag_ui_test::{click, fixture, pump};
use gpui::{Modifiers, TestAppContext, VisualTestContext};
use hex_project::{BeatTime, ClipEdit, Command};

#[gpui::test]
fn backspace_deletes_shift_selected_mixed_clips_with_either_primary_and_one_history_edge(
    cx: &mut TestAppContext,
) {
    delete_mixed_clips(cx, "backspace");
}

#[gpui::test]
fn x_deletes_shift_selected_mixed_clips_and_cmd_z_restores_them(cx: &mut TestAppContext) {
    delete_mixed_clips(cx, "x");
}

fn delete_mixed_clips(cx: &mut TestAppContext, key: &str) {
    for midi_primary in [false, true] {
        let (_files, handle, editor, audio) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddMidiTrack {
                name: "notes".into()
            })))
        });
        pump(&editor, &mut visual);
        let track = editor.read_with(&visual, |v, _| {
            v.state.as_ref().unwrap().project.tracks()[1].id()
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start: BeatTime::ONE,
                length: BeatTime::ONE
            })))
        });
        pump(&editor, &mut visual);
        let midi = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(track)
                .unwrap()
                .midi_clips()[0]
                .id()
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
                clip: midi,
                start: BeatTime::ZERO,
                duration: BeatTime::from_raw(1 << 31),
                pitch: 61,
                channel: 2,
                velocity: 91
            })))
        });
        pump(&editor, &mut visual);
        click(&mut visual, audio[0], false);
        let header = Box::leak(format!("clip-name-{}", audio[1].value()).into_boxed_str());
        let hit = visual.debug_bounds(header).unwrap();
        visual.simulate_click(
            hit.center(),
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        visual.run_until_parked();
        click(&mut visual, midi, true);
        if !midi_primary {
            // Toggle twice to retain membership while making an audio clip primary.
            click(&mut visual, audio[0], true);
            click(&mut visual, audio[0], true);
        }
        let mut expected_ids = vec![audio[0], audio[1], midi];
        expected_ids.sort();
        let (before, untouched) = editor.read_with(&visual, |v, _| {
            assert_eq!(v.selected_clip_ids(), expected_ids);
            assert_eq!(v.selected_midi_clip_data().is_some(), midi_primary);
            let p = &v.state.as_ref().unwrap().project;
            (p.snapshot(), p.clip(audio[2]).unwrap().clone())
        });
        visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
        visual.simulate_keystrokes(key);
        editor.read_with(&visual, |v, _| {
            assert!(
                matches!(
                    &v.last_enqueued_edit,
                    Some(Command::EditClips { clips, edit: ClipEdit::Delete })
                        if clips == &expected_ids
                ),
                "one atomic selection intent"
            );
        });
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            let p = &v.state.as_ref().unwrap().project;
            assert!(
                p.clip(audio[0]).is_none()
                    && p.clip(audio[1]).is_none()
                    && p.midi_clip(midi).is_none()
            );
            assert_eq!(p.tracks()[0].clips().len(), 1);
            assert_eq!(p.tracks()[0].clips()[0], untouched);
            assert_eq!(p.assets().count(), before.assets.len());
            assert_eq!(p.tracks().len(), before.tracks.len());
            assert!(
                v.selected_clip_ids()
                    .iter()
                    .all(|id| !expected_ids.contains(id)),
                "deleted members must not remain selected"
            );
            p.snapshot()
        });
        visual.simulate_keystrokes("cmd-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
        });
        visual.simulate_keystrokes("cmd-shift-z");
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
        });
    }
}
