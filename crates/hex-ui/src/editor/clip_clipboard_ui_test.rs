//! Real rendered key/mouse routing through the worker's retained whole-clip payload.
use super::{EditorView, musical_time::SnapGrid};
use gpui::{AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point};
use hex_project::{Command, TrackId};
use hex_session::service::{Event, Request, Service};
use std::time::{Duration, Instant};

fn next(service: &Service) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(Instant::now() < deadline, "clipboard worker timeout");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        let done = editor.update(&mut *visual, |view, cx| {
            if view.tick(cx) {
                cx.notify();
            }
            view.worker_requests_pending == 0 && view.pending_requests.is_empty()
        });
        if done {
            visual.run_until_parked();
            return;
        }
        assert!(Instant::now() < deadline, "UI clipboard request timeout");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn click(visual: &mut VisualTestContext, selector: &'static str, modifiers: Modifiers) {
    let target = visual
        .debug_bounds(selector)
        .expect("rendered target")
        .center();
    visual.simulate_mouse_down(target, MouseButton::Left, modifiers);
    visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
    visual.run_until_parked();
}
fn destination(
    editor: &Entity<EditorView>,
    visual: &mut VisualTestContext,
    track: TrackId,
    fraction: f32,
) -> u64 {
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let target = point(
        lane.left() + lane.size.width * fraction,
        lane.top() + lane.size.height * 0.8,
    );
    visual.simulate_mouse_down(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&*visual, |view, _| {
        assert!(
            view.selected_clip.is_none(),
            "destination no longer needs source selection"
        );
        assert!(view.normalized_time_selection().is_none());
        let cursor = view.edit_cursor.expect("actual clicked edit cursor");
        assert_eq!(cursor.track, track);
        cursor.frame
    })
}
fn copied_whole_clips_survive_destination_click_repeated_paste_undo_and_text_focus(
    cx: &mut TestAppContext,
    multiple: bool,
) {
    cx.update(super::bind_editor_keys);
    let files = super::external_audio_drop_ui_test::BatchFiles::new();
    let path = files.wav("whole.wav", 48_000, 48_000);
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(path.clone())).unwrap();
    let imported = next(&service);
    let source = imported.state.project.tracks()[0].id();
    let first = imported.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(Command::CopyClipTo {
            clip: first,
            track: source,
            start: 96_000,
        }))
        .unwrap();
    let second_event = next(&service);
    let second = second_event
        .state
        .project
        .track(source)
        .unwrap()
        .clips()
        .iter()
        .find(|c| c.start() == 96_000)
        .unwrap()
        .id();
    service
        .submit(Request::Edit(Command::AddTrack {
            name: "destination".into(),
        }))
        .unwrap();
    let state = next(&service).state;
    let target = state.project.tracks()[1].id();
    let originals = state.project.track(source).unwrap().clips().to_vec();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline UI test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(state);
                view.snap_grid = SnapGrid::Off;
                // An old unrelated range must not turn a header copy into a crop.
                view.time_selection = Some(super::time_selection::TimeSelection {
                    anchor_track: source,
                    end_track: source,
                    anchor_frame: 12_000,
                    end_frame: 24_000,
                });
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    click(
        &mut visual,
        Box::leak(format!("clip-name-{}", first.value()).into_boxed_str()),
        Modifiers::default(),
    );
    if multiple {
        click(
            &mut visual,
            Box::leak(format!("clip-name-{}", second.value()).into_boxed_str()),
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
    }
    let copied_count = if multiple { 2 } else { 1 };
    visual.simulate_keystrokes("cmd-c");
    pump(&editor, &mut visual);
    let before = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.snapshot()
    });
    let start = destination(&editor, &mut visual, target, 0.55);
    visual.simulate_keystrokes("cmd-v");
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let clips = project.track(target).unwrap().clips();
        assert_eq!(clips.len(), copied_count);
        for (index, old) in originals.iter().take(copied_count).enumerate() {
            let copy = clips
                .iter()
                .find(|c| c.start() == start + index as u64 * 96_000)
                .expect("whole clip at relative offset");
            assert_ne!(copy.id(), old.id());
            assert_eq!(copy.duration(), old.duration());
            assert_eq!(copy.asset(), old.asset());
            assert_eq!(copy.source_offset(), old.source_offset());
            assert_eq!(copy.source_length(), old.source_length());
        }
        project.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let mut expected = before.clone();
        expected.next_id = after.next_id;
        assert_eq!(
            view.state.as_ref().unwrap().project.snapshot(),
            expected,
            "one atomic Undo"
        );
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
    });
    // Use a second track so repeated paste cannot overwrite the first result.
    editor.update(&mut visual, |view, _| {
        view.enqueue(Request::Edit(Command::AddTrack {
            name: "repeat".into(),
        }));
    });
    pump(&editor, &mut visual);
    let repeat = editor.read_with(&visual, |view, _| {
        view.state.as_ref().unwrap().project.tracks()[2].id()
    });
    let repeated_start = destination(&editor, &mut visual, repeat, 0.35);
    visual.simulate_keystrokes("cmd-v");
    pump(&editor, &mut visual);
    let saved = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let clips = project.track(repeat).unwrap().clips();
        assert_eq!(clips.len(), copied_count);
        assert!(
            clips
                .iter()
                .any(|c| c.start() == repeated_start && c.duration() == 48_000)
        );
        project.snapshot()
    });
    visual.update(|window, cx| {
        editor.read_with(cx, |view, cx| view.browser.search.read(cx).focus(window))
    });
    visual.simulate_keystrokes("cmd-a");
    visual.simulate_input("kept text");
    visual.simulate_keystrokes("cmd-c cmd-v");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, cx| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), saved);
        assert!(view.browser.search.read(cx).text().contains("kept text"));
    });
    // Text copy must not replace the independent Arrangement clipboard.
    let text_destination = destination(&editor, &mut visual, target, 0.10);
    visual.simulate_keystrokes("cmd-v");
    pump(&editor, &mut visual);
    let persisted = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        for index in 0..copied_count {
            assert!(
                project
                    .track(target)
                    .unwrap()
                    .clips()
                    .iter()
                    .any(|c| c.start() == text_destination + index as u64 * 96_000
                        && c.duration() == 48_000)
            );
        }
        project.snapshot()
    });
    let saved_path = path.with_file_name(if multiple {
        "multiple.hex"
    } else {
        "single.hex"
    });
    editor.update(&mut visual, |view, _| {
        view.enqueue(Request::SaveNew(saved_path.clone()));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |view, _| {
        view.enqueue(Request::Open(saved_path));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), persisted)
    });
}

