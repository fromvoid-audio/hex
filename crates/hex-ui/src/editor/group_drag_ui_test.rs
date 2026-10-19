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
        let retained_payload =
            editor.read_with(&visual, |v, _| v.active_clip_drag.as_ref().unwrap().clone());
        assert!(retained_payload.group.borrow().capture.is_some());
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
        assert!(
            retained_payload.group.borrow().capture.is_none(),
            "retired Rc payload cannot keep capture metadata"
        );
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
                let item = group.previews().iter().find(|p| p.id() == *id).unwrap();
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
            let plan = drag.group.borrow().previews().to_vec();
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

#[gpui::test]
fn group_outside_lane_clears_all_previews_and_restores_originals(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    let target = begin_group(&mut visual, &clips);
    visual.run_until_parked();
    for id in &clips {
        let selector = Box::leak(format!("group-clip-preview-{}", id.value()).into_boxed_str());
        assert!(
            visual.debug_bounds(selector).is_some(),
            "valid landing preview actually painted"
        );
    }
    // A real pointer move left of every painted timeline lane, as the ordinary
    // single-clip drag fixture does; no fabricated planner error.
    let outside = point(px(200.0), target.y);
    visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| {
        let drag = v.active_clip_drag.as_ref().unwrap();
        assert!(drag.preview.get().is_none());
        assert!(drag.destination.get().is_none());
        assert!(
            drag.group.borrow().previews().is_empty(),
            "outside lane must invalidate all group destinations"
        );
    });
    for id in &clips {
        let preview = Box::leak(format!("group-clip-preview-{}", id.value()).into_boxed_str());
        let original = Box::leak(format!("clip-name-{}", id.value()).into_boxed_str());
        assert!(
            visual.debug_bounds(preview).is_none(),
            "stale group overlay hidden"
        );
        assert!(
            visual.debug_bounds(original).is_some(),
            "original reappears outside lane"
        );
    }
    visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(v.worker_requests_pending, 0);
        assert!(
            v.last_enqueued_edit.is_none(),
            "outside release cannot commit old landing"
        );
    });
}

#[gpui::test]
fn group_vertical_autoscroll_invalidates_all_previews_before_repaint(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for index in 0..24 {
        editor.update(&mut visual, |v, _cx| {
            v.enqueue(Request::Edit(Command::AddTrack {
                name: format!("scroll {index}"),
            }))
        });
        pump(&editor, &mut visual);
    }
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
    begin_group(&mut visual, &clips);
    visual.run_until_parked();
    let target = editor.read_with(&visual, |v, _| {
        let body = v.arrangement_bounds.unwrap();
        point(
            px(body.left + body.width * 0.6),
            px(body.top + body.height - 5.0),
        )
    });
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.update(&mut visual, |v, cx| {
        assert_eq!(
            v.active_clip_drag
                .as_ref()
                .unwrap()
                .group
                .borrow()
                .previews()
                .len(),
            3
        );
        let offset = v.track_scroll.offset().y;
        let generation = v.drag_layout_generation;
        let now = Instant::now();
        v.tick_clip_gesture_autoscroll(now, cx);
        if v.track_scroll.offset().y == offset {
            assert!(v.tick_clip_gesture_autoscroll(now + Duration::from_millis(50), cx));
        }
        assert!(
            v.track_scroll.offset().y < offset,
            "actual UniformList scroll occurred"
        );
        assert!(v.drag_layout_generation > generation);
        assert_ne!(v.arrangement_bounds_epoch, v.drag_layout_generation);
        let drag = v.active_clip_drag.as_ref().unwrap();
        assert!(drag.preview.get().is_none());
        assert!(drag.destination.get().is_none());
        assert!(
            drag.group.borrow().previews().is_empty(),
            "vertical autoscroll invalidates every group destination before repaint"
        );
    });
    // No repaint may turn the stale row into authority for this release.
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
        assert_eq!(v.worker_requests_pending, 0);
    });
}

