//! Actual multi-selection/header drag → worker candidate → one history step.
use super::super::test_environment::Environment;
use super::super::{EditorView, musical_time::SnapGrid};
use gpui::{
    AppContext, Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
};
use hex_project::{ClipId, Command, Created};
use hex_session::service::{Event, Request, Service};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

// Armed before any Service work can escape. Only an actual successful Import
// response disarms retention; shutdown is deliberately not treated as a join.
struct PendingSetup {
    environment: Arc<Environment>,
    acknowledged: bool,
}
impl PendingSetup {
    fn new(environment: Arc<Environment>) -> Self {
        Self {
            environment,
            acknowledged: false,
        }
    }
    fn acknowledge_import(&mut self, event: &Event, path: &Path) {
        assert!(event.error.is_none(), "{:?}", event.error);
        assert!(event.request_complete);
        assert_eq!(event.status.as_deref(), Some("audio imported"));
        assert!(
            event.state.project.assets().any(|asset| {
                asset.path() == path
                    && asset.source_sample_rate() == 48_000
                    && asset.source_frames() == 96_000
            }),
            "ACK must contain the actual fixture import"
        );
        self.acknowledged = true;
    }
}
impl Drop for PendingSetup {
    fn drop(&mut self) {
        if !self.acknowledged {
            self.environment.retain_for_unacknowledged_setup_failure();
        }
    }
}

fn fixture_wav(environment: &Environment) -> PathBuf {
    // Identical96k-frame mono PCM fixture; all bytes live under the leased root.
    let rate = 48_000_u32;
    let frames = 96_000_u32;
    let bytes = frames * 2;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + bytes).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&bytes.to_le_bytes());
    for _ in 0..frames {
        wav.extend_from_slice(&8192_i16.to_le_bytes());
    }
    let path = environment.root().join("group.wav");
    std::fs::write(&path, wav).unwrap();
    path
}

fn next(service: &Service) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            assert!(event.error.is_none(), "{:?}", event.error);
            return event;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "next: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
