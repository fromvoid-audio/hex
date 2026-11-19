//! Rendered G grouping dispatches one canonical MIDI clip edit.
use super::super::drag::group_drag_ui_test::{click, fixture, pump};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
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

#[gpui::test]
fn dragging_an_empty_lane_range_then_g_creates_a_blank_clip(cx: &mut TestAppContext) {
    let (_files, handle, editor, _) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let track = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0].id()
    });
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let viewport = editor.read_with(&visual, |view, _| {
        view.visible_viewport(view.state.as_ref().unwrap().timeline_frames)
    });
    let y = lane.origin.y + px(22.0);
    let start = point(lane.origin.x + px(1.0), y);
    let end = point(
        lane.origin.x + lane.size.width * viewport.fraction(24_000).unwrap(),
        y,
    );
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    let range = editor.read_with(&visual, |view, _| view.normalized_time_selection().unwrap());
    visual.simulate_keystrokes("g");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        let [clip] = project.track(track).unwrap().midi_clips() else {
            panic!("blank clip");
        };
        let tempo = project.tempo_map();
        assert_eq!(tempo.beat_to_frame(clip.start()).unwrap(), range.start());
        assert_eq!(
            tempo
                .beat_to_frame(BeatTime::from_raw(clip.start().raw() + clip.length().raw()))
                .unwrap(),
            range.end()
        );
        assert!(clip.notes().is_empty());
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let mut restored = view.state.as_ref().unwrap().project.snapshot();
        restored.next_id = before.next_id;
        assert_eq!(restored, before);
    });
}

#[gpui::test]
fn g_time_selection_cuts_boundary_clips_and_overrides_retained_clip_selection(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, clips[2], false);
    let before = editor.update(&mut visual, |v, _| {
        let track = v.state.as_ref().unwrap().project.tracks()[0].id();
        v.time_selection = Some(super::super::time_selection::TimeSelection {
            anchor_track: Some(track),
            end_track: Some(track),
            anchor_frame: 60_000,
            end_frame: 108_000,
        });
        v.state.as_ref().unwrap().project.clone()
    });
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    visual.simulate_keystrokes("g");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let p = &v.state.as_ref().unwrap().project;
        let group = p.clip(v.selected_clip.unwrap()).unwrap();
        assert_eq!((group.start(), group.duration()), (60_000, 48_000));
        assert_eq!(group.audio_events().events.len(), 2);
        assert_eq!(p.clip(clips[2]).unwrap(), before.clip(clips[2]).unwrap());
        assert_eq!(p.tracks()[0].clips().len(), 4);
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let mut restored = v.state.as_ref().unwrap().project.snapshot();
        restored.next_id = before.snapshot().next_id;
        assert_eq!(restored, before.snapshot());
    });
}

#[gpui::test]
fn g_time_selection_keeps_empty_margins_and_original_event_timing(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    click(&mut visual, clips[2], false);
    editor.update(&mut visual, |v, _| {
        let track = v.state.as_ref().unwrap().project.tracks()[0].id();
        v.time_selection = Some(super::super::time_selection::TimeSelection {
            anchor_track: Some(track),
            end_track: Some(track),
            anchor_frame: 36_000,
            end_frame: 132_000,
        });
    });
    visual.update(|window, cx| editor.read_with(cx, |v, _| v.focus.focus(window)));
    visual.simulate_keystrokes("g");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let p = &v.state.as_ref().unwrap().project;
        let group = p.clip(v.selected_clip.unwrap()).unwrap();
        assert_eq!((group.start(), group.duration()), (36_000, 96_000));
        assert_eq!(
            group
                .audio_events()
                .events
                .iter()
                .map(|event| (event.start, event.end))
                .collect::<Vec<_>>(),
            vec![(12_000, 36_000), (60_000, 84_000)]
        );
        assert_eq!(p.clip(clips[2]).unwrap().start(), 144_000);
        assert_eq!(p.tracks()[0].clips().len(), 2);
    });
}

#[gpui::test]
fn g_retains_a_repeated_audio_group_and_selects_its_one_result(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::GroupClips(clips[..2].to_vec()))));
    });
    pump(&editor, &mut visual);
    let group = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[0]
            .clips()
            .iter()
            .find(|c| c.has_retained_audio_roots())
            .unwrap()
            .id()
    });
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::ResizeAudioClipView {
            clip: group,
            operation: hex_project::AudioViewResizeOperation::RepeatOuterEnd {
                project_frame: 140_000
            },
        })));
    });
    pump(&editor, &mut visual);
    click(&mut visual, group, false);
    click(&mut visual, clips[2], true);
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    visual.update(|window, cx| editor.read_with(cx, |view, _| view.focus.focus(window)));
    visual.simulate_keystrokes("g");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(view.error.is_none(), "{:?}", view.error);
        let project = &view.state.as_ref().unwrap().project;
        let selected = view.selected_clip.unwrap();
        assert_eq!(project.tracks()[0].clips().len(), 1);
        assert_eq!(view.selected_clip_ids(), vec![selected]);
        assert!(
            project
                .clip(selected)
                .unwrap()
                .audio_events()
                .sources
                .iter()
                .any(|root| root.clip.has_retained_audio_roots())
        );
        assert!(!project.clip(selected).unwrap().looping());
        assert!(view.bounce_control.is_none());
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let restored = view.state.as_ref().unwrap().project.snapshot();
        let mut expected = before;
        expected.next_id = restored.next_id;
        assert_eq!(restored, expected);
        let mut selected = vec![group, clips[2]];
        selected.sort();
        assert_eq!(view.selected_clip_ids(), selected);
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let selected = view.selected_clip.unwrap();
        assert_eq!(view.selected_clip_ids(), vec![selected]);
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .clip(selected)
                .is_some()
        );
    });
}