fn with_project_view(
    state: &hex_session::service::ViewState,
    project: std::sync::Arc<hex_project::Project>,
) -> std::sync::Arc<hex_session::service::ViewState> {
    std::sync::Arc::new(hex_session::service::ViewState {
        canonical_audio_source: state.canonical_audio_source.clone(),
        project,
        waveforms: state.waveforms.clone(),
        audio_clip_waveforms: state.audio_clip_waveforms.clone(),
        midi_preview: state.midi_preview.clone(),
        gain_preview: state.gain_preview.clone(),
        timeline_frames: state.timeline_frames,
        can_undo: state.can_undo,
        can_redo: state.can_redo,
        revision: state.revision,
        unsaved: state.unsaved,
        document_path: state.document_path.clone(),
    })
}
#[gpui::test]
fn group_capture_checks_source_allocation_and_selection_epoch(cx: &mut TestAppContext) {
    for case in 0..3 {
        let replace_project = case == 1;
        let invalid = case != 0;
        let (_files, handle, editor, clips) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.snapshot());
        let target = begin_group(&mut visual, &clips);
        visual.run_until_parked();
        let payload =
            editor.read_with(&visual, |v, _| v.active_clip_drag.as_ref().unwrap().clone());
        editor.update(&mut visual, |v, cx| {
            let old = v.state.as_ref().unwrap();
            let project = if replace_project {
                std::sync::Arc::new((*old.project).clone())
            } else {
                old.project.clone()
            };
            v.state = Some(with_project_view(old, project));
            if case == 2 {
                let track = v.state.as_ref().unwrap().project.tracks()[0].id();
                v.clip_selection.only(track, clips[0]);
            }
            cx.notify();
        });
        visual.simulate_mouse_move(
            point(target.x + px(5.0), target.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        visual.run_until_parked();
        editor.read_with(&visual, |v, _| {
            assert_eq!(
                v.state.as_ref().unwrap().project.snapshot(),
                before,
                "model/revision content stayed equal"
            );
            assert_eq!(
                payload.group.borrow().previews().len(),
                if invalid { 0 } else { 3 }
            );
            if invalid {
                assert!(
                    payload.group.borrow().capture.is_none(),
                    "replacement retires capture despite equal IDs/revision"
                );
            }
        });
        if invalid {
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            pump(&editor, &mut visual);
            editor.read_with(&visual, |v, _| {
                assert_eq!(v.worker_requests_pending, 0);
                assert_eq!(v.state.as_ref().unwrap().project.snapshot(), before);
            });
        } else {
            visual.simulate_keystrokes("escape");
            visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
            pump(&editor, &mut visual);
        }
    }
}

#[gpui::test]
fn group_capture_profiles_actual_redraw_separately_from_projection(cx: &mut TestAppContext) {
    for total in [128, 16_384] {
        let (_files, handle, editor, clips) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        editor.update(&mut visual, |v, cx| {
            let old = v.state.as_ref().unwrap();
            let mut snapshot = old.project.snapshot();
            let originals = std::mem::take(snapshot.tracks[0].audio_clips_mut().unwrap());
            for index in 0..total - originals.len() {
                let mut c = originals[0].clone();
                c.id = snapshot.next_id;
                snapshot.next_id += 1;
                c.start = 10_000_000 + index as u64 * 48_000;
                snapshot.tracks[0].audio_clips_mut().unwrap().push(c);
            }
            snapshot.tracks[0]
                .audio_clips_mut()
                .unwrap()
                .extend(originals);
            let project =
                std::sync::Arc::new(hex_project::Project::from_snapshot(snapshot).unwrap());
            // Metadata-only renderer fixture: no publication/source authority or
            // commit is inferred from this view; original real-Service cases run separately.
            v.state = Some(with_project_view(old, project));
            cx.notify();
        });
        visual.run_until_parked();
        let target = begin_group(&mut visual, &clips);
        visual.run_until_parked();
        let (row, position) = editor.read_with(&visual, |v, _| {
            let drag = v.active_clip_drag.as_ref().unwrap();
            let preview = drag.preview.get().unwrap();
            (
                preview.target_track,
                super::group_position(preview.position),
            )
        });
        let start = Instant::now();
        editor.update(&mut visual, |v, _| {
            let state = v.state.as_ref().unwrap();
            let drag = v.active_clip_drag.as_ref().unwrap();
            let mut group = drag.group.borrow_mut();
            let capture = group.capture.as_mut().unwrap();
            for _ in 0..32 {
                capture.update(&state.project, row, position).unwrap();
            }
        });
        let projection = start.elapsed();
        // Repaint complete actual GPUI view (including unrelated row cost),
        // separately from the projection loop. No native/deadline threshold.
        for _ in 0..2 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
        }
        let start = Instant::now();
        for _ in 0..4 {
            editor.update(&mut visual, |_, cx| cx.notify());
            visual.run_until_parked();
        }
        let painting = start.elapsed();
        for id in &clips {
            let selector = Box::leak(format!("group-clip-preview-{}", id.value()).into_boxed_str());
            assert!(
                visual.debug_bounds(selector).is_some(),
                "actual indexed-source overlays painted"
            );
        }
        eprintln!(
            "group capture profile total={total} selected=3 projection_ns_per_update={} full_gpui_redraw_ns={} profile=debug/no_deadline_claim",
            projection.as_nanos() / 32,
            painting.as_nanos() / 4
        );
        visual.simulate_keystrokes("escape");
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
    }
}

#[gpui::test]
fn group_capture_reenters_valid_lane_and_commits_only_current_projection(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let target = begin_group(&mut visual, &clips);
    visual.run_until_parked();
    let payload = editor.read_with(&visual, |v, _| v.active_clip_drag.as_ref().unwrap().clone());
    visual.simulate_mouse_move(
        point(px(200.0), target.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.run_until_parked();
    assert!(payload.group.borrow().previews().is_empty());
    assert!(
        payload.group.borrow().capture.is_some(),
        "temporary no-target keeps reusable metadata"
    );
    let returned = point(target.x + px(13.0), target.y);
    visual.simulate_mouse_move(returned, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    let plan = payload.group.borrow().previews().to_vec();
    assert_eq!(plan.len(), 3);
    visual.simulate_mouse_up(returned, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    assert!(
        payload.group.borrow().capture.is_none(),
        "accepted release retires shared capture"
    );
    editor.read_with(&visual, |v, _| {
        let project = &v.state.as_ref().unwrap().project;
        for item in &plan {
            let hex_project::ClipGroupContent::Audio(c) = &item.content else {
                panic!("audio fixture")
            };
            assert_eq!(
                project.clip(c.id()),
                Some(c),
                "fresh canonical release equals current landing"
            );
        }
        assert!(v.error.is_none(), "{:?}", v.error);
    });
}