pub(in crate::editor) fn pump(editor: &Entity<EditorView>, visual: &mut VisualTestContext) {
    let deadline = crate::test_wait::hang_deadline();
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
        assert!(
            std::time::Instant::now() < deadline,
            "pump: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
pub(in crate::editor) fn fixture(
    cx: &mut TestAppContext,
) -> (
    Arc<Environment>,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    Vec<ClipId>,
) {
    fixture_with_service(cx, Service::new(48_000, 32, 8_000_000).unwrap())
}

pub(in crate::editor) fn fixture_with_service(
    cx: &mut TestAppContext,
    service: Service,
) -> (
    Arc<Environment>,
    gpui::AnyWindowHandle,
    Entity<EditorView>,
    Vec<ClipId>,
) {
    cx.update(super::super::bind_editor_keys);
    let files = Environment::new().unwrap();
    let mut setup = PendingSetup::new(Arc::clone(&files));
    let wav = fixture_wav(&files);
    let directory = files.root();
    std::fs::create_dir(directory.join("samples")).unwrap();
    next(&service);
    service.submit(Request::Import(wav.clone())).unwrap();
    let event = next(&service);
    setup.acknowledge_import(&event, &wav);
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
                EditorView::new_for_test_with_environment(window, cx, service, Arc::clone(&files))
            });
            editor.update(cx, |v, cx| {
                assert!(Arc::ptr_eq(&files, &v.test_environment));
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
pub(in crate::editor) fn click(visual: &mut VisualTestContext, clip: ClipId, additive: bool) {
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
                assert!(v.pending_clip_move.is_some());
                let refused = next_with_error(&v.service);
                assert!(refused.error.as_ref().unwrap().contains("project changed"));
                v.receive(refused);
                assert!(v.pending_clip_move.is_none());
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
                assert_eq!(current.name(), "independent rename");
            }
            assert_eq!(project.clip(clips[0]).unwrap().name(), "independent rename");
            if hold_ack {
                assert!(matches!(&v.last_enqueued_edit, Some(Command::MoveClipsWithOverwrite { clips: selected, .. }) if selected == &clips));
                assert!(v.error.as_ref().unwrap().contains("project changed"));
            } else {
                assert!(matches!(&v.last_enqueued_edit, Some(Command::EditClips { clips: selected, edit: hex_project::ClipEdit::Rename { name } }) if selected == &clips && name == "independent rename"));
            }
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
        comparison_audio: state.comparison_audio.clone(),
        project,
        waveforms: state.waveforms.clone(),
        source_overviews: state.source_overviews.clone(),
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

fn prove_fixture_catalog_custody(cx: &mut TestAppContext, unwind: bool) {
    let (files, handle, editor, _) = fixture(cx);
    let root = files.root().to_owned();
    let weak = Arc::downgrade(&files);
    let gate = files.gate_next_catalog_save();
    editor.read_with(cx, |view, _| {
        assert!(Arc::ptr_eq(&files, &view.test_environment));
        assert_eq!(
            view.browser.directory,
            root.join("samples"),
            "actual filesystem-job owner must own fixture scan path, not a separate root"
        );
        assert!(
            view.state
                .as_ref()
                .unwrap()
                .project
                .assets()
                .all(|asset| asset.path().starts_with(&root))
        );
        assert_eq!(
            view.recent_projects.test_path(),
            root.join("config/recent-projects.json")
        );
        assert_eq!(
            view.theme_settings.test_path(),
            root.join("config/theme.json")
        );
        // These are the exact four paths resolved by the injected constructor;
        // the catalog is derived by its real worker from the browser path.
        let empty = PathBuf::new();
        let paths = files.resolve_paths(
            (empty.clone(), empty.clone(), empty.clone(), empty),
            [true; 4],
        );
        assert_eq!(
            paths,
            (
                root.join("config/browser.json"),
                root.join("config/recent-projects.json"),
                root.join("config/theme.json"),
                root.join("config/project-directory.json")
            )
        );
    });
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    assert!(
        gate.reached.load(Ordering::Acquire),
        "paused inside actual catalog worker"
    );
    assert!(!gate.finished.load(Ordering::Acquire));
    visual.update(|window, cx| {
        let _ = window.replace_root(cx, |_, _| gpui::Empty);
    });
    let editor_weak = editor.downgrade();
    drop(editor);
    visual.update(|_, _| {});
    visual.run_until_parked();
    assert!(
        editor_weak.upgrade().is_none(),
        "actual view retired before I/O continuation"
    );
    let retired = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _fixture = files;
        if unwind {
            panic!("intentional post-ACK fixture unwind");
        }
    }));
    assert_eq!(retired.is_err(), unwind);
    assert_eq!(
        weak.strong_count(),
        1,
        "only real held filesystem job retains SAME root"
    );
    assert!(root.is_dir());
    gate.release();
    visual.run_until_parked();
    assert!(gate.finished.load(Ordering::Acquire));
    assert!(gate.save_succeeded.load(Ordering::Relaxed));
    assert!(gate.root_present_during_save.load(Ordering::Relaxed));
    visual.update(|_, _| {});
    assert!(
        weak.upgrade().is_none(),
        "fixture/view/job owners released without Arc leak"
    );
    let deadline = crate::test_wait::hang_deadline();
    while root.exists() {
        assert!(
            Instant::now() < deadline,
            "owned background cleanup timed out: {root:?}"
        );
        std::thread::yield_now();
    }
    visual.run_until_parked();
    assert!(!root.exists(), "drained job cannot recreate the root");
    eprintln!(
        "group_fixture_catalog_custody unwind={unwind} actual_save=true same_root=true cleaned=true"
    );
}

#[gpui::test]
fn group_fixture_same_root_survives_real_catalog_job_after_view_retirement(
    cx: &mut TestAppContext,
) {
    prove_fixture_catalog_custody(cx, false);
}

#[gpui::test]
fn group_fixture_post_ack_unwind_keeps_actual_job_root_until_save_and_cleanup(
    cx: &mut TestAppContext,
) {
    prove_fixture_catalog_custody(cx, true);
}

#[test]
fn group_fixture_pre_ack_real_import_failure_retains_disk_but_not_environment_memory() {
    let environment = Environment::new().unwrap();
    let root = environment.root().to_owned();
    let weak = Arc::downgrade(&environment);
    let observed_refusal = std::cell::Cell::new(false);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut setup = PendingSetup::new(environment);
        let missing = root.join("missing-import.wav");
        let service = Service::new(48_000, 32, 8_000_000).unwrap();
        next(&service);
        service.submit(Request::Import(missing.clone())).unwrap();
        let deadline = crate::test_wait::hang_deadline();
        let event = loop {
            if let Ok(event) = service.try_event() {
                break event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "group_fixture_pre_ack_real_import_failure_retains_disk_but_not_environment_memory: hang guard expired"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        assert!(event.request_complete);
        assert!(
            event.error.is_some(),
            "real missing-file import must refuse"
        );
        observed_refusal.set(true);
        setup.acknowledge_import(&event, &missing); // Must unwind BEFORE disarming.
    }));
    assert!(caught.is_err());
    assert!(
        observed_refusal.get(),
        "caught panic came after actual request completion, not timeout"
    );
    assert!(
        weak.upgrade().is_none(),
        "retain disk evidence without leaking Environment/Arc"
    );
    assert!(
        root.is_dir(),
        "unacknowledged successful setup cannot remove evidence"
    );
    eprintln!(
        "group_fixture_pre_ack_retention actual_refusal=true weak_expired=true disk_retained=true root={root:?}"
    );
    // Only this artificial refusal case has proven its sole I/O request finished
    // and has launched NO UI jobs. Explicit cleanup of that exclusive test root
    // is safe here; general unacknowledged failures have no automatic cleanup.
    std::fs::remove_dir_all(root).unwrap();
}

