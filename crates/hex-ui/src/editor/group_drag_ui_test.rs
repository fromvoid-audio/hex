//! Actual multi-selection/header drag → worker candidate → one history step.
use super::super::{EditorView, musical_time::SnapGrid};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{ClipId, Command, Created};
use hex_session::service::{Event, Request, Service};
use std::time::{Duration, Instant};

fn next(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let done = editor.update(&mut *visual, |v, cx| {
            if v.tick(cx) {
                cx.notify();
            }
            v.worker_requests_pending == 0 && v.pending_requests.is_empty()
        });
        if done {
            visual.run_until_parked();
            return;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn fixture(
    cx: &mut TestAppContext,
) -> (
    super::super::external_audio_drop_ui_test::BatchFiles,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    Vec<ClipId>,
) {
    cx.update(super::super::bind_editor_keys);
    let files = super::super::external_audio_drop_ui_test::BatchFiles::new();
    let wav = files.wav("group.wav", 48_000, 96_000);
    let directory = wav.parent().unwrap();
    std::fs::create_dir(directory.join("samples")).unwrap();
    let service = Service::new(48_000, 32, 8_000_000).unwrap();
    next(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let event = next(&service);
    let source = &event.state.project.tracks()[0];
    let track = source.id();
    let original = source.clips()[0].id();
    let asset = source.clips()[0].asset();
    service
        .submit(Request::Edit(Command::RemoveClip(original)))
        .unwrap();
    next(&service);
    let mut clips = Vec::new();
    for i in 0..3 {
        service
            .submit(Request::Edit(Command::PlaceClip {
                track,
                asset,
                start: 48_000 + i * 48_000,
                duration: 24_000,
                source_offset: i * 24_000,
                source_length: 24_000,
                looping: false,
            }))
            .unwrap();
        let event = next(&service);
        let Created::Clip(clip) = event.created.unwrap() else {
            panic!("placed clip");
        };
        clips.push(clip);
    }
    // Consume the current worker state without making a synthetic project view.
    service
        .submit(Request::Edit(Command::Gain { track, gain: 1.0 }))
        .unwrap();
    let event = next(&service);
    let handle = *cx.add_window(|_, _| gpui::Empty);
    let editor = cx
        .update_window(handle, |_, window, cx| {
            let editor = window.replace_root(cx, |window, cx| {
                EditorView::new_for_test_with_browser_directory(window, cx, service, directory)
            });
            editor.update(cx, |v, cx| {
                assert_eq!(v.browser.directory, directory.join("samples"));
                assert_eq!(v.browser.places, vec![directory.join("samples")]);
                v.state = Some(event.state);
                v.snap_grid = SnapGrid::default().with_platform_modifier(true);
                v.focus.focus(window);
                cx.notify();
            });
            editor
        })
        .unwrap();
    (files, handle, editor, clips)
}
fn click(visual: &mut VisualTestContext, clip: ClipId, additive: bool) {
    visual.run_until_parked();
    let selector = Box::leak(format!("clip-name-{}", clip.value()).into_boxed_str());
    let center = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_click(
        center,
        Modifiers {
            platform: additive,
            ..Modifiers::default()
        },
    );
    visual.run_until_parked();
}
#[gpui::test]
fn dragging_nonprimary_selected_header_moves_all_three_clips_with_gaps_and_one_undo(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (i, clip) in clips.iter().enumerate() {
        click(&mut visual, *clip, i != 0);
    }
    let before = editor.read_with(&visual, |v, _| {
        assert_eq!(v.selected_clip, clips.last().copied());
        assert_eq!(v.selected_clip_ids(), clips);
        v.state.as_ref().unwrap().project.snapshot()
    });
    let anchor = clips[1];
    let selector = Box::leak(format!("clip-drag-surface-{}", anchor.value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    let target = point(down.x + px(63.0), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.active_clip_drag.as_ref().unwrap().clip, anchor);
        assert_eq!(
            v.selected_clip_ids(),
            clips,
            "dragging a member must retain all selected IDs"
        );
    });
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let p = &v.state.as_ref().unwrap().project;
        let moved_anchor = p.clip(anchor).unwrap().start();
        let delta = i128::from(moved_anchor) - 96_000;
        assert!(delta > 0, "actual dragged member moved");
        for (i, id) in clips.iter().enumerate() {
            let c = p
                .clip(*id)
                .expect("every selected ID survives group movement");
            let expected = 48_000 + i as i128 * 48_000 + delta;
            eprintln!(
                "group anchor={} clip={} delta={delta} actual={} expected={expected}",
                anchor.value(),
                id.value(),
                c.start()
            );
            assert_eq!(
                i128::from(c.start()),
                expected,
                "all selected clips use the anchor delta"
            );
            assert_eq!(c.source_offset(), i as u64 * 24_000);
            assert_eq!(c.source_length(), 24_000);
        }
        assert_eq!(v.selected_clip_ids(), clips);
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

fn begin_group(visual: &mut VisualTestContext, clips: &[ClipId]) -> gpui::Point<gpui::Pixels> {
    for (i, clip) in clips.iter().enumerate() {
        click(visual, *clip, i != 0);
    }
    let selector = Box::leak(format!("clip-drag-surface-{}", clips[1].value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let target = point(down.x + px(63.0), down.y);
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    target
}
#[gpui::test]
fn group_escape_and_actual_gpui_capture_loss_never_commit_any_member(cx: &mut TestAppContext) {
    for escape in [true, false] {
        let (_files, handle, editor, clips) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let target = begin_group(&mut visual, &clips);
        for id in &clips {
            let selector = Box::leak(format!("group-clip-preview-{}", id.value()).into_boxed_str());
            assert!(
                visual.debug_bounds(selector).is_some(),
                "all selected previews are actually rendered"
            );
        }
        if escape {
            visual.simulate_keystrokes("escape");
        } else {
            // Exercise actual GPUI drag-payload retirement, not a fabricated
            // worker/canonical failure or claimed physical window deactivation.
            visual.update(|window, cx| {
                assert!(cx.stop_active_drag(window));
            });
        }
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.active_clip_drag.is_none());
            assert_eq!(v.selected_clip_ids(), clips);
            assert_eq!(v.worker_requests_pending, 0);
            assert!(v.last_enqueued_edit.is_none());
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
}
#[gpui::test]
fn group_pending_or_actual_revision_change_refuses_all_members_without_losing_independent_edit(
    cx: &mut TestAppContext,
) {
    for hold_ack in [true, false] {
        let (_files, handle, editor, clips) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let target = begin_group(&mut visual, &clips);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
        let event = editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::RenameClip {
                clip: clips[0],
                name: "independent rename".into()
            })));
            next(&v.service)
        });
        if hold_ack {
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            editor.update(&mut visual, |v, cx| {
                v.receive(event);
                cx.notify();
            });
        } else {
            editor.update(&mut visual, |v, cx| {
                v.receive(event);
                cx.notify();
            });
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        }
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            let project = &v.state.as_ref().unwrap().project;
            assert!(v.active_clip_drag.is_none());
            for id in &clips {
                let old = before.clip(*id).unwrap();
                let current = project.clip(*id).unwrap();
                assert_eq!(current.start(), old.start());
                assert_eq!(current.source_offset(), old.source_offset());
            }
            assert_eq!(project.clip(clips[0]).unwrap().name(), "independent rename");
            assert!(
                matches!(&v.last_enqueued_edit, Some(Command::RenameClip { .. })),
                "no group command after pending/stale refusal"
            );
        });
    }
}
#[gpui::test]
fn stationary_group_snap_modifier_uses_one_anchor_delta_and_authoritative_release(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, cx| {
        v.snap_grid = SnapGrid::default();
        cx.notify();
    });
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let target = begin_group(&mut visual, &clips);
    let positions = |v: &EditorView| {
        let group = v.active_clip_drag.as_ref().unwrap().group.borrow();
        let starts = clips
            .iter()
            .map(|id| {
                let item = group.previews.iter().find(|p| p.id() == *id).unwrap();
                let hex_project::ClipGroupContent::Audio(c) = &item.content else {
                    panic!("audio group")
                };
                c.start()
            })
            .collect::<Vec<_>>();
        for pair in starts.windows(2) {
            assert_eq!(
                pair[1] - pair[0],
                48_000,
                "one shared snap delta preserves off-grid gaps"
            );
        }
        starts
    };
    let snapped = editor.read_with(&visual, |v, _| positions(v));
    let command = Modifiers {
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_modifiers_change(command);
    let free = editor.read_with(&visual, |v, _| positions(v));
    assert_ne!(
        free, snapped,
        "stationary modifier actually changes snapping"
    );
    visual.simulate_modifiers_change(Modifiers::default());
    editor.read_with(&visual, |v, _| assert_eq!(positions(v), snapped));
    // Release's current modifier wins without a preceding changed event.
    visual.simulate_mouse_up(target, MouseButton::Left, command);
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        for (id, expected) in clips.iter().zip(free) {
            assert_eq!(
                v.state.as_ref().unwrap().project.clip(*id).unwrap().start(),
                expected
            );
        }
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before)
    });
}