#[gpui::test]
fn copied_single_clip_survives_destination_paste_history_and_text_focus(cx: &mut TestAppContext) {
    copied_whole_clips_survive_destination_click_repeated_paste_undo_and_text_focus(cx, false);
}

#[gpui::test]
fn copied_clip_group_survives_destination_paste_history_and_text_focus(cx: &mut TestAppContext) {
    copied_whole_clips_survive_destination_click_repeated_paste_undo_and_text_focus(cx, true);
}

#[gpui::test]
fn pasted_clip_group_is_selected_and_unrequested_batch_births_are_not(cx: &mut TestAppContext) {
    use std::collections::BTreeSet;
    cx.update(super::bind_editor_keys);
    let files = super::external_audio_drop_ui_test::BatchFiles::new();
    let path = files.wav("group.wav", 48_000, 48_000);
    let recovery = path
        .parent()
        .unwrap()
        .join("project/recovery/untitled.hex-journal");
    let service = Service::with_recovery(48_000, 32, 8_000_000, recovery).unwrap();
    next(&service);
    service.submit(Request::Import(path)).unwrap();
    let imported = next(&service);
    let source = imported.state.project.tracks()[0].id();
    let first = imported.state.project.tracks()[0].clips()[0].id();
    service
        .submit(Request::Edit(Command::CopyClipTo {
            clip: first,
            track: source,
            start: 96_000,
        }))
        .unwrap();
    let second = next(&service)
        .state
        .project
        .track(source)
        .unwrap()
        .clips()
        .iter()
        .find(|c| c.start() == 96_000)
        .unwrap()
        .id();
    for name in ["destination", "other"] {
        service
            .submit(Request::Edit(Command::AddTrack { name: name.into() }))
            .unwrap();
        if name == "destination" {
            next(&service);
        }
    }
    let state = next(&service).state;
    let target = state.project.tracks()[1].id();
    let other = state.project.tracks()[2].id();
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new(window, cx, service, Err("offline UI test".into()))
            });
            editor.update(cx, |view, cx| {
                view.state = Some(state);
                view.snap_grid = SnapGrid::Off;
                cx.notify();
            });
            editor
        })
        .unwrap();
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    click(
        &mut visual,
        Box::leak(format!("clip-name-{}", first.value()).into_boxed_str()),
        Modifiers::default(),
    );
    click(
        &mut visual,
        Box::leak(format!("clip-name-{}", second.value()).into_boxed_str()),
        shift,
    );
    visual.simulate_keystrokes("cmd-c");
    pump(&editor, &mut visual);
    destination(&editor, &mut visual, target, 0.55);
    visual.simulate_keystrokes("cmd-v");
    pump(&editor, &mut visual);
    let pasted = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let pasted: BTreeSet<_> = project
            .track(target)
            .unwrap()
            .clips()
            .iter()
            .map(|c| c.id())
            .collect();
        assert_eq!(pasted.len(), 2);
        assert_eq!(
            view.selected_clip_ids()
                .into_iter()
                .collect::<BTreeSet<_>>(),
            pasted,
            "paste selects exactly the pasted clips"
        );
        assert!(view.selected_clip.is_some_and(|c| pasted.contains(&c)));
        pasted
    });
    let coherent = |view: &EditorView| {
        let project = &view.state.as_ref().unwrap().project;
        view.selected_clip_ids().iter().all(|id| {
            project
                .tracks()
                .iter()
                .any(|t| t.clips().iter().any(|c| c.id() == *id))
        })
    };
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert!(project.track(target).unwrap().clips().is_empty());
        assert!(coherent(view), "undo leaves no removed clip selected");
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert!(coherent(view), "redo keeps the selection on live clips");
    });
    // An unrelated bounce must not replace the user's selection.
    let anchor = *pasted.iter().next().unwrap();
    click(
        &mut visual,
        Box::leak(format!("clip-name-{}", anchor.value()).into_boxed_str()),
        Modifiers::default(),
    );
    let before = editor.read_with(&visual, |view, _| {
        (view.selected_clip, view.selected_clip_ids())
    });
    editor.update(&mut visual, |view, _| {
        let revision = view.state.as_ref().unwrap().revision;
        assert!(view.enqueue(Request::BounceArrangement {
            project_generation: view.project_generation,
            revision,
            target: hex_project::BounceTarget::Clips(pasted.iter().copied().collect()),
            in_place: false,
            adoption: hex_project::BounceAdoption::Drop(hex_project::BounceDrop {
                anchor,
                track: other,
                position: hex_project::ClipGroupPosition::Audio(0),
            }),
            control: hex_session::service::ExportControl::default(),
        }));
    });
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        assert!(!project.track(other).unwrap().clips().is_empty());
        assert_eq!(
            (view.selected_clip, view.selected_clip_ids()),
            before,
            "an unrelated bounce leaves the selection alone"
        );
    });
}