#[gpui::test]
fn review_group_landing_paints_unselected_overwrite_victim(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    for (i, clip) in clips[..2].iter().enumerate() {
        click(&mut visual, *clip, i != 0);
    }
    let selector = Box::leak(format!("clip-drag-surface-{}", clips[1].value()).into_boxed_str());
    let victim = Box::leak(format!("clip-name-{}", clips[2].value()).into_boxed_str());
    let down = visual.debug_bounds(selector).unwrap().center();
    let target = visual.debug_bounds(victim).unwrap().center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let drag = view.active_clip_drag.as_ref().unwrap();
        let group = drag.group.borrow();
        assert_eq!(group.previews().len(), 2);
        let state = view.state.as_ref().unwrap();
        let victim = state.project.clip(clips[2]).unwrap();
        assert!(
            group.previews().iter().any(|p| match &p.content {
                hex_project::ClipGroupContent::Audio(c) =>
                    c.start() < victim.start() + victim.duration()
                        && victim.start() < c.start() + c.duration(),
                _ => false,
            }),
            "actual valid group landing must overlap the unselected victim"
        );
    });
    assert!(
        visual.debug_bounds("clip-drop-overlap-0").is_some(),
        "group landing must paint actual overwrite victims"
    );
    visual.simulate_keystrokes("escape");
}

#[gpui::test]
fn copied_mixed_group_selects_actual_worker_births_then_next_drag_moves_only_copies(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    let track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[0].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::PlaceMidiClip {
            track,
            start: hex_project::BeatTime::from_quarters(8).unwrap(),
            length: hex_project::BeatTime::ONE
        })));
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
            start: hex_project::BeatTime::ZERO,
            duration: hex_project::BeatTime::ONE,
            pitch: 64,
            channel: 2,
            velocity: 91
        })));
    });
    pump(&editor, &mut visual);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddMidiTrack {
            name: "copy row".into()
        })));
    });
    pump(&editor, &mut visual);
    let audio_track = editor.read_with(&visual, |v, _| {
        v.state.as_ref().unwrap().project.tracks()[1].id()
    });
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::CopyClipTo {
            clip: clips[0],
            track: audio_track,
            start: 48_000
        })));
    });
    pump(&editor, &mut visual);
    let audio = editor.read_with(&visual, |v, _| {
        v.state
            .as_ref()
            .unwrap()
            .project
            .track(audio_track)
            .unwrap()
            .clips()[0]
            .id()
    });
    // MIDI is born first in row order, then its NoteId, then the copied Audio
    // ClipId: the actual response range contains a note hole.
    let originals = [midi, audio];
    editor.update(&mut visual, |v, cx| {
        // Keep the intended short overlap inside the original loop cycle.
        v.viewport = Some(super::super::viewport::Viewport::fit(384_000));
        cx.notify();
    });
    visual.run_until_parked();
    let target = begin_group(&mut visual, &originals);
    let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
    visual.simulate_mouse_up(
        target,
        MouseButton::Left,
        Modifiers {
            alt: true,
            ..Modifiers::default()
        },
    );
    pump(&editor, &mut visual);
    let (copies, after) = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert!(v.pending_group_copy.is_none());
        let ids = v.selected_clip_ids();
        assert_eq!(ids.len(), 2);
        assert!(
            ids[1].value() > ids[0].value() + 1,
            "actual copied note ID lies inside the ClipId endpoints"
        );
        assert!(ids.iter().all(|id| !originals.contains(id)));
        let p = &v.state.as_ref().unwrap().project;
        // This short copy overlaps its old selected members. Incoming copies
        // win; old prefixes keep their IDs and source content, rather than
        // leaving an invalid overlapping full original behind.
        let new_audio = ids.iter().find_map(|id| p.clip(*id)).unwrap();
        let old_audio = before.clip(audio).unwrap();
        let prefix = p.clip(audio).unwrap();
        assert_eq!(prefix.start(), old_audio.start());
        assert_eq!(prefix.duration(), new_audio.start() - old_audio.start());
        assert_eq!(prefix.source_length(), prefix.duration());
        assert_eq!(prefix.source_offset(), old_audio.source_offset());
        assert_eq!(prefix.asset(), old_audio.asset());
        assert_eq!(prefix.fade(), old_audio.fade());
        let new_midi = ids.iter().find_map(|id| p.midi_clip(*id)).unwrap();
        let old_midi = before.midi_clip(midi).unwrap();
        let prefix = p.midi_clip(midi).unwrap();
        assert_eq!(prefix.start(), old_midi.start());
        assert_eq!(
            prefix.length().raw(),
            new_midi.start().raw() - old_midi.start().raw()
        );
        assert_eq!(prefix.source(), old_midi.source());
        assert_eq!(
            hex_project::Project::from_snapshot(p.snapshot()).unwrap(),
            **p
        );
        assert_eq!(new_midi.notes().len(), 1);
        assert_eq!(
            (new_midi.notes()[0].pitch(), new_midi.notes()[0].velocity()),
            (64, 91)
        );
        (ids, p.clone())
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let mut expected = before.snapshot();
        expected.next_id = v.state.as_ref().unwrap().project.snapshot().next_id;
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), expected);
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project, after)
    });
    let target = begin_group(&mut visual, &copies);
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        assert_eq!(p.clip(audio), after.clip(audio));
        assert_eq!(p.midi_clip(midi), after.midi_clip(midi));
        let copy_audio = copies.iter().find_map(|id| p.clip(*id)).unwrap();
        assert_ne!(
            copy_audio.start(),
            after.clip(copy_audio.id()).unwrap().start()
        );
        assert_eq!(v.selected_clip_ids(), copies);
    });
}