#[gpui::test]
fn mixed_audio_midi_group_uses_real_track_offsets_or_refuses_whole_vertical_move(
    cx: &mut TestAppContext,
) {
    for fits in [true, false] {
        let (_files, handle, editor, clips) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let mut targets = Vec::new();
        for name in ["middle", "last"] {
            editor.update(&mut visual, |v, _| {
                assert!(v.enqueue(Request::Edit(Command::AddMidiTrack { name: name.into() })));
            });
            pump(&editor, &mut visual);
            targets.push(editor.read_with(&visual, |v, _| {
                v.state
                    .as_ref()
                    .unwrap()
                    .project
                    .tracks()
                    .last()
                    .unwrap()
                    .id()
            }));
        }
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
                track: targets[0],
                start: hex_project::BeatTime::from_quarters(6).unwrap(),
                length: hex_project::BeatTime::ONE
            })));
        });
        pump(&editor, &mut visual);
        let midi = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .track(targets[0])
                .unwrap()
                .midi_clips()[0]
                .id()
        });
        editor.update(&mut visual, |v, _| {
            assert!(v.enqueue(Request::Edit(Command::AddMidiNote {
                clip: midi,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::ONE,
                pitch: 69,
                channel: 0,
                velocity: 100
            })));
        });
        pump(&editor, &mut visual);
        let anchor = clips[1];
        click(&mut visual, anchor, false);
        click(&mut visual, midi, true);
        let before = editor.read_with(&visual, |v, _| {
            assert_eq!(v.selected_clip_ids(), vec![anchor, midi]);
            v.state.as_ref().unwrap().project.snapshot()
        });
        let selector = Box::leak(format!("clip-drag-surface-{}", anchor.value()).into_boxed_str());
        let down = visual.debug_bounds(selector).unwrap().center();
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        let destination = targets[usize::from(!fits)];
        let target = editor.read_with(&visual, |v, _| {
            let lane = v.drag_lanes[&destination];
            point(down.x + px(63.0), px(lane.top + lane.height / 2.0))
        });
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        let planned = editor.read_with(&visual, |v, _| {
            assert_eq!(v.selected_clip_ids(), vec![anchor, midi]);
            assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            let drag = v.active_clip_drag.as_ref().unwrap();
            let plan = drag.group.borrow().previews.clone();
            assert_eq!(plan.len(), if fits { 2 } else { 0 });
            plan
        });
        if fits {
            for id in [anchor, midi] {
                let selector =
                    Box::leak(format!("group-clip-preview-{}", id.value()).into_boxed_str());
                assert!(
                    visual.debug_bounds(selector).is_some(),
                    "both audio and MIDI previews actually paint"
                );
            }
        }
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        let after = editor.read_with(&visual, |v, _| {
            let project = &v.state.as_ref().unwrap().project;
            if !fits {
                assert_eq!(project.snapshot(), before);
                return project.snapshot();
            }
            assert!(v.error.is_none(), "{:?}", v.error);
            for item in &planned {
                let row = project.track(item.track).unwrap();
                match &item.content {
                    hex_project::ClipGroupContent::Audio(c) => assert!(row.clips().contains(c)),
                    hex_project::ClipGroupContent::Midi(c) => assert!(row.midi_clips().contains(c)),
                }
            }
            assert!(
                project
                    .track(targets[0])
                    .unwrap()
                    .clips()
                    .iter()
                    .any(|c| c.id() == anchor)
            );
            assert!(
                project
                    .track(targets[1])
                    .unwrap()
                    .midi_clips()
                    .iter()
                    .any(|c| c.id() == midi)
            );
            project.snapshot()
        });
        if fits {
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
}