#[gpui::test]
fn group_copy_actual_completion_does_not_steal_new_selection_and_keeps_ancillary_error(
    cx: &mut TestAppContext,
) {
    for supersede in [false, true] {
        let (_files, handle, editor, clips) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        let target = begin_group(&mut visual, &clips[..2]);
        visual.simulate_mouse_up(
            target,
            MouseButton::Left,
            Modifiers {
                alt: true,
                ..Modifiers::default()
            },
        );
        let mut event = editor.update(&mut visual, |v, _| next(&v.service));
        assert!(matches!(event.created, Some(Created::Clips { .. })));
        if supersede {
            click(&mut visual, clips[2], false);
        }
        // This is a real committed worker response. Only ancillary presentation
        // error is injected; model/Created/revision/request_complete stay real.
        event.error = Some("post-commit observation unavailable".into());
        editor.update(&mut visual, |v, cx| {
            v.receive(event);
            cx.notify();
        });
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_group_copy.is_none());
            assert_eq!(
                v.error.as_deref(),
                Some("post-commit observation unavailable")
            );
            let selected = v.selected_clip_ids();
            if supersede {
                assert_eq!(selected, vec![clips[2]]);
            } else {
                assert_eq!(selected.len(), 2);
                assert!(selected.iter().all(|id| !clips.contains(id)));
            }
        });
    }
}

#[gpui::test]
fn group_copy_deferred_submission_counts_only_own_fifo_terminal_and_actual_refusal(
    cx: &mut TestAppContext,
) {
    for refuse in [false, true] {
        let (_files, handle, editor, clips) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        for (i, id) in clips[..2].iter().enumerate() {
            click(&mut visual, *id, i != 0);
        }
        let before = editor.read_with(&visual, |v, _| v.state.as_ref().unwrap().project.clone());
        editor.update(&mut visual, |v, cx| {
            let track = v.state.as_ref().unwrap().project.tracks()[0].id();
            let command = Command::MoveClipsWithOverwrite {
                clips: clips[..2].to_vec(),
                anchor: clips[0],
                track,
                position: hex_project::ClipGroupPosition::Audio(if refuse {
                    u64::MAX
                } else {
                    480_000
                }),
                copy: true,
            };
            let intent = super::super::group_copy::Pending::capture(v, &command).unwrap();
            // Force the normal deferred queue route. The earlier REAL worker
            // SaveCurrent refusal cannot change this untitled document revision.
            v.pending_requests.push_back(Request::SaveCurrent);
            assert!(v.enqueue(Request::Edit(command)));
            v.pending_group_copy = Some(intent);
            assert_eq!(v.worker_requests_pending, 0);
            assert!(!v.pending_requests.is_empty());
            v.tick(cx); // Both requests really enter Service here, not at enqueue.
            assert_eq!(v.worker_requests_pending, 2);
        });
        let earlier = editor.update(&mut visual, |v, _| next_with_error(&v.service));
        assert!(earlier.save_complete && earlier.error.is_some());
        editor.update(&mut visual, |v, _| {
            v.receive(earlier);
            assert!(v.pending_group_copy.is_some());
            assert_eq!(v.selected_clip_ids(), clips[..2]);
        });
        let own = editor.update(&mut visual, |v, _| next_with_error(&v.service));
        if refuse {
            assert!(own.error.is_some());
            assert!(own.created.is_none());
        } else {
            assert!(own.error.is_none());
            assert!(matches!(own.created, Some(Created::Clips { .. })));
        }
        editor.update(&mut visual, |v, cx| {
            v.receive(own);
            cx.notify();
        });
        editor.read_with(&visual, |v, _| {
            assert!(v.pending_group_copy.is_none());
            assert_eq!(v.worker_requests_pending, 0);
            if refuse {
                assert_eq!(v.state.as_ref().unwrap().project, before);
                assert_eq!(v.selected_clip_ids(), clips[..2]);
            } else {
                assert_eq!(v.selected_clip_ids().len(), 2);
                assert!(v.selected_clip_ids().iter().all(|id| !clips.contains(id)));
            }
        });
    }
}
fn next_with_error(service: &Service) -> Event {
    let deadline = crate::test_wait::hang_deadline();
    loop {
        if let Ok(event) = service.try_event() {
            return event;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "next_with_error: hang guard expired"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[gpui::test]
fn group_overlap_is_passive_clipped_and_excludes_selected_originals_in_both_modes(
    cx: &mut TestAppContext,
) {
    for copy in [false, true] {
        let (_files, handle, editor, clips) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        for (i, id) in clips[..2].iter().enumerate() {
            click(&mut visual, *id, i != 0);
        }
        let down = visual
            .debug_bounds(Box::leak(
                format!("clip-drag-surface-{}", clips[1].value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        let target = visual
            .debug_bounds(Box::leak(
                format!("clip-name-{}", clips[2].value()).into_boxed_str(),
            ))
            .unwrap()
            .center();
        let modifiers = Modifiers {
            alt: copy,
            ..Modifiers::default()
        };
        visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(
            point(down.x + px(12.0), down.y),
            MouseButton::Left,
            modifiers,
        );
        visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
        visual.run_until_parked();
        let bounds = visual.debug_bounds("clip-drop-overlap-0").unwrap();
        assert!(bounds.contains(&target));
        assert!(
            visual.debug_bounds("clip-drop-overlap-1").is_none(),
            "selected originals are protected, not victims"
        );
        visual.simulate_mouse_move(point(target.x, px(0.0)), MouseButton::Left, modifiers);
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-drop-overlap-0").is_none());
        visual.simulate_mouse_move(target, MouseButton::Left, modifiers);
        visual.run_until_parked();
        assert!(visual.debug_bounds("clip-drop-overlap-0").is_some());
        // The passive layer has no handlers: normal authoritative drop still
        // reaches the lane through the highlighted victim at this coordinate.
        visual.simulate_mouse_up(target, MouseButton::Left, modifiers);
        pump(&editor, &mut visual);
        editor.read_with(&visual, |v, _| {
            assert!(v.error.is_none(), "{:?}", v.error);
            assert!(v.active_clip_drag.is_none());
        });
    }
}

#[gpui::test]
fn integration_group_console_refuses_partial_primary_copy(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    editor.update(&mut visual, |view, cx| {
        assert!(view.enqueue(Request::Edit(Command::AddScene {
            name: "group target".into(),
        })));
        cx.notify();
    });
    pump(&editor, &mut visual);
    let (track, scene, before) = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        (
            project.tracks()[0].id(),
            project.scenes()[0].id(),
            project.snapshot(),
        )
    });
    begin_group(&mut visual, &clips);
    visual.run_until_parked();
    editor.read_with(&visual, |view, _| {
        let drag = view
            .active_clip_drag
            .as_ref()
            .expect("actual group pointer capture");
        assert_eq!(drag.group.borrow().ids.len(), 3);
        assert_eq!(view.selected_clip_ids(), clips);
        assert!(view.drag_start_is_current(drag));
    });
    assert!(visual.update(|_, app| app.has_active_drag()));
    editor.update(&mut visual, |view, cx| {
        view.last_enqueued_edit = None;
        view.set_mode(super::super::EditorMode::Session);
        cx.notify();
    });
    visual.run_until_parked();
    let selector =
        Box::leak(format!("console-cell-{}-{}", track.value(), scene.value()).into_boxed_str());
    let target = visual
        .debug_bounds(selector)
        .expect("actual Console target")
        .center();
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    let submitted = editor.read_with(&visual, |view, _| view.last_enqueued_edit.clone());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        let state = view.state.as_ref().unwrap();
        eprintln!(
            "actual selected={} submitted={submitted:?} cell={:?} error={:?}",
            clips.len(),
            state.project.session_cell(track, scene),
            view.error
        );
        assert!(
            submitted.is_none(),
            "unsupported group Console drop must not enqueue a primary-only copy"
        );
        assert_eq!(state.project.snapshot(), before);
        assert!(view.active_clip_drag.is_none());
        assert_eq!(view.worker_requests_pending, 0);
    });
}

#[gpui::test]
fn console_midi_primary_group_cell_insertion_and_stale_release_are_atomic(cx: &mut TestAppContext) {
    for (insert, stale) in [(false, false), (true, false), (false, true)] {
        let cx = &mut cx.new_app();
        let (_files, handle, editor, clips) = fixture(cx);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.update(|window, _| window.activate_window());
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddMidiTrack {
                name: "group midi".into(),
            })));
        });
        pump(&editor, &mut visual);
        let track = editor.read_with(&visual, |v, _| {
            v.state
                .as_ref()
                .unwrap()
                .project
                .tracks()
                .last()
                .unwrap()
                .id()
        });
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::PlaceMidiClip {
                track,
                start: hex_project::BeatTime::from_quarters(6).unwrap(),
                length: hex_project::BeatTime::ONE,
            })));
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
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddMidiNote {
                clip: midi,
                start: hex_project::BeatTime::ZERO,
                duration: hex_project::BeatTime::ONE,
                pitch: 69,
                channel: 0,
                velocity: 100,
            })));
        });
        pump(&editor, &mut visual);
        editor.update(&mut visual, |view, _| {
            assert!(view.enqueue(Request::Edit(Command::AddScene {
                name: "midi group target".into(),
            })));
        });
        pump(&editor, &mut visual);
        let scene = editor.read_with(&visual, |v, _| {
            v.state.as_ref().unwrap().project.scenes()[0].id()
        });
        // The actual dragged/primary member is MIDI; its Audio neighbour must
        // neither be flattened into it nor silently left behind by this receiver.
        let selected = vec![clips[1], midi];
        begin_group(&mut visual, &selected);
        let retained = editor.read_with(&visual, |view, _| {
            let drag = view.active_clip_drag.as_ref().unwrap();
            assert_eq!(drag.clip, midi);
            assert_eq!(drag.group.borrow().ids, selected);
            assert!(drag.group.borrow().capture.is_some());
            assert!(view.drag_start_is_current(drag));
            drag.clone()
        });
        if stale {
            // A genuine worker revision, not a fabricated stale Project/Event.
            editor.update(&mut visual, |view, _| {
                assert!(view.enqueue(Request::Edit(Command::Gain { track, gain: 0.75 })));
            });
            pump(&editor, &mut visual);
        }
        let (before, revision, history, selection, primary) =
            editor.read_with(&visual, |view, _| {
                let state = view.state.as_ref().unwrap();
                (
                    state.project.snapshot(),
                    state.revision,
                    (state.can_undo, state.can_redo),
                    view.selected_clip_ids(),
                    view.selected_clip,
                )
            });
        editor.update(&mut visual, |view, cx| {
            view.last_enqueued_edit = None;
            view.set_mode(super::super::EditorMode::Session);
            cx.notify();
        });
        visual.run_until_parked();
        let selector = if insert {
            format!("console-insert-before-{}-{}", track.value(), scene.value())
        } else {
            format!("console-cell-{}-{}", track.value(), scene.value())
        };
        let target = visual
            .debug_bounds(Box::leak(selector.into_boxed_str()))
            .unwrap()
            .center();
        visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert!(
                view.last_enqueued_edit.is_none(),
                "insert={insert} stale={stale}"
            );
            assert_eq!(state.project.snapshot(), before);
            assert_eq!(state.revision, revision);
            assert_eq!((state.can_undo, state.can_redo), history);
            assert_eq!(view.selected_clip_ids(), selection);
            assert_eq!(view.selected_clip, primary);
            assert!(state.project.session_cell(track, scene).is_none());
            assert!(view.active_clip_drag.is_none());
            assert_eq!(view.worker_requests_pending, 0);
            assert!(view.pending_requests.is_empty());
            assert!(
                view.error
                    .as_deref()
                    .is_some_and(|s| s.contains("arrangement target"))
            );
        });
        assert!(retained.group.borrow().capture.is_none());
        assert!(retained.preview.get().is_none());
        assert!(retained.destination.get().is_none());
        assert!(!retained.lane_hovered.get());
        assert!(retained.insertion_line_y.get().is_none());
        assert!(!visual.update(|_, app| app.has_active_drag()));
        // Refusal did not insert even a no-op history entry: the next actual
        // Undo is the prior authored scene (or independent Gain in stale case).
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            let project = &view.state.as_ref().unwrap().project;
            if stale {
                assert_eq!(project.track(track).unwrap().gain(), 1.0);
                assert_eq!(project.scenes().len(), 1);
            } else {
                assert!(project.scenes().is_empty());
            }
        });
        editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
        pump(&editor, &mut visual);
        editor.read_with(&visual, |view, _| {
            assert_eq!(view.state.as_ref().unwrap().project.snapshot(), before);
        });
    }
}

#[gpui::test]
fn console_single_audio_arrangement_payload_still_copies_once_with_one_history_step(
    cx: &mut TestAppContext,
) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    visual.update(|window, _| window.activate_window());
    editor.update(&mut visual, |view, _| {
        assert!(view.enqueue(Request::Edit(Command::AddScene {
            name: "single target".into(),
        })));
    });
    pump(&editor, &mut visual);
    let (track, scene, asset, before) = editor.read_with(&visual, |view, _| {
        let project = &view.state.as_ref().unwrap().project;
        let source = &project.tracks()[0].clips()[1];
        assert_eq!(source.id(), clips[1]);
        (
            project.tracks()[0].id(),
            project.scenes()[0].id(),
            source.asset(),
            project.snapshot(),
        )
    });
    click(&mut visual, clips[1], false);
    let down = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clips[1].value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    editor.update(&mut visual, |view, cx| {
        assert_eq!(
            view.active_clip_drag.as_ref().unwrap().group.borrow().ids,
            vec![clips[1]]
        );
        view.last_enqueued_edit = None;
        view.set_mode(super::super::EditorMode::Session);
        cx.notify();
    });
    visual.run_until_parked();
    let target = visual
        .debug_bounds(Box::leak(
            format!("console-cell-{}-{}", track.value(), scene.value()).into_boxed_str(),
        ))
        .unwrap()
        .center();
    visual.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |view, _| {
        assert!(matches!(view.last_enqueued_edit,
            Some(Command::CopyArrangementAudioClipToSession { source, scene: target })
                if source == clips[1] && target == scene));
        let project = &view.state.as_ref().unwrap().project;
        let Some(hex_project::SessionCellContent::Audio(copied)) =
            project.session_cell(track, scene)
        else {
            panic!("actual single Audio copy must reach canonical Console state");
        };
        assert_eq!(copied.asset(), asset);
        assert_eq!(copied.source_start(), 24_000);
        assert_eq!(copied.source_end(), 48_000);
        assert_eq!(project.tracks()[0].clips().len(), 3);
        // Existing Created::SessionClip completion selects its Console cell.
        assert_eq!(view.selected_cell, Some((track, scene)));
        assert!(view.selected_clip_ids().is_empty());
        assert!(view.active_clip_drag.is_none());
        assert_eq!(view.worker_requests_pending, 0);
        assert!(view.pending_requests.is_empty());
        project.snapshot()
    });
    assert!(!visual.update(|_, app| app.has_active_drag()));
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Undo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        // Undo restores every authored field but deliberately keeps committed
        // identity high-water, exactly like the existing group-copy oracle.
        let mut expected = before.clone();
        assert!(after.next_id > before.next_id);
        expected.next_id = after.next_id;
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), expected)
    });
    editor.update(&mut visual, |view, _| assert!(view.enqueue(Request::Redo)));
    pump(&editor, &mut visual);
    editor.read_with(&visual, |view, _| {
        assert_eq!(view.state.as_ref().unwrap().project.snapshot(), after)
    });
}

#[gpui::test]
fn alias_group_drag_selects_worker_births_and_survives_delayed_ack(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "alias destination".into()
        })))
    });
    pump(&editor, &mut visual);
    let (track, before) = editor.read_with(&visual, |v, _| {
        let p = &v.state.as_ref().unwrap().project;
        (p.tracks()[1].id(), p.snapshot())
    });
    let target = begin_group(&mut visual, &clips);
    visual.run_until_parked();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", track.value()).into_boxed_str(),
        ))
        .unwrap();
    let target = point(target.x, lane.center().y);
    let alias = Modifiers {
        alt: true,
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_modifiers_change(alias);
    visual.simulate_mouse_move(target, MouseButton::Left, alias);
    visual.run_until_parked();
    assert!(visual.debug_bounds("clip-alias-drag-indicator").is_some());
    visual.simulate_mouse_up(target, MouseButton::Left, alias);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.pending_clip_move.is_some()));
    assert!(visual.debug_bounds("clip-alias-drag-indicator").is_some());
    pump(&editor, &mut visual);
    let after = editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        assert!(v.pending_clip_move.is_none());
        let p = &v.state.as_ref().unwrap().project;
        let copies = v.selected_clip_ids();
        assert_eq!(copies.len(), clips.len());
        for (original, copy) in clips.iter().zip(copies.iter()) {
            assert!(p.clip(*original).is_some());
            assert!(p.clip(*copy).is_some());
            assert!(p.clip_alias(*original).is_some());
            assert_eq!(p.clip_alias(*original), p.clip_alias(*copy));
        }
        p.snapshot()
    });
    visual.simulate_keystrokes("cmd-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        let mut expected = before.clone();
        expected.next_id = v.state.as_ref().unwrap().project.snapshot().next_id;
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), expected);
    });
    visual.simulate_keystrokes("cmd-shift-z");
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert_eq!(v.state.as_ref().unwrap().project.snapshot(), after)
    });
}

#[gpui::test]
fn alias_drag_to_new_track_keeps_insertion_preview_and_selects_birth(cx: &mut TestAppContext) {
    let (_files, handle, editor, clips) = fixture(cx);
    let mut visual = VisualTestContext::from_window(handle, cx);
    editor.update(&mut visual, |v, _| {
        assert!(v.enqueue(Request::Edit(Command::AddTrack {
            name: "destination".into()
        })))
    });
    pump(&editor, &mut visual);
    let (source, next) = editor.read_with(&visual, |v, _| {
        let tracks = v.state.as_ref().unwrap().project.tracks();
        (tracks[0].id(), tracks[1].id())
    });
    click(&mut visual, clips[0], false);
    let surface = visual
        .debug_bounds(Box::leak(
            format!("clip-drag-surface-{}", clips[0].value()).into_boxed_str(),
        ))
        .unwrap();
    let lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", source.value()).into_boxed_str(),
        ))
        .unwrap();
    let next_lane = visual
        .debug_bounds(Box::leak(
            format!("arrangement-lane-{}", next.value()).into_boxed_str(),
        ))
        .unwrap();
    let down = surface.center();
    visual.simulate_mouse_down(down, MouseButton::Left, Modifiers::default());
    visual.simulate_mouse_move(
        point(down.x + px(12.0), down.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    let target = point(
        lane.origin.x + lane.size.width * 0.73,
        (lane.origin.y + lane.size.height + next_lane.origin.y) / 2.0,
    );
    let alias = Modifiers {
        alt: true,
        platform: true,
        ..Modifiers::default()
    };
    visual.simulate_mouse_move(target, MouseButton::Left, alias);
    visual.run_until_parked();
    assert!(visual.debug_bounds("track-insert-preview").is_some());
    assert!(visual.debug_bounds("clip-alias-drag-indicator").is_some());
    visual.simulate_mouse_up(target, MouseButton::Left, alias);
    visual.run_until_parked();
    editor.read_with(&visual, |v, _| assert!(v.pending_clip_move.is_some()));
    assert!(visual.debug_bounds("track-insert-preview").is_some());
    pump(&editor, &mut visual);
    editor.read_with(&visual, |v, _| {
        assert!(v.error.is_none(), "{:?}", v.error);
        let p = &v.state.as_ref().unwrap().project;
        let ids = v.selected_clip_ids();
        assert_eq!(ids.len(), 1);
        assert_ne!(ids[0], clips[0]);
        assert!(p.clip_alias(clips[0]).is_some());
        assert_eq!(p.clip_alias(clips[0]), p.clip_alias(ids[0]));
        assert_eq!(p.tracks().len(), 3);
        assert!(p.tracks()[1].clips().iter().any(|c| c.id() == ids[0]));
    });
}
